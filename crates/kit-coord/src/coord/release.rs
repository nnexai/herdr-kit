use super::{
    Ctx,
    adapter::AppSnapshot,
    error, git,
    ids::{TaskId, WsId},
    journal::{Journal, Locked},
    kinds::{Actor, Check, Kind, ResType, ResourceRef, SessionMode},
    state::{ResourceRec, Retained, State, TaskState, WsState},
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Command,
    Recheck,
}
pub struct Outcome {
    pub cleaned: bool,
    pub released: Vec<ResourceRef>,
    pub retained: Option<Retained>,
}
fn check(name: &str, ok: bool, detail: Option<String>) -> Check {
    Check {
        name: name.into(),
        ok,
        detail,
    }
}
fn ws<'a>(state: &'a State, rec: &ResourceRec) -> Result<&'a super::state::Ws> {
    state
        .workstreams
        .get(rec.ws.as_ref().context("resource has no workstream")?)
        .context("unknown workstream")
}
fn repo(rec: &ResourceRec) -> Result<&Path> {
    Ok(rec
        .r
        .repo
        .as_deref()
        .context("resource has no repository")?)
}
fn delivery<'a>(state: &'a State, rec: &ResourceRec) -> Result<&'a str> {
    Ok(ws(state, rec)?
        .delivery_branch
        .as_deref()
        .context("workstream has no delivery branch")?)
}
fn owned(rec: &ResourceRec) -> Check {
    check("owned", rec.owned && !rec.released, None)
}

pub fn check_tab(ctx: &Ctx, state: &State, rec: &ResourceRec) -> Result<Vec<Check>> {
    let term = ctx.hc.get(&rec.r.name)?;
    let task = rec.task.as_ref().and_then(|id| state.tasks.get(id));
    let closed = ws(state, rec)?.state != WsState::Open;
    let session = task.and_then(|t| t.session.as_ref()).is_some_and(|s| {
        if s.mode == SessionMode::None {
            closed
        } else {
            s.id.as_ref().is_some_and(|id| !id.is_empty())
        }
    });
    let identity = task.zip(term.as_ref()).is_some_and(|(task, term)| {
        task.terminal.as_deref() == Some(&term.address)
            && term.address == rec.r.name
            && task.pid.is_some()
            && task.pid == term.pid
            && task.created_ms == Some(term.created_ms)
            && rec
                .task
                .as_ref()
                .is_some_and(|id| term.label("coord.task") == Some(id.as_str()))
            && rec
                .ws
                .as_ref()
                .is_some_and(|id| term.label("coord.ws") == Some(id.as_str()))
            && matches!(task.state, TaskState::Reported | TaskState::Closed)
    });
    Ok(vec![
        check(
            "owned",
            rec.owned && !rec.released && identity && term.as_ref().is_some_and(|t| t.managed()),
            None,
        ),
        check(
            "idle",
            term.as_ref().is_some_and(|t| !t.live() || t.settled()),
            None,
        ),
        check(
            "result_recorded",
            task.is_some_and(|t| {
                t.reported_seq.is_some_and(|seq| seq > t.last_activity_seq)
                    || (identity
                        && term.as_ref().is_some_and(|term| {
                            term.state == "finished" && term.foreground.is_none()
                        })
                        && t.reconciled_result_seq
                            .is_some_and(|seq| seq > t.last_activity_seq))
            }),
            None,
        ),
        check("session_recorded", session, None),
        check(
            "nothing_running",
            term.as_ref().is_some_and(|t| {
                !t.live()
                    || t.foreground
                        .as_ref()
                        .zip(t.pid)
                        .is_some_and(|(f, pid)| u32::try_from(f.pid).ok() == Some(pid))
            }),
            None,
        ),
    ])
}
pub fn check_space(
    ctx: &Ctx,
    state: &State,
    rec: &ResourceRec,
    app: &AppSnapshot,
) -> Result<Vec<Check>> {
    let mut no_person = true;
    if let Some(space) = app.space(&rec.r.name) {
        for pane in space.tabs.iter().flat_map(|t| &t.terminals) {
            if !ctx.hc.get(&pane.terminal)?.is_some_and(|t| t.managed()) {
                no_person = false;
            }
        }
    }
    let tabs_released = !state.resources.iter().any(|r| {
        r.r.r#type == ResType::Tab
            && r.r.space.as_deref() == Some(&rec.r.name)
            && r.owned
            && !r.released
    });
    Ok(vec![
        owned(rec),
        check("no_person_terminal", no_person, None),
        check("tabs_released", tabs_released, None),
    ])
}
pub fn check_worktree(ctx: &Ctx, state: &State, rec: &ResourceRec) -> Result<Vec<Check>> {
    let path = Path::new(&rec.r.name);
    let dirty = git::dirty(path)?;
    let head = git::rev(path, "HEAD")?;
    let count = git::unreachable_count(repo(rec)?, &head, delivery(state, rec)?)?;
    let mut clear = true;
    for address in ctx.hc.list(false, None)? {
        if let Some(t) = ctx.hc.get(&address)? {
            if t.live()
                && (Path::new(&t.cwd).starts_with(path)
                    || (t.managed()
                        && t.foreground
                            .as_ref()
                            .is_some_and(|f| Path::new(&f.cwd).starts_with(path))))
            {
                clear = false;
            }
        }
    }
    Ok(vec![
        owned(rec),
        check("ws_closed", ws(state, rec)?.state != WsState::Open, None),
        check(
            "clean",
            dirty.is_empty(),
            if dirty.is_empty() {
                None
            } else {
                Some(dirty.join("\n"))
            },
        ),
        check(
            "reachable",
            count == 0,
            if count == 0 {
                None
            } else {
                Some(format!("{count} unreachable commits"))
            },
        ),
        check("no_terminal_inside", clear, None),
    ])
}
pub fn check_branch(state: &State, rec: &ResourceRec) -> Result<Vec<Check>> {
    let repo = repo(rec)?;
    let sha = git::rev(repo, &format!("refs/heads/{}", rec.r.name))?;
    let merged = git::is_ancestor(repo, &sha, &format!("refs/heads/{}", delivery(state, rec)?))?;
    let checked_out = git::worktree_list(repo)?
        .iter()
        .any(|w| w.branch.as_deref() == Some(&rec.r.name));
    Ok(vec![
        owned(rec),
        check("merged", merged, None),
        check("not_checked_out", !checked_out, None),
    ])
}
fn type_name(t: ResType) -> &'static str {
    match t {
        ResType::Tab => "tab",
        ResType::Space => "space",
        ResType::Worktree => "worktree",
        ResType::Branch => "branch",
    }
}
fn record_checks(
    lock: &mut Locked<'_>,
    actor: &Actor,
    now: u64,
    rec: &ResourceRec,
    checks: &[Check],
    mode: Mode,
) -> Result<bool> {
    let latest = lock.state().resources.iter().find(|r| {
        r.r.r#type == rec.r.r#type
            && r.r.name == rec.r.name
            && r.r.repo == rec.r.repo
            && r.ws == rec.ws
            && !r.released
    });
    let mut items = Vec::new();
    for c in checks {
        if mode == Mode::Command
            || latest.and_then(|r| r.last_checks.get(&c.name)) != Some(&(c.ok, c.detail.clone()))
        {
            items.push((
                rec.ws.clone(),
                rec.task.clone(),
                Kind::ResReleaseChecked {
                    resource: format!("{}:{}", type_name(rec.r.r#type), rec.r.name),
                    r#type: rec.r.r#type,
                    name: rec.r.name.clone(),
                    check: c.name.clone(),
                    ok: c.ok,
                    detail: c.detail.clone(),
                },
            ));
        }
    }
    let changed = !items.is_empty();
    lock.append(actor, now, items)?;
    Ok(changed)
}
fn released(
    lock: &mut Locked<'_>,
    actor: &Actor,
    now: u64,
    rec: &ResourceRec,
    note: Option<&str>,
) -> Result<()> {
    lock.append(
        actor,
        now,
        vec![(
            rec.ws.clone(),
            rec.task.clone(),
            Kind::ResReleased {
                r#type: rec.r.r#type,
                name: rec.r.name.clone(),
                note: note.map(str::to_owned),
            },
        )],
    )?;
    Ok(())
}
fn missing_ok(err: &anyhow::Error, what: &str) -> bool {
    format!("{err:#}")
        .to_ascii_lowercase()
        .contains(&format!("no {what}"))
}
fn close_empty_tab(ctx: &Ctx, tab: Option<&str>) -> Result<()> {
    if let Some(tab) = tab {
        let app = ctx.app.ls()?;
        if app
            .spaces
            .iter()
            .flat_map(|s| &s.tabs)
            .any(|t| t.id == tab && t.terminals.is_empty())
        {
            if let Err(e) = ctx
                .app
                .command("tab.close", json!({"id":tab,"confirmed":true}))
            {
                if !missing_ok(&e, "tab") {
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}
fn end_tab(ctx: &Ctx, rec: &ResourceRec) -> Result<()> {
    match ctx.hc.get(&rec.r.name)? {
        None => return Ok(()),
        Some(term) if term.state == "finished" => {}
        Some(term) if term.state == "live" => ctx.hc.kill(&rec.r.name)?,
        Some(_) => anyhow::bail!("terminal state is unknown"),
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match ctx.hc.rm(&rec.r.name) {
            Ok(()) => break,
            Err(e) if format!("{e:#}").contains("live") && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(250))
            }
            Err(e) => return Err(e),
        }
    }
    close_empty_tab(ctx, rec.r.tab.as_deref())
}
fn reason_name(name: &str) -> &str {
    match name {
        "clean" => "uncommitted_files",
        "reachable" => "unpushed_commits",
        "merged" => "branch_not_merged",
        "no_person_terminal" => "in_use_by_person",
        "idle" => "agent_busy",
        "result_recorded" => "no_result",
        "session_recorded" => "no_session",
        "nothing_running" => "program_running",
        "tabs_released" => "tabs_open",
        "no_terminal_inside" => "terminal_in_worktree",
        _ => name,
    }
}
fn retained(checks: Vec<Check>, now: u64) -> Retained {
    let first = checks.iter().find(|c| !c.ok).expect("failed checks");
    let code = reason_name(&first.name).to_owned();
    Retained {
        reason: format!("Release blocked: {}.", code.replace('_', " ")),
        reason_code: code,
        since_ms: now,
        checked_ms: Some(now),
        checks,
    }
}
/// Record recovery provenance without manufacturing a delegate report or removing a resource.
pub fn reconcile_result(
    ctx: &Ctx,
    j: &Journal,
    actor: &Actor,
    task_id: &TaskId,
    source_id: &TaskId,
    result: &Path,
) -> Result<serde_json::Value> {
    ensure!(matches!(actor, Actor::Coordinator), "coordinator only");
    ensure!(task_id != source_id, "source task must be distinct");
    ensure!(
        result.is_absolute() && result.is_file(),
        "result must be an existing absolute file"
    );
    let mut lock = j.lock()?;
    let state = lock.state();
    let task = state.tasks.get(task_id).context("unknown task")?;
    ensure!(task.state == TaskState::Closed, "task is not closed");
    ensure!(
        task.reported_seq
            .is_none_or(|seq| seq <= task.last_activity_seq),
        "task already has a current report"
    );
    ensure!(
        task.reconciled_result_seq.is_none(),
        "result already reconciled"
    );
    let source = state.tasks.get(source_id).context("unknown source task")?;
    ensure!(
        source.report == Some(super::kinds::Outcome::Done)
            && source.result.as_deref() == Some(result),
        "source task did not report this result"
    );
    let terminal = task.terminal.as_deref().context("task has no terminal")?;
    let rec = state
        .resources
        .iter()
        .find(|r| {
            r.task.as_ref() == Some(task_id)
                && r.ws.as_ref() == Some(&task.ws)
                && r.r.r#type == ResType::Tab
                && r.r.name == terminal
                && r.owned
                && !r.released
        })
        .context("owned task terminal not found")?;
    let tab = rec
        .r
        .tab
        .as_deref()
        .context("resource has no recorded tab")?;
    let _guard = ctx.terminal_lock(terminal)?;
    let term = ctx.hc.get(terminal)?.context("terminal not found")?;
    ensure!(
        term.state == "finished" && term.foreground.is_none(),
        "terminal is not definitively finished"
    );
    ensure!(
        term.managed()
            && term.address == terminal
            && task.pid.is_some()
            && task.pid == term.pid
            && task.created_ms == Some(term.created_ms)
            && term.label("coord.task") == Some(task_id.as_str())
            && term.label("coord.ws") == Some(task.ws.as_str()),
        "terminal ownership identity mismatch"
    );
    let app = ctx.app.ls()?;
    ensure!(
        !app.spaces
            .iter()
            .flat_map(|s| &s.tabs)
            .any(|t| t.id == tab || t.terminals.iter().any(|p| p.terminal == terminal)),
        "terminal or recorded tab remains in app"
    );
    let ws = task.ws.clone();
    let terminal = terminal.to_owned();
    lock.append(
        actor,
        ctx.now(),
        vec![(
            Some(ws),
            Some(task_id.clone()),
            Kind::TaskResultReconciled {
                source_task: source_id.clone(),
                result: result.to_path_buf(),
                terminal: terminal.clone(),
            },
        )],
    )?;
    Ok(json!({"task": task_id, "source_task": source_id, "result": result, "terminal": terminal}))
}
pub fn release_task_tab(
    ctx: &Ctx,
    j: &Journal,
    actor: &Actor,
    task: &TaskId,
) -> Result<Vec<Check>> {
    let mut lock = j.lock()?;
    let state = lock.state().clone();
    let Some(rec) = state.resources.iter().find(|r| {
        r.task.as_ref() == Some(task) && r.r.r#type == ResType::Tab && r.owned && !r.released
    }) else {
        return Ok(Vec::new());
    };
    let _terminal_guard = ctx.terminal_lock(&rec.r.name)?;
    if ctx.hc.get(&rec.r.name)?.is_none() {
        released(&mut lock, actor, ctx.now(), rec, Some("gone"))?;
        return Ok(Vec::new());
    }
    let checks = check_tab(ctx, &state, rec)?;
    record_checks(&mut lock, actor, ctx.now(), rec, &checks, Mode::Command)?;
    if checks.iter().all(|c| c.ok) {
        drop(lock);
        end_tab(ctx, rec)?;
        released(&mut j.lock()?, actor, ctx.now(), rec, None)?;
    }
    Ok(checks)
}
/// The workstream's owned resources of one type that are not released yet.
fn owned_of<'a>(
    state: &'a State,
    id: &'a WsId,
    typ: ResType,
) -> impl Iterator<Item = &'a ResourceRec> {
    state
        .resources
        .iter()
        .filter(move |r| r.ws.as_ref() == Some(id) && r.r.r#type == typ && r.owned && !r.released)
}
/// Whether the resource no longer exists outside the journal.
fn gone(ctx: &Ctx, rec: &ResourceRec) -> Result<bool> {
    Ok(match rec.r.r#type {
        ResType::Tab => ctx.hc.get(&rec.r.name)?.is_none(),
        ResType::Space => ctx.app.ls()?.space(&rec.r.name).is_none(),
        ResType::Worktree => !git::worktree_list(repo(rec)?)?
            .iter()
            .any(|w| w.path == Path::new(&rec.r.name)),
        ResType::Branch => !git::branch_exists(repo(rec)?, &rec.r.name)?,
    })
}
/// The release checks of one resource. A branch must also be merged at `sha`,
/// the commit its deletion is pinned to.
fn release_checks(
    ctx: &Ctx,
    state: &State,
    rec: &ResourceRec,
    sha: Option<&str>,
) -> Result<Vec<Check>> {
    Ok(match rec.r.r#type {
        ResType::Tab => check_tab(ctx, state, rec)?,
        ResType::Space => check_space(ctx, state, rec, &ctx.app.ls()?)?,
        ResType::Worktree => check_worktree(ctx, state, rec)?,
        ResType::Branch => {
            let mut checks = check_branch(state, rec)?;
            checks[1].ok &= git::is_ancestor(
                repo(rec)?,
                sha.context("branch has no pinned commit")?,
                &format!("refs/heads/{}", delivery(state, rec)?),
            )?;
            checks
        }
    })
}
pub fn release_ws(ctx: &Ctx, j: &Journal, actor: &Actor, id: &WsId, mode: Mode) -> Result<Outcome> {
    let mut lock = j.lock()?;
    let initial = lock.state().clone();
    let workstream = initial.workstreams.get(id).context("unknown workstream")?;
    if workstream.state == WsState::Open {
        return Err(error::refused("ws_open", "workstream is open"));
    }
    if workstream.state == WsState::Cleaned {
        return Ok(Outcome {
            cleaned: true,
            released: vec![],
            retained: None,
        });
    }
    let mut result = Outcome {
        cleaned: false,
        released: vec![],
        retained: None,
    };
    let mut changed = false;
    for typ in [
        ResType::Tab,
        ResType::Space,
        ResType::Worktree,
        ResType::Branch,
    ] {
        let records: Vec<_> = owned_of(lock.state(), id, typ).cloned().collect();
        for rec in records {
            let state = lock.state().clone();
            let _terminal_guard = if typ == ResType::Tab {
                match ctx.terminal_lock(&rec.r.name) {
                    Ok(guard) => Some(guard),
                    Err(e) => {
                        let checks = vec![check("terminal_operation", false, Some(e.to_string()))];
                        changed |= record_checks(&mut lock, actor, ctx.now(), &rec, &checks, mode)?;
                        if result.retained.is_none() {
                            result.retained = Some(retained(checks, ctx.now()));
                        }
                        continue;
                    }
                }
            } else {
                None
            };
            // A worktree stays until its owned space is released, and a branch
            // until its worktree is.
            let holder = match typ {
                ResType::Worktree => Some(ResType::Space),
                ResType::Branch => Some(ResType::Worktree),
                ResType::Tab | ResType::Space => None,
            };
            if holder.is_some_and(|holder| owned_of(&state, id, holder).next().is_some()) {
                continue;
            }
            if gone(ctx, &rec)? {
                released(&mut lock, actor, ctx.now(), &rec, Some("gone"))?;
                result.released.push(rec.r.clone());
                continue;
            }
            let sha = if typ == ResType::Branch {
                Some(git::rev(
                    repo(&rec)?,
                    &format!("refs/heads/{}", rec.r.name),
                )?)
            } else {
                None
            };
            let checks = release_checks(ctx, &state, &rec, sha.as_deref())?;
            changed |= record_checks(&mut lock, actor, ctx.now(), &rec, &checks, mode)?;
            if checks.iter().any(|c| !c.ok) {
                if result.retained.is_none() {
                    result.retained = Some(retained(checks, ctx.now()));
                }
                continue;
            }
            match typ {
                ResType::Tab => {
                    drop(lock);
                    let result = end_tab(ctx, &rec);
                    lock = j.lock()?;
                    result?;
                }
                ResType::Space => {
                    if let Err(e) = ctx
                        .app
                        .command("space.close", json!({"id":rec.r.name,"confirmed":true}))
                    {
                        if !missing_ok(&e, "space") {
                            return Err(e);
                        }
                    }
                }
                ResType::Worktree => git::worktree_remove(repo(&rec)?, Path::new(&rec.r.name))?,
                ResType::Branch => {
                    git::delete_branch(repo(&rec)?, &rec.r.name, sha.as_deref().unwrap())?
                }
            }
            released(&mut lock, actor, ctx.now(), &rec, None)?;
            result.released.push(rec.r.clone());
        }
    }
    if let Some(r) = &result.retained {
        let previous = workstream.retained.as_ref();
        if mode == Mode::Command
            || changed
            || previous.is_none_or(|p| p.reason_code != r.reason_code || p.checks != r.checks)
        {
            lock.append(
                actor,
                ctx.now(),
                vec![(
                    Some(id.clone()),
                    None,
                    Kind::WsRetained {
                        reason_code: r.reason_code.clone(),
                        reason: r.reason.clone(),
                        checks: r.checks.clone(),
                    },
                )],
            )?;
        }
    } else {
        ensure!(
            !lock
                .state()
                .resources
                .iter()
                .any(|r| r.ws.as_ref() == Some(id) && r.owned && !r.released),
            "owned resources remain"
        );
        lock.append(
            actor,
            ctx.now(),
            vec![(Some(id.clone()), None, Kind::WsCleaned {})],
        )?;
        result.cleaned = true;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::{
        adapter::{
            AppSpace, AppTab, AppTerm, Term,
            fake::{FakeApp, FakeHc},
        },
        journal,
        kinds::{Outcome as TaskOutcome, SessionRef},
        state::{Task, TaskState, Ws},
    };
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    fn setup() -> (State, ResourceRec, Term) {
        let id = WsId::parse("ws-abcdef").unwrap();
        let tid = TaskId::new(&id, 1);
        let mut state = State::default();
        state.workstreams.insert(
            id.clone(),
            Ws {
                id: id.clone(),
                title: "work".into(),
                repo: None,
                delivery_target: None,
                delivery_branch: None,
                state: WsState::Open,
                retained: None,
                summary: None,
                opened_ms: 0,
                closed_ms: None,
                last_event_ms: 0,
                space: None,
                cwd: None,
                tasks: vec![tid.clone()],
                request: None,
                outcome: None,
                rating: None,
            },
        );
        state.tasks.insert(
            tid.clone(),
            Task {
                id: tid.clone(),
                ws: id.clone(),
                n: 1,
                title: "task".into(),
                state: TaskState::Reported,
                report: Some(TaskOutcome::Done),
                start_blocked: None,
                stalled: None,
                start_blocked_ms: None,
                stalled_ms: None,
                attention: None,
                blocked: None,
                state_since_ms: 0,
                placed_ms: 0,
                started_ms: None,
                last_event_ms: 0,
                rung: 2,
                reason: String::new(),
                rejected: vec![],
                role: String::new(),
                harness: String::new(),
                model: None,
                effort: None,
                summary: None,
                terminal: Some("t1".into()),
                space: Some("sp".into()),
                tab: Some("tb".into()),
                cwd: None,
                pid: Some(1),
                created_ms: None,
                session: Some(SessionRef {
                    mode: SessionMode::Dir,
                    id: Some("session".into()),
                    dir: None,
                }),
                brief: PathBuf::new(),
                result: None,
                nudges: 0,
                last_nudge: None,
                last_activity_seq: 1,
                reported_seq: Some(2),
                reconciled_result_seq: None,
                request: None,
                follows: None,
                role_hash: String::new(),
                brief_bytes: 0,
                outcome: None,
                rating: None,
                turns: 0,
                person_turns: 0,
                working_ms: 0,
                attachments: vec![],
            },
        );
        let rec = ResourceRec {
            r: ResourceRef {
                r#type: ResType::Tab,
                name: "t1".into(),
                repo: None,
                folder: None,
                label: None,
                branch: None,
                base: None,
                base_sha: None,
                create_branch: None,
                space: Some("sp".into()),
                tab: Some("tb".into()),
            },
            ws: Some(id),
            task: Some(tid),
            owned: true,
            released: false,
            created_seq: 1,
            last_checks: BTreeMap::new(),
        };
        state
            .tasks
            .get_mut(rec.task.as_ref().unwrap())
            .unwrap()
            .created_ms = Some(0);
        let term: Term = serde_json::from_value(json!({"address":"t1","state":"live","status":{"state":"done","app":null,"kind":null,"msg":null},"status_ms":null,"labels":{"coord.managed":"1","coord.ws":rec.ws.as_ref().unwrap().as_str(),"coord.task":rec.task.as_ref().unwrap().as_str()},"pid":1,"created_ms":0,"cwd":"/tmp","foreground":{"pid":1,"name":"omp","cwd":"/tmp"},"reason":null,"name":null})).unwrap();
        (state, rec, term)
    }
    fn ctx(hc: FakeHc) -> Ctx {
        let root = PathBuf::from("/tmp");
        Ctx {
            paths: super::super::paths::CoordPaths::new(&root),
            root,
            hc: Box::new(hc),
            app: Box::new(FakeApp::default()),
            clock: Box::new(|| 100),
            op: "test".into(),
            cause: None,
            path: Vec::new(),
        }
    }
    #[test]
    fn tab_check_table() {
        let (mut state, rec, term) = setup();
        let hc = FakeHc::default();
        for _ in 0..5 {
            hc.gets.borrow_mut().push_back(Ok(Some(term.clone())));
        }
        let mut busy = term.clone();
        busy.status.as_mut().unwrap().state = "working".into();
        hc.gets.borrow_mut().push_back(Ok(Some(busy)));
        let ctx = ctx(hc);
        assert!(check_tab(&ctx, &state, &rec).unwrap().iter().all(|c| c.ok));
        state
            .tasks
            .get_mut(rec.task.as_ref().unwrap())
            .unwrap()
            .reported_seq = None;
        assert!(!check_tab(&ctx, &state, &rec).unwrap()[2].ok);
        let task = state.tasks.get_mut(rec.task.as_ref().unwrap()).unwrap();
        task.reported_seq = Some(2);
        task.session.as_mut().unwrap().mode = SessionMode::None;
        assert!(!check_tab(&ctx, &state, &rec).unwrap()[3].ok);
        state
            .workstreams
            .get_mut(rec.ws.as_ref().unwrap())
            .unwrap()
            .state = WsState::Closed;
        assert!(check_tab(&ctx, &state, &rec).unwrap()[3].ok);
        let mut adopted = rec.clone();
        adopted.owned = false;
        assert!(!check_tab(&ctx, &state, &adopted).unwrap()[0].ok);
        assert!(!check_tab(&ctx, &state, &rec).unwrap()[1].ok);
    }
    #[test]
    fn space_person_terminal_and_tabs() {
        let (mut state, mut rec, term) = setup();
        state.resources.push(rec.clone());
        rec.r.r#type = ResType::Space;
        rec.r.name = "sp".into();
        rec.task = None;
        let app = AppSnapshot {
            spaces: vec![AppSpace {
                id: "sp".into(),
                name: String::new(),
                folder: String::new(),
                machine: None,
                tabs: vec![AppTab {
                    id: "tb".into(),
                    name: String::new(),
                    terminals: vec![
                        AppTerm {
                            terminal: "t1".into(),
                            pane: "p".into(),
                        },
                        AppTerm {
                            terminal: "person".into(),
                            pane: "p2".into(),
                        },
                    ],
                }],
            }],
        };
        let hc = FakeHc::default();
        hc.gets.borrow_mut().push_back(Ok(Some(term.clone())));
        let mut person = term;
        person.labels.clear();
        hc.gets.borrow_mut().push_back(Ok(Some(person)));
        let checks = check_space(&ctx(hc), &state, &rec, &app).unwrap();
        assert!(checks[0].ok);
        assert!(!checks[1].ok);
        assert!(!checks[2].ok);
    }
    #[test]
    fn retained_reason_mapping() {
        for (name, expected) in [
            ("clean", "uncommitted_files"),
            ("reachable", "unpushed_commits"),
            ("merged", "branch_not_merged"),
            ("no_person_terminal", "in_use_by_person"),
            ("idle", "agent_busy"),
            ("result_recorded", "no_result"),
            ("session_recorded", "no_session"),
            ("nothing_running", "program_running"),
            ("tabs_released", "tabs_open"),
            ("no_terminal_inside", "terminal_in_worktree"),
            ("owned", "owned"),
        ] {
            assert_eq!(
                retained(vec![check(name, false, None)], 42).reason_code,
                expected
            );
        }
    }
    #[test]
    fn recovered_result_requires_finished_owned_terminal_and_preserves_report_provenance() {
        for scenario in [
            "finished",
            "live",
            "unknown",
            "missing",
            "wrong_owner",
            "lookup_error",
            "visible",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let paths = super::super::paths::CoordPaths::new(temp.path());
            let j = Journal::open(&paths).unwrap();
            let (_, rec, mut term) = setup();
            let ws = rec.ws.clone().unwrap();
            let task = rec.task.clone().unwrap();
            let source = TaskId::new(&ws, 2);
            let result = temp.path().join("recovered.md");
            std::fs::write(&result, "independent recovery").unwrap();
            let mut lock = j.lock().unwrap();
            let entries = [
                (None, json!({"kind":"ws.opened","data":{"title":"work"}})),
                (
                    Some(task.clone()),
                    json!({"kind":"task.placed","data":{"rung":2,"reason":"","rejected":[],"role":"r","harness":"omp","title":"task","brief":"/tmp/brief"}}),
                ),
                (
                    Some(task.clone()),
                    json!({"kind":"res.created","data":rec.r}),
                ),
                (
                    Some(task.clone()),
                    json!({"kind":"task.started","data":{"terminal":"t1","tab":"tb","tab_name":"tb","space":"sp","cwd":"/tmp","pid":1,"created_ms":0,"session":{"mode":"dir","id":"session"},"resumed":false}}),
                ),
                (
                    Some(task.clone()),
                    json!({"kind":"task.closed","data":{"reason":"superseded"}}),
                ),
                (
                    Some(source.clone()),
                    json!({"kind":"task.placed","data":{"rung":2,"reason":"","rejected":[],"role":"r","harness":"omp","title":"recovery","brief":"/tmp/brief"}}),
                ),
                (
                    Some(source.clone()),
                    json!({"kind":"task.started","data":{"terminal":"t2","tab_name":"source","space":"sp","cwd":"/tmp","pid":2,"created_ms":0,"resumed":false}}),
                ),
                (
                    Some(source.clone()),
                    json!({"kind":"task.reported","data":{"outcome":"done","summary":"recovered","result":result}}),
                ),
                (
                    None,
                    json!({"kind":"ws.closed","data":{"summary":"closed"}}),
                ),
            ];
            for (tid, value) in entries {
                lock.append(
                    &Actor::Cli,
                    1,
                    vec![(
                        Some(ws.clone()),
                        tid,
                        serde_json::from_value(value).unwrap(),
                    )],
                )
                .unwrap();
            }
            drop(lock);
            term.state = match scenario {
                "live" => "live",
                "unknown" => "unknown",
                _ => "finished",
            }
            .into();
            term.foreground = None;
            if scenario == "wrong_owner" {
                term.labels
                    .insert("coord.task".into(), source.as_str().into());
            }
            let hc = FakeHc::default();
            hc.gets.borrow_mut().push_back(match scenario {
                "missing" => Ok(None),
                "lookup_error" => Err(anyhow::anyhow!("lookup failed")),
                _ => Ok(Some(term.clone())),
            });
            if scenario == "finished" {
                for _ in 0..3 {
                    hc.gets.borrow_mut().push_back(Ok(Some(term.clone())));
                }
            }
            let app = FakeApp::default();
            let spaces = if scenario == "visible" {
                vec![AppSpace {
                    id: "sp".into(),
                    name: String::new(),
                    folder: String::new(),
                    machine: None,
                    tabs: vec![AppTab {
                        id: "tb".into(),
                        name: String::new(),
                        terminals: vec![],
                    }],
                }]
            } else {
                vec![]
            };
            app.snapshots
                .borrow_mut()
                .push_back(Ok(AppSnapshot { spaces }));
            app.snapshots
                .borrow_mut()
                .push_back(Ok(AppSnapshot { spaces: vec![] }));
            let mut ctx = ctx(hc);
            ctx.root = temp.path().into();
            ctx.paths = paths;
            ctx.app = Box::new(app);
            assert!(reconcile_result(&ctx, &j, &Actor::Cli, &task, &source, &result).is_err());
            let outcome = reconcile_result(&ctx, &j, &Actor::Coordinator, &task, &source, &result);
            if scenario != "finished" {
                assert!(outcome.is_err(), "{scenario}");
                assert!(
                    j.read_state().unwrap().tasks[&task]
                        .reconciled_result_seq
                        .is_none()
                );
                continue;
            }
            outcome.unwrap();
            let state = j.read_state().unwrap();
            assert_eq!(state.tasks[&task].state, TaskState::Closed);
            assert!(state.tasks[&task].report.is_none());
            assert!(state.tasks[&task].reported_seq.is_none());
            assert!(state.tasks[&task].reconciled_result_seq.is_some());
            let hc_live = FakeHc::default();
            let mut live = term.clone();
            live.state = "live".into();
            hc_live.gets.borrow_mut().push_back(Ok(Some(live)));
            assert!(!check_tab(&self::ctx(hc_live), &state, &state.resources[0]).unwrap()[2].ok);
            let entries = j.read_after(0).unwrap();
            let event = entries.last().unwrap();
            assert_eq!(event.kind, "task.result_reconciled");
            assert_eq!(event.actor, Actor::Coordinator);
            assert_eq!(event.data["source_task"], source.as_str());
            assert_eq!(event.data["result"], result.to_str().unwrap());
            assert!(
                release_ws(&ctx, &j, &Actor::Coordinator, &ws, Mode::Command)
                    .unwrap()
                    .cleaned
            );
            assert!(j.read_state().unwrap().resources[0].released);
        }
    }
    #[test]
    fn send_after_initial_check_prevents_kill() {
        let temp = tempfile::tempdir().unwrap();
        let j = Journal::open(&super::super::paths::CoordPaths::new(temp.path())).unwrap();
        let (_, rec, term) = setup();
        let ws = rec.ws.clone().unwrap();
        let task = rec.task.clone().unwrap();
        let kinds = [
            json!({"kind":"ws.opened","data":{"title":"work"}}),
            json!({"kind":"task.placed","data":{"rung":2,"reason":"","rejected":[],"role":"r","harness":"omp","title":"task","brief":"/tmp/brief"}}),
            json!({"kind":"res.created","data":rec.r}),
            json!({"kind":"task.started","data":{"terminal":"t1","tab":"tb","tab_name":"tb","space":"sp","cwd":"/tmp","pid":1,"created_ms":0,"session":{"mode":"dir","id":"session"},"resumed":false}}),
            json!({"kind":"task.reported","data":{"outcome":"done","summary":"done"}}),
        ];
        let mut lock = j.lock().unwrap();
        for (i, value) in kinds.into_iter().enumerate() {
            lock.append(
                &Actor::Cli,
                1,
                vec![(
                    Some(ws.clone()),
                    if i == 0 { None } else { Some(task.clone()) },
                    serde_json::from_value(value).unwrap(),
                )],
            )
            .unwrap();
        }
        drop(lock);
        let hc = FakeHc::default();
        for _ in 0..3 {
            hc.gets.borrow_mut().push_back(Ok(Some(term.clone())));
        }
        hc.mutations
            .borrow_mut()
            .push_back(Err(anyhow::anyhow!("kill must not occur")));
        let ctx = ctx(hc);
        let initial = j.read_state().unwrap();
        assert!(
            check_tab(&ctx, &initial, &initial.resources[0])
                .unwrap()
                .iter()
                .all(|c| c.ok)
        );
        journal::transact(&j, &Actor::Cli, 2, |_| {
            Ok((
                vec![(
                    Some(ws),
                    Some(task.clone()),
                    Kind::TaskSent {
                        message: "/tmp/msg".into(),
                        via: super::super::kinds::SendVia::Send,
                        terminal: "t1".into(),
                        started: None,
                    },
                )],
                (),
            ))
        })
        .unwrap();
        let checks = release_task_tab(&ctx, &j, &Actor::Cli, &task).unwrap();
        assert!(checks.iter().any(|c| !c.ok));
        assert!(!j.read_state().unwrap().resources[0].released);
    }
    #[test]
    fn recheck_records_only_changed_results() {
        let temp = tempfile::tempdir().unwrap();
        let paths = super::super::paths::CoordPaths::new(temp.path());
        let j = Journal::open(&paths).unwrap();
        let (_, rec, _) = setup();
        let id = rec.ws.clone().unwrap();
        journal::transact(&j, &Actor::Cli, 1, |_| {
            Ok((
                vec![
                    (
                        Some(id.clone()),
                        None,
                        Kind::WsOpened {
                            title: "work".into(),
                            repo: None,
                            delivery_target: None,
                            delivery_branch: None,
                            purpose: None,
                            request: None,
                        },
                    ),
                    (
                        Some(id.clone()),
                        rec.task.clone(),
                        Kind::ResIntent(rec.r.clone()),
                    ),
                    (
                        Some(id.clone()),
                        rec.task.clone(),
                        Kind::ResCreated(rec.r.clone()),
                    ),
                ],
                (),
            ))
        })
        .unwrap();
        let checks = vec![check("idle", false, None), check("owned", true, None)];
        let mut lock = j.lock().unwrap();
        assert!(record_checks(&mut lock, &Actor::Cli, 2, &rec, &checks, Mode::Recheck).unwrap());
        assert!(!record_checks(&mut lock, &Actor::Cli, 3, &rec, &checks, Mode::Recheck).unwrap());
        let changed = vec![check("idle", true, None), check("owned", true, None)];
        assert!(record_checks(&mut lock, &Actor::Cli, 4, &rec, &changed, Mode::Recheck).unwrap());
        assert!(record_checks(&mut lock, &Actor::Cli, 5, &rec, &changed, Mode::Command).unwrap());
    }
}
