//! Periodic reconciliation of journal intents and external resources.
use super::{
    Ctx,
    adapter::{AppSnapshot, AppSpace, Term},
    git,
    ids::{TaskId, WsId},
    journal::{Journal, Locked},
    kinds::{Actor, Check, Entry, Kind, ResType, SessionRef},
    launch,
    release::{self, Mode},
    rules::Facts,
    state::{Intent, ResourceRec, State, TaskState, WsState},
};
use anyhow::Result;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

/// An intent older than this was not completed by the command that recorded it.
const STALE_INTENT_MS: u64 = 120_000;

pub fn reconcile(ctx: &Ctx, j: &Journal, facts: &mut Facts) -> Result<Vec<Entry>> {
    let before = j.last_seq()?;
    refresh_terminals(ctx, facts)?;
    let app = ctx.app.ls().ok();
    let history = j.read_after(0)?;
    let mut lock = j.lock()?;
    let state = lock.state().clone();
    for intent in state.intents.values().filter(|i| {
        (i.ws.is_some() || !matches!(i.r.r#type, ResType::Space | ResType::Tab))
            && ctx.now().saturating_sub(i.ts_ms) >= STALE_INTENT_MS
    }) {
        settle_intent(ctx, &mut lock, intent, &facts.terms, app.as_ref(), &history)?;
    }
    discover_sessions(ctx, &mut lock, &state)?;
    release_removed_worktrees(ctx, &mut lock, &state)?;
    release_gone_resources(ctx, &mut lock, &state, app.as_ref())?;
    drop(lock);
    if let Some(app) = &app {
        for rec in state.resources.iter().filter(|r| r.owned && !r.released) {
            match rec.r.r#type {
                ResType::Space => {
                    if let Some(space) = app.space(&rec.r.name) {
                        align_space(ctx, j, &state, &history, space, rec)?;
                    }
                }
                ResType::Tab => align_tab(ctx, &state, app, rec)?,
                _ => {}
            }
        }
    }
    for ws in state
        .workstreams
        .values()
        .filter(|w| w.state == WsState::Closed && w.retained.is_some())
    {
        release::release_ws(ctx, j, &Actor::Watcher, &ws.id, Mode::Recheck)?;
    }
    j.read_after(before)
}

fn note(
    ctx: &Ctx,
    lock: &mut Locked<'_>,
    ws: &Option<WsId>,
    task: &Option<TaskId>,
    kind: Kind,
) -> Result<()> {
    lock.append(
        &Actor::Watcher,
        ctx.now(),
        vec![(ws.clone(), task.clone(), kind)],
    )?;
    Ok(())
}

fn refresh_terminals(ctx: &Ctx, facts: &mut Facts) -> Result<()> {
    let addresses = ctx.hc.list(true, Some(("coord.managed", "1")))?;
    let mut terms = BTreeMap::new();
    for address in addresses {
        if let Some(term) = ctx.hc.get(&address)? {
            terms.insert(address, term);
        }
    }
    facts.terms = terms;
    facts.initialized = true;
    Ok(())
}

/// Records a stale intent as created when its resource exists, as never created otherwise.
fn settle_intent(
    ctx: &Ctx,
    lock: &mut Locked<'_>,
    intent: &Intent,
    terms: &BTreeMap<String, Term>,
    app: Option<&AppSnapshot>,
    history: &[Entry],
) -> Result<()> {
    let runs_task = |t: &Term| {
        t.managed()
            && intent
                .task
                .as_ref()
                .is_some_and(|id| t.label("coord.task") == Some(id.as_str()))
    };
    let mut resource = intent.r.clone();
    let matching = terms.values().find(|t| {
        runs_task(t)
            && resource
                .space
                .as_deref()
                .is_none_or(|s| t.label("herdr.space") == Some(s))
    });
    let exists = match resource.r#type {
        ResType::Worktree => match &resource.repo {
            Some(repo) => git::worktree_list(repo)?.iter().any(|w| {
                w.path == Path::new(&resource.name)
                    && resource
                        .branch
                        .as_ref()
                        .is_none_or(|b| w.branch.as_ref() == Some(b))
            }),
            None => false,
        },
        ResType::Space => terms
            .values()
            .any(|t| runs_task(t) && t.label("herdr.space") == Some(resource.name.as_str())),
        ResType::Tab => matching.is_some(),
        ResType::Branch => match &resource.repo {
            Some(repo) => git::branch_exists(repo, &resource.name)?,
            None => false,
        },
    };
    if !exists {
        return abandon_intent(ctx, lock, intent);
    }
    // A tab or branch is only adopted with the provenance its intent recorded.
    let mut launched = None;
    match resource.r#type {
        ResType::Tab => {
            let placed = history
                .iter()
                .rev()
                .find(|e| e.task == intent.task && e.kind == "task.placed")
                .and_then(|e| e.data.get("session"));
            let Some(placed) = placed else {
                return provenance_missing(ctx, lock, intent, history);
            };
            let session = serde_json::from_value::<Option<SessionRef>>(placed.clone())?;
            let term = matching.expect("matched terminal");
            resource.name = term.address.clone();
            resource.tab = app
                .and_then(|a| a.tab_of(&term.address))
                .map(|(_, t)| t.id.clone())
                .or_else(|| term.label("herdr.tab").map(str::to_owned));
            launched = Some((term, session));
        }
        ResType::Branch => {
            let base_sha = history
                .iter()
                .find(|e| e.seq == intent.seq)
                .and_then(|e| e.data.get("base_sha"))
                .and_then(|v| v.as_str());
            let Some(base_sha) = base_sha else {
                return provenance_missing(ctx, lock, intent, history);
            };
            resource.base = Some(base_sha.to_owned());
        }
        ResType::Space | ResType::Worktree => {}
    }
    let tab = resource.tab.clone();
    note(
        ctx,
        lock,
        &intent.ws,
        &intent.task,
        Kind::ResCreated(resource),
    )?;
    let (Some(id), Some((term, session))) = (&intent.task, launched) else {
        return Ok(());
    };
    let Some(title) = lock
        .state()
        .tasks
        .get(id)
        .filter(|t| t.state == TaskState::Placed)
        .map(|t| t.title.clone())
    else {
        return Ok(());
    };
    let cwd = Path::new(&term.cwd);
    let session = session
        .map(|base| launch::discover_session(ctx, &base, cwd, term.created_ms, 0).unwrap_or(base));
    note(
        ctx,
        lock,
        &intent.ws,
        &intent.task,
        Kind::TaskStarted {
            terminal: term.address.clone(),
            tab,
            tab_name: title,
            space: term.label("herdr.space").unwrap_or_default().into(),
            cwd: cwd.into(),
            pid: term.pid,
            created_ms: term.created_ms,
            session,
            resumed: false,
        },
    )
}

fn provenance_missing(
    ctx: &Ctx,
    lock: &mut Locked<'_>,
    intent: &Intent,
    history: &[Entry],
) -> Result<()> {
    let summary = format!("Intent {} lacks recovery provenance", intent.seq);
    let reported = history.iter().any(|e| {
        matches!(e.typed(), Kind::ReconcileFinding { code, summary: s }
            if code == "intent_provenance_missing" && s == summary)
    });
    if reported {
        return Ok(());
    }
    note(
        ctx,
        lock,
        &intent.ws,
        &intent.task,
        Kind::ReconcileFinding {
            code: "intent_provenance_missing".into(),
            summary,
        },
    )
}

fn abandon_intent(ctx: &Ctx, lock: &mut Locked<'_>, intent: &Intent) -> Result<()> {
    note(
        ctx,
        lock,
        &intent.ws,
        &intent.task,
        Kind::ResReleased {
            r#type: intent.r.r#type,
            name: intent.r.name.clone(),
            note: Some("never created".into()),
        },
    )?;
    let placed = intent.r.r#type == ResType::Tab
        && intent.task.as_ref().is_some_and(|id| {
            lock.state()
                .tasks
                .get(id)
                .is_some_and(|t| t.state == TaskState::Placed)
        });
    if placed {
        note(
            ctx,
            lock,
            &intent.ws,
            &intent.task,
            Kind::TaskClosed {
                reason: "launch interrupted".into(),
                outcome: None,
            },
        )?;
    }
    Ok(())
}

/// Picks up session files that appeared after their task was started.
fn discover_sessions(ctx: &Ctx, lock: &mut Locked<'_>, state: &State) -> Result<()> {
    for task in state.tasks.values() {
        let (Some(session), Some(cwd)) = (&task.session, &task.cwd) else {
            continue;
        };
        if session.id.is_some() || task.state == TaskState::Closed {
            continue;
        }
        if let Some(found) =
            launch::discover_session(ctx, session, cwd, task.created_ms.unwrap_or(0), 0)
                .filter(|s| s.id.is_some())
        {
            lock.append(
                &Actor::Watcher,
                ctx.now(),
                vec![(
                    Some(task.ws.clone()),
                    Some(task.id.clone()),
                    Kind::TaskSession { session: found },
                )],
            )?;
        }
    }
    Ok(())
}

fn release_removed_worktrees(ctx: &Ctx, lock: &mut Locked<'_>, state: &State) -> Result<()> {
    for rec in state
        .resources
        .iter()
        .filter(|r| r.owned && !r.released && r.r.r#type == ResType::Worktree)
    {
        let Some(repo) = &rec.r.repo else { continue };
        if git::worktree_list(repo)?
            .iter()
            .any(|w| w.path == Path::new(&rec.r.name))
        {
            continue;
        }
        lock.append(
            &Actor::Watcher,
            ctx.now(),
            vec![
                (
                    rec.ws.clone(),
                    rec.task.clone(),
                    Kind::ResReleased {
                        r#type: ResType::Worktree,
                        name: rec.r.name.clone(),
                        note: Some("removed outside".into()),
                    },
                ),
                (
                    rec.ws.clone(),
                    rec.task.clone(),
                    Kind::ReconcileFinding {
                        code: "worktree_removed".into(),
                        summary: format!("Worktree {} was removed outside", rec.r.name),
                    },
                ),
            ],
        )?;
    }
    Ok(())
}

fn release_gone_resources(
    ctx: &Ctx,
    lock: &mut Locked<'_>,
    state: &State,
    app: Option<&AppSnapshot>,
) -> Result<()> {
    // Coordinator space and tab identities are retained for lifecycle restarts.
    for rec in state.resources.iter().filter(|r| {
        (r.ws.is_some() || !matches!(r.r.r#type, ResType::Space | ResType::Tab))
            && r.owned
            && !r.released
            && r.r.r#type != ResType::Worktree
    }) {
        let gone = match rec.r.r#type {
            ResType::Tab => ctx.hc.get(&rec.r.name)?.is_none(),
            ResType::Space => app.is_some_and(|a| a.space(&rec.r.name).is_none()),
            ResType::Branch => match &rec.r.repo {
                Some(repo) => !git::branch_exists(repo, &rec.r.name)?,
                None => false,
            },
            ResType::Worktree => false,
        };
        if gone {
            note(
                ctx,
                lock,
                &rec.ws,
                &rec.task,
                Kind::ResReleased {
                    r#type: rec.r.r#type,
                    name: rec.r.name.clone(),
                    note: Some("gone".into()),
                },
            )?;
        }
    }
    Ok(())
}

/// Restores the space name and reports person terminals appearing in or leaving it.
fn align_space(
    ctx: &Ctx,
    j: &Journal,
    state: &State,
    history: &[Entry],
    space: &AppSpace,
    rec: &ResourceRec,
) -> Result<()> {
    if let Some(name) = &rec.r.label {
        if space.name != *name {
            ctx.app
                .command("space.rename", json!({"id":space.id,"name":name}))?;
        }
    }
    let mut person = false;
    for pane in space.tabs.iter().flat_map(|t| &t.terminals) {
        if !ctx.hc.get(&pane.terminal)?.is_some_and(|t| t.managed()) {
            person = true;
        }
    }
    let summary = format!("Person terminal in owned space {}", space.id);
    let last = history.iter().rev().find_map(|e| match e.typed() {
        Kind::ReconcileFinding { code, summary: s }
            if s == summary
                && (code == "person_terminal_in_owned_space"
                    || code == "person_terminal_cleared") =>
        {
            Some(code)
        }
        _ => None,
    });
    let reported = last.as_deref() == Some("person_terminal_in_owned_space");
    if !person {
        if reported {
            note(
                ctx,
                &mut j.lock()?,
                &rec.ws,
                &None,
                Kind::ReconcileFinding {
                    code: "person_terminal_cleared".into(),
                    summary,
                },
            )?;
        }
        return Ok(());
    }
    if !reported {
        note(
            ctx,
            &mut j.lock()?,
            &rec.ws,
            &None,
            Kind::ReconcileFinding {
                code: "person_terminal_in_owned_space".into(),
                summary: summary.clone(),
            },
        )?;
    }
    // Once retained, release::recheck owns the reason; replacing it here
    // would make the two paths alternate when another resource also blocks.
    let closed = rec
        .ws
        .as_ref()
        .and_then(|id| state.workstreams.get(id))
        .filter(|w| w.state == WsState::Closed && w.retained.is_none());
    if let Some(ws) = closed {
        note(
            ctx,
            &mut j.lock()?,
            &Some(ws.id.clone()),
            &None,
            Kind::WsRetained {
                reason_code: "in_use_by_person".into(),
                reason: summary,
                checks: vec![Check {
                    name: "no_person_terminal".into(),
                    ok: false,
                    detail: None,
                }],
            },
        )?;
    }
    Ok(())
}

fn align_tab(ctx: &Ctx, state: &State, app: &AppSnapshot, rec: &ResourceRec) -> Result<()> {
    let Some((_, tab)) = app.tab_of(&rec.r.name) else {
        return Ok(());
    };
    if let Some(task) = rec.task.as_ref().and_then(|id| state.tasks.get(id)) {
        if tab.name != task.title {
            ctx.app
                .command("tab.rename", json!({"id":tab.id,"name":task.title}))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::{
        adapter::fake::{FakeApp, FakeHc},
        kinds::ResourceRef,
        paths::CoordPaths,
    };
    #[test]
    fn late_session_file_is_discovered_once() {
        use super::super::kinds::SessionMode;
        let dir = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(dir.path());
        let j = Journal::open(&paths).unwrap();
        let ws = WsId::parse("ws-abcdef").unwrap();
        let task = TaskId::new(&ws, 1);
        let session_dir = paths.session_dir(task.as_str());
        std::fs::create_dir_all(&session_dir).unwrap();
        let session = SessionRef {
            mode: SessionMode::Dir,
            id: None,
            dir: Some(session_dir.clone()),
        };
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                1,
                vec![
                    (
                        Some(ws.clone()),
                        None,
                        Kind::WsOpened {
                            title: "test".into(),
                            repo: None,
                            delivery_target: None,
                            delivery_branch: None,
                            purpose: None,
                            request: None,
                        },
                    ),
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
                        Kind::TaskPlaced {
                            rung: 2,
                            reason: "test".into(),
                            rejected: vec![],
                            role: "r".into(),
                            harness: "omp".into(),
                            model: None,
                            title: "task".into(),
                            brief: "/tmp/brief".into(),
                            session_of: None,
                            session: Some(session.clone()),
                            request: None,
                            follows: None,
                            role_hash: String::new(),
                            brief_bytes: 0,
                        },
                    ),
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
                        Kind::TaskStarted {
                            terminal: "t1".into(),
                            tab: None,
                            tab_name: "task".into(),
                            space: "sp".into(),
                            cwd: dir.path().into(),
                            pid: Some(1),
                            created_ms: 1,
                            session: Some(session),
                            resumed: false,
                        },
                    ),
                ],
            )
            .unwrap();
        let hc = FakeHc::default();
        for _ in 0..3 {
            hc.lists.borrow_mut().push_back(Ok(vec![]));
        }
        let ctx = Ctx {
            root: dir.path().into(),
            paths,
            hc: Box::new(hc),
            app: Box::new(FakeApp::default()),
            clock: Box::new(|| 121_001),
            op: "test".into(),
            cause: None,
        };
        let mut facts = Facts::default();
        reconcile(&ctx, &j, &mut facts).unwrap();
        assert!(
            j.read_state().unwrap().tasks[&task]
                .session
                .as_ref()
                .unwrap()
                .id
                .is_none()
        );
        std::fs::write(session_dir.join("2026-11-13_late-session.jsonl"), b"{}\n").unwrap();
        reconcile(&ctx, &j, &mut facts).unwrap();
        assert_eq!(
            j.read_state().unwrap().tasks[&task]
                .session
                .as_ref()
                .unwrap()
                .id
                .as_deref(),
            Some("late-session")
        );
        reconcile(&ctx, &j, &mut facts).unwrap();
        assert_eq!(
            j.read_after(0)
                .unwrap()
                .iter()
                .filter(|e| e.kind == "task.session")
                .count(),
            1
        );
    }
    #[test]
    fn missing_owned_tab_completes_interrupted_release() {
        let dir = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(dir.path());
        let j = Journal::open(&paths).unwrap();
        let rec = ResourceRef {
            space: Some("sp".into()),
            ..ResourceRef::new(ResType::Tab, "t1")
        };
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                1,
                vec![(
                    Some(WsId::parse("ws-abcdef").unwrap()),
                    None,
                    Kind::ResCreated(rec),
                )],
            )
            .unwrap();
        let hc = FakeHc::default();
        hc.lists.borrow_mut().push_back(Ok(vec![]));
        hc.gets.borrow_mut().push_back(Ok(None));
        let ctx = Ctx {
            root: dir.path().into(),
            paths,
            hc: Box::new(hc),
            app: Box::new(FakeApp::default()),
            clock: Box::new(|| 121_001),
            op: "test".into(),
            cause: None,
        };
        reconcile(&ctx, &j, &mut Facts::default()).unwrap();
        assert!(j.read_state().unwrap().resources[0].released);
    }
    #[test]
    fn person_space_finding_reopens_after_clear() {
        let dir = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(dir.path());
        let j = Journal::open(&paths).unwrap();
        let ws = WsId::parse("ws-abcdef").unwrap();
        let rec = ResourceRef {
            folder: Some("/tmp".into()),
            label: Some("space".into()),
            ..ResourceRef::new(ResType::Space, "sp")
        };
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                1,
                vec![
                    (
                        Some(ws.clone()),
                        None,
                        Kind::WsOpened {
                            title: "test".into(),
                            repo: None,
                            delivery_target: None,
                            delivery_branch: None,
                            purpose: None,
                            request: None,
                        },
                    ),
                    (Some(ws), None, Kind::ResCreated(rec)),
                ],
            )
            .unwrap();
        let hc = FakeHc::default();
        for _ in 0..4 {
            hc.lists.borrow_mut().push_back(Ok(vec![]));
        }
        let person: super::super::adapter::Term = serde_json::from_value(json!({"address":"person","state":"live","status":null,"status_ms":null,"turn":0,"reported":false,"labels":{},"pid":1,"created_ms":1,"cwd":"/tmp","foreground":null,"reason":null,"name":null})).unwrap();
        for _ in 0..3 {
            hc.gets.borrow_mut().push_back(Ok(Some(person.clone())));
        }
        let app = FakeApp::default();
        for occupied in [true, true, false, true] {
            let terminals = if occupied {
                vec![json!({"terminal":"person","pane":"p"})]
            } else {
                vec![]
            };
            app.snapshots.borrow_mut().push_back(Ok(serde_json::from_value(json!({"spaces":[{"id":"sp","name":"space","folder":"/tmp","machine":null,"tabs":[{"id":"tb","name":"tab","terminals":terminals}]}]})).unwrap()));
        }
        let ctx = Ctx {
            root: dir.path().into(),
            paths,
            hc: Box::new(hc),
            app: Box::new(app),
            clock: Box::new(|| 121_001),
            op: "test".into(),
            cause: None,
        };
        let mut facts = Facts::default();
        for _ in 0..4 {
            reconcile(&ctx, &j, &mut facts).unwrap();
        }
        let entries = j.read_after(0).unwrap();
        assert_eq!(entries.iter().filter(|e| matches!(e.typed(), Kind::ReconcileFinding { code, .. } if code == "person_terminal_in_owned_space")).count(), 2);
        assert_eq!(entries.iter().filter(|e| matches!(e.typed(), Kind::ReconcileFinding { code, .. } if code == "person_terminal_cleared")).count(), 1);
    }
    #[test]
    fn tab_recovery_respects_recorded_session_provenance() {
        for (live, provenance) in [(false, false), (true, false), (true, true)] {
            let dir = tempfile::tempdir().unwrap();
            let paths = CoordPaths::new(dir.path());
            let j = Journal::open(&paths).unwrap();
            let ws = WsId::parse("ws-abcdef").unwrap();
            let task = TaskId::new(&ws, 1);
            let intent = ResourceRef {
                space: Some("sp".into()),
                ..ResourceRef::new(ResType::Tab, task.as_str())
            };
            let space = ResourceRef {
                folder: Some("/tmp".into()),
                label: Some("test".into()),
                ..ResourceRef::new(ResType::Space, "sp")
            };
            j.lock()
                .unwrap()
                .append(
                    &Actor::Watcher,
                    1,
                    vec![
                        (
                            Some(ws.clone()),
                            None,
                            Kind::WsOpened {
                                title: "test".into(),
                                repo: None,
                                delivery_target: None,
                                delivery_branch: None,
                                purpose: None,
                                request: None,
                            },
                        ),
                        (
                            Some(ws.clone()),
                            Some(task.clone()),
                            Kind::TaskPlaced {
                                rung: 2,
                                reason: "test".into(),
                                rejected: vec![],
                                role: "r".into(),
                                harness: "claude".into(),
                                model: None,
                                title: "task".into(),
                                brief: "/tmp/brief".into(),
                                session_of: None,
                                session: None,
                                request: None,
                                follows: None,
                                role_hash: String::new(),
                                brief_bytes: 0,
                            },
                        ),
                        (Some(ws.clone()), Some(task.clone()), Kind::ResIntent(space)),
                        (
                            Some(ws.clone()),
                            Some(task.clone()),
                            Kind::ResIntent(intent),
                        ),
                    ],
                )
                .unwrap();
            let mut entries = j.read_after(0).unwrap();
            if provenance {
                entries[1]
                    .data
                    .insert("session".into(), json!({"mode":"preset","id":"saved-id"}));
            } else {
                entries[1].data.remove("session");
            }
            let text = entries
                .iter()
                .map(|e| serde_json::to_string(e).unwrap() + "\n")
                .collect::<String>();
            std::fs::write(paths.month_file(1), text).unwrap();
            std::fs::remove_file(&paths.state).unwrap();
            let hc = FakeHc::default();
            hc.lists
                .borrow_mut()
                .push_back(Ok(if live { vec!["t1".into()] } else { vec![] }));
            if live {
                let term = serde_json::from_value(json!({"address":"t1","state":"live","status":null,"status_ms":null,"turn":0,"reported":false,"labels":{"coord.managed":"1","coord.task":task.as_str(),"herdr.space":"sp"},"pid":1,"created_ms":1,"cwd":"/tmp","foreground":null,"reason":null,"name":null})).unwrap();
                hc.gets.borrow_mut().push_back(Ok(Some(term)));
            }
            let ctx = Ctx {
                root: dir.path().into(),
                paths,
                hc: Box::new(hc),
                app: Box::new(FakeApp::default()),
                clock: Box::new(|| 121_001),
                op: "test".into(),
                cause: None,
            };
            reconcile(&ctx, &j, &mut Facts::default()).unwrap();
            let state = j.read_state().unwrap();
            assert_eq!(
                state
                    .resources
                    .iter()
                    .any(|r| r.r.r#type == ResType::Space && r.r.name == "sp"),
                live
            );
            if !live {
                assert!(state.intents.is_empty());
                assert_eq!(state.tasks[&task].state, TaskState::Closed);
            } else if !provenance {
                assert_eq!(state.intents.len(), 1);
                assert!(j.read_after(0).unwrap().iter().any(|e| matches!(e.typed(), Kind::ReconcileFinding { code, .. } if code == "intent_provenance_missing")));
            } else {
                assert!(state.intents.is_empty());
                assert_eq!(
                    state.tasks[&task]
                        .session
                        .as_ref()
                        .and_then(|s| s.id.as_deref()),
                    Some("saved-id")
                );
            }
        }
    }
    #[test]
    fn worktree_and_branch_intents() {
        const CHILD: &str = "HERDR_KIT_RECONCILE_GIT_TEST";
        if std::env::var_os(CHILD).is_none() {
            let home = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "coord::reconcile::tests::worktree_and_branch_intents",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("HOME", home.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = crate::git_env::git()
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ]);
        let base = git(&["rev-parse", "HEAD"]);
        let wt = dir.path().join("feature");
        git(&["worktree", "add", "-b", "feature", wt.to_str().unwrap()]);
        git(&["branch", "legacy"]);
        let paths = CoordPaths::new(dir.path());
        let j = Journal::open(&paths).unwrap();
        let resource = |typ, name: String, branch: Option<String>| ResourceRef {
            repo: Some(repo.clone()),
            branch,
            create_branch: Some(true),
            ..ResourceRef::new(typ, name)
        };
        let existing = resource(
            ResType::Worktree,
            wt.to_string_lossy().into(),
            Some("feature".into()),
        );
        let absent = resource(
            ResType::Worktree,
            dir.path().join("absent").to_string_lossy().into(),
            Some("missing".into()),
        );
        let branch = resource(ResType::Branch, "feature".into(), None);
        let absent_branch = resource(ResType::Branch, "missing".into(), None);
        let legacy = resource(ResType::Branch, "legacy".into(), None);
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                1,
                [existing, absent, branch, absent_branch, legacy]
                    .into_iter()
                    .map(|r| (None, None, Kind::ResIntent(r)))
                    .collect(),
            )
            .unwrap();
        let mut entries = j.read_after(0).unwrap();
        entries[2].data.insert("base_sha".into(), json!(base));
        std::fs::write(
            paths.month_file(1),
            entries
                .iter()
                .map(|e| serde_json::to_string(e).unwrap() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        std::fs::remove_file(&paths.state).unwrap();
        let hc = FakeHc::default();
        hc.lists.borrow_mut().push_back(Ok(vec![]));
        let ctx = Ctx {
            root: dir.path().into(),
            paths,
            hc: Box::new(hc),
            app: Box::new(FakeApp::default()),
            clock: Box::new(|| 121_001),
            op: "test".into(),
            cause: None,
        };
        reconcile(&ctx, &j, &mut Facts::default()).unwrap();
        let state = j.read_state().unwrap();
        assert_eq!(state.intents.len(), 1);
        assert_eq!(state.intents.values().next().unwrap().r.name, "legacy");
        assert!(j.read_after(0).unwrap().iter().any(|e| matches!(e.typed(), Kind::ReconcileFinding { code, .. } if code == "intent_provenance_missing")));
        assert!(
            state
                .resources
                .iter()
                .any(|r| r.r.r#type == ResType::Worktree && r.r.name == wt.to_string_lossy())
        );
        assert!(state.resources.iter().any(|r| r.r.r#type == ResType::Branch
            && r.r.name == "feature"
            && r.r.base.as_deref() == Some(base.as_str())));
        assert_eq!(
            j.read_after(0)
                .unwrap()
                .iter()
                .filter(|e| matches!(e.typed(), Kind::ResReleased { .. }))
                .count(),
            2
        );
    }
}
