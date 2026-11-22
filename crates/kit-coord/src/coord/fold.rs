//! Deterministic projection of journal entries.
use super::kinds::{AttentionState, Entry, Kind, NudgeTarget, ResType, ResourceRef, TurnOrigin};
use super::state::*;
use std::collections::BTreeMap;

fn anomaly(s: &mut State, e: &Entry, message: &str) {
    s.anomalies.push(Anomaly {
        seq: e.seq,
        message: message.to_owned(),
    });
}
fn same(a: &ResourceRef, b: &ResourceRef) -> bool {
    a.r#type == b.r#type
        && a.name == b.name
        && (!matches!(a.r#type, ResType::Branch | ResType::Worktree) || a.repo == b.repo)
}
fn counter(value: &str, counter: &mut u64) {
    if let Some(n) = value
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse::<u64>()
        .ok()
    {
        *counter = (*counter).max(n.saturating_add(1));
    }
}

fn block(t: &mut Task, e: &Entry, text: String, outcome: super::kinds::Outcome) {
    if let Some(item) = &mut t.blocked {
        item.text = text;
        item.outcome = outcome;
    } else {
        t.blocked = Some(BlockedEpisode {
            seq: e.seq,
            since_ms: e.ts_ms,
            text,
            outcome,
            reminders: 0,
            last_reminded_ms: e.ts_ms,
        });
    }
}
#[cfg(test)]
pub fn fold<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> State {
    let mut state = State::default();
    for entry in entries {
        apply(&mut state, entry);
    }
    state
}
/// Applies one journal entry: the bookkeeping every entry gets, the projection
/// of its kind, and the questions a blocked episode takes with it when it ends.
pub fn apply(s: &mut State, e: &Entry) {
    if e.seq <= s.last_seq {
        anomaly(s, e, "out-of-order or duplicate sequence");
        return;
    }
    s.last_seq = e.seq;
    s.last_ts_ms = e.ts_ms;
    let kind = e.typed();
    if kind.is_waking(&e.actor) && e.seq > s.cursor {
        s.waking.insert(e.seq);
    }
    if let Some(ws) = e.ws.as_ref().and_then(|id| s.workstreams.get_mut(id)) {
        ws.last_event_ms = e.ts_ms;
    }
    if let Some(task) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) {
        task.last_event_ms = e.ts_ms;
    }
    let prior_episode = e
        .task
        .as_ref()
        .and_then(|id| s.tasks.get(id))
        .and_then(|t| t.blocked.as_ref())
        .map(|item| item.seq);
    project(s, e, kind);
    if let Some((id, episode_seq)) = e.task.as_ref().zip(prior_episode) {
        close_episode_questions(s, e, id, episode_seq);
    }
}
/// Answers the open questions about a task's blocked episode once an entry has
/// ended the episode, so the person is not left with a question nobody waits on.
fn close_episode_questions(s: &mut State, e: &Entry, id: &super::ids::TaskId, episode_seq: u64) {
    if !s.tasks.get(id).is_some_and(|t| t.blocked.is_none()) {
        return;
    }
    for q in s.questions.values_mut().filter(|q| {
        q.task.as_ref() == Some(id) && q.asked_seq >= episode_seq && q.answered.is_none()
    }) {
        q.answered = Some(Answer {
            text: "Resolved by delegate state change.".into(),
            seq: e.seq,
            ts_ms: e.ts_ms,
        });
    }
}
/// Whether recording resource `r` with entry `e` fulfils the intent: it names
/// the same resource, or it is the tab of a task whose intent was written under
/// the task id before the terminal had an address.
fn fulfils(i: &Intent, e: &Entry, r: &ResourceRef) -> bool {
    i.ws == e.ws
        && (same(&i.r, r)
            || (r.r#type == ResType::Tab
                && i.r.r#type == ResType::Tab
                && i.task.is_some()
                && i.task == e.task
                && i.task.as_ref().is_some_and(|id| i.r.name == id.as_str())))
}
/// The projection of one entry by kind. An entry the state cannot accept is
/// recorded as an anomaly and changes nothing else.
fn project(s: &mut State, e: &Entry, kind: Kind) {
    match kind {
        Kind::WsOpened {
            title,
            repo,
            delivery_target,
            delivery_branch,
            purpose,
            request,
        } => {
            let Some(id) = &e.ws else {
                anomaly(s, e, "workstream missing");
                return;
            };
            if s.workstreams.contains_key(id) {
                anomaly(s, e, "duplicate workstream");
                return;
            }
            if let (Some(repo), Some(purpose)) = (&repo, purpose) {
                s.projects.insert(repo.clone(), Project { purpose });
            }
            s.workstreams.insert(
                id.clone(),
                Ws {
                    id: id.clone(),
                    title,
                    repo,
                    delivery_target,
                    delivery_branch,
                    state: WsState::Open,
                    retained: None,
                    summary: None,
                    opened_ms: e.ts_ms,
                    closed_ms: None,
                    last_event_ms: e.ts_ms,
                    space: None,
                    cwd: None,
                    tasks: Vec::new(),
                    request,
                    outcome: None,
                    rating: None,
                },
            );
        }
        Kind::WsClosed { summary, outcome } => {
            let Some(ws) = e.ws.as_ref().and_then(|id| s.workstreams.get_mut(id)) else {
                anomaly(s, e, "unknown workstream");
                return;
            };
            if ws.state != WsState::Open
                || ws.tasks.iter().any(|id| {
                    s.tasks
                        .get(id)
                        .is_some_and(|t| matches!(t.state, TaskState::Placed | TaskState::Running))
                })
            {
                anomaly(s, e, "invalid workstream close");
                return;
            }
            ws.state = WsState::Closed;
            ws.summary = Some(summary);
            ws.closed_ms = Some(e.ts_ms);
            ws.outcome = outcome;
        }
        Kind::WsRetained {
            reason_code,
            reason,
            checks,
        } => {
            let Some(ws) = e.ws.as_ref().and_then(|id| s.workstreams.get_mut(id)) else {
                anomaly(s, e, "unknown workstream");
                return;
            };
            if ws.state != WsState::Closed {
                anomaly(s, e, "invalid workstream retention");
                return;
            }
            ws.retained = Some(Retained {
                reason_code,
                reason,
                since_ms: e.ts_ms,
                checked_ms: Some(e.ts_ms),
                checks,
            });
        }
        Kind::WsCleaned {} => {
            let Some(ws) = e.ws.as_ref().and_then(|id| s.workstreams.get_mut(id)) else {
                anomaly(s, e, "unknown workstream");
                return;
            };
            if ws.state != WsState::Closed {
                anomaly(s, e, "invalid workstream cleanup");
                return;
            }
            ws.state = WsState::Cleaned;
            ws.retained = None;
            ws.space = None;
            ws.cwd = None;
        }
        Kind::TaskPlaced {
            rung,
            reason,
            rejected,
            role,
            harness,
            model,
            effort,
            title,
            brief,
            session_of: _,
            session,
            request,
            follows,
            role_hash,
            brief_bytes,
        } => {
            let (Some(id), Some(ws_id)) = (&e.task, &e.ws) else {
                anomaly(s, e, "task or workstream missing");
                return;
            };
            if id.ws() != *ws_id || s.tasks.contains_key(id) || !s.workstreams.contains_key(ws_id) {
                anomaly(s, e, "invalid or duplicate task");
                return;
            }
            s.workstreams.get_mut(ws_id).unwrap().tasks.push(id.clone());
            s.tasks.insert(
                id.clone(),
                Task {
                    id: id.clone(),
                    ws: ws_id.clone(),
                    n: id.n(),
                    title,
                    state: TaskState::Placed,
                    report: None,
                    start_blocked: None,
                    stalled: None,
                    attention: None,
                    blocked: None,
                    start_blocked_ms: None,
                    stalled_ms: None,
                    state_since_ms: e.ts_ms,
                    placed_ms: e.ts_ms,
                    started_ms: None,
                    last_event_ms: e.ts_ms,
                    rung,
                    reason,
                    rejected,
                    role,
                    harness,
                    model,
                    effort,
                    summary: None,
                    terminal: None,
                    space: None,
                    tab: None,
                    cwd: None,
                    pid: None,
                    created_ms: None,
                    session,
                    brief,
                    result: None,
                    nudges: 0,
                    last_nudge: None,
                    last_activity_seq: 0,
                    reported_seq: None,
                    request,
                    follows,
                    role_hash,
                    brief_bytes,
                    outcome: None,
                    rating: None,
                    turns: 0,
                    person_turns: 0,
                    working_ms: 0,
                },
            );
        }
        Kind::TaskStarted {
            terminal,
            tab,
            tab_name: _,
            space,
            cwd,
            pid,
            created_ms,
            session,
            resumed: _,
        } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if !matches!(
                t.state,
                TaskState::Placed | TaskState::Reported | TaskState::Dormant
            ) {
                anomaly(s, e, "invalid task start");
                return;
            }
            t.state = TaskState::Running;
            t.state_since_ms = e.ts_ms;
            t.started_ms = Some(e.ts_ms);
            t.report = None;
            t.start_blocked = None;
            t.stalled = None;
            t.start_blocked_ms = None;
            t.stalled_ms = None;
            t.attention = None;
            t.blocked = None;
            t.last_activity_seq = e.seq;
            if let Some(r) = s
                .resources
                .iter_mut()
                .find(|r| r.r.r#type == ResType::Tab && r.r.name == terminal && !r.released)
            {
                r.task = e.task.clone();
                r.ws = e.ws.clone();
            }
            t.terminal = Some(terminal);
            t.tab = tab;
            t.space = Some(space);
            t.cwd = Some(cwd);
            t.pid = pid;
            t.created_ms = Some(created_ms);
            t.session = session;
        }
        Kind::TaskLaunchDetails {
            terminal,
            tab,
            session,
        } => {
            if let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) {
                if t.terminal.as_deref() == Some(&terminal) {
                    if tab.is_some() {
                        t.tab = tab.clone();
                    }
                    if let Some(next) = session {
                        if next.id.is_some()
                            || !t.session.as_ref().is_some_and(|cur| cur.id.is_some())
                        {
                            t.session = Some(next);
                        }
                    }
                    if tab.is_some() {
                        if let Some(r) = s.resources.iter_mut().find(|r| {
                            r.r.r#type == ResType::Tab && r.r.name == terminal && !r.released
                        }) {
                            r.r.tab = tab;
                        }
                    }
                }
            }
        }
        Kind::TaskSent { terminal, .. } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if !matches!(
                t.state,
                TaskState::Running | TaskState::Reported | TaskState::Dormant
            ) {
                anomaly(s, e, "invalid task send");
                return;
            }
            t.state = TaskState::Running;
            t.state_since_ms = e.ts_ms;
            t.report = None;
            t.blocked = None;
            t.stalled = None;
            t.stalled_ms = None;
            t.last_activity_seq = e.seq;
            t.terminal = Some(terminal);
        }
        Kind::TaskReported {
            outcome,
            summary,
            result,
        } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if !matches!(t.state, TaskState::Running | TaskState::Reported) {
                anomaly(s, e, "invalid task report");
                return;
            }
            t.state = TaskState::Reported;
            t.state_since_ms = e.ts_ms;
            t.report = Some(outcome);
            t.summary = Some(summary);
            t.result = result;
            t.start_blocked = None;
            t.stalled = None;
            t.start_blocked_ms = None;
            t.stalled_ms = None;
            t.reported_seq = Some(e.seq);
            t.attention = None;
            if matches!(
                outcome,
                super::kinds::Outcome::Blocked | super::kinds::Outcome::Question
            ) {
                block(t, e, t.summary.clone().unwrap_or_default(), outcome);
            } else {
                t.blocked = None;
            }
        }
        Kind::TaskProgress { summary } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if !matches!(t.state, TaskState::Running | TaskState::Reported) {
                anomaly(s, e, "invalid task progress");
                return;
            }
            t.summary = Some(summary);
        }
        Kind::TaskDormant { .. } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if !matches!(
                t.state,
                TaskState::Running | TaskState::Reported | TaskState::Placed
            ) {
                anomaly(s, e, "invalid task dormant");
                return;
            }
            t.state = TaskState::Dormant;
            t.state_since_ms = e.ts_ms;
            t.terminal = None;
        }
        Kind::TaskClosed { outcome, .. } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if t.state == TaskState::Closed {
                anomaly(s, e, "duplicate task close");
                return;
            }
            t.state = TaskState::Closed;
            t.state_since_ms = e.ts_ms;
            t.outcome = outcome;
            t.blocked = None;
            t.attention = None;
        }
        Kind::TaskTurn {
            origin, working_ms, ..
        } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            t.turns += 1;
            t.person_turns += u64::from(origin == TurnOrigin::Person);
            t.working_ms = t.working_ms.saturating_add(working_ms.unwrap_or(0));
        }
        Kind::CoordTurn {
            origin, working_ms, ..
        } => {
            s.coordinator.turns += 1;
            s.coordinator.person_turns += u64::from(origin == TurnOrigin::Person);
            s.coordinator.inbox_turns += u64::from(origin == TurnOrigin::Inbox);
            s.coordinator.working_ms = s
                .coordinator
                .working_ms
                .saturating_add(working_ms.unwrap_or(0));
        }
        Kind::Rating { target, value, .. } => {
            if let Ok(id) = super::ids::TaskId::parse(&target) {
                if let Some(t) = s.tasks.get_mut(&id) {
                    t.rating = Some(value);
                } else {
                    anomaly(s, e, "unknown rated task");
                }
            } else if let Ok(id) = super::ids::WsId::parse(&target) {
                if let Some(w) = s.workstreams.get_mut(&id) {
                    w.rating = Some(value);
                } else {
                    anomaly(s, e, "unknown rated workstream");
                }
            } else {
                anomaly(s, e, "invalid rating target");
            }
        }
        Kind::ConfigChanged { .. } => {}
        Kind::TaskSession { session } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if t.state == TaskState::Closed {
                anomaly(s, e, "session on closed task");
                return;
            }
            t.session = Some(session);
        }
        Kind::TaskStartBlocked { .. } | Kind::TaskStalled { .. } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if t.state != TaskState::Running {
                anomaly(s, e, "invalid task warning");
                return;
            }
            if matches!(kind, Kind::TaskStartBlocked { .. }) {
                t.start_blocked = Some(e.seq);
                t.start_blocked_ms = Some(e.ts_ms);
            } else {
                t.stalled = Some(e.seq);
                t.stalled_ms = Some(e.ts_ms);
            }
        }
        Kind::TaskAttention { state, msg } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if t.state == TaskState::Closed {
                anomaly(s, e, "attention on closed task");
                return;
            }
            if state == AttentionState::Clear {
                t.attention = None;
                t.start_blocked = None;
                t.start_blocked_ms = None;
                t.blocked = None;
            } else {
                t.attention = Some(state);
                if state == AttentionState::Blocked {
                    block(
                        t,
                        e,
                        msg.unwrap_or_else(|| "Delegate is waiting on the person.".into()),
                        super::kinds::Outcome::Blocked,
                    );
                }
            }
        }
        Kind::TaskBlockedReminder { task, episode_seq } => {
            if let Some(item) = s
                .tasks
                .get_mut(&task)
                .and_then(|t| t.blocked.as_mut())
                .filter(|item| item.seq == episode_seq)
            {
                item.reminders = item.reminders.saturating_add(1);
                item.last_reminded_ms = e.ts_ms;
            }
        }
        Kind::TaskNote { .. } => {
            if !e.task.as_ref().is_some_and(|id| s.tasks.contains_key(id)) {
                anomaly(s, e, "unknown task");
            }
        }
        Kind::ResIntent(r) => {
            if s.intents.values().any(|i| i.ws == e.ws && same(&i.r, &r)) {
                anomaly(s, e, "duplicate resource intent");
                return;
            }
            s.intents.insert(
                e.seq,
                Intent {
                    seq: e.seq,
                    ts_ms: e.ts_ms,
                    ws: e.ws.clone(),
                    task: e.task.clone(),
                    r,
                },
            );
        }
        Kind::ResCreated(r) | Kind::ResAdopted(r) => {
            let owned = e.kind == "res.created";
            if s.resources.iter().any(|v| {
                !v.released
                    && same(&v.r, &r)
                    && (if owned {
                        v.owned
                    } else {
                        !v.owned && v.ws == e.ws
                    })
            }) {
                anomaly(s, e, "duplicate resource");
                return;
            }
            if owned {
                if let Some(seq) = s
                    .intents
                    .iter()
                    .find(|(_, i)| fulfils(i, e, &r))
                    .map(|(seq, _)| *seq)
                {
                    s.intents.remove(&seq);
                }
                // Branches and discovered tabs may be recorded without an intent.
            }
            if r.r#type == ResType::Space {
                if let Some(ws) = e.ws.as_ref().and_then(|id| s.workstreams.get_mut(id)) {
                    if let Some(folder) = &r.folder {
                        ws.cwd = Some(folder.clone());
                        ws.space = Some(SpaceRec {
                            id: r.name.clone(),
                            name: r.label.clone().unwrap_or_else(|| r.name.clone()),
                            owned,
                            folder: folder.clone(),
                        });
                    }
                }
            }
            s.resources.push(ResourceRec {
                r,
                ws: e.ws.clone(),
                task: e.task.clone(),
                owned,
                released: false,
                created_seq: e.seq,
                last_checks: BTreeMap::new(),
            });
        }
        Kind::ResReleaseChecked {
            r#type,
            name,
            check,
            ok,
            detail,
            ..
        } => {
            let Some(r) = s.resources.iter_mut().rev().find(|r| {
                r.ws == e.ws && r.r.r#type == r#type && r.r.name == name && r.owned && !r.released
            }) else {
                anomaly(s, e, "unknown owned resource");
                return;
            };
            r.last_checks.insert(check, (ok, detail));
        }
        Kind::ResReleased {
            r#type,
            name,
            note: _,
        } => {
            if let Some(seq) = s
                .intents
                .iter()
                .find(|(_, i)| i.ws == e.ws && i.r.r#type == r#type && i.r.name == name)
                .map(|(seq, _)| *seq)
            {
                s.intents.remove(&seq);
            } else if let Some(r) = s.resources.iter_mut().rev().find(|r| {
                r.ws == e.ws && r.r.r#type == r#type && r.r.name == name && r.owned && !r.released
            }) {
                r.released = true;
            } else {
                anomaly(s, e, "unknown owned resource or intent");
            }
        }
        Kind::NudgeSent {
            target: NudgeTarget::Coordinator,
            upto_seq,
            ..
        } => {
            if let Some(n) = upto_seq {
                s.last_coordinator_nudge_upto = s.last_coordinator_nudge_upto.max(n);
            }
        }
        Kind::NudgeSent {
            target: NudgeTarget::Task,
            ..
        } => {
            let Some(t) = e.task.as_ref().and_then(|id| s.tasks.get_mut(id)) else {
                anomaly(s, e, "unknown task");
                return;
            };
            if t.state != TaskState::Running {
                anomaly(s, e, "invalid task nudge");
                return;
            }
            t.nudges += 1;
            t.last_nudge = Some((e.seq, e.ts_ms));
        }
        Kind::InboxDrained { cursor, .. } => {
            s.cursor = s.cursor.max(cursor);
            s.waking.retain(|seq| *seq > s.cursor);
        }
        Kind::WatcherStarted {
            release,
            terminal,
            pid,
        } => {
            s.watcher = WatcherRec {
                release: Some(release),
                terminal,
                pid: Some(pid),
                started_ms: Some(e.ts_ms),
                stopped_ms: None,
                stop_reason: None,
            };
        }
        Kind::WatcherStopped { reason } => {
            s.watcher.stopped_ms = Some(e.ts_ms);
            s.watcher.stop_reason = Some(reason);
            s.watcher.pid = None;
        }
        Kind::CoordSessionStarted {
            role,
            model,
            effort,
            terminal,
            pid,
            created_ms,
            space,
            tab,
            session,
            release,
            role_hash,
            ..
        } => {
            s.coordinator.terminal = Some(terminal);
            s.coordinator.pid = pid;
            s.coordinator.created_ms = created_ms;
            s.coordinator.space = Some(space);
            s.coordinator.tab = tab;
            s.coordinator.session = session;
            s.coordinator.role = Some(role);
            s.coordinator.model = model;
            s.coordinator.effort = effort;
            s.coordinator.release = Some(release);
            s.coordinator.role_hash = role_hash;
            s.coordinator.started_ms = Some(e.ts_ms);
            s.coordinator.sessions_started += 1;
            s.coordinator.handoff = None;
            s.coordinator.handoff_note = None;
        }
        Kind::CoordSessionEnded { terminal, .. } => {
            if s.coordinator.terminal.as_deref() == Some(&terminal) {
                s.coordinator.terminal = None;
            } else {
                anomaly(s, e, "unknown coordinator session");
            }
        }
        Kind::CoordHandoffRequested { by, note } => {
            s.coordinator.handoff = Some((e.seq, by));
            if note.is_some() {
                s.coordinator.handoff_note = note;
            }
        }
        Kind::Decision {
            what,
            why,
            tier,
            qid,
        } => {
            s.decisions.push_back(DecisionRec {
                seq: e.seq,
                ts_ms: e.ts_ms,
                what,
                why,
                tier,
                qid,
                ws: e.ws.clone(),
                task: e.task.clone(),
            });
            if s.decisions.len() > 50 {
                s.decisions.pop_front();
            }
        }
        Kind::QuestionAsked { qid, text, options } => {
            if s.questions.contains_key(&qid) {
                anomaly(s, e, "duplicate question");
                return;
            }
            counter(&qid, &mut s.counters.question);
            s.questions.insert(
                qid,
                Question {
                    text,
                    options,
                    ws: e.ws.clone(),
                    task: e.task.clone(),
                    asked_seq: e.seq,
                    asked_ms: e.ts_ms,
                    answered: None,
                },
            );
        }
        Kind::QuestionAnswered { qid, text } => {
            let Some(q) = s.questions.get_mut(&qid) else {
                anomaly(s, e, "unknown question");
                return;
            };
            if q.answered.is_some() {
                anomaly(s, e, "duplicate answer");
                return;
            }
            q.answered = Some(Answer {
                text,
                seq: e.seq,
                ts_ms: e.ts_ms,
            });
            if let Some(t) = q.task.as_ref().and_then(|id| s.tasks.get_mut(id)) {
                if t.blocked
                    .as_ref()
                    .is_some_and(|item| item.seq <= q.asked_seq)
                {
                    t.blocked = None;
                }
            }
        }
        Kind::InstructionSet { id, text } => {
            counter(&id, &mut s.counters.instruction);
            if let Some(text) = text {
                s.instructions.insert(id, text);
            } else {
                s.instructions.remove(&id);
            }
        }
        Kind::ThreadOpened { id, title, note } => {
            if s.threads.contains_key(&id) {
                anomaly(s, e, "duplicate thread");
                return;
            }
            counter(&id, &mut s.counters.thread);
            s.threads.insert(
                id.clone(),
                Thread {
                    id,
                    title,
                    note,
                    opened_seq: e.seq,
                    opened_ms: e.ts_ms,
                },
            );
        }
        Kind::ThreadClosed { id, .. } => {
            if s.threads.remove(&id).is_none() {
                anomaly(s, e, "unknown thread");
            }
        }
        Kind::ReconcileFinding { .. }
        | Kind::ScoutResult { .. }
        | Kind::Correction { .. }
        | Kind::CoordSent { .. }
        | Kind::Unknown => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::ids::{TaskId, WsId};
    use super::super::kinds::{Actor, Outcome};
    use super::*;
    use serde_json::json;

    #[test]
    fn repository_identity_and_workstream_scoped_release() {
        let a = WsId::parse("ws-aaaaaa").unwrap();
        let b = WsId::parse("ws-bbbbbb").unwrap();
        let mut s = State::default();
        let mut seq = 0;
        for (ws, repo) in [(&a, "/a"), (&b, "/b")] {
            for (kind, data) in [
                ("ws.opened", json!({"title":"work"})),
                (
                    "res.intent",
                    json!({"type":"branch","name":"topic","repo":repo}),
                ),
                (
                    "res.created",
                    json!({"type":"branch","name":"topic","repo":repo}),
                ),
            ] {
                seq += 1;
                apply(&mut s, &entry(seq, Some(ws), None, kind, data));
            }
        }
        seq += 1;
        apply(
            &mut s,
            &entry(
                seq,
                Some(&a),
                None,
                "res.release_checked",
                json!({"resource":"branch:topic","type":"branch","name":"topic","check":"merged","ok":true}),
            ),
        );
        seq += 1;
        apply(
            &mut s,
            &entry(
                seq,
                Some(&a),
                None,
                "res.released",
                json!({"type":"branch","name":"topic"}),
            ),
        );
        assert!(s.resources[0].released);
        assert!(!s.resources[1].released);
        assert!(s.resources[0].last_checks.contains_key("merged"));
        assert!(s.resources[1].last_checks.is_empty());
        assert!(s.anomalies.is_empty());
    }
    #[test]
    fn shared_space_adoption_does_not_transfer_ownership() {
        let a = WsId::parse("ws-aaaaaa").unwrap();
        let b = WsId::parse("ws-bbbbbb").unwrap();
        let c = WsId::parse("ws-cccccc").unwrap();
        let mut s = State::default();
        let mut seq = 0;
        for ws in [&a, &b, &c] {
            seq += 1;
            apply(
                &mut s,
                &entry(seq, Some(ws), None, "ws.opened", json!({"title":"work"})),
            );
            seq += 1;
            apply(
                &mut s,
                &entry(
                    seq,
                    Some(ws),
                    None,
                    if ws == &a {
                        "res.created"
                    } else {
                        "res.adopted"
                    },
                    json!({"type":"space","name":"shared","folder":"/repo"}),
                ),
            );
        }
        assert_eq!(s.resources.len(), 3);
        assert_eq!(s.resources.iter().filter(|r| r.owned).count(), 1);
        for ws in [&b, &c] {
            assert_eq!(
                s.workstreams[ws].cwd.as_deref(),
                Some(std::path::Path::new("/repo"))
            );
            assert!(!s.workstreams[ws].space.as_ref().unwrap().owned);
        }
        seq += 1;
        apply(
            &mut s,
            &entry(
                seq,
                Some(&b),
                None,
                "res.released",
                json!({"type":"space","name":"shared"}),
            ),
        );
        assert!(s.resources.iter().all(|r| !r.released));
    }
    fn entry(
        seq: u64,
        ws: Option<&WsId>,
        task: Option<&TaskId>,
        kind: &str,
        data: serde_json::Value,
    ) -> Entry {
        Entry {
            v: 1,
            seq,
            ts_ms: seq,
            actor: Actor::Cli,
            kind: kind.into(),
            ws: ws.cloned(),
            task: task.cloned(),
            op: None,
            cause: None,
            data: data.as_object().unwrap().clone(),
        }
    }
    #[test]
    fn tab_intent_and_handover() {
        let ws = WsId::parse("ws-abc234").unwrap();
        let old = TaskId::new(&ws, 1);
        let new = TaskId::new(&ws, 2);
        let mut s = State::default();
        let place = json!({"rung":1,"reason":"reuse","rejected":[],"role":"r","harness":"omp","title":"t","brief":"/b"});
        let start = json!({"terminal":"term","tab_name":"t","space":"sp","cwd":"/repo","created_ms":1,"resumed":true});
        for e in [
            entry(1, Some(&ws), None, "ws.opened", json!({"title":"w"})),
            entry(2, Some(&ws), Some(&old), "task.placed", place.clone()),
            entry(
                3,
                Some(&ws),
                Some(&old),
                "res.intent",
                json!({"type":"tab","name":old.as_str()}),
            ),
            entry(
                4,
                Some(&ws),
                Some(&old),
                "res.created",
                json!({"type":"tab","name":"term"}),
            ),
            entry(5, Some(&ws), Some(&old), "task.started", start.clone()),
            entry(6, Some(&ws), Some(&new), "task.placed", place),
            entry(
                7,
                Some(&ws),
                Some(&old),
                "task.closed",
                json!({"reason":"superseded by ws-abc234-t2"}),
            ),
            entry(8, Some(&ws), Some(&new), "task.started", start),
        ] {
            apply(&mut s, &e);
        }
        assert!(s.intents.is_empty());
        assert_eq!(s.resources[0].task, Some(new));
        assert!(s.anomalies.is_empty());
    }
    #[test]
    fn transitions_resources_and_anomalies() {
        let ws = WsId::parse("ws-abc234").unwrap();
        let task = TaskId::new(&ws, 1);
        let mut s = State::default();
        let mut seq = 0;
        let mut push = |s: &mut State, kind: &str, data: serde_json::Value| {
            seq += 1;
            apply(s, &entry(seq, Some(&ws), Some(&task), kind, data));
        };
        push(
            &mut s,
            "ws.opened",
            json!({"title":"w","repo":"/repo","purpose":"p"}),
        );
        push(
            &mut s,
            "task.placed",
            json!({"rung":1,"reason":"reuse","rejected":[],"role":"r","harness":"omp","title":"t","brief":"/b"}),
        );
        push(
            &mut s,
            "task.started",
            json!({"terminal":"term","tab_name":"t","space":"sp","cwd":"/repo","created_ms":1,"resumed":false}),
        );
        push(
            &mut s,
            "task.start_blocked",
            json!({"terminal":"term","after_secs":1}),
        );
        assert_eq!(s.tasks[&task].start_blocked_ms, Some(4));
        push(&mut s, "task.attention", json!({"state":"blocked"}));
        push(&mut s, "task.attention", json!({"state":"clear"}));
        assert!(s.tasks[&task].attention.is_none() && s.tasks[&task].start_blocked.is_none());
        assert!(s.tasks[&task].start_blocked_ms.is_none());
        push(&mut s, "nudge.sent", json!({"target":"task","text":"n"}));
        push(
            &mut s,
            "task.stalled",
            json!({"silent_secs":3,"nudge_seq":7}),
        );
        assert_eq!(s.tasks[&task].stalled_ms, Some(8));
        push(&mut s, "task.progress", json!({"summary":"progress"}));
        push(
            &mut s,
            "task.reported",
            json!({"outcome":"done","summary":"done","result":null}),
        );
        assert_eq!(s.tasks[&task].report, Some(Outcome::Done));
        assert!(s.tasks[&task].stalled.is_none());
        assert!(s.tasks[&task].stalled_ms.is_none());
        push(
            &mut s,
            "task.sent",
            json!({"message":"/m","via":"send","terminal":"term"}),
        );
        push(&mut s, "task.dormant", json!({"reason":"idle"}));
        assert_eq!(s.tasks[&task].state, TaskState::Dormant);
        push(
            &mut s,
            "task.started",
            json!({"terminal":"term","tab_name":"t","space":"sp","cwd":"/repo","created_ms":1,"resumed":true}),
        );
        push(
            &mut s,
            "task.closed",
            json!({"reason":"superseded by ws-abc234-t2"}),
        );
        assert_eq!(s.tasks[&task].state, TaskState::Closed);
        push(
            &mut s,
            "res.intent",
            json!({"type":"worktree","name":"/tree"}),
        );
        push(
            &mut s,
            "res.created",
            json!({"type":"worktree","name":"/tree"}),
        );
        assert!(s.intents.is_empty());
        push(
            &mut s,
            "res.release_checked",
            json!({"resource":"worktree:/tree","type":"worktree","name":"/tree","check":"clean","ok":true}),
        );
        push(
            &mut s,
            "res.released",
            json!({"type":"worktree","name":"/tree"}),
        );
        assert!(s.resources[0].released);
        push(&mut s, "res.intent", json!({"type":"space","name":"sp"}));
        push(
            &mut s,
            "res.released",
            json!({"type":"space","name":"sp","note":"never created"}),
        );
        push(
            &mut s,
            "res.adopted",
            json!({"type":"space","name":"existing","folder":"/repo"}),
        );
        push(&mut s, "ws.closed", json!({"summary":"done"}));
        push(
            &mut s,
            "ws.retained",
            json!({"reason_code":"dirty","reason":"dirty","checks":[]}),
        );
        push(&mut s, "ws.cleaned", json!({}));
        assert_eq!(s.workstreams[&ws].state, WsState::Cleaned);
        assert!(!s.resources[1].owned);
        assert!(s.intents.is_empty());
        assert!(s.anomalies.is_empty(), "{:?}", s.anomalies);
        push(&mut s, "task.closed", json!({"reason":"again"}));
        assert_eq!(s.anomalies.len(), 1);
    }
    #[test]
    fn waking_and_cursor() {
        let mut s = State::default();
        apply(
            &mut s,
            &entry(
                1,
                None,
                None,
                "reconcile.finding",
                json!({"code":"x","summary":"x"}),
            ),
        );
        apply(
            &mut s,
            &entry(
                2,
                None,
                None,
                "instruction.set",
                json!({"id":"i1","text":"x"}),
            ),
        );
        assert_eq!(s.waking.iter().copied().collect::<Vec<_>>(), vec![1, 2]);
        apply(
            &mut s,
            &entry(
                3,
                None,
                None,
                "inbox.drained",
                json!({"cursor":1,"count":1}),
            ),
        );
        assert_eq!(s.waking.iter().copied().collect::<Vec<_>>(), vec![2]);
        apply(
            &mut s,
            &entry(
                4,
                None,
                None,
                "nudge.sent",
                json!({"target":"coordinator","text":"n","upto_seq":2}),
            ),
        );
        assert_eq!(s.last_coordinator_nudge_upto, 2);
    }

    #[test]
    fn launch_details_never_erases_discovered_session_id() {
        let ws = super::super::ids::WsId::parse("ws-abcdef").unwrap();
        let task = super::super::ids::TaskId::new(&ws, 1);
        let mut s = State::default();
        for (seq, kind, data) in [
            (1, "ws.opened", json!({"title":"x"})),
            (
                2,
                "task.placed",
                json!({"rung":2,"reason":"x","rejected":[],"role":"agy-default","harness":"agy","title":"x","brief":"/b","session":{"mode":"agy-conversation","id":null}}),
            ),
            (
                3,
                "task.started",
                json!({"terminal":"t1","tab":null,"tab_name":"x","space":"sp","cwd":"/tmp","pid":1,"created_ms":1,"session":{"mode":"agy-conversation","id":null},"resumed":false}),
            ),
            (
                4,
                "task.session",
                json!({"session":{"mode":"agy-conversation","id":"720d8fe2-d9d4-40b4-af0d-fb7a29307ff0"}}),
            ),
            (
                5,
                "task.launch_details",
                json!({"terminal":"t1","tab":"tb1","session":{"mode":"agy-conversation","id":null}}),
            ),
            (
                6,
                "task.launch_details",
                json!({"terminal":"t1","tab":null,"session":null}),
            ),
        ] {
            apply(&mut s, &entry(seq, Some(&ws), Some(&task), kind, data));
        }
        let rec = &s.tasks[&task];
        assert_eq!(rec.tab.as_deref(), Some("tb1"));
        assert_eq!(
            rec.session.as_ref().and_then(|s| s.id.as_deref()),
            Some("720d8fe2-d9d4-40b4-af0d-fb7a29307ff0")
        );
    }
}

#[cfg(test)]
mod insight_tests {
    use super::super::kinds::Actor;
    use super::*;
    use serde_json::json;
    #[test]
    fn folds_turn_counts_and_time() {
        let ws = super::super::ids::WsId::parse("ws-abcdef").unwrap();
        let task = super::super::ids::TaskId::new(&ws, 1);
        let mut s = State::default();
        for (seq, kind, data) in [
            (1, "ws.opened", json!({"title":"x"})),
            (
                2,
                "task.placed",
                json!({"rung":2,"reason":"x","rejected":[],"role":"x","harness":"omp","title":"x","brief":"/b"}),
            ),
            (
                3,
                "task.turn",
                json!({"turn":1,"origin":"start","working_ms":20}),
            ),
            (4, "task.turn", json!({"turn":2,"origin":"person"})),
            (
                5,
                "coord.turn",
                json!({"turn":1,"origin":"inbox","working_ms":30}),
            ),
            (
                6,
                "coord.turn",
                json!({"turn":2,"origin":"person","working_ms":10}),
            ),
        ] {
            let e = Entry {
                v: 1,
                seq,
                ts_ms: seq,
                actor: Actor::Watcher,
                kind: kind.into(),
                ws: Some(ws.clone()),
                task: Some(task.clone()),
                op: None,
                cause: None,
                data: data.as_object().unwrap().clone(),
            };
            apply(&mut s, &e);
        }
        assert_eq!(
            (
                s.tasks[&task].turns,
                s.tasks[&task].person_turns,
                s.tasks[&task].working_ms
            ),
            (2, 1, 20)
        );
        assert_eq!(
            (
                s.coordinator.turns,
                s.coordinator.person_turns,
                s.coordinator.inbox_turns,
                s.coordinator.working_ms
            ),
            (2, 1, 1, 40)
        );
        assert!(s.waking.is_empty());
    }
}
