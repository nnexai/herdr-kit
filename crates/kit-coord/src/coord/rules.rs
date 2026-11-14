//! Pure watcher rules over journal-derived state and observed terminal facts.
use super::{
    adapter::Term,
    config::Config,
    ids::{TaskId, WsId},
    kinds::{AttentionState, Entry, Kind, NudgeTarget, TurnOrigin},
    state::{State, TaskState},
};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Facts {
    pub terms: BTreeMap<String, Term>,
    pub events_ok: bool,
    pub initialized: bool,
    pub entries: Vec<Entry>,
    pub working_since: BTreeMap<String, u64>,
}
#[derive(Clone, Debug)]
pub enum Action {
    Send {
        terminal: String,
        text: String,
        record: Kind,
        ws: Option<WsId>,
        task: Option<TaskId>,
    },
    Append {
        ws: Option<WsId>,
        task: Option<TaskId>,
        kind: Kind,
    },
    CloseLeftover {
        task: TaskId,
        terminal: String,
    },
    RestartCoordinator {
        reason: String,
    },
}
const DELEGATE_NUDGE: &str = "Please report your result or blocker using kit-coord report.";
fn elapsed(now: u64, since: Option<u64>, secs: u64) -> bool {
    since.is_some_and(|t| now.saturating_sub(t) >= secs.saturating_mul(1000))
}
fn append(out: &mut Vec<Action>, task: Option<&TaskId>, kind: Kind) {
    out.push(Action::Append {
        ws: task.map(TaskId::ws),
        task: task.cloned(),
        kind,
    });
}

// Entries are associated with a terminal by replaying its starts and sends.
// Turn records identify the current terminal through that replay.
fn turn_info(entries: &[Entry], terminal: &str, task: Option<&TaskId>) -> (u64, TurnOrigin) {
    let mut current = None::<String>;
    let mut last_turn = 0;
    let mut last_seq = 0;
    let mut candidates = Vec::new();
    for e in entries {
        if task.is_some() && e.task.as_ref() != task {
            continue;
        }
        match e.typed() {
            Kind::TaskStarted { terminal: t, .. } if task.is_some() => {
                current = Some(t.clone());
                if t == terminal {
                    candidates.push((e.seq, TurnOrigin::Start));
                }
            }
            Kind::TaskSent { terminal: t, .. } if task.is_some() => {
                current = Some(t.clone());
                if t == terminal {
                    candidates.push((e.seq, TurnOrigin::Send));
                }
            }
            Kind::CoordSessionStarted { terminal: t, .. } if task.is_none() => {
                current = Some(t.clone());
                if t == terminal {
                    candidates.push((e.seq, TurnOrigin::Start));
                }
            }
            Kind::NudgeSent { target, .. }
                if current.as_deref() == Some(terminal)
                    && ((task.is_some() && target == NudgeTarget::Task)
                        || (task.is_none() && target == NudgeTarget::Coordinator)) =>
            {
                candidates.push((
                    e.seq,
                    if task.is_some() {
                        TurnOrigin::Nudge
                    } else {
                        TurnOrigin::Inbox
                    },
                ))
            }
            Kind::TaskTurn { turn, .. }
                if task.is_some() && current.as_deref() == Some(terminal) =>
            {
                last_turn = turn;
                last_seq = e.seq;
            }
            Kind::CoordTurn { turn, .. }
                if task.is_none() && current.as_deref() == Some(terminal) =>
            {
                last_turn = turn;
                last_seq = e.seq;
            }
            _ => {}
        }
    }
    let origin = candidates
        .into_iter()
        .find(|(seq, _)| *seq > last_seq)
        .map(|(_, o)| o)
        .unwrap_or(TurnOrigin::Person);
    (last_turn, origin)
}

pub fn evaluate(state: &State, facts: &Facts, cfg: &Config, now_ms: u64) -> Vec<Action> {
    let mut out = Vec::new();
    if !facts.initialized {
        return out;
    }
    for t in state.tasks.values() {
        let Some(address) = t.terminal.as_deref() else {
            continue;
        };
        let term = facts.terms.get(address);
        let missing = term.is_none();
        let finished = term.is_some_and(|v| !v.live());
        let replaced = term.is_some_and(|v| v.live() && t.pid.is_some() && v.pid != t.pid);
        if matches!(
            t.state,
            TaskState::Placed | TaskState::Running | TaskState::Reported
        ) && (missing || finished || replaced)
        {
            if replaced
                && state.resources.iter().any(|r| {
                    r.owned && !r.released && r.task.as_ref() == Some(&t.id) && r.r.name == address
                })
            {
                out.push(Action::CloseLeftover {
                    task: t.id.clone(),
                    terminal: address.into(),
                });
            }
            if !t.session.as_ref().is_some_and(|s| s.id.is_some()) {
                append(
                    &mut out,
                    Some(&t.id),
                    Kind::ReconcileFinding {
                        code: "agent_gone_unresumable".into(),
                        summary: format!(
                            "Agent {} ended without a recorded session",
                            t.id.as_str()
                        ),
                    },
                );
            }
            append(
                &mut out,
                Some(&t.id),
                Kind::TaskDormant {
                    reason: if replaced {
                        "replaced_after_restart"
                    } else if finished {
                        "terminal_finished"
                    } else {
                        "terminal_missing"
                    }
                    .into(),
                },
            );
            continue;
        }
        let Some(term) = term else { continue };
        let (last_turn, origin) = turn_info(&facts.entries, address, Some(&t.id));
        if term.turn > last_turn {
            append(
                &mut out,
                Some(&t.id),
                Kind::TaskTurn {
                    turn: term.turn,
                    origin,
                    working_ms: facts
                        .working_since
                        .get(address)
                        .map(|v| term.status_ms.unwrap_or(now_ms).saturating_sub(*v)),
                },
            );
        }
        if t.state != TaskState::Running {
            continue;
        }
        let attention = match term.word() {
            Some("blocked") => Some(AttentionState::Blocked),
            Some("error") => Some(AttentionState::Error),
            _ => None,
        };
        if let Some(attention) = attention {
            if t.attention != Some(attention) {
                append(
                    &mut out,
                    Some(&t.id),
                    Kind::TaskAttention {
                        state: attention,
                        msg: term.status.as_ref().and_then(|s| s.msg.clone()),
                    },
                );
            }
        } else if term.word() == Some("working")
            && (t.attention.is_some() || t.start_blocked.is_some())
        {
            append(
                &mut out,
                Some(&t.id),
                Kind::TaskAttention {
                    state: AttentionState::Clear,
                    msg: None,
                },
            );
        }
        if term.turn == 0
            && term.word() != Some("working")
            && t.start_blocked.is_none()
            && elapsed(now_ms, t.started_ms, cfg.start_blocked_secs)
        {
            append(
                &mut out,
                Some(&t.id),
                Kind::TaskStartBlocked {
                    terminal: address.into(),
                    after_secs: cfg.start_blocked_secs,
                },
            );
        }
        let nudged = t.last_nudge.filter(|(seq, _)| *seq > t.last_activity_seq);
        let reported = t.reported_seq.is_some_and(|seq| seq > t.last_activity_seq);
        if !reported {
            if let Some((seq, ts)) = nudged {
                if t.stalled.is_none() && elapsed(now_ms, Some(ts), cfg.stall_secs) {
                    append(
                        &mut out,
                        Some(&t.id),
                        Kind::TaskStalled {
                            silent_secs: now_ms.saturating_sub(ts) / 1000,
                            nudge_seq: seq,
                        },
                    );
                }
            } else if term.sendable() && elapsed(now_ms, term.status_ms, cfg.nudge_delegate_secs) {
                out.push(Action::Send {
                    terminal: address.into(),
                    text: DELEGATE_NUDGE.into(),
                    record: Kind::NudgeSent {
                        target: NudgeTarget::Task,
                        text: DELEGATE_NUDGE.into(),
                        upto_seq: None,
                    },
                    ws: Some(t.ws.clone()),
                    task: Some(t.id.clone()),
                });
            }
        }
    }
    if let Some(address) = state.coordinator.terminal.as_deref() {
        let term = facts.terms.get(address);
        let identity = facts.entries.iter().rev().find(|e| {
            e.kind == "coord.session_started"
                && e.data.get("terminal").and_then(|v| v.as_str()) == Some(address)
        });
        let replaced = term.is_some_and(|t| {
            identity.is_some_and(|e| {
                e.data
                    .get("pid")
                    .and_then(|v| v.as_u64())
                    .is_some_and(|p| Some(p) != t.pid.map(u64::from))
                    || e.data
                        .get("created_ms")
                        .and_then(|v| v.as_u64())
                        .is_some_and(|ms| ms != t.created_ms)
            })
        });
        if replaced
            || term.is_none_or(|t| !t.live() || t.label("coord.role") != Some("coordinator"))
        {
            append(
                &mut out,
                None,
                Kind::CoordSessionEnded {
                    reason: "terminal_gone".into(),
                    terminal: address.into(),
                },
            );
        } else if let Some(term) = term {
            let (last_turn, origin) = turn_info(&facts.entries, address, None);
            if term.turn > last_turn {
                append(
                    &mut out,
                    None,
                    Kind::CoordTurn {
                        turn: term.turn,
                        origin,
                        working_ms: facts
                            .working_since
                            .get(address)
                            .map(|v| term.status_ms.unwrap_or(now_ms).saturating_sub(*v)),
                    },
                );
            }
            if term.sendable() {
                let upto = state.waking.iter().next_back().copied();
                if upto.is_some_and(|seq| seq > state.last_coordinator_nudge_upto)
                    && elapsed(now_ms, term.status_ms, cfg.nudge_coordinator_secs)
                {
                    let text = format!("inbox: {} new", state.waking.len());
                    out.push(Action::Send {
                        terminal: address.into(),
                        text: text.clone(),
                        record: Kind::NudgeSent {
                            target: NudgeTarget::Coordinator,
                            text,
                            upto_seq: upto,
                        },
                        ws: None,
                        task: None,
                    });
                } else if state.waking.is_empty()
                    && state.questions.values().all(|q| q.answered.is_some())
                {
                    if let Some((_, by)) = &state.coordinator.handoff {
                        out.push(Action::RestartCoordinator {
                            reason: if by == "user" { "user" } else { "handoff" }.into(),
                        });
                    } else if elapsed(
                        now_ms,
                        term.status_ms,
                        cfg.idle_restart_hours.saturating_mul(3600),
                    ) {
                        out.push(Action::RestartCoordinator {
                            reason: "idle".into(),
                        });
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::{
        fold,
        journal::Journal,
        kinds::{Actor, SendVia},
        paths::CoordPaths,
    };
    fn fixture() -> (tempfile::TempDir, Journal, TaskId, Facts) {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::open(&CoordPaths::new(dir.path())).unwrap();
        let ws = WsId::parse("ws-abcdef").unwrap();
        let id = TaskId::new(&ws, 1);
        let items = vec![
            (
                Some(ws.clone()),
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
                Some(ws.clone()),
                Some(id.clone()),
                Kind::TaskPlaced {
                    rung: 2,
                    reason: "reason".into(),
                    rejected: vec![],
                    role: "coord-default".into(),
                    harness: "omp".into(),
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
            (
                Some(ws.clone()),
                Some(id.clone()),
                Kind::TaskStarted {
                    terminal: "t1".into(),
                    tab: None,
                    tab_name: "task".into(),
                    space: "sp".into(),
                    cwd: "/tmp".into(),
                    pid: Some(1),
                    created_ms: 0,
                    session: None,
                    resumed: false,
                },
            ),
        ];
        journal
            .lock()
            .unwrap()
            .append(&Actor::Watcher, 1000, items)
            .unwrap();
        let term: Term = serde_json::from_value(serde_json::json!({"address":"t1","state":"live","status":{"state":"done","app":null,"kind":null,"msg":null},"status_ms":1000,"turn":1,"reported":true,"labels":{"coord.managed":"1","coord.task":id.as_str()},"pid":1,"created_ms":0,"cwd":"/tmp","foreground":null,"reason":null,"name":null})).unwrap();
        let mut facts = Facts {
            initialized: true,
            entries: journal.read_after(0).unwrap(),
            ..Facts::default()
        };
        facts.terms.insert("t1".into(), term);
        (dir, journal, id, facts)
    }
    #[test]
    fn nudge_stall_and_progress_are_distinct() {
        let (_dir, j, id, mut facts) = fixture();
        let cfg = Config::default();
        let mut state = j.read_state().unwrap();
        assert!(
            evaluate(&state, &facts, &cfg, 61_000)
                .iter()
                .any(|a| matches!(a, Action::Send { task: Some(_), .. }))
        );
        facts.terms.get_mut("t1").unwrap().turn = 0;
        assert!(
            !evaluate(&state, &facts, &cfg, 61_000)
                .iter()
                .any(|a| matches!(a, Action::Send { .. }))
        );
        facts.terms.get_mut("t1").unwrap().turn = 1;
        facts.terms.get_mut("t1").unwrap().reported = false;
        assert!(
            !evaluate(&state, &facts, &cfg, 61_000)
                .iter()
                .any(|a| matches!(a, Action::Send { .. }))
        );
        facts.terms.get_mut("t1").unwrap().reported = true;
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                61_000,
                vec![
                    (
                        Some(id.ws()),
                        Some(id.clone()),
                        Kind::NudgeSent {
                            target: NudgeTarget::Task,
                            text: "n".into(),
                            upto_seq: None,
                        },
                    ),
                    (
                        Some(id.ws()),
                        Some(id.clone()),
                        Kind::TaskProgress {
                            summary: "progress".into(),
                        },
                    ),
                ],
            )
            .unwrap();
        state = j.read_state().unwrap();
        let actions = evaluate(&state, &facts, &cfg, 361_000);
        assert!(!actions.iter().any(|a| matches!(a, Action::Send { .. })));
        assert!(actions.iter().any(|a| matches!(
            a,
            Action::Append {
                kind: Kind::TaskStalled { .. },
                ..
            }
        )));
    }
    #[test]
    fn origins_and_working_time() {
        let (_dir, j, id, mut facts) = fixture();
        assert_eq!(
            turn_info(&facts.entries, "t1", Some(&id)),
            (0, TurnOrigin::Start)
        );
        let append = |kind| {
            j.lock()
                .unwrap()
                .append(
                    &Actor::Watcher,
                    2000,
                    vec![(Some(id.ws()), Some(id.clone()), kind)],
                )
                .unwrap();
        };
        append(Kind::TaskTurn {
            turn: 1,
            origin: TurnOrigin::Start,
            working_ms: None,
        });
        append(Kind::TaskSent {
            message: "/tmp/message".into(),
            via: SendVia::Send,
            terminal: "t1".into(),
            started: Some(true),
        });
        append(Kind::TaskSent {
            message: "/tmp/second".into(),
            via: SendVia::Send,
            terminal: "t1".into(),
            started: Some(true),
        });
        append(Kind::NudgeSent {
            target: NudgeTarget::Task,
            text: "n".into(),
            upto_seq: None,
        });
        facts.entries = j.read_after(0).unwrap();
        assert_eq!(
            turn_info(&facts.entries, "t1", Some(&id)),
            (1, TurnOrigin::Send)
        );
        append(Kind::TaskTurn {
            turn: 2,
            origin: TurnOrigin::Send,
            working_ms: None,
        });
        facts.entries = j.read_after(0).unwrap();
        assert_eq!(
            turn_info(&facts.entries, "t1", Some(&id)),
            (2, TurnOrigin::Person)
        );
        append(Kind::NudgeSent {
            target: NudgeTarget::Task,
            text: "n".into(),
            upto_seq: None,
        });
        facts.entries = j.read_after(0).unwrap();
        assert_eq!(
            turn_info(&facts.entries, "t1", Some(&id)).1,
            TurnOrigin::Nudge
        );
        facts.terms.get_mut("t1").unwrap().turn = 3;
        facts.terms.get_mut("t1").unwrap().status_ms = Some(5000);
        facts.working_since.insert("t1".into(), 3000);
        let state = fold::fold(facts.entries.iter());
        assert!(
            evaluate(&state, &facts, &Config::default(), 5000)
                .iter()
                .any(|a| matches!(
                    a,
                    Action::Append {
                        kind: Kind::TaskTurn {
                            working_ms: Some(2000),
                            ..
                        },
                        ..
                    }
                ))
        );
    }
    #[test]
    fn start_attention_dormancy_and_coordinator_rules() {
        let (_dir, j, id, mut facts) = fixture();
        let mut state = j.read_state().unwrap();
        let cfg = Config::default();
        let term = facts.terms.get_mut("t1").unwrap();
        term.turn = 0;
        term.status = None;
        assert!(
            evaluate(&state, &facts, &cfg, 31_000)
                .iter()
                .any(|a| matches!(
                    a,
                    Action::Append {
                        kind: Kind::TaskStartBlocked { .. },
                        ..
                    }
                ))
        );
        facts.terms.get_mut("t1").unwrap().status = Some(super::super::adapter::AgentStatus {
            state: "blocked".into(),
            app: None,
            kind: None,
            msg: None,
        });
        assert!(
            evaluate(&state, &facts, &cfg, 31_000)
                .iter()
                .any(|a| matches!(
                    a,
                    Action::Append {
                        kind: Kind::TaskAttention {
                            state: AttentionState::Blocked,
                            ..
                        },
                        ..
                    }
                ))
        );
        state.tasks.get_mut(&id).unwrap().attention = Some(AttentionState::Blocked);
        facts
            .terms
            .get_mut("t1")
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .state = "working".into();
        assert!(
            evaluate(&state, &facts, &cfg, 31_000)
                .iter()
                .any(|a| matches!(
                    a,
                    Action::Append {
                        kind: Kind::TaskAttention {
                            state: AttentionState::Clear,
                            ..
                        },
                        ..
                    }
                ))
        );
        facts.terms.get_mut("t1").unwrap().state = "finished".into();
        assert!(evaluate(&state, &facts, &cfg, 31_000).iter().any(|a| matches!(a, Action::Append { kind:Kind::TaskDormant { reason }, .. } if reason == "terminal_finished")));
        facts.terms.get_mut("t1").unwrap().state = "live".into();
        facts.terms.get_mut("t1").unwrap().pid = Some(2);
        assert!(evaluate(&state, &facts, &cfg, 31_000).iter().any(|a| matches!(a, Action::Append { kind:Kind::TaskDormant { reason }, .. } if reason == "replaced_after_restart")));
        facts.terms.remove("t1");
        assert!(
            evaluate(&state, &facts, &cfg, 31_000)
                .iter()
                .any(|a| matches!(
                    a,
                    Action::Append {
                        kind: Kind::TaskDormant { .. },
                        ..
                    }
                ))
        );
        let coord: Term = serde_json::from_value(serde_json::json!({"address":"c1","state":"live","status":{"state":"done","app":null,"kind":null,"msg":null},"status_ms":1000,"turn":1,"reported":true,"labels":{"coord.managed":"1","coord.role":"coordinator"},"pid":4,"created_ms":0,"cwd":"/tmp","foreground":null,"reason":null,"name":null})).unwrap();
        facts.terms.insert("c1".into(), coord);
        state.coordinator.terminal = Some("c1".into());
        state.waking.insert(10);
        assert!(
            evaluate(&state, &facts, &cfg, 11_000)
                .iter()
                .any(|a| matches!(a, Action::Send { task: None, .. }))
        );
        state.last_coordinator_nudge_upto = 10;
        assert!(
            !evaluate(&state, &facts, &cfg, 11_000)
                .iter()
                .any(|a| matches!(a, Action::Send { task: None, .. }))
        );
        state.waking.clear();
        state.coordinator.handoff = Some((11, "user".into()));
        assert!(
            evaluate(&state, &facts, &cfg, 11_000)
                .iter()
                .any(|a| matches!(a, Action::RestartCoordinator { reason } if reason == "user"))
        );
        state.coordinator.handoff = None;
        assert!(
            evaluate(&state, &facts, &cfg, 30_000_000)
                .iter()
                .any(|a| matches!(a, Action::RestartCoordinator { reason } if reason == "idle"))
        );
    }
    #[test]
    fn coordinator_turn_origins_distinguish_start_person_and_inbox() {
        let make = |seq, kind: &str, data| {
            serde_json::from_value::<Entry>(serde_json::json!({"v":1,"seq":seq,"ts_ms":1,"actor":"watcher","kind":kind,"ws":null,"task":null,"data":data})).unwrap()
        };
        let mut entries = vec![make(
            1,
            "coord.session_started",
            serde_json::json!({"role":"coord-main","terminal":"c1","space":"sp","release":"dev","reason":"up","brief":"/tmp/brief"}),
        )];
        assert_eq!(turn_info(&entries, "c1", None), (0, TurnOrigin::Start));
        entries.push(make(
            2,
            "coord.turn",
            serde_json::json!({"turn":1,"origin":"start"}),
        ));
        entries.push(make(
            3,
            "nudge.sent",
            serde_json::json!({"target":"coordinator","text":"inbox"}),
        ));
        assert_eq!(turn_info(&entries, "c1", None), (1, TurnOrigin::Inbox));
        entries.push(make(
            4,
            "coord.turn",
            serde_json::json!({"turn":2,"origin":"inbox"}),
        ));
        assert_eq!(turn_info(&entries, "c1", None), (2, TurnOrigin::Person));
    }
    #[test]
    fn missing_coordinator_only_ends_session() {
        let mut state = State::default();
        state.coordinator.terminal = Some("t99".into());
        let facts = Facts {
            initialized: true,
            ..Facts::default()
        };
        let actions = evaluate(&state, &facts, &Config::default(), 1_000_000);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            Action::Append {
                kind: Kind::CoordSessionEnded { .. },
                ..
            }
        ));
    }
}
