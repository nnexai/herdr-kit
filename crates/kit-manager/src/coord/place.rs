use super::caller::Caller;
use super::ids::{TaskId, WsId};
use super::journal::Journal;
use super::kinds::{
    CorrectionWhat, Follows, Kind, Outcome, Rejected, Relation, ResType, ResourceRef, SendVia,
    SessionMode, SessionRef, TaskOutcome, Tier, WsOutcome,
};
use super::state::{State, TaskState, WsState};
use super::{Ctx, brief, cli, error, git, ids, launch, release, role};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

fn refuse(code: &'static str, msg: impl Into<String>) -> anyhow::Error {
    error::refused(code, msg)
}
fn resource(kind: ResType, name: String) -> ResourceRef {
    ResourceRef {
        r#type: kind,
        name,
        repo: None,
        folder: None,
        label: None,
        branch: None,
        base: None,
        base_sha: None,
        create_branch: None,
        space: None,
        tab: None,
    }
}
pub fn role_hash(role: &role::Role) -> Result<String> {
    let text = match &role.source {
        role::RoleSource::Builtin => role::builtin(&role.name)
            .context("built-in role missing")?
            .as_bytes()
            .to_vec(),
        role::RoleSource::File(path) => fs::read(path)?,
    };
    Ok(format!("{:x}", Sha256::digest(text))[..12].to_owned())
}
fn append(
    ctx: &Ctx,
    c: &Caller,
    ws: Option<WsId>,
    task: Option<TaskId>,
    kind: Kind,
) -> Result<Value> {
    let j = ctx.journal()?;
    let entries = j
        .lock()?
        .append(&c.actor(), ctx.now(), vec![(ws, task, kind)])?;
    Ok(json!({"seq":entries[0].seq}))
}
fn read_brief(path: &Path) -> Result<String> {
    let mut text = String::new();
    if path == Path::new("-") {
        io::stdin().take(1_048_577).read_to_string(&mut text)?;
    } else {
        fs::File::open(path)?
            .take(1_048_577)
            .read_to_string(&mut text)?;
    }
    anyhow::ensure!(text.len() <= 1_048_576, "brief exceeds 1 MiB");
    Ok(text)
}
pub(super) fn send_guard(term: &super::adapter::Term) -> Result<()> {
    if matches!(term.word(), Some("blocked" | "error")) {
        return Err(refuse("agent_blocked", "agent is blocked"));
    }
    if !term.sendable() {
        return Err(error::not_ready(
            "agent_not_ready",
            "agent has not completed a settled turn",
        ));
    }
    Ok(())
}
fn launch_failed(
    ctx: &Ctx,
    j: &Journal,
    c: &Caller,
    ws: &WsId,
    task: &TaskId,
    error: &anyhow::Error,
) -> Result<()> {
    let mut lock = j.lock()?;
    let mut entries = vec![(
        Some(ws.clone()),
        Some(task.clone()),
        Kind::TaskClosed {
            reason: format!("launch failed: {error}"),
            outcome: Some(TaskOutcome::Abandoned),
        },
    )];
    for intent in lock
        .state()
        .intents
        .values()
        .filter(|i| i.task.as_ref() == Some(task))
    {
        entries.push((
            Some(ws.clone()),
            Some(task.clone()),
            Kind::ResReleased {
                r#type: intent.r.r#type,
                name: intent.r.name.clone(),
                note: Some("never created".into()),
            },
        ));
    }
    lock.append(&c.actor(), ctx.now(), entries)?;
    Ok(())
}
pub fn ws_open(ctx: &Ctx, c: &Caller, a: &cli::WsOpen) -> Result<Value> {
    let repo = a.repo.as_deref().map(git::main_checkout).transpose()?;
    let branch = match (&repo, &a.delivery_branch) {
        (_, Some(b)) => Some(b.clone()),
        (Some(r), None) => git::current_branch(r)?,
        _ => None,
    };
    let j = ctx.journal()?;
    let mut lock = j.lock()?;
    let id = loop {
        let id = WsId::generate()?;
        if !lock.state().workstreams.contains_key(&id) {
            break id;
        }
    };
    lock.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(id.clone()),
            None,
            Kind::WsOpened {
                title: a.title.clone(),
                repo,
                delivery_target: Some(a.target.clone()),
                delivery_branch: branch,
                purpose: a.purpose.clone(),
                request: a.request.clone(),
            },
        )],
    )?;
    Ok(json!({"ws":id}))
}
pub fn ws_close(
    ctx: &Ctx,
    c: &Caller,
    ws: &WsId,
    summary: &str,
    outcome: WsOutcome,
) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let w = l
        .state()
        .workstreams
        .get(ws)
        .ok_or_else(|| refuse("unknown_ws", "unknown workstream"))?;
    if w.state != WsState::Open {
        return Err(refuse("ws_not_open", "workstream is not open"));
    }
    if w.tasks.iter().any(|t| {
        l.state()
            .tasks
            .get(t)
            .is_some_and(|t| matches!(t.state, TaskState::Placed | TaskState::Running))
    }) {
        return Err(refuse("tasks_running", "workstream has running tasks"));
    }
    let entries = l.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(ws.clone()),
            None,
            Kind::WsClosed {
                summary: summary.into(),
                outcome: Some(outcome),
            },
        )],
    )?;
    Ok(json!({"seq":entries[0].seq}))
}
pub fn ws_list(ctx: &Ctx, all: bool) -> Result<Value> {
    let state = ctx.journal()?.read_state()?;
    Ok(
        json!({"workstreams":state.workstreams.values().filter(|w| all || w.state != WsState::Cleaned).collect::<Vec<_>>()}),
    )
}

pub fn task_start(ctx: &Ctx, c: &Caller, a: &cli::TaskStart) -> Result<Value> {
    let ws = WsId::parse(&a.ws)?;
    if !(1..=5).contains(&a.rung) || a.reason.trim().is_empty() {
        return Err(refuse("rung_arguments", "rung and reason required"));
    }
    let mut rejected = Vec::new();
    for item in &a.rejected {
        let (n, reason) = item
            .split_once('=')
            .ok_or_else(|| refuse("ladder_reason_extra", "invalid rejection"))?;
        let n: u8 = n
            .parse()
            .map_err(|_| refuse("ladder_reason_extra", "invalid rejection rung"))?;
        if n == 0 || n >= a.rung || rejected.iter().any(|r: &Rejected| r.rung == n) {
            return Err(refuse("ladder_reason_extra", "unexpected rejection"));
        }
        if reason.trim().is_empty() {
            return Err(refuse("ladder_reason_missing", "empty rejection reason"));
        }
        rejected.push(Rejected {
            rung: n,
            reason: reason.into(),
        });
    }
    if rejected.len() != usize::from(a.rung - 1) {
        return Err(refuse(
            "ladder_reason_missing",
            "a reason for each lower rung is required",
        ));
    }
    rejected.sort_by_key(|r| r.rung);
    let valid = match a.rung {
        1 => {
            a.session_of.is_some()
                && a.space.is_none()
                && a.branch.is_none()
                && a.base.is_none()
                && a.folder.is_none()
        }
        2 => {
            a.session_of.is_none()
                && a.space.is_none()
                && a.branch.is_none()
                && a.base.is_none()
                && a.folder.is_none()
        }
        3 => {
            a.session_of.is_none()
                && a.space.is_some()
                && a.branch.is_none()
                && a.base.is_none()
                && a.folder.is_none()
        }
        4 => {
            a.session_of.is_none() && a.space.is_none() && a.branch.is_some() && a.folder.is_none()
        }
        5 => {
            a.session_of.is_none()
                && a.space.is_none()
                && a.branch.is_none()
                && a.base.is_none()
                && a.folder.as_ref().is_some_and(|p| p.is_absolute())
        }
        _ => false,
    };
    if !valid {
        return Err(refuse("rung_arguments", "invalid rung arguments"));
    }
    let snapshot = ctx.app.ls()?;
    let input = read_brief(&a.brief)?;
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let state = l.state();
    let w = state
        .workstreams
        .get(&ws)
        .ok_or_else(|| refuse("unknown_ws", "unknown workstream"))?;
    if w.state != WsState::Open {
        return Err(refuse("ws_not_open", "workstream is not open"));
    }
    if a.rung >= 4 && !w.tasks.is_empty() {
        return Err(refuse(
            "follow_up_cannot_create_space",
            "follow-up cannot create a space",
        ));
    }
    let previous = a
        .session_of
        .as_ref()
        .map(|s| TaskId::parse(s))
        .transpose()?
        .map(|id| {
            state
                .tasks
                .get(&id)
                .cloned()
                .ok_or_else(|| refuse("no_session", "unknown predecessor"))
        })
        .transpose()?;
    if previous
        .as_ref()
        .is_some_and(|p| p.ws != ws || p.session.as_ref().and_then(|s| s.id.as_ref()).is_none())
    {
        return Err(refuse("no_session", "predecessor has no session"));
    }
    if previous
        .as_ref()
        .is_some_and(|p| a.role.as_ref().is_some_and(|r| *r != p.role))
    {
        return Err(refuse(
            "role_mismatch",
            "resumed session must keep its role",
        ));
    }
    let role_name = a
        .role
        .as_deref()
        .or_else(|| previous.as_ref().map(|p| p.role.as_str()))
        .ok_or_else(|| refuse("role_invalid", "role required"))?;
    let role =
        role::load(&ctx.paths, role_name).map_err(|e| refuse("role_invalid", e.to_string()))?;
    let follows = match (&a.follows, a.relation) {
        (Some(task), Some(relation)) => {
            let task = TaskId::parse(task)?;
            if !state.tasks.contains_key(&task) {
                return Err(refuse("unknown_task", "followed task does not exist"));
            }
            Some(Follows {
                task,
                relation: match relation {
                    cli::RelationArg::FollowUp => Relation::FollowUp,
                    cli::RelationArg::Rework => Relation::Rework,
                    cli::RelationArg::Escalation => Relation::Escalation,
                },
            })
        }
        (None, None) => previous.as_ref().map(|p| Follows {
            task: p.id.clone(),
            relation: Relation::FollowUp,
        }),
        _ => {
            return Err(refuse(
                "rung_arguments",
                "follows and relation must be supplied together",
            ));
        }
    };
    let role_hash = role_hash(&role)?;
    let id = TaskId::new(
        &ws,
        w.tasks
            .iter()
            .map(TaskId::n)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .context("task id overflow")?,
    );
    let title = a.title.clone().unwrap_or_else(|| brief::title_of(&input));
    let mut live_previous = None;
    if let Some(p) = &previous {
        if let Some(t) = &p.terminal {
            if let Some(term) = ctx.hc.get(t)? {
                if term.live() && term.pid == p.pid {
                    send_guard(&term)?;
                    live_previous = Some(term);
                }
            }
        }
    }
    let (space, cwd, new_space, space_label) = match a.rung {
        1 => {
            let p = previous.as_ref().unwrap();
            (
                p.space
                    .clone()
                    .ok_or_else(|| refuse("no_space", "predecessor has no space"))?,
                p.cwd
                    .clone()
                    .ok_or_else(|| refuse("no_space", "predecessor has no cwd"))?,
                false,
                None,
            )
        }
        2 => (
            w.space
                .as_ref()
                .ok_or_else(|| refuse("no_space", "workstream has no space"))?
                .id
                .clone(),
            w.cwd
                .clone()
                .ok_or_else(|| refuse("no_space", "workstream has no cwd"))?,
            false,
            None,
        ),
        3 => {
            let s = snapshot
                .space(a.space.as_deref().unwrap())
                .ok_or_else(|| refuse("no_space", "space not found"))?;
            if state.coordinator.space.as_deref() == Some(&s.id) {
                return Err(refuse("coordinator_space", "cannot use coordinator space"));
            }
            (
                s.id.clone(),
                PathBuf::from(&s.folder),
                false,
                Some(s.name.clone()),
            )
        }
        4 => {
            let repo = w
                .repo
                .as_ref()
                .ok_or_else(|| refuse("rung_arguments", "worktree needs a repo"))?;
            (
                ids::space_id()?,
                git::worktree_path(
                    &ctx.config()?.worktree_root()?,
                    repo,
                    a.branch.as_deref().unwrap(),
                ),
                true,
                Some(format!(
                    "{} · {}",
                    repo.file_name().unwrap_or_default().to_string_lossy(),
                    w.title
                )),
            )
        }
        _ => (
            ids::space_id()?,
            a.folder.clone().unwrap(),
            true,
            Some(w.title.clone()),
        ),
    };
    let trusted = w.repo.as_ref().is_some_and(|repo| {
        snapshot.spaces.iter().any(|s| {
            let owned = state.resources.iter().any(|r| {
                r.r.r#type == ResType::Space && r.r.name == s.id && r.owned && !r.released
            });
            !owned && git::main_checkout(Path::new(&s.folder)).ok().as_ref() == Some(repo)
        }) || state.resources.iter().any(|r| {
            r.r.r#type == ResType::Space
                && !r.owned
                && r.r
                    .folder
                    .as_deref()
                    .and_then(|p| git::main_checkout(p).ok())
                    .as_ref()
                    == Some(repo)
        })
    });
    let repo = w.repo.clone();
    let delivery_branch = w.delivery_branch.clone();
    let session = if let Some(p) = &previous {
        p.session.clone()
    } else {
        let dir = if role.session == SessionMode::Dir {
            Some(ctx.paths.session_dir(id.as_str()))
        } else {
            None
        };
        if let Some(dir) = &dir {
            fs::create_dir_all(dir)?;
        }
        Some(SessionRef {
            mode: role.session,
            id: if role.session == SessionMode::Preset {
                Some(ids::uuid_v4()?)
            } else {
                None
            },
            dir,
        })
    };
    let message = if previous.is_some() {
        brief::follow_up(&input, &ctx.cli(), &ctx.paths.result(&id))
    } else {
        brief::delegate_first(&role, &input, &ctx.cli(), &ctx.paths.result(&id))
    };
    let brief_path = ctx.paths.brief(id.as_str());
    fs::write(&brief_path, &message)?;
    let argv_message = brief::for_argv(message.clone(), &brief_path);
    let vars = role::Vars {
        brief: &argv_message,
        session_dir: session.as_ref().and_then(|s| s.dir.as_deref()),
        session_id: session.as_ref().and_then(|s| s.id.as_deref()),
        repo_root: repo.as_deref(),
        cwd: &cwd,
    };
    let template = if previous.is_some() && live_previous.is_none() {
        role.resume
            .as_ref()
            .ok_or_else(|| refuse("no_session", "role cannot resume"))?
    } else {
        &role.start
    };
    if live_previous.is_none() && !template.iter().any(|arg| arg.contains("{brief}")) {
        return Err(refuse("role_invalid", "role command must include {brief}"));
    }
    let argv = role::expand(template, &vars, trusted.then_some(role.trust.as_slice()))?;
    let mut items = vec![(
        Some(ws.clone()),
        Some(id.clone()),
        Kind::TaskPlaced {
            rung: a.rung,
            reason: a.reason.clone(),
            rejected,
            role: role.name.clone(),
            harness: role.harness.clone(),
            model: role::model_of(&argv),
            title: title.clone(),
            brief: brief_path.clone(),
            session_of: previous.as_ref().map(|p| p.id.clone()),
            session: session.clone(),
            request: a.request.clone(),
            follows,
            role_hash,
            brief_bytes: input.len() as u64,
        },
    )];
    let mut intents = Vec::new();
    if a.rung == 3
        && !state.resources.iter().any(|r| {
            r.r.r#type == ResType::Space
                && r.r.name == space
                && r.ws.as_ref() == Some(&ws)
                && !r.released
        })
    {
        let mut r = resource(ResType::Space, space.clone());
        r.folder = Some(cwd.clone());
        r.label = space_label.clone();
        items.push((Some(ws.clone()), Some(id.clone()), Kind::ResAdopted(r)));
    }
    if a.rung == 4 {
        let mut r = resource(ResType::Worktree, cwd.to_string_lossy().into_owned());
        r.repo = repo.clone();
        r.branch = a.branch.clone();
        r.base = a.base.clone();
        r.create_branch = Some(!git::branch_exists(
            repo.as_deref().unwrap(),
            a.branch.as_deref().unwrap(),
        )?);
        let branch_intent = if r.create_branch == Some(true) {
            let sha = git::rev(
                repo.as_deref().unwrap(),
                a.base
                    .as_deref()
                    .or(delivery_branch.as_deref())
                    .unwrap_or("HEAD"),
            )?;
            let mut b = resource(ResType::Branch, a.branch.clone().unwrap());
            b.repo = repo.clone();
            b.base_sha = Some(sha);
            Some(b)
        } else {
            None
        };
        intents.push(r);
        if let Some(branch) = branch_intent {
            intents.push(branch);
        }
    }
    if new_space {
        let mut r = resource(ResType::Space, space.clone());
        r.folder = Some(cwd.clone());
        r.label = space_label.clone();
        intents.push(r);
    }
    if live_previous.is_none() {
        let mut r = resource(ResType::Tab, id.as_str().into());
        r.space = Some(space.clone());
        intents.push(r);
    }
    for r in &intents {
        items.push((
            Some(ws.clone()),
            Some(id.clone()),
            Kind::ResIntent(r.clone()),
        ));
    }
    if let (Some(term), Some(p)) = (&live_previous, &previous) {
        if !term.managed()
            || term.label("coord.task") != Some(p.id.as_str())
            || !state.resources.iter().any(|r| {
                r.owned
                    && !r.released
                    && r.r.r#type == ResType::Tab
                    && r.r.name == term.address
                    && r.task.as_ref() == Some(&p.id)
            })
        {
            return Err(refuse(
                "terminal_not_owned",
                "predecessor terminal is not owned",
            ));
        }
    }
    let _handover_guard = live_previous
        .as_ref()
        .map(|term| ctx.terminal_lock(&term.address))
        .transpose()?;
    l.append(&c.actor(), ctx.now(), items)?;
    if let Some(term) = live_previous {
        let p = previous.as_ref().unwrap();
        l.append(
            &c.actor(),
            ctx.now(),
            vec![
                (
                    Some(ws.clone()),
                    Some(p.id.clone()),
                    Kind::TaskClosed {
                        reason: format!("superseded by {}", id.as_str()),
                        outcome: Some(TaskOutcome::Superseded),
                    },
                ),
                (
                    Some(ws.clone()),
                    Some(id.clone()),
                    Kind::TaskStarted {
                        terminal: term.address.clone(),
                        tab: p.tab.clone(),
                        tab_name: title.clone(),
                        space: space.clone(),
                        cwd: cwd.clone(),
                        pid: term.pid,
                        created_ms: term.created_ms,
                        session: session.clone(),
                        resumed: true,
                    },
                ),
                (
                    Some(ws.clone()),
                    Some(id.clone()),
                    Kind::TaskSent {
                        message: brief_path,
                        via: SendVia::Send,
                        terminal: term.address.clone(),
                        started: None,
                    },
                ),
            ],
        )?;
        drop(l);
        let action = ctx
            .hc
            .label(
                &term.address,
                &[("coord.task", id.as_str()), ("coord.ws", ws.as_str())],
            )
            .and_then(|()| ctx.hc.send(&term.address, &message).map(|_| ()));
        if let Err(e) = action {
            append(
                ctx,
                c,
                Some(ws.clone()),
                Some(id.clone()),
                Kind::TaskAttention {
                    state: super::kinds::AttentionState::Error,
                    msg: Some(format!("handover failed: {e}")),
                },
            )?;
            return Err(e);
        }
        if let Some(tab) = &p.tab {
            ctx.app
                .command("tab.rename", json!({"id":tab,"name":title}))?;
        }
        return Ok(
            json!({"task":id,"state":"running","terminal":term.address,"space":space,"tab":p.tab,"session":session.and_then(|s| s.id)}),
        );
    }
    drop(l);
    let create_result: Result<()> = (|| {
        if a.rung == 4 {
            let r = &intents[0];
            let branch_intent = intents.iter().find(|r| r.r#type == ResType::Branch);
            let base = branch_intent.and_then(|r| r.base_sha.as_deref());
            if let Some(parent) = cwd.parent() {
                fs::create_dir_all(parent)?;
            }
            git::worktree_add(
                repo.as_deref().unwrap(),
                &cwd,
                a.branch.as_deref().unwrap(),
                base,
            )?;
            let mut entries = vec![(
                Some(ws.clone()),
                Some(id.clone()),
                Kind::ResCreated(r.clone()),
            )];
            if let Some(intent) = branch_intent {
                let mut branch = intent.clone();
                branch.base = branch.base_sha.clone();
                entries.push((Some(ws.clone()), Some(id.clone()), Kind::ResCreated(branch)));
            }
            j.lock()?.append(&c.actor(), ctx.now(), entries)?;
        } else if a.rung == 5 {
            fs::create_dir_all(&cwd)?;
        }
        Ok(())
    })();
    if let Err(e) = create_result {
        // A failed worktree operation can still have created a checkout. Leave
        // the intent for reconciliation unless git proves the path absent.
        let safely_absent = a.rung != 4
            || repo
                .as_deref()
                .and_then(|r| git::worktree_list(r).ok())
                .is_some_and(|trees| !trees.iter().any(|t| t.path == cwd));
        if safely_absent {
            launch_failed(ctx, &j, c, &ws, &id, &e)?;
        }
        return Err(e);
    }
    let plan = launch::LaunchPlan {
        name: id.as_str().into(),
        role_label: "delegate",
        ws: Some(ws.clone()),
        task: Some(id.clone()),
        space: space.clone(),
        tab: None,
        cwd: cwd.clone(),
        argv,
        session,
        tab_name: title.clone(),
        space_name: new_space.then_some(space_label.unwrap_or_default()),
    };
    let launched = match launch::launch_with_started(ctx, &plan, |term| {
        let mut entries = Vec::new();
        if let Some(p) = &previous {
            if p.state != TaskState::Closed {
                entries.push((
                    Some(ws.clone()),
                    Some(p.id.clone()),
                    Kind::TaskClosed {
                        reason: format!("superseded by {}", id.as_str()),
                        outcome: Some(TaskOutcome::Superseded),
                    },
                ));
            }
        }
        if new_space {
            let r = intents
                .iter()
                .find(|r| r.r#type == ResType::Space)
                .unwrap()
                .clone();
            entries.push((Some(ws.clone()), Some(id.clone()), Kind::ResCreated(r)));
        }
        let mut tab = resource(ResType::Tab, term.address.clone());
        tab.space = Some(space.clone());
        entries.push((Some(ws.clone()), Some(id.clone()), Kind::ResCreated(tab)));
        entries.push((
            Some(ws.clone()),
            Some(id.clone()),
            Kind::TaskStarted {
                terminal: term.address.clone(),
                tab: None,
                tab_name: title.clone(),
                space: space.clone(),
                cwd: cwd.clone(),
                pid: term.pid,
                created_ms: term.created_ms,
                session: plan.session.clone(),
                resumed: previous.is_some(),
            },
        ));
        j.lock()?.append(&c.actor(), ctx.now(), entries)?;
        Ok(())
    }) {
        Ok(v) => v,
        Err(e) => {
            if e.downcast_ref::<launch::PostLaunchError>().is_none() {
                launch_failed(ctx, &j, c, &ws, &id, &e)?;
            }
            return Err(e);
        }
    };
    j.lock()?.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(ws.clone()),
            Some(id.clone()),
            Kind::TaskLaunchDetails {
                terminal: launched.term.address.clone(),
                tab: launched.tab.clone(),
                session: launched.session.clone(),
            },
        )],
    )?;
    Ok(
        json!({"task":id,"state":"running","terminal":launched.term.address,"space":space,"tab":launched.tab,"session":launched.session.and_then(|s| s.id)}),
    )
}

pub fn task_send(ctx: &Ctx, c: &Caller, task: &TaskId, text: &str) -> Result<Value> {
    let j = ctx.journal()?;
    let state = j.read_state()?;
    let t = state
        .tasks
        .get(task)
        .ok_or_else(|| refuse("unknown_task", "unknown task"))?;
    if t.state == TaskState::Closed {
        return Err(refuse("task_closed", "task is closed"));
    }
    let message = brief::follow_up(text, &ctx.cli(), &ctx.paths.result(task));
    let path = ctx.paths.brief(&format!(
        "{}-followup-{}",
        task.as_str(),
        state.last_seq + 1
    ));
    let live = if let Some(addr) = &t.terminal {
        ctx.hc.get(addr)?.filter(|v| v.live() && v.pid == t.pid)
    } else {
        None
    };
    if let Some(term) = live {
        let _terminal_guard = ctx.terminal_lock(&term.address)?;
        send_guard(&term)?;
        fs::write(&path, &message)?;
        let mut l = j.lock()?;
        let current = l
            .state()
            .tasks
            .get(task)
            .ok_or_else(|| refuse("unknown_task", "task disappeared"))?;
        if current.last_activity_seq != t.last_activity_seq
            || current.terminal != t.terminal
            || current.state != t.state
        {
            return Err(error::not_ready(
                "agent_not_ready",
                "task changed before send",
            ));
        }
        if !term.managed()
            || term.label("coord.task") != Some(task.as_str())
            || !l.state().resources.iter().any(|r| {
                r.owned
                    && !r.released
                    && r.r.r#type == ResType::Tab
                    && r.r.name == term.address
                    && r.task.as_ref() == Some(task)
            })
        {
            return Err(refuse("terminal_not_owned", "task terminal is not owned"));
        }
        l.append(
            &c.actor(),
            ctx.now(),
            vec![(
                Some(t.ws.clone()),
                Some(task.clone()),
                Kind::TaskSent {
                    message: path,
                    via: SendVia::Send,
                    terminal: term.address.clone(),
                    started: None,
                },
            )],
        )?;
        drop(l);
        let sent_result = ctx.hc.send(&term.address, &message);
        let sent = match sent_result {
            Ok(sent) => sent,
            Err(e) => {
                append(
                    ctx,
                    c,
                    Some(t.ws.clone()),
                    Some(task.clone()),
                    Kind::TaskAttention {
                        state: super::kinds::AttentionState::Error,
                        msg: Some(format!("send failed: {e}")),
                    },
                )?;
                return Err(e);
            }
        };
        return Ok(json!({"task":task,"sent":true,"turn":sent.turn,"started":sent.started}));
    }
    let session = t
        .session
        .clone()
        .filter(|s| s.id.is_some())
        .ok_or_else(|| refuse("no_session", "task has no resumable session"))?;
    let role = role::load(&ctx.paths, &t.role)?;
    let cwd = t
        .cwd
        .clone()
        .ok_or_else(|| refuse("no_space", "task has no cwd"))?;
    let space = t
        .space
        .clone()
        .ok_or_else(|| refuse("no_space", "task has no space"))?;
    let repo = state.workstreams.get(&t.ws).and_then(|w| w.repo.as_deref());
    let snapshot = ctx.app.ls()?;
    let trusted =
        repo.is_some_and(|repo| {
            snapshot.spaces.iter().any(|s| {
                !state.resources.iter().any(|r| {
                    r.owned && !r.released && r.r.r#type == ResType::Space && r.r.name == s.id
                }) && git::main_checkout(Path::new(&s.folder)).ok().as_deref() == Some(repo)
            }) || state.resources.iter().any(|r| {
                !r.owned
                    && r.r.r#type == ResType::Space
                    && r.r
                        .folder
                        .as_deref()
                        .and_then(|p| git::main_checkout(p).ok())
                        .as_deref()
                        == Some(repo)
            })
        });
    let arg = brief::for_argv(message.clone(), &path);
    let vars = role::Vars {
        brief: &arg,
        session_dir: session.dir.as_deref(),
        session_id: session.id.as_deref(),
        repo_root: repo,
        cwd: &cwd,
    };
    let template = role
        .resume
        .as_ref()
        .ok_or_else(|| refuse("no_session", "role cannot resume"))?;
    if !template.iter().any(|arg| arg.contains("{brief}")) {
        return Err(refuse(
            "role_invalid",
            "role resume command must include {brief}",
        ));
    }
    let argv = role::expand(template, &vars, trusted.then_some(role.trust.as_slice()))?;
    fs::write(&path, &message)?;
    let mut intent = resource(ResType::Tab, task.as_str().into());
    intent.space = Some(space.clone());
    j.lock()?.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(t.ws.clone()),
            Some(task.clone()),
            Kind::ResIntent(intent.clone()),
        )],
    )?;
    let plan = launch::LaunchPlan {
        name: task.as_str().into(),
        role_label: "delegate",
        ws: Some(t.ws.clone()),
        task: Some(task.clone()),
        space: space.clone(),
        tab: None,
        cwd: cwd.clone(),
        argv,
        session: Some(session),
        tab_name: t.title.clone(),
        space_name: None,
    };
    let launched = match launch::launch_with_started(ctx, &plan, |term| {
        let mut tab = resource(ResType::Tab, term.address.clone());
        tab.space = Some(space.clone());
        let mut entries = Vec::new();
        if t.state != TaskState::Dormant && t.state != TaskState::Reported {
            entries.push((
                Some(t.ws.clone()),
                Some(task.clone()),
                Kind::TaskDormant {
                    reason: "terminal_missing".into(),
                },
            ));
        }
        entries.extend([
            (
                Some(t.ws.clone()),
                Some(task.clone()),
                Kind::ResCreated(tab),
            ),
            (
                Some(t.ws.clone()),
                Some(task.clone()),
                Kind::TaskStarted {
                    terminal: term.address.clone(),
                    tab: None,
                    tab_name: t.title.clone(),
                    space: space.clone(),
                    cwd: cwd.clone(),
                    pid: term.pid,
                    created_ms: term.created_ms,
                    session: plan.session.clone(),
                    resumed: true,
                },
            ),
            (
                Some(t.ws.clone()),
                Some(task.clone()),
                Kind::TaskSent {
                    message: path.clone(),
                    via: SendVia::Resume,
                    terminal: term.address.clone(),
                    started: None,
                },
            ),
        ]);
        j.lock()?.append(&c.actor(), ctx.now(), entries)?;
        Ok(())
    }) {
        Ok(v) => v,
        Err(e) => {
            if e.downcast_ref::<launch::PostLaunchError>().is_none() {
                j.lock()?.append(
                    &c.actor(),
                    ctx.now(),
                    vec![(
                        Some(t.ws.clone()),
                        Some(task.clone()),
                        Kind::ResReleased {
                            r#type: ResType::Tab,
                            name: intent.name,
                            note: Some("never created".into()),
                        },
                    )],
                )?;
            }
            return Err(e);
        }
    };
    j.lock()?.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(t.ws.clone()),
            Some(task.clone()),
            Kind::TaskLaunchDetails {
                terminal: launched.term.address.clone(),
                tab: launched.tab.clone(),
                session: launched.session.clone(),
            },
        )],
    )?;
    Ok(
        json!({"task":task,"state":"running","terminal":launched.term.address,"space":space,"tab":launched.tab,"session":launched.session.and_then(|s|s.id)}),
    )
}
pub fn task_close(
    ctx: &Ctx,
    c: &Caller,
    task: &TaskId,
    reason: &str,
    outcome: Option<TaskOutcome>,
) -> Result<Value> {
    if matches!(c, Caller::Coordinator { .. }) && outcome.is_none() {
        return Err(refuse(
            "outcome_required",
            "coordinator must specify an outcome",
        ));
    }
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let t = l
        .state()
        .tasks
        .get(task)
        .ok_or_else(|| refuse("unknown_task", "unknown task"))?;
    if t.state == TaskState::Closed {
        return Err(refuse("task_closed", "task already closed"));
    }
    let ws = t.ws.clone();
    let entries = l.append(
        &c.actor(),
        ctx.now(),
        vec![(
            Some(ws),
            Some(task.clone()),
            Kind::TaskClosed {
                reason: reason.into(),
                outcome,
            },
        )],
    )?;
    drop(l);
    let checks = release::release_task_tab(ctx, &j, &c.actor(), task)?;
    Ok(json!({"seq":entries[0].seq,"checks":checks}))
}
fn delegate<'a>(state: &'a State, c: &Caller) -> Result<&'a super::state::Task> {
    let Caller::Delegate { task, ws, terminal } = c else {
        return Err(refuse("not_a_delegate", "only delegates can report"));
    };
    let t = state
        .tasks
        .get(task)
        .ok_or_else(|| refuse("unknown_task", "unknown task"))?;
    if &t.ws != ws || t.terminal.as_deref() != Some(terminal) || t.state == TaskState::Closed {
        return Err(refuse(
            "not_a_delegate",
            "delegate does not own an active task",
        ));
    }
    Ok(t)
}
pub fn report(
    ctx: &Ctx,
    c: &Caller,
    outcome: cli::ReportKind,
    summary: &str,
    result: Option<&Path>,
) -> Result<Value> {
    let result = result
        .map(|p| -> Result<PathBuf> {
            let p = fs::canonicalize(p)?;
            anyhow::ensure!(p.is_file(), "result must be a regular file");
            Ok(p)
        })
        .transpose()?;
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let t = delegate(l.state(), c)?;
    let ws = t.ws.clone();
    let id = t.id.clone();
    let kind = match outcome {
        cli::ReportKind::Progress => Kind::TaskProgress {
            summary: summary.into(),
        },
        _ => Kind::TaskReported {
            outcome: match outcome {
                cli::ReportKind::Done => Outcome::Done,
                cli::ReportKind::Blocked => Outcome::Blocked,
                cli::ReportKind::Question => Outcome::Question,
                cli::ReportKind::Progress => unreachable!(),
            },
            summary: summary.into(),
            result,
        },
    };
    let entries = l.append(&c.actor(), ctx.now(), vec![(Some(ws), Some(id), kind)])?;
    Ok(json!({"seq":entries[0].seq}))
}
pub fn note(ctx: &Ctx, c: &Caller, text: &str) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let t = delegate(l.state(), c)?;
    let ws = t.ws.clone();
    let id = t.id.clone();
    let entries = l.append(
        &c.actor(),
        ctx.now(),
        vec![(Some(ws), Some(id), Kind::TaskNote { text: text.into() })],
    )?;
    Ok(json!({"seq":entries[0].seq}))
}

fn scope(ws: &Option<String>, task: &Option<String>) -> Result<(Option<WsId>, Option<TaskId>)> {
    let task = task.as_deref().map(TaskId::parse).transpose()?;
    let ws = ws
        .as_deref()
        .map(WsId::parse)
        .transpose()?
        .or_else(|| task.as_ref().map(TaskId::ws));
    if let (Some(w), Some(t)) = (&ws, &task) {
        if *w != t.ws() {
            return Err(refuse(
                "invalid_scope",
                "task does not belong to workstream",
            ));
        }
    }
    Ok((ws, task))
}
pub fn decide(ctx: &Ctx, c: &Caller, a: &cli::Decide) -> Result<Value> {
    let (ws, task) = scope(&a.ws, &a.task)?;
    let tier = match a.tier {
        cli::DecisionTier::Silent => Tier::Silent,
        cli::DecisionTier::Digest => Tier::Digest,
        cli::DecisionTier::Ask => Tier::Ask,
    };
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let qid = (tier == Tier::Ask).then(|| format!("q{}", l.state().counters.question));
    let mut entries = vec![(
        ws.clone(),
        task.clone(),
        Kind::Decision {
            what: a.what.clone(),
            why: a.why.clone(),
            tier,
            qid: qid.clone(),
        },
    )];
    if let Some(qid) = &qid {
        entries.push((
            ws,
            task,
            Kind::QuestionAsked {
                qid: qid.clone(),
                text: a.what.clone(),
                options: (!a.options.is_empty()).then(|| a.options.clone()),
            },
        ));
    }
    let entries = l.append(&c.actor(), ctx.now(), entries)?;
    Ok(json!({"seq":entries[0].seq,"qid":qid}))
}
pub(super) fn answer_entry(state: &State, qid: &str, text: &str) -> Result<(Option<WsId>, Option<TaskId>, Kind)> {
    let q = state.questions.get(qid).ok_or_else(|| refuse("unknown_question", "unknown question"))?;
    if q.answered.is_some() {
        return Err(refuse("question_answered", "question already answered"));
    }
    Ok((q.ws.clone(), q.task.clone(), Kind::QuestionAnswered { qid: qid.to_owned(), text: text.to_owned() }))
}

pub fn ask(ctx: &Ctx, c: &Caller, a: &cli::Ask) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    if let Some(qid) = &a.answer {
        let item = answer_entry(l.state(), qid, &a.text)?;
        let e = l.append(&c.actor(), ctx.now(), vec![item])?;
        return Ok(json!({"seq":e[0].seq,"qid":qid}));
    }
    let (ws, task) = scope(&a.ws, &a.task)?;
    let qid = format!("q{}", l.state().counters.question);
    let e = l.append(
        &c.actor(),
        ctx.now(),
        vec![(
            ws,
            task,
            Kind::QuestionAsked {
                qid: qid.clone(),
                text: a.text.clone(),
                options: (!a.options.is_empty()).then(|| a.options.clone()),
            },
        )],
    )?;
    Ok(json!({"seq":e[0].seq,"qid":qid}))
}
pub fn instruct(ctx: &Ctx, c: &Caller, a: &cli::Instruct) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let id = a
        .remove
        .clone()
        .or_else(|| a.replace.clone())
        .unwrap_or_else(|| format!("i{}", l.state().counters.instruction));
    if (a.remove.is_some() || a.replace.is_some()) && !l.state().instructions.contains_key(&id) {
        return Err(refuse("unknown_instruction", "unknown instruction"));
    }
    let e = l.append(
        &c.actor(),
        ctx.now(),
        vec![(
            None,
            None,
            Kind::InstructionSet {
                id,
                text: a.text.clone(),
            },
        )],
    )?;
    Ok(json!({"seq":e[0].seq}))
}
pub fn thread(ctx: &Ctx, c: &Caller, a: &cli::ThreadCmd) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let kind = match a {
        cli::ThreadCmd::Open { title, note } => Kind::ThreadOpened {
            id: format!("th{}", l.state().counters.thread),
            title: title.clone(),
            note: note.clone(),
        },
        cli::ThreadCmd::Close { id, note } => {
            let t = l
                .state()
                .threads
                .get(id)
                .ok_or_else(|| refuse("unknown_thread", "unknown thread"))?;
            Kind::ThreadClosed {
                id: id.clone(),
                title: t.title.clone(),
                note: note.clone(),
            }
        }
    };
    let e = l.append(&c.actor(), ctx.now(), vec![(None, None, kind)])?;
    Ok(json!({"seq":e[0].seq}))
}
pub fn correct(ctx: &Ctx, c: &Caller, a: &cli::Correct) -> Result<Value> {
    let what = match a.what {
        cli::CorrectionKind::Placement => CorrectionWhat::Placement,
        cli::CorrectionKind::Model => CorrectionWhat::Model,
        cli::CorrectionKind::Cleanup => CorrectionWhat::Cleanup,
        cli::CorrectionKind::Other => CorrectionWhat::Other,
    };
    let (ws, task) = if let Ok(task) = TaskId::parse(&a.reference) {
        (Some(task.ws()), Some(task))
    } else {
        (Some(WsId::parse(&a.reference)?), None)
    };
    append(
        ctx,
        c,
        ws,
        task,
        Kind::Correction {
            what,
            why: a.why.clone(),
            r#ref: a.reference.clone(),
        },
    )
}
pub fn scout(ctx: &Ctx, c: &Caller, a: &cli::Scout) -> Result<Value> {
    append(
        ctx,
        c,
        a.ws.as_deref().map(WsId::parse).transpose()?,
        None,
        Kind::ScoutResult {
            question: a.question.clone(),
            summary: a.summary.clone(),
        },
    )
}
pub fn inbox_ack(ctx: &Ctx, c: &Caller, seq: u64) -> Result<Value> {
    let j = ctx.journal()?;
    let mut l = j.lock()?;
    let state = l.state();
    if seq < state.cursor || seq > state.last_seq {
        return Err(refuse("invalid_cursor", "cursor outside journal"));
    }
    let count = state.waking.range(..=seq).count() as u64;
    l.append(
        &c.actor(),
        ctx.now(),
        vec![(None, None, Kind::InboxDrained { cursor: seq, count })],
    )?;
    Ok(json!({"cursor":seq,"count":count}))
}

#[cfg(test)]
mod tests {
    use super::super::adapter::{
        AgentStatus, Term,
        fake::{FakeApp, FakeHc},
    };
    use super::*;
    fn setup(root: &Path) -> Ctx {
        Ctx {
            root: root.into(),
            paths: super::super::paths::CoordPaths::new(root),
            hc: Box::new(FakeHc::default()),
            app: Box::new(FakeApp::default()),
            clock: Box::new(|| 1000),
            op: "test".into(),
            cause: None,
        }
    }
    fn person() -> Caller {
        Caller::Person { terminal: None }
    }
    #[test]
    fn send_readiness_refusals() {
        let mut t: Term = serde_json::from_str(include_str!(
            "../../../../docs/coordinator/examples/captures/hc-get-agent.json"
        ))
        .unwrap();
        t.state = "live".into();
        t.reported = true;
        t.turn = 0;
        t.status = Some(AgentStatus {
            state: "idle".into(),
            app: None,
            kind: None,
            msg: None,
        });
        assert_eq!(
            error::exit_of(&send_guard(&t).unwrap_err()),
            ("agent_not_ready", 4)
        );
        t.turn = 1;
        t.status.as_mut().unwrap().state = "blocked".into();
        assert_eq!(
            error::exit_of(&send_guard(&t).unwrap_err()),
            ("agent_blocked", 3)
        );
        t.status.as_mut().unwrap().state = "done".into();
        assert!(send_guard(&t).is_ok());
    }
    #[test]
    fn ladder_and_non_delegate_refuse_without_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = setup(tmp.path());
        let a = cli::TaskStart {
            ws: "ws-abcdef".into(),
            rung: 3,
            reason: "reason".into(),
            brief: "unused".into(),
            role: Some("coord-light".into()),
            rejected: vec!["1=no".into()],
            title: None,
            session_of: None,
            space: Some("sp".into()),
            branch: None,
            base: None,
            folder: None,
            request: None,
            follows: None,
            relation: None,
        };
        assert_eq!(
            error::exit_of(&task_start(&ctx, &person(), &a).unwrap_err()),
            ("ladder_reason_missing", 3)
        );
        assert_eq!(
            error::exit_of(
                &report(&ctx, &person(), cli::ReportKind::Done, "done", None).unwrap_err()
            ),
            ("not_a_delegate", 3)
        );
        assert_eq!(Journal::open(&ctx.paths).unwrap().last_seq().unwrap(), 0);
    }
    #[test]
    fn workstream_close_refuses_placed_task() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = setup(tmp.path());
        let c = person();
        let opened = ws_open(
            &ctx,
            &c,
            &cli::WsOpen {
                title: "x".into(),
                target: "x".into(),
                repo: None,
                delivery_branch: None,
                purpose: None,
                request: None,
            },
        )
        .unwrap();
        let ws = WsId::parse(opened["ws"].as_str().unwrap()).unwrap();
        let id = TaskId::new(&ws, 1);
        let j = Journal::open(&ctx.paths).unwrap();
        j.lock()
            .unwrap()
            .append(
                &c.actor(),
                ctx.now(),
                vec![(
                    Some(ws.clone()),
                    Some(id),
                    Kind::TaskPlaced {
                        rung: 5,
                        reason: "x".into(),
                        rejected: vec![],
                        role: "coord-light".into(),
                        harness: "omp".into(),
                        model: None,
                        title: "x".into(),
                        brief: "/tmp/x".into(),
                        session_of: None,
                        session: None,
                        request: None,
                        follows: None,
                        role_hash: String::new(),
                        brief_bytes: 0,
                    },
                )],
            )
            .unwrap();
        assert_eq!(
            error::exit_of(&ws_close(&ctx, &c, &ws, "done", WsOutcome::Delivered).unwrap_err()),
            ("tasks_running", 3)
        );
    }
}

#[cfg(test)]
mod placement_failure_tests {
    use super::super::adapter::{
        AppSnapshot,
        fake::{FakeApp, FakeHc},
    };
    use super::*;
    #[test]
    fn launch_failure_closes_task_and_resolves_intents() {
        let temp = tempfile::tempdir().unwrap();
        let app = FakeApp::default();
        app.snapshots
            .borrow_mut()
            .push_back(Ok(AppSnapshot { spaces: vec![] }));
        let hc = FakeHc::default();
        hc.runs
            .borrow_mut()
            .push_back(Err(anyhow::anyhow!("run failed")));
        let ctx = Ctx {
            root: temp.path().into(),
            paths: super::super::paths::CoordPaths::new(temp.path()),
            hc: Box::new(hc),
            app: Box::new(app),
            clock: Box::new(|| 1000),
            op: "test".into(),
            cause: None,
        };
        let c = Caller::Person { terminal: None };
        let opened = ws_open(
            &ctx,
            &c,
            &cli::WsOpen {
                title: "test".into(),
                target: "test".into(),
                repo: None,
                delivery_branch: None,
                purpose: None,
                request: None,
            },
        )
        .unwrap();
        let ws = opened["ws"].as_str().unwrap().to_owned();
        let input = temp.path().join("input");
        fs::write(&input, "brief").unwrap();
        let a = cli::TaskStart {
            ws: ws.clone(),
            rung: 5,
            reason: "new project".into(),
            brief: input,
            role: Some("coord-light".into()),
            rejected: (1..5).map(|n| format!("{n}=not suitable")).collect(),
            title: None,
            session_of: None,
            space: None,
            branch: None,
            base: None,
            folder: Some(temp.path().join("project")),
            request: None,
            follows: None,
            relation: None,
        };
        assert!(task_start(&ctx, &c, &a).is_err());
        let j = Journal::open(&ctx.paths).unwrap();
        let state = j.read_state().unwrap();
        assert!(state.intents.is_empty());
        assert_eq!(
            state.tasks.values().next().unwrap().state,
            TaskState::Closed
        );
        let kinds: Vec<_> = j
            .read_after(0)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                "ws.opened",
                "task.placed",
                "res.intent",
                "res.intent",
                "task.closed",
                "res.released",
                "res.released"
            ]
        );
    }
}

#[cfg(test)]
mod placement_acceptance_tests {
    use super::super::adapter::{
        App, AppSnapshot, AppSpace, AppTab, AppTerm, Hc, RunSpec, SendOutcome, Term,
    };
    use super::*;
    use std::cell::RefCell;
    use std::process::Command;
    use std::rc::Rc;

    #[derive(Default)]
    struct Recording {
        runs: Vec<RunSpec>,
        spaces: Vec<AppSpace>,
        live: bool,
        labels: Vec<Vec<(String, String)>>,
        sends: Vec<String>,
        journal_at_label: Vec<String>,
        report_on_rename: Option<super::super::paths::CoordPaths>,
    }
    struct RecordingHc {
        shared: Rc<RefCell<Recording>>,
        paths: super::super::paths::CoordPaths,
    }
    impl Hc for RecordingHc {
        fn run(&self, spec: &RunSpec) -> Result<Term> {
            let mut s = self.shared.borrow_mut();
            s.runs.push(spec.clone());
            let address = format!("terminal-{}", s.runs.len());
            let mut term: Term = serde_json::from_str(include_str!(
                "../../../../docs/coordinator/examples/captures/hc-get-agent.json"
            ))?;
            term.address = address.clone();
            term.state = "live".into();
            term.pid = Some(123);
            term.turn = 1;
            term.reported = true;
            term.status = Some(super::super::adapter::AgentStatus {
                state: "idle".into(),
                app: None,
                kind: None,
                msg: None,
            });
            term.labels = spec.labels.iter().cloned().collect();
            let id = spec
                .labels
                .iter()
                .find(|(k, _)| k == "herdr.space")
                .unwrap()
                .1
                .clone();
            if !s.spaces.iter().any(|space| space.id == id) {
                s.spaces.push(AppSpace {
                    id: id.clone(),
                    name: id.clone(),
                    folder: spec.cwd.to_string_lossy().into_owned(),
                    machine: None,
                    tabs: vec![],
                });
            }
            s.spaces
                .iter_mut()
                .find(|space| space.id == id)
                .unwrap()
                .tabs
                .push(AppTab {
                    id: format!("tab-{}", address),
                    name: "tab".into(),
                    terminals: vec![AppTerm {
                        terminal: address,
                        pane: "pane".into(),
                    }],
                });
            Ok(term)
        }
        fn get(&self, terminal: &str) -> Result<Option<Term>> {
            let s = self.shared.borrow();
            if !s.live {
                return Ok(None);
            }
            let Some(spec) = s.runs.first() else {
                return Ok(None);
            };
            let mut term: Term = serde_json::from_str(include_str!(
                "../../../../docs/coordinator/examples/captures/hc-get-agent.json"
            ))?;
            term.address = terminal.into();
            term.state = "live".into();
            term.pid = Some(123);
            term.turn = 1;
            term.reported = true;
            term.status = Some(super::super::adapter::AgentStatus {
                state: "idle".into(),
                app: None,
                kind: None,
                msg: None,
            });
            term.labels = spec.labels.iter().cloned().collect();
            Ok(Some(term))
        }
        fn list(&self, _: bool, _: Option<(&str, &str)>) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn send(&self, _: &str, text: &str) -> Result<SendOutcome> {
            self.shared.borrow_mut().sends.push(text.into());
            Ok(SendOutcome {
                turn: 2,
                started: Some(true),
                status: None,
            })
        }
        fn send_command(&self, _: &str, _: &str) -> Command {
            unreachable!()
        }
        fn label(&self, _: &str, labels: &[(&str, &str)]) -> Result<()> {
            let kinds = Journal::open(&self.paths)?
                .read_after(0)?
                .into_iter()
                .map(|e| e.kind)
                .collect();
            let mut s = self.shared.borrow_mut();
            s.journal_at_label = kinds;
            s.labels.push(
                labels
                    .iter()
                    .map(|(k, v)| ((*k).into(), (*v).into()))
                    .collect(),
            );
            Ok(())
        }
        fn kill(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn rm(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn events_command(&self) -> Command {
            Command::new("false")
        }
    }
    struct RecordingApp(Rc<RefCell<Recording>>);
    impl App for RecordingApp {
        fn ls(&self) -> Result<AppSnapshot> {
            Ok(AppSnapshot {
                spaces: self.0.borrow().spaces.clone(),
            })
        }
        fn command(&self, _: &str, _: Value) -> Result<Value> {
            let report = {
                let mut s = self.0.borrow_mut();
                s.report_on_rename
                    .take()
                    .map(|paths| (paths, s.runs.last().unwrap().clone()))
            };
            if let Some((paths, spec)) = report {
                let task = TaskId::parse(
                    spec.labels
                        .iter()
                        .find(|(k, _)| k == "coord.task")
                        .unwrap()
                        .1
                        .as_str(),
                )?;
                let ws = task.ws();
                let ctx = Ctx {
                    root: paths.dir.parent().unwrap().to_path_buf(),
                    paths,
                    hc: Box::new(super::super::adapter::fake::FakeHc::default()),
                    app: Box::new(super::super::adapter::fake::FakeApp::default()),
                    clock: Box::new(|| 1000),
                    op: "report".into(),
                    cause: None,
                };
                super::report(
                    &ctx,
                    &Caller::Delegate {
                        terminal: "terminal-1".into(),
                        task,
                        ws,
                    },
                    cli::ReportKind::Done,
                    "early",
                    None,
                )?;
            }
            Ok(json!({}))
        }
    }
    fn setup(root: &Path) -> (Ctx, Rc<RefCell<Recording>>) {
        let paths = super::super::paths::CoordPaths::new(root);
        paths.ensure().unwrap();
        fs::write(paths.roles.join("accept.md"), "---\nharness: omp\nstart: runner start {session_id} {cwd} {brief}\nresume: runner resume {session_id} {cwd} {brief}\nsession: preset\ntrust: --trusted\n---\nRole instructions\n").unwrap();
        let shared = Rc::new(RefCell::new(Recording::default()));
        let mut ctx = Ctx::new(root).unwrap();
        ctx.hc = Box::new(RecordingHc {
            shared: shared.clone(),
            paths,
        });
        ctx.app = Box::new(RecordingApp(shared.clone()));
        ctx.clock = Box::new(|| 1000);
        (ctx, shared)
    }
    fn person() -> Caller {
        Caller::Person { terminal: None }
    }
    fn open(ctx: &Ctx, repo: Option<&Path>) -> WsId {
        let result = ws_open(
            ctx,
            &person(),
            &cli::WsOpen {
                title: "Work".into(),
                target: "target".into(),
                repo: repo.map(Path::to_path_buf),
                delivery_branch: None,
                purpose: None,
                request: None,
            },
        )
        .unwrap();
        WsId::parse(result["ws"].as_str().unwrap()).unwrap()
    }
    fn input(root: &Path) -> PathBuf {
        let p = root.join("input");
        fs::write(&p, "Do the work").unwrap();
        p
    }
    fn args(ws: &WsId, rung: u8, input: &Path) -> cli::TaskStart {
        cli::TaskStart {
            ws: ws.as_str().into(),
            rung,
            reason: "chosen".into(),
            brief: input.into(),
            role: Some("accept".into()),
            rejected: (1..rung).map(|n| format!("{n}=unsuitable")).collect(),
            title: Some("Task title".into()),
            session_of: None,
            space: None,
            branch: None,
            base: None,
            folder: None,
            request: None,
            follows: None,
            relation: None,
        }
    }
    fn space(shared: &Rc<RefCell<Recording>>, id: &str, folder: &Path) {
        shared.borrow_mut().spaces.push(AppSpace {
            id: id.into(),
            name: format!("Name {id}"),
            folder: folder.to_string_lossy().into_owned(),
            machine: None,
            tabs: vec![],
        });
    }
    fn start_in_space(ctx: &Ctx, ws: &WsId, input: &Path, name: &str) -> Value {
        let mut a = args(ws, 3, input);
        a.space = Some(name.into());
        task_start(ctx, &person(), &a).unwrap()
    }
    fn assert_spec(
        ctx: &Ctx,
        spec: &RunSpec,
        ws: &WsId,
        id: &TaskId,
        space: &str,
        cwd: &Path,
        mode: &str,
        session: &str,
        trusted: bool,
        text: &str,
    ) {
        assert_eq!(spec.name, id.as_str());
        assert_eq!(spec.cwd, cwd);
        assert_eq!(
            spec.labels,
            vec![
                ("herdr.space".into(), space.into()),
                ("coord.managed".into(), "1".into()),
                ("coord.role".into(), "delegate".into()),
                ("coord.ws".into(), ws.as_str().into()),
                ("coord.task".into(), id.as_str().into())
            ]
        );
        let role = role::load(&ctx.paths, "accept").unwrap();
        let message = if mode == "start" {
            brief::delegate_first(&role, text, &ctx.cli(), &ctx.paths.result(id))
        } else {
            brief::follow_up(text, &ctx.cli(), &ctx.paths.result(id))
        };
        let mut expected = vec!["runner".to_owned()];
        if trusted {
            expected.push("--trusted".into());
        }
        expected.extend([
            mode.to_owned(),
            session.to_owned(),
            cwd.to_string_lossy().into_owned(),
            message,
        ]);
        assert_eq!(spec.argv, expected);
    }
    fn init_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        for args in [
            &["init", "-q"][..],
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ][..],
        ] {
            assert!(
                Command::new("git")
                    .env("HOME", root)
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        repo
    }
    #[test]
    fn report_during_post_launch_rename_remains_reported() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let ws = open(&ctx, None);
        space(&rec, "existing", temp.path());
        rec.borrow_mut().report_on_rename = Some(ctx.paths.clone());
        let result = start_in_space(&ctx, &ws, &input, "existing");
        let id = TaskId::parse(result["task"].as_str().unwrap()).unwrap();
        let state = ctx.journal().unwrap().read_state().unwrap();
        let task = &state.tasks[&id];
        assert_eq!(task.state, TaskState::Reported);
        assert_eq!(task.summary.as_deref(), Some("early"));
        assert_eq!(task.tab.as_deref(), Some("tab-terminal-1"));
    }
    #[test]
    fn run_specs_for_all_five_rungs() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let repo = init_repo(temp.path());
        let mut config = super::super::config::Config::default();
        config.worktree_root = temp.path().join("trees").to_string_lossy().into_owned();
        config.save(&ctx.paths).unwrap();
        let ws = open(&ctx, Some(&repo));
        space(&rec, "existing", &repo);
        let first = start_in_space(&ctx, &ws, &input, "existing");
        let id1 = TaskId::parse(first["task"].as_str().unwrap()).unwrap();
        let session1 = first["session"].as_str().unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[0],
            &ws,
            &id1,
            "existing",
            &repo,
            "start",
            session1,
            true,
            "Do the work",
        );
        let mut a = args(&ws, 2, &input);
        let second = task_start(&ctx, &person(), &a).unwrap();
        let id2 = TaskId::parse(second["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[1],
            &ws,
            &id2,
            "existing",
            &repo,
            "start",
            second["session"].as_str().unwrap(),
            true,
            "Do the work",
        );
        a = args(&ws, 1, &input);
        a.session_of = Some(id1.as_str().into());
        let resumed = task_start(&ctx, &person(), &a).unwrap();
        let id3 = TaskId::parse(resumed["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[2],
            &ws,
            &id3,
            "existing",
            &repo,
            "resume",
            session1,
            true,
            "Do the work",
        );
        let ws4 = open(&ctx, Some(&repo));
        let mut a4 = args(&ws4, 4, &input);
        a4.branch = Some("feature".into());
        let fourth = task_start(&ctx, &person(), &a4).unwrap();
        let id4 = TaskId::parse(fourth["task"].as_str().unwrap()).unwrap();
        let tree = temp.path().join("trees/repo/feature");
        assert_spec(
            &ctx,
            &rec.borrow().runs[3],
            &ws4,
            &id4,
            fourth["space"].as_str().unwrap(),
            &tree,
            "start",
            fourth["session"].as_str().unwrap(),
            true,
            "Do the work",
        );
        let ws5 = open(&ctx, None);
        let folder = temp.path().join("new-folder");
        let mut a5 = args(&ws5, 5, &input);
        a5.folder = Some(folder.clone());
        let fifth = task_start(&ctx, &person(), &a5).unwrap();
        let id5 = TaskId::parse(fifth["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[4],
            &ws5,
            &id5,
            fifth["space"].as_str().unwrap(),
            &folder,
            "start",
            fifth["session"].as_str().unwrap(),
            false,
            "Do the work",
        );
    }
    #[test]
    fn dormant_send_resumes_same_task_and_session() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let ws = open(&ctx, None);
        space(&rec, "existing", temp.path());
        let first = start_in_space(&ctx, &ws, &input, "existing");
        let id = TaskId::parse(first["task"].as_str().unwrap()).unwrap();
        Journal::open(&ctx.paths)
            .unwrap()
            .lock()
            .unwrap()
            .append(
                &person().actor(),
                ctx.now(),
                vec![(
                    Some(ws.clone()),
                    Some(id.clone()),
                    Kind::TaskDormant {
                        reason: "terminal exited".into(),
                    },
                )],
            )
            .unwrap();
        let result = task_send(&ctx, &person(), &id, "Next step").unwrap();
        assert_eq!(result["task"], id.as_str());
        assert_eq!(result["session"], first["session"]);
        assert_spec(
            &ctx,
            &rec.borrow().runs[1],
            &ws,
            &id,
            "existing",
            temp.path(),
            "resume",
            first["session"].as_str().unwrap(),
            false,
            "Next step",
        );
        let entries = Journal::open(&ctx.paths).unwrap().read_after(0).unwrap();
        assert_eq!(
            entries
                .iter()
                .rev()
                .take(5)
                .map(|e| e.kind.as_str())
                .collect::<Vec<_>>(),
            vec![
                "task.launch_details",
                "task.sent",
                "task.started",
                "res.created",
                "res.intent"
            ]
        );
        assert_eq!(
            entries
                .iter()
                .rev()
                .find(|e| e.kind == "task.sent")
                .unwrap()
                .data["via"],
            "resume"
        );
    }
    #[test]
    fn live_handover_is_journaled_before_relabel_and_released_tab_refused() {
        for released in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (ctx, rec) = setup(temp.path());
            let input = input(temp.path());
            let ws = open(&ctx, None);
            space(&rec, "existing", temp.path());
            let first = start_in_space(&ctx, &ws, &input, "existing");
            let id = TaskId::parse(first["task"].as_str().unwrap()).unwrap();
            rec.borrow_mut().live = true;
            if released {
                Journal::open(&ctx.paths)
                    .unwrap()
                    .lock()
                    .unwrap()
                    .append(
                        &person().actor(),
                        ctx.now(),
                        vec![(
                            Some(ws.clone()),
                            Some(id.clone()),
                            Kind::ResReleased {
                                r#type: ResType::Tab,
                                name: "terminal-1".into(),
                                note: None,
                            },
                        )],
                    )
                    .unwrap();
            }
            let mut a = args(&ws, 1, &input);
            a.session_of = Some(id.as_str().into());
            let result = task_start(&ctx, &person(), &a);
            if released {
                assert_eq!(
                    error::exit_of(&result.unwrap_err()),
                    ("terminal_not_owned", 3)
                );
                assert!(rec.borrow().labels.is_empty());
                assert!(rec.borrow().sends.is_empty());
            } else {
                let next = result.unwrap();
                let s = rec.borrow();
                assert_eq!(s.runs.len(), 1);
                assert_eq!(s.sends.len(), 1);
                assert_eq!(
                    s.labels,
                    vec![vec![
                        ("coord.task".into(), next["task"].as_str().unwrap().into()),
                        ("coord.ws".into(), ws.as_str().into())
                    ]]
                );
                assert_eq!(
                    &s.journal_at_label[s.journal_at_label.len() - 4..],
                    ["task.placed", "task.closed", "task.started", "task.sent"]
                );
            }
        }
    }
    #[test]
    fn trust_requires_a_person_space_for_the_same_repo() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        let repo = init_repo(temp.path());
        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        assert!(
            Command::new("git")
                .env("HOME", temp.path())
                .arg("-C")
                .arg(&other)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        space(&rec, "other", &other);
        let ws = open(&ctx, Some(&repo));
        let mut a = args(&ws, 5, &input);
        a.folder = Some(repo.clone());
        let first = task_start(&ctx, &person(), &a).unwrap();
        let id = TaskId::parse(first["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[0],
            &ws,
            &id,
            first["space"].as_str().unwrap(),
            a.folder.as_ref().unwrap(),
            "start",
            first["session"].as_str().unwrap(),
            false,
            "Do the work",
        );
        let a_owned = args(&ws, 2, &input);
        let owned = task_start(&ctx, &person(), &a_owned).unwrap();
        let owned_id = TaskId::parse(owned["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[1],
            &ws,
            &owned_id,
            first["space"].as_str().unwrap(),
            &repo,
            "start",
            owned["session"].as_str().unwrap(),
            false,
            "Do the work",
        );
        space(&rec, "person", &repo);
        let ws2 = open(&ctx, Some(&repo));
        let mut a2 = args(&ws2, 5, &input);
        a2.folder = Some(temp.path().join("second"));
        let second = task_start(&ctx, &person(), &a2).unwrap();
        let id2 = TaskId::parse(second["task"].as_str().unwrap()).unwrap();
        assert_spec(
            &ctx,
            &rec.borrow().runs[2],
            &ws2,
            &id2,
            second["space"].as_str().unwrap(),
            a2.folder.as_ref().unwrap(),
            "start",
            second["session"].as_str().unwrap(),
            true,
            "Do the work",
        );
    }
    #[test]
    fn rung_three_adopts_space_for_each_workstream() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, rec) = setup(temp.path());
        let input = input(temp.path());
        space(&rec, "shared", temp.path());
        let ws1 = open(&ctx, None);
        let ws2 = open(&ctx, None);
        start_in_space(&ctx, &ws1, &input, "shared");
        start_in_space(&ctx, &ws2, &input, "shared");
        let state = Journal::open(&ctx.paths).unwrap().read_state().unwrap();
        for ws in [&ws1, &ws2] {
            assert!(state.resources.iter().any(|r| r.ws.as_ref() == Some(ws)
                && r.r.r#type == ResType::Space
                && r.r.name == "shared"
                && !r.owned
                && !r.released));
            assert_eq!(state.workstreams[ws].space.as_ref().unwrap().id, "shared");
        }
        assert!(state.anomalies.is_empty(), "{:?}", state.anomalies);
    }
}

pub fn rate(
    ctx: &Ctx,
    c: &Caller,
    target: &str,
    value: super::kinds::RatingValue,
    note: Option<&str>,
) -> Result<Value> {
    if !matches!(c, Caller::Person { .. }) {
        return Err(refuse("forbidden_caller", "only the person can rate work"));
    }
    let j = ctx.journal()?;
    let mut lock = j.lock()?;
    let (ws, task) = if let Ok(id) = TaskId::parse(target) {
        if !lock.state().tasks.contains_key(&id) {
            return Err(refuse("unknown_task", "unknown task"));
        }
        (Some(id.ws()), Some(id))
    } else {
        let id = WsId::parse(target)?;
        if !lock.state().workstreams.contains_key(&id) {
            return Err(refuse("unknown_ws", "unknown workstream"));
        }
        (Some(id), None)
    };
    let entries = lock.append(
        &c.actor(),
        ctx.now(),
        vec![(
            ws,
            task,
            Kind::Rating {
                target: target.into(),
                value,
                note: note.map(str::to_owned),
            },
        )],
    )?;
    Ok(json!({"seq":entries[0].seq}))
}
pub fn config_set(ctx: &Ctx, c: &Caller, key: &str, value: &str) -> Result<Value> {
    if !matches!(c, Caller::Person { .. }) {
        return Err(refuse(
            "forbidden_caller",
            "only the person can change configuration",
        ));
    }
    let j = ctx.journal()?;
    let mut lock = j.lock()?;
    let cfg = ctx.config()?;
    let mut object = serde_json::to_value(&cfg)?;
    let old = object
        .get(key)
        .cloned()
        .ok_or_else(|| refuse("unknown_config_key", "unknown configuration key"))?;
    let new = match old {
        Value::String(_) => Value::String(value.into()),
        Value::Bool(_) => Value::Bool(
            value
                .parse()
                .map_err(|_| refuse("invalid_config_value", "expected true or false"))?,
        ),
        Value::Number(_) => json!(
            value
                .parse::<u64>()
                .map_err(|_| refuse("invalid_config_value", "expected unsigned integer"))?
        ),
        _ => {
            return Err(refuse(
                "invalid_config_value",
                "unsupported configuration type",
            ));
        }
    };
    if old == new {
        return Ok(json!({"changed":false}));
    }
    object[key] = new.clone();
    let next: super::config::Config = serde_json::from_value(object)?;
    next.save(&ctx.paths)?;
    let entries = lock.append(
        &c.actor(),
        ctx.now(),
        vec![(
            None,
            None,
            Kind::ConfigChanged {
                key: key.into(),
                old,
                new,
            },
        )],
    )?;
    Ok(json!({"changed":true,"seq":entries[0].seq}))
}
pub fn stats(ctx: &Ctx, since: Option<&str>) -> Result<Value> {
    super::stats::stats(ctx, since)
}

#[cfg(test)]
mod insight_handler_tests {
    use super::*;
    #[test]
    fn config_only_records_changes_and_delegate_cannot_rate() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = Ctx::new(temp.path()).unwrap();
        ctx.cause = Some(9);
        let person = Caller::Person { terminal: None };
        assert_eq!(
            config_set(&ctx, &person, "stall_secs", "300").unwrap()["changed"],
            false
        );
        assert_eq!(
            config_set(&ctx, &person, "stall_secs", "301").unwrap()["changed"],
            true
        );
        let entries = ctx.journal().unwrap().read_after(0).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, "config.changed");
        assert_eq!(entries[0].data["old"], 300);
        assert_eq!(entries[0].data["new"], 301);
        assert_eq!(entries[0].cause, Some(9));
        let ws = WsId::parse("ws-abcdef").unwrap();
        let task = TaskId::new(&ws, 1);
        let delegate = Caller::Delegate {
            terminal: "t".into(),
            task,
            ws,
        };
        assert_eq!(
            error::exit_of(
                &rate(
                    &ctx,
                    &delegate,
                    "ws-abcdef",
                    super::super::kinds::RatingValue::Good,
                    None
                )
                .unwrap_err()
            ),
            ("forbidden_caller", 3)
        );
    }
}
