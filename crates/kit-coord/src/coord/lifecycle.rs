use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use kit_core::paths::{SymlinkMessage, no_links};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    Ctx, brief,
    caller::Caller,
    config::Config,
    ids,
    journal::Journal,
    kinds::{Actor, Kind, ResType, ResourceRef, SessionMode, SessionRef, Tier},
    launch::{self, LaunchPlan},
    paths::CoordPaths,
    place, role, views,
};

pub const HEARTBEAT_STALE_MS: u64 = 30_000;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Heartbeat {
    pub pid: u32,
    pub terminal: Option<String>,
    pub release: String,
    pub started_ms: u64,
    pub beat_ms: u64,
    pub events_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_error: Option<String>,
}

pub fn read_heartbeat(paths: &CoordPaths) -> Result<Option<Heartbeat>> {
    no_links(&paths.heartbeat, SymlinkMessage::WithPath)?;
    match fs::read(&paths.heartbeat) {
        Ok(data) => Ok(Some(serde_json::from_slice(&data)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn write_heartbeat(paths: &CoordPaths, hb: &Heartbeat) -> Result<()> {
    paths.ensure()?;
    no_links(&paths.heartbeat, SymlinkMessage::WithPath)?;
    let mut tmp = tempfile::NamedTempFile::new_in(&paths.dir)?;
    serde_json::to_writer(&mut tmp, hb)?;
    tmp.flush()?;
    tmp.as_file().sync_all()?;
    tmp.persist(&paths.heartbeat).map_err(|e| e.error)?;
    fs::File::open(&paths.dir)?.sync_all()?;
    Ok(())
}
pub fn active_release(root: &Path) -> String {
    let _ = root;
    crate::deployment::BUILD_REVISION.to_owned()
}
fn up_lock(ctx: &Ctx) -> Result<fs::File> {
    ctx.paths.ensure()?;
    no_links(&ctx.paths.up_lock, SymlinkMessage::WithPath)?;
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&ctx.paths.up_lock)?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match f.try_lock_exclusive() {
            Ok(()) => return Ok(f),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50))
            }
            Err(e) => return Err(e.into()),
        }
    }
}
fn append(ctx: &Ctx, j: &Journal, actor: Actor, kind: Kind) -> Result<()> {
    j.lock()?
        .append(&actor, ctx.now(), vec![(None, None, kind)])?;
    Ok(())
}
fn coordinator_space(ctx: &Ctx, id: &str) -> ResourceRef {
    ResourceRef {
        folder: Some(ctx.paths.home.clone()),
        label: Some("coordinator".into()),
        ..ResourceRef::new(ResType::Space, id)
    }
}
fn space(ctx: &Ctx, j: &Journal) -> Result<(String, bool)> {
    let mut lock = j.lock()?;
    if let Some(r) = lock.state().resources.iter().find(|r| {
        r.ws.is_none()
            && r.owned
            && !r.released
            && r.r.r#type == ResType::Space
            && r.r.folder.as_ref() == Some(&ctx.paths.home)
    }) {
        return Ok((r.r.name.clone(), false));
    }
    if let Some(i) = lock.state().intents.values().find(|i| {
        i.ws.is_none()
            && i.r.r#type == ResType::Space
            && i.r.folder.as_ref() == Some(&ctx.paths.home)
    }) {
        return Ok((i.r.name.clone(), true));
    }
    if let Some(r) = lock.state().resources.iter().rev().find(|r| {
        r.ws.is_none()
            && r.owned
            && r.released
            && r.r.r#type == ResType::Space
            && r.r.folder.as_ref() == Some(&ctx.paths.home)
    }) {
        let r = r.r.clone();
        let id = r.name.clone();
        lock.append(
            &Actor::Cli,
            ctx.now(),
            vec![(None, None, Kind::ResIntent(r))],
        )?;
        return Ok((id, true));
    }
    let id = ids::space_id()?;
    lock.append(
        &Actor::Cli,
        ctx.now(),
        vec![(None, None, Kind::ResIntent(coordinator_space(ctx, &id)))],
    )?;
    Ok((id, true))
}
fn managed(term: &super::adapter::Term, role: &str) -> bool {
    term.managed() && term.label("coord.role") == Some(role)
}
pub(super) fn same_coordinator(t: &super::adapter::Term, rec: &super::state::CoordRec) -> bool {
    t.live()
        && managed(t, "coordinator")
        && rec
            .pid
            .is_none_or(|p| t.pid.and_then(|v| i32::try_from(v).ok()) == Some(p))
        && rec.created_ms.is_none_or(|v| t.created_ms == v)
}
fn stop_locked(ctx: &Ctx, address: &str, role: &str) -> Result<()> {
    if let Some(t) = ctx.hc.get(address)? {
        if managed(&t, role) {
            if t.live() {
                ctx.hc.kill(address)?;
            }
            ctx.hc.rm(address)?;
        }
    }
    Ok(())
}
fn stop(ctx: &Ctx, address: &str, role: &str) -> Result<()> {
    let _terminal = ctx.terminal_lock(address)?;
    stop_locked(ctx, address, role)
}
fn find(ctx: &Ctx, role: &str) -> Result<Option<super::adapter::Term>> {
    for id in ctx.hc.list(false, Some(("coord.role", role)))? {
        if let Some(t) = ctx.hc.get(&id)? {
            if t.live() && managed(&t, role) {
                return Ok(Some(t));
            }
        }
    }
    Ok(None)
}
fn fresh(h: &Heartbeat, now: u64) -> bool {
    h.events_ok && now.saturating_sub(h.beat_ms) <= HEARTBEAT_STALE_MS
}
fn watcher(
    ctx: &Ctx,
    j: &Journal,
    sp: &str,
    space_intent: bool,
    actions: &mut Vec<String>,
) -> Result<()> {
    let hb = read_heartbeat(&ctx.paths).ok().flatten();
    let old = hb
        .as_ref()
        .and_then(|h| h.terminal.clone())
        .or_else(|| j.read_state().ok().and_then(|s| s.watcher.terminal));
    let existing = match old.as_deref() {
        Some(t) => ctx.hc.get(t)?,
        None => find(ctx, "watcher")?,
    };
    if let (Some(h), Some(t)) = (&hb, &existing) {
        if t.live()
            && managed(t, "watcher")
            && (!space_intent || t.label("herdr.space") == Some(sp))
            && t.pid == Some(h.pid)
            && h.terminal.as_deref() == Some(&t.address)
            && fresh(h, ctx.now())
            && h.release == active_release(&ctx.root)
        {
            if space_intent {
                append(
                    ctx,
                    j,
                    Actor::Cli,
                    Kind::ResCreated(coordinator_space(ctx, sp)),
                )?;
                ctx.app
                    .command("space.rename", json!({"id":sp,"name":"coordinator"}))?;
            }
            return Ok(());
        }
    }
    let tab = existing
        .as_ref()
        .and_then(|t| {
            t.label("herdr.tab").map(str::to_owned).or_else(|| {
                ctx.app
                    .ls()
                    .ok()
                    .and_then(|s| s.tab_of(&t.address).map(|(_, tab)| tab.id.clone()))
            })
        })
        .or_else(|| {
            old.as_deref().and_then(|id| {
                j.read_state()
                    .ok()?
                    .resources
                    .iter()
                    .rev()
                    .find(|r| r.ws.is_none() && r.r.r#type == ResType::Tab && r.r.name == id)
                    .and_then(|r| r.r.tab.clone())
            })
        });
    if let Some(t) = existing {
        stop(ctx, &t.address, "watcher")?;
    }
    let mut argv = ctx.cli_argv();
    argv.push("watch".into());
    let launched = launch::launch(
        ctx,
        &LaunchPlan {
            name: "coord-watcher".into(),
            role_label: "watcher",
            ws: None,
            task: None,
            space: sp.into(),
            tab,
            cwd: ctx.paths.home.clone(),
            argv,
            session: None,
            session_markers: vec![],
            tab_name: "watcher".into(),
            space_name: Some("coordinator".into()),
        },
    )?;
    let mut items = Vec::new();
    if space_intent {
        items.push((None, None, Kind::ResCreated(coordinator_space(ctx, sp))));
    }
    let mut r = ResourceRef::new(ResType::Tab, launched.term.address.clone());
    r.space = Some(sp.into());
    r.tab = launched.tab;
    items.push((None, None, Kind::ResCreated(r)));
    j.lock()?.append(&Actor::Cli, ctx.now(), items)?;
    actions.push("watcher_started".into());
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if read_heartbeat(&ctx.paths).ok().flatten().is_some_and(|h| {
            h.terminal.as_deref() == Some(&launched.term.address)
                && fresh(&h, ctx.now())
                && h.release == active_release(&ctx.root)
        }) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
fn start_with_tab(ctx: &Ctx, j: &Journal, reason: &str, tab: Option<String>) -> Result<()> {
    let state = j.read_state()?;
    let sp = state
        .resources
        .iter()
        .find(|r| {
            r.ws.is_none()
                && r.owned
                && !r.released
                && r.r.r#type == ResType::Space
                && r.r.folder.as_ref() == Some(&ctx.paths.home)
        })
        .map(|r| r.r.name.clone())
        .context("coordinator space missing")?;
    let role = role::load(&ctx.paths, &ctx.config()?.coordinator_role)?;
    let name = format!("coordinator-{}", state.coordinator.sessions_started + 1);
    let dir = ctx.paths.session_dir(&name);
    fs::create_dir_all(&dir)?;
    let path = ctx.paths.brief(&name);
    let waking = j
        .read_after(state.cursor)?
        .into_iter()
        .filter(|e| state.waking.contains(&e.seq))
        .collect::<Vec<_>>();
    let msg = brief::coordinator_first(
        &role,
        &ctx.cli(),
        &views::startup_read(&state, &waking, ctx.now()),
    );
    no_links(&path, SymlinkMessage::WithPath)?;
    fs::write(&path, &msg)?;
    let id = if role.session == SessionMode::Preset {
        Some(ids::uuid_v4()?)
    } else {
        None
    };
    let session = SessionRef {
        mode: role.session,
        id: id.clone(),
        dir: Some(dir.clone()),
    };
    let arg = brief::for_argv(msg, &path);
    let vars = role::Vars {
        brief: &arg,
        session_dir: Some(&dir),
        session_id: id.as_deref(),
        repo_root: None,
        cwd: &ctx.paths.home,
        model: None,
        effort: None,
    };
    let argv = role.expand_start(&vars, false)?;
    let model = role.effective_model().or_else(|| role::model_of(&argv));
    let effort = role.effective_effort().or_else(|| role::effort_of(&argv));
    let launched = launch::launch(
        ctx,
        &LaunchPlan {
            name: "coordinator".into(),
            role_label: "coordinator",
            ws: None,
            task: None,
            space: sp.clone(),
            tab,
            cwd: ctx.paths.home.clone(),
            argv,
            session: Some(session),
            session_markers: launch::coordinator_session_markers(&ctx.cli(), &path),
            tab_name: "coordinator".into(),
            space_name: None,
        },
    )?;
    if let Some(tab) = &launched.tab {
        ctx.app.command("tab.move", json!({"id":tab,"index":1}))?;
    }
    ctx.app.command(
        "terminal.pin",
        json!({"terminal":launched.term.address,"pinned":true}),
    )?;
    append(
        ctx,
        j,
        Actor::Cli,
        Kind::CoordSessionStarted {
            role: role.name.clone(),
            model,
            effort,
            pid: launched.term.pid.and_then(|p| i32::try_from(p).ok()),
            created_ms: Some(launched.term.created_ms),
            terminal: launched.term.address,
            space: sp,
            tab: launched.tab,
            session: launched.session,
            release: active_release(&ctx.root),
            role_hash: place::role_hash(&role)?,
            reason: reason.into(),
            brief: path,
        },
    )
}
pub fn restart_coordinator(ctx: &Ctx, j: &Journal, reason: &str) -> Result<()> {
    let address = j
        .read_state()?
        .coordinator
        .terminal
        .context("coordinator is not running")?;
    let _terminal = ctx.terminal_lock(&address)?;
    let term = ctx
        .hc
        .get(&address)?
        .context("coordinator terminal missing")?;
    let mut lock = j.lock()?;
    let rec = lock.state().coordinator.clone();
    ensure!(
        rec.terminal.as_deref() == Some(address.as_str()),
        "coordinator terminal changed"
    );
    ensure!(
        same_coordinator(&term, &rec),
        "coordinator terminal changed"
    );
    if reason != "forced" {
        let state = lock.state();
        ensure!(
            term.sendable()
                && state.waking.is_empty()
                && state.questions.values().all(|q| q.answered.is_some()),
            "coordinator is not ready for handoff"
        );
        let idle = term.status_ms.is_some_and(|at| {
            ctx.now().saturating_sub(at)
                >= ctx
                    .config()
                    .map(|c| c.idle_restart_hours.saturating_mul(3_600_000))
                    .unwrap_or(u64::MAX)
        });
        ensure!(
            state.coordinator.handoff.is_some() || idle,
            "coordinator restart is no longer requested"
        );
    }
    lock.append(
        &Actor::Cli,
        ctx.now(),
        vec![(
            None,
            None,
            Kind::CoordSessionEnded {
                reason: reason.into(),
                terminal: address.clone(),
            },
        )],
    )?;
    drop(lock);
    ctx.hc.kill(&address)?;
    ctx.hc.rm(&address)?;
    start_with_tab(ctx, j, reason, rec.tab)
}
fn config_autostart(ctx: &Ctx, j: &Journal, enabled: bool) -> Result<Config> {
    let old = Config::load(&ctx.paths)?;
    let mut cfg = old.clone().unwrap_or_default();
    if let Some(old) = old {
        if old.autostart != enabled {
            cfg.autostart = enabled;
            cfg.save(&ctx.paths)?;
            append(
                ctx,
                j,
                Actor::Cli,
                Kind::ConfigChanged {
                    key: "autostart".into(),
                    old: json!(old.autostart),
                    new: json!(enabled),
                },
            )?;
        }
    } else {
        cfg.autostart = enabled;
        cfg.save(&ctx.paths)?;
    }
    Ok(cfg)
}
fn setup(cfg: &Config) -> Vec<Value> {
    let Ok(home) = kit_core::paths::home() else {
        return vec![];
    };
    let Ok(root) = cfg.worktree_root() else {
        return vec![];
    };
    setup_at(&home, &root)
}
fn setup_at(home: &Path, root: &Path) -> Vec<Value> {
    let Ok(data) = fs::read(home.join(".claude.json")) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_slice::<Value>(&data) else {
        return vec![];
    };
    let key = root.to_string_lossy();
    if v.get("projects")
        .and_then(|p| p.get(key.as_ref()))
        .and_then(|p| p.get("hasTrustDialogAccepted"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        vec![]
    } else {
        vec![
            json!({"code":"claude_worktree_trust","message":format!("Open claude once in {} and accept the folder trust prompt, so delegates in owned worktrees do not stop at it.",root.display())}),
        ]
    }
}
pub fn up(ctx: &Ctx, if_enabled: bool) -> Result<Value> {
    if if_enabled && !Config::load(&ctx.paths)?.is_some_and(|c| c.autostart) {
        return Ok(json!({"state":"disabled"}));
    }
    let _guard = up_lock(ctx)?;
    if if_enabled && !Config::load(&ctx.paths)?.is_some_and(|c| c.autostart) {
        return Ok(json!({"state":"disabled"}));
    }
    crate::deployment::ensure_current(&ctx.root)?;
    let j = ctx.journal()?;
    let cfg = config_autostart(ctx, &j, true)?;
    let (sp, intent) = space(ctx, &j)?;
    let mut actions = Vec::new();
    watcher(ctx, &j, &sp, intent, &mut actions)?;
    let rec = j.read_state()?.coordinator;
    let existing = match rec.terminal.as_deref() {
        Some(id) => ctx.hc.get(id)?,
        None => None,
    };
    let kept = existing.as_ref().is_some_and(|t| same_coordinator(t, &rec));
    if !kept {
        if let Some(id) = rec.terminal.as_ref() {
            append(
                ctx,
                &j,
                Actor::Cli,
                Kind::CoordSessionEnded {
                    reason: "terminal_gone".into(),
                    terminal: id.clone(),
                },
            )?;
        }
        if let Some(t) =
            find(ctx, "coordinator")?.filter(|t| rec.terminal.as_deref() != Some(&t.address))
        {
            let role = role::load(&ctx.paths, &cfg.coordinator_role)?;
            let tab = ctx.app.ls()?.tab_of(&t.address).map(|(_, t)| t.id.clone());
            append(
                ctx,
                &j,
                Actor::Cli,
                Kind::CoordSessionStarted {
                    role: role.name.clone(),
                    model: None,
                    effort: None,
                    pid: t.pid.and_then(|p| i32::try_from(p).ok()),
                    created_ms: Some(t.created_ms),
                    terminal: t.address,
                    space: sp.clone(),
                    tab,
                    session: None,
                    release: active_release(&ctx.root),
                    role_hash: place::role_hash(&role)?,
                    reason: "recovered".into(),
                    brief: ctx.paths.brief("coordinator-recovered"),
                },
            )?;
            actions.push("coordinator_recovered".into());
        } else {
            start_with_tab(ctx, &j, "up", rec.tab.clone())?;
            actions.push("coordinator_started".into());
        }
    }
    let rec = j.read_state()?.coordinator;
    let release = active_release(&ctx.root);
    if let Some(old) = rec.release.filter(|r| r != &release) {
        if rec.handoff.is_none() {
            let mut lock = j.lock()?;
            lock.append(
                &Actor::Cli,
                ctx.now(),
                vec![
                    (
                        None,
                        None,
                        Kind::Decision {
                            tier: Tier::Digest,
                            what: "handoff after update".into(),
                            why: format!("release {old} → {release}"),
                            qid: None,
                        },
                    ),
                    (
                        None,
                        None,
                        Kind::CoordHandoffRequested {
                            by: "cli".into(),
                            note: None,
                        },
                    ),
                ],
            )?;
        }
    }
    let status = status(ctx)?;
    Ok(json!({"state":status["state"],"actions":actions,"setup":setup(&cfg),"status":status}))
}
pub fn down(ctx: &Ctx) -> Result<Value> {
    let _guard = up_lock(ctx)?;
    let j = ctx.journal()?;
    config_autostart(ctx, &j, false)?;
    let state = j.read_state()?;
    if let Some(id) = state.coordinator.terminal {
        let _terminal = ctx.terminal_lock(&id)?;
        let mut lock = j.lock()?;
        if lock.state().coordinator.terminal.as_deref() == Some(id.as_str()) {
            lock.append(
                &Actor::Cli,
                ctx.now(),
                vec![(
                    None,
                    None,
                    Kind::CoordSessionEnded {
                        reason: "down".into(),
                        terminal: id.clone(),
                    },
                )],
            )?;
            drop(lock);
            stop_locked(ctx, &id, "coordinator")?;
        }
    }
    let watcher = read_heartbeat(&ctx.paths)
        .ok()
        .flatten()
        .and_then(|h| h.terminal)
        .or(state.watcher.terminal);
    if let Some(id) = watcher {
        stop(ctx, &id, "watcher")?;
        append(
            ctx,
            &j,
            Actor::Cli,
            Kind::WatcherStopped {
                reason: "down".into(),
            },
        )?;
    }
    Ok(json!({"state":"not_running"}))
}
pub fn handoff(ctx: &Ctx, c: &Caller, note: Option<&str>) -> Result<Value> {
    ensure!(
        matches!(c, Caller::Coordinator),
        "only the coordinator may request handoff"
    );
    append(
        ctx,
        &ctx.journal()?,
        c.actor(),
        Kind::CoordHandoffRequested {
            by: "coordinator".into(),
            note: note.map(str::to_owned),
        },
    )?;
    Ok(json!({"state":"requested"}))
}
pub fn restart(ctx: &Ctx, c: &Caller, force: bool) -> Result<Value> {
    ensure!(
        matches!(c, Caller::Person { .. }),
        "only the person may restart the coordinator"
    );
    let _guard = up_lock(ctx)?;
    let j = ctx.journal()?;
    if force {
        restart_coordinator(ctx, &j, "forced")?;
        Ok(json!({"state":"restarted"}))
    } else {
        append(
            ctx,
            &j,
            c.actor(),
            Kind::CoordHandoffRequested {
                by: "user".into(),
                note: None,
            },
        )?;
        Ok(json!({"state":"requested"}))
    }
}
pub fn status(ctx: &Ctx) -> Result<Value> {
    let cfg = Config::load(&ctx.paths)?;
    let state = if ctx.paths.journal.exists() {
        ctx.journal()?.read_state()?
    } else {
        Default::default()
    };
    let hb = read_heartbeat(&ctx.paths).ok().flatten();
    let now = ctx.now();
    let mut unreachable = false;
    let get = |id: Option<&str>, unreachable: &mut bool| -> Option<super::adapter::Term> {
        id.and_then(|id| match ctx.hc.get(id) {
            Ok(t) => t,
            Err(_) => {
                *unreachable = true;
                None
            }
        })
    };
    let wt = get(
        hb.as_ref()
            .and_then(|h| h.terminal.as_deref())
            .or(state.watcher.terminal.as_deref()),
        &mut unreachable,
    );
    let ct = get(state.coordinator.terminal.as_deref(), &mut unreachable);
    let wr = wt.as_ref().is_some_and(|t| {
        t.live() && managed(t, "watcher") && hb.as_ref().is_none_or(|h| t.pid == Some(h.pid))
    });
    let cr = ct
        .as_ref()
        .is_some_and(|t| same_coordinator(t, &state.coordinator));
    let mut degraded = Vec::new();
    if let Err(error) = crate::deployment::read(&ctx.root) {
        degraded.push(json!({"code":"coordinator_deployment_invalid","since_ms":null,"detail":error.to_string()}));
    } else if let Some(error) = hb.as_ref().and_then(|h| h.deployment_error.as_ref()) {
        degraded.push(json!({"code":"coordinator_deployment_invalid","since_ms":hb.as_ref().map(|h|h.beat_ms),"detail":error}));
    }
    if unreachable {
        degraded
            .push(json!({"code":"hc_unreachable","since_ms":null,"detail":"hc is unreachable"}));
    } else if wr
        || cr
        || state.coordinator.sessions_started > 0 && cfg.as_ref().is_some_and(|c| c.autostart)
    {
        if !wr {
            degraded.push(json!({"code":"watcher_missing","since_ms":state.watcher.stopped_ms,"detail":"watcher is not running"}));
        } else if !hb.as_ref().is_some_and(|h| fresh(h, now)) {
            degraded.push(json!({"code":"watcher_heartbeat_stale","since_ms":hb.as_ref().map(|h|h.beat_ms),"detail":"watcher heartbeat is stale"}));
        }
        if !cr {
            degraded.push(json!({"code":"coordinator_terminal_missing","since_ms":null,"detail":"coordinator terminal is missing"}));
        }
    }
    let word = ct.as_ref().and_then(|t| t.word());
    let counts = views::counts(&state, views::attention(&state, word).len());
    let status = if !degraded.is_empty() {
        "degraded"
    } else if wr && cr {
        "running"
    } else {
        "not_running"
    };
    Ok(
        json!({"schema_version":1,"generated_at_ms":now,"release":active_release(&ctx.root),"state":status,"enabled":cfg.is_some(),"autostart":cfg.as_ref().is_some_and(|c|c.autostart),"degraded":degraded,"watcher":{"running":wr,"terminal":wt.as_ref().map(|t|&t.address),"heartbeat_ms":hb.as_ref().map(|h|h.beat_ms),"release":hb.as_ref().map(|h|&h.release)},"coordinator":{"terminal":state.coordinator.terminal,"status":word,"space_id":state.coordinator.space,"tab_id":state.coordinator.tab,"session_started_ms":state.coordinator.started_ms,"restart_pending":state.coordinator.handoff.is_some()},"counts":counts,"journal_dir":ctx.paths.journal,"journal_seq":state.last_seq}),
    )
}

#[cfg(test)]
mod tests {
    use super::super::adapter::fake::{FakeApp, FakeHc};
    use super::*;

    fn context(root: &Path) -> Ctx {
        Ctx {
            root: root.into(),
            paths: CoordPaths::new(root),
            hc: Box::new(FakeHc::default()),
            app: Box::new(FakeApp::default()),
            clock: Box::new(|| 100_000),
            op: "testop".into(),
            cause: None,
        }
    }

    #[test]
    fn disabled_up_and_empty_status_have_no_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        assert_eq!(up(&ctx, true).unwrap(), json!({"state":"disabled"}));
        let s = status(&ctx).unwrap();
        assert_eq!(s["state"], "not_running");
        assert_eq!(s["enabled"], false);
        assert_eq!(s["schema_version"], 1);
        assert_eq!(s["journal_seq"], 0);
        assert_eq!(s["counts"]["needs_you"], 0);
        assert!(!ctx.paths.dir.exists());
    }

    #[test]
    fn heartbeat_and_config_events_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        let hb = Heartbeat {
            pid: 3,
            terminal: Some("t1".into()),
            release: "r".into(),
            started_ms: 1,
            beat_ms: 99_999,
            events_ok: true,
            deployment_error: None,
        };
        write_heartbeat(&ctx.paths, &hb).unwrap();
        assert!(fresh(
            &read_heartbeat(&ctx.paths).unwrap().unwrap(),
            ctx.now()
        ));
        let j = ctx.journal().unwrap();
        config_autostart(&ctx, &j, true).unwrap();
        config_autostart(&ctx, &j, false).unwrap();
        let events = j.read_after(0).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "config.changed");
        assert_eq!(events[0].data["old"], true);
        assert_eq!(events[0].data["new"], false);
    }

    #[test]
    fn restart_without_force_records_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path());
        restart(&ctx, &Caller::Person { terminal: None }, false).unwrap();
        let entries = ctx.journal().unwrap().read_after(0).unwrap();
        assert_eq!(entries[0].kind, "coord.handoff_requested");
        assert_eq!(entries[0].data["by"], "user");
    }

    #[test]
    fn claude_trust_is_read_only() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("worktrees");
        let file = home.path().join(".claude.json");
        fs::write(&file, "{}").unwrap();
        assert_eq!(
            setup_at(home.path(), &root)[0]["code"],
            "claude_worktree_trust"
        );
        let text=json!({"projects":{root.to_string_lossy().to_string():{"hasTrustDialogAccepted":true}}}).to_string();
        fs::write(&file, &text).unwrap();
        assert!(setup_at(home.path(), &root).is_empty());
        assert_eq!(fs::read_to_string(file).unwrap(), text);
    }
    #[test]
    fn coordinator_identity_detects_replacement() {
        let t = terminal("tc", "coordinator");
        let mut rec = super::super::state::CoordRec {
            pid: Some(1),
            created_ms: Some(t.created_ms),
            ..Default::default()
        };
        assert!(same_coordinator(&t, &rec));
        rec.pid = Some(2);
        assert!(!same_coordinator(&t, &rec));
        rec.pid = Some(1);
        rec.created_ms = Some(t.created_ms + 1);
        assert!(!same_coordinator(&t, &rec));
    }
    fn terminal(id: &str, role: &str) -> super::super::adapter::Term {
        let mut t: super::super::adapter::Term =
            serde_json::from_str(include_str!("testdata/hc-get-agent.json")).unwrap();
        t.address = id.into();
        t.state = "live".into();
        t.pid = Some(1);
        t.labels.insert("coord.managed".into(), "1".into());
        t.labels.insert("coord.role".into(), role.into());
        t
    }

    #[test]
    fn status_reports_health_and_degraded_codes() {
        for case in [
            "running",
            "stale",
            "watcher_missing",
            "coordinator_missing",
            "unreachable",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let hc = FakeHc::default();
            let w = terminal("tw", "watcher");
            let c = terminal("tc", "coordinator");
            match case {
                "watcher_missing" => hc.gets.borrow_mut().push_back(Ok(None)),
                "unreachable" => hc
                    .gets
                    .borrow_mut()
                    .push_back(Err(anyhow::anyhow!("offline"))),
                _ => hc.gets.borrow_mut().push_back(Ok(Some(w))),
            }
            hc.gets
                .borrow_mut()
                .push_back(Ok(if case == "coordinator_missing" {
                    None
                } else {
                    Some(c)
                }));
            let ctx = Ctx {
                root: dir.path().into(),
                paths: CoordPaths::new(dir.path()),
                hc: Box::new(hc),
                app: Box::new(FakeApp::default()),
                clock: Box::new(|| 100_000),
                op: "testop".into(),
                cause: None,
            };
            Config::default().save(&ctx.paths).unwrap();
            let j = ctx.journal().unwrap();
            append(
                &ctx,
                &j,
                Actor::Cli,
                Kind::CoordSessionStarted {
                    role: "coord-main".into(),
                    model: None,
                    effort: None,
                    pid: None,
                    created_ms: None,
                    terminal: "tc".into(),
                    space: "sp".into(),
                    tab: Some("tab".into()),
                    session: None,
                    release: crate::deployment::BUILD_REVISION.into(),
                    role_hash: "hash".into(),
                    reason: "up".into(),
                    brief: ctx.paths.brief("c"),
                },
            )
            .unwrap();
            write_heartbeat(
                &ctx.paths,
                &Heartbeat {
                    pid: 1,
                    terminal: Some("tw".into()),
                    release: crate::deployment::BUILD_REVISION.into(),
                    started_ms: 0,
                    beat_ms: if case == "stale" { 1 } else { 99_999 },
                    events_ok: true,
                    deployment_error: None,
                },
            )
            .unwrap();
            let result = status(&ctx).unwrap();
            if case == "running" {
                assert_eq!(result["state"], "running");
                assert_eq!(result["degraded"], json!([]));
            } else {
                let code = match case {
                    "stale" => "watcher_heartbeat_stale",
                    "watcher_missing" => "watcher_missing",
                    "coordinator_missing" => "coordinator_terminal_missing",
                    _ => "hc_unreachable",
                };
                assert_eq!(result["state"], "degraded");
                assert!(
                    result["degraded"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v["code"] == code)
                );
            }
            for key in [
                "schema_version",
                "generated_at_ms",
                "release",
                "state",
                "enabled",
                "autostart",
                "degraded",
                "watcher",
                "coordinator",
                "counts",
                "journal_dir",
                "journal_seq",
            ] {
                assert!(result.get(key).is_some(), "{key}");
            }
        }
    }
    #[derive(Default)]
    struct World {
        terms: std::collections::BTreeMap<String, super::super::adapter::Term>,
        specs: Vec<super::super::adapter::RunSpec>,
        calls: Vec<String>,
        reconcile_on_watcher_start: bool,
    }
    struct TestHc {
        world: std::rc::Rc<std::cell::RefCell<World>>,
        paths: CoordPaths,
        root: std::path::PathBuf,
    }
    impl super::super::adapter::Hc for TestHc {
        fn run(
            &self,
            spec: &super::super::adapter::RunSpec,
        ) -> Result<super::super::adapter::Term> {
            let id = format!("t{}", self.world.borrow().specs.len() + 1);
            let mut t = terminal(
                &id,
                &spec
                    .labels
                    .iter()
                    .find(|(k, _)| k == "coord.role")
                    .unwrap()
                    .1,
            );
            t.pid = Some(1);
            t.labels = spec.labels.iter().cloned().collect();
            t.labels
                .entry("herdr.tab".into())
                .or_insert_with(|| format!("tab-{id}"));
            self.world.borrow_mut().specs.push(spec.clone());
            self.world.borrow_mut().terms.insert(id.clone(), t.clone());
            if spec.name == "coord-watcher" {
                if self.world.borrow().reconcile_on_watcher_start {
                    let app = FakeApp::default();
                    app.snapshots
                        .borrow_mut()
                        .push_back(Ok(super::super::adapter::AppSnapshot { spaces: vec![] }));
                    let ctx = Ctx {
                        root: self.root.clone(),
                        paths: self.paths.clone(),
                        hc: Box::new(TestHc {
                            world: self.world.clone(),
                            paths: self.paths.clone(),
                            root: self.root.clone(),
                        }),
                        app: Box::new(app),
                        clock: Box::new(|| 500_000),
                        op: "race".into(),
                        cause: None,
                    };
                    super::super::reconcile::reconcile(
                        &ctx,
                        &ctx.journal()?,
                        &mut super::super::rules::Facts::default(),
                    )?;
                }
                write_heartbeat(
                    &self.paths,
                    &Heartbeat {
                        pid: 1,
                        terminal: Some(id),
                        release: active_release(&self.root),
                        started_ms: 99_999,
                        beat_ms: 99_999,
                        events_ok: true,
                        deployment_error: None,
                    },
                )?;
            }
            Ok(t)
        }
        fn get(&self, id: &str) -> Result<Option<super::super::adapter::Term>> {
            Ok(self.world.borrow().terms.get(id).cloned())
        }
        fn list(&self, _: bool, label: Option<(&str, &str)>) -> Result<Vec<String>> {
            Ok(self
                .world
                .borrow()
                .terms
                .values()
                .filter(|t| label.is_none_or(|(k, v)| t.label(k) == Some(v)))
                .map(|t| t.address.clone())
                .collect())
        }
        fn send(&self, _: &str, _: &str) -> Result<super::super::adapter::SendOutcome> {
            anyhow::bail!("unexpected send")
        }
        fn send_command(&self, _: &str, _: &str) -> std::process::Command {
            unreachable!("unexpected send command")
        }
        fn label(&self, _: &str, _: &[(&str, &str)]) -> Result<()> {
            anyhow::bail!("unexpected label")
        }
        fn kill(&self, id: &str) -> Result<()> {
            self.world.borrow_mut().calls.push(format!("kill {id}"));
            if let Some(t) = self.world.borrow_mut().terms.get_mut(id) {
                t.state = "finished".into();
            }
            Ok(())
        }
        fn rm(&self, id: &str) -> Result<()> {
            self.world.borrow_mut().calls.push(format!("rm {id}"));
            self.world.borrow_mut().terms.remove(id);
            Ok(())
        }
        fn events_command(&self) -> std::process::Command {
            std::process::Command::new("false")
        }
    }
    struct TestApp(std::rc::Rc<std::cell::RefCell<World>>);
    impl super::super::adapter::App for TestApp {
        fn ls(&self) -> Result<super::super::adapter::AppSnapshot> {
            use super::super::adapter::{AppSnapshot, AppSpace, AppTab, AppTerm};
            let w = self.0.borrow();
            let tabs = w
                .terms
                .values()
                .map(|t| AppTab {
                    id: t.label("herdr.tab").unwrap().into(),
                    name: String::new(),
                    terminals: vec![AppTerm {
                        terminal: t.address.clone(),
                        pane: "pane".into(),
                    }],
                })
                .collect();
            Ok(AppSnapshot {
                spaces: vec![AppSpace {
                    id: "space".into(),
                    name: String::new(),
                    folder: String::new(),
                    machine: None,
                    tabs,
                }],
            })
        }
        fn command(&self, name: &str, args: Value) -> Result<Value> {
            self.0.borrow_mut().calls.push(format!("{name} {args}"));
            Ok(json!({"ok":true}))
        }
    }
    #[test]
    fn up_repair_release_and_down_use_managed_terminals() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let paths = CoordPaths::new(root);
        paths.ensure().unwrap();
        fs::write(
            paths.roles.join("coord-main.md"),
            "---\nharness: omp\nstart: fake {brief}\nsession: none\n---\nrole\n",
        )
        .unwrap();
        let world = std::rc::Rc::new(std::cell::RefCell::new(World::default()));
        let ctx = Ctx {
            root: root.into(),
            paths: paths.clone(),
            hc: Box::new(TestHc {
                world: world.clone(),
                paths: paths.clone(),
                root: root.into(),
            }),
            app: Box::new(TestApp(world.clone())),
            clock: Box::new(|| 100_000),
            op: "testop".into(),
            cause: None,
        };
        assert_eq!(up(&ctx, false).unwrap()["state"], "running");
        {
            let w = world.borrow();
            assert_eq!(w.specs.len(), 2);
            assert_eq!(w.specs[0].name, "coord-watcher");
            assert_eq!(w.specs[1].name, "coordinator");
            assert!(w.specs[0].argv.ends_with(&["watch".into()]));
            assert!(w.calls.iter().any(|c| c.starts_with("tab.move")));
            assert!(w.calls.iter().any(|c| c.starts_with("terminal.pin")));
        }
        up(&ctx, false).unwrap();
        assert_eq!(world.borrow().specs.len(), 2);
        let mut hb = read_heartbeat(&paths).unwrap().unwrap();
        hb.beat_ms = 1;
        write_heartbeat(&paths, &hb).unwrap();
        up(&ctx, false).unwrap();
        {
            let w = world.borrow();
            assert_eq!(w.specs.len(), 3);
            assert!(
                w.specs[2]
                    .labels
                    .iter()
                    .any(|(k, v)| k == "herdr.tab" && v == "tab-t1")
            );
            assert!(w.calls.iter().any(|c| c.starts_with("tab.rename")));
        }
        let launched_before_suite_update = world.borrow().specs.len();
        fs::write(
            root.join("state.json"),
            r#"{"current":"unrelated-suite-update"}"#,
        )
        .unwrap();
        up(&ctx, false).unwrap();
        assert_eq!(world.borrow().specs.len(), launched_before_suite_update);
        let entries = ctx.journal().unwrap().read_after(0).unwrap();
        assert!(
            !entries
                .iter()
                .any(|entry| entry.kind == "coord.handoff_requested")
        );
        let before = world.borrow().calls.len();
        restart(&ctx, &Caller::Person { terminal: None }, true).unwrap();
        {
            let w = world.borrow();
            assert!(
                w.specs
                    .last()
                    .unwrap()
                    .labels
                    .iter()
                    .any(|(k, v)| k == "herdr.tab" && v == "tab-t2")
            );
            let calls = &w.calls[before..];
            assert!(calls.iter().any(|s| s.starts_with("tab.rename")));
            assert!(calls.iter().any(|s| s.starts_with("tab.move")));
            assert!(calls.iter().any(|s| s.starts_with("terminal.pin")));
        }
        let mut delegate = terminal("delegate", "delegate");
        delegate
            .labels
            .insert("herdr.tab".into(), "delegate-tab".into());
        world.borrow_mut().terms.insert("delegate".into(), delegate);
        down(&ctx).unwrap();
        assert!(!Config::load(&paths).unwrap().unwrap().autostart);
        assert_eq!(
            world.borrow().terms.keys().cloned().collect::<Vec<_>>(),
            vec!["delegate"]
        );
    }
    #[test]
    fn down_up_preserves_space_and_tabs_during_stale_snapshot_reconcile() {
        for previously_released in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let paths = CoordPaths::new(root);
            paths.ensure().unwrap();
            fs::write(
                paths.roles.join("coord-main.md"),
                "---\nharness: omp\nstart: fake {brief}\nsession: none\n---\nrole\n",
            )
            .unwrap();
            let world = std::rc::Rc::new(std::cell::RefCell::new(World::default()));
            let ctx = Ctx {
                root: root.into(),
                paths: paths.clone(),
                hc: Box::new(TestHc {
                    world: world.clone(),
                    paths: paths.clone(),
                    root: root.into(),
                }),
                app: Box::new(TestApp(world.clone())),
                clock: Box::new(|| 100_000),
                op: "testop".into(),
                cause: None,
            };
            up(&ctx, false).unwrap();
            let state = ctx.journal().unwrap().read_state().unwrap();
            let space = state.coordinator.space.unwrap();
            let coordinator_tab = state.coordinator.tab.unwrap();
            let watcher_tab = state
                .resources
                .iter()
                .find(|r| r.r.r#type == ResType::Tab && r.r.name == "t1")
                .unwrap()
                .r
                .tab
                .clone()
                .unwrap();
            down(&ctx).unwrap();
            if previously_released {
                ctx.journal()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .append(
                        &Actor::Watcher,
                        ctx.now(),
                        vec![(
                            None,
                            None,
                            Kind::ResReleased {
                                r#type: ResType::Space,
                                name: space.clone(),
                                note: Some("gone".into()),
                            },
                        )],
                    )
                    .unwrap();
            }
            world.borrow_mut().reconcile_on_watcher_start = true;
            up(&ctx, false).unwrap();
            let state = ctx.journal().unwrap().read_state().unwrap();
            assert_eq!(state.coordinator.space.as_deref(), Some(space.as_str()));
            assert!(
                state
                    .resources
                    .iter()
                    .any(|r| r.r.name == space && r.owned && !r.released)
            );
            let w = world.borrow();
            for (spec, tab) in [(&w.specs[2], &watcher_tab), (&w.specs[3], &coordinator_tab)] {
                assert!(
                    spec.labels
                        .iter()
                        .any(|(k, v)| k == "herdr.space" && v == &space)
                );
                assert!(
                    spec.labels
                        .iter()
                        .any(|(k, v)| k == "herdr.tab" && v == tab)
                );
            }
            assert!(
                w.calls
                    .iter()
                    .any(|c| c.starts_with("space.rename") && c.contains("coordinator"))
            );
        }
    }
    #[test]
    fn up_recovers_released_space_with_live_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let paths = CoordPaths::new(root);
        paths.ensure().unwrap();
        fs::write(
            paths.roles.join("coord-main.md"),
            "---\nharness: omp\nstart: fake {brief}\nsession: none\n---\nrole\n",
        )
        .unwrap();
        let world = std::rc::Rc::new(std::cell::RefCell::new(World::default()));
        let ctx = Ctx {
            root: root.into(),
            paths: paths.clone(),
            hc: Box::new(TestHc {
                world: world.clone(),
                paths: paths.clone(),
                root: root.into(),
            }),
            app: Box::new(TestApp(world.clone())),
            clock: Box::new(|| 100_000),
            op: "testop".into(),
            cause: None,
        };
        up(&ctx, false).unwrap();
        let state = ctx.journal().unwrap().read_state().unwrap();
        let space = state.coordinator.space.unwrap();
        world.borrow_mut().terms.remove("t2");
        ctx.journal()
            .unwrap()
            .lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                ctx.now(),
                vec![
                    (
                        None,
                        None,
                        Kind::CoordSessionEnded {
                            reason: "terminal_gone".into(),
                            terminal: "t2".into(),
                        },
                    ),
                    (
                        None,
                        None,
                        Kind::ResReleased {
                            r#type: ResType::Space,
                            name: space.clone(),
                            note: Some("gone".into()),
                        },
                    ),
                ],
            )
            .unwrap();
        up(&ctx, false).unwrap();
        let state = ctx.journal().unwrap().read_state().unwrap();
        assert!(
            state
                .resources
                .iter()
                .any(|r| r.r.name == space && r.owned && !r.released)
        );
        assert_eq!(world.borrow().specs.len(), 3);
    }
}
