use super::{
    adapter::Term,
    config::Config,
    fold,
    ids::TaskId,
    journal::Journal,
    kinds::{Actor, AttentionState, Entry, Kind, Outcome, SendVia},
    paths::CoordPaths,
    rules::{self, Action, Facts},
    views,
};

const START: u64 = 1_794_500_064_000;

fn fixture() -> (tempfile::TempDir, Journal, TaskId, Facts) {
    let dir = tempfile::tempdir().unwrap();
    let paths = CoordPaths::new(dir.path());
    let j = Journal::open(&paths).unwrap();
    let entries: Vec<Entry> = include_str!("testdata/journal-sample.jsonl")
        .lines()
        .take(10)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    std::fs::write(
        paths.month_file(START),
        entries
            .iter()
            .map(|e| format!("{}\n", serde_json::to_string(e).unwrap()))
            .collect::<String>(),
    )
    .unwrap();
    let id = TaskId::parse("ws-k3m2ab-t1").unwrap();
    let mut facts = Facts {
        initialized: true,
        entries,
        ..Facts::default()
    };
    for (address, role, pid, word) in [
        ("t31", "delegate", 412233, "blocked"),
        ("t21", "coordinator", 1, "done"),
    ] {
        let term: Term = serde_json::from_value(serde_json::json!({
            "address":address,"state":"live","status":{"state":word,"msg":"Continue cutover"},
            "status_ms":START,"turn":1,"reported":true,"pid":pid,"created_ms":1794500063000u64,
            "cwd":"/tmp",
            "labels":{"coord.managed":"1","coord.role":role,"coord.task":id.as_str()}
        }))
        .unwrap();
        facts.terms.insert(address.into(), term);
    }
    (dir, j, id, facts)
}

fn task_event(j: &Journal, id: &TaskId, at: u64, kind: Kind) {
    j.lock()
        .unwrap()
        .append(
            &Actor::Watcher,
            at,
            vec![(Some(id.ws()), Some(id.clone()), kind)],
        )
        .unwrap();
}
fn global(j: &Journal, at: u64, kind: Kind) {
    j.lock()
        .unwrap()
        .append(&Actor::Coordinator, at, vec![(None, None, kind)])
        .unwrap();
}
fn block(j: &Journal, id: &TaskId) {
    task_event(
        j,
        id,
        START,
        Kind::TaskAttention {
            state: AttentionState::Blocked,
            msg: Some("Continue cutover".into()),
        },
    );
}
fn ack(j: &Journal, at: u64) {
    let state = j.read_state().unwrap();
    global(
        j,
        at,
        Kind::InboxDrained {
            cursor: state.last_seq,
            count: state.waking.len() as u64,
        },
    );
}
fn reminder(j: &Journal, facts: &Facts, cfg: &Config, at: u64) -> Option<Kind> {
    rules::evaluate(&j.read_state().unwrap(), facts, cfg, at)
        .into_iter()
        .find_map(|action| match action {
            Action::Send {
                terminal,
                record: record @ Kind::TaskBlockedReminder { .. },
                text,
                task,
                ..
            } => {
                assert_eq!(terminal, "t21");
                assert!(
                    task.is_none(),
                    "the reminder must go to the coordinator, not the blocked terminal"
                );
                assert!(text.contains("ws-k3m2ab-t1") && text.contains("kit board"));
                Some(record)
            }
            _ => None,
        })
}

#[test]
fn blocked_item_survives_ack_and_handoff_and_consumes_note_once() {
    let (_dir, j, id, _) = fixture();
    block(&j, &id);
    let first = j.read_state().unwrap().tasks[&id].blocked.clone().unwrap();
    task_event(
        &j,
        &id,
        START + 1,
        Kind::TaskAttention {
            state: AttentionState::Blocked,
            msg: Some("Continue cutover".into()),
        },
    );
    task_event(
        &j,
        &id,
        START + 2,
        Kind::TaskProgress {
            summary: "stale preflight progress".into(),
        },
    );
    ack(&j, START + 3);
    global(
        &j,
        START + 4,
        Kind::CoordHandoffRequested {
            by: "coordinator".into(),
            note: Some("t6 waits in its tab; ask the person to continue".into()),
        },
    );
    // An update/restart request without a note must not erase the last note.
    global(
        &j,
        START + 5,
        Kind::CoordHandoffRequested {
            by: "user".into(),
            note: None,
        },
    );
    global(
        &j,
        START + 6,
        Kind::CoordSessionEnded {
            terminal: "t21".into(),
            reason: "handoff".into(),
        },
    );
    let s = j.read_state().unwrap();
    assert!(s.waking.is_empty());
    let items = views::attention(&s, None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].since_ms, START);
    assert_eq!(s.tasks[&id].blocked.as_ref().unwrap().seq, first.seq);
    let read = views::startup_read(&s, &[], START + 25 * 60_000);
    assert!(read.contains("## Needs you\n- ws-k3m2ab-t1: Continue cutover — tab tb6f0000, terminal t31, since 1794500064000 (25 min ago)"));
    assert!(read.contains("(Needs you)"));
    assert!(!read.contains("(Running)"));
    assert!(read.contains("0 waking entries pending"));
    assert!(read.contains("## Handoff note\n\nt6 waits in its tab; ask the person to continue"));
    let counts = views::counts(&s, items.len());
    assert_eq!((counts.running, counts.needs_you), (0, 1));
    assert_eq!(
        views::board(&s, None, START, false)["workstreams"][0]["tasks"][0]["needs_you"],
        true
    );
    let started = j.read_after(0).unwrap()[0].typed();
    global(&j, START + 7, started);
    assert!(
        !views::startup_read(&j.read_state().unwrap(), &[], START + 8).contains("## Handoff note")
    );
    assert_eq!(views::attention(&j.read_state().unwrap(), None).len(), 1);
}

#[test]
fn blocked_item_closes_on_clear_answer_send_close_or_later_report() {
    for resolution in [
        Kind::TaskAttention {
            state: AttentionState::Clear,
            msg: None,
        },
        Kind::TaskReported {
            outcome: Outcome::Done,
            summary: "finished".into(),
            result: None,
        },
        Kind::TaskSent {
            message: "/tmp/answer".into(),
            via: SendVia::Send,
            terminal: "t31".into(),
            started: None,
        },
        Kind::TaskClosed {
            reason: "resolved".into(),
            outcome: None,
        },
    ] {
        let (_dir, j, id, _) = fixture();
        block(&j, &id);
        task_event(
            &j,
            &id,
            START,
            Kind::QuestionAsked {
                qid: "q1".into(),
                text: "Continue in the tab?".into(),
                options: None,
            },
        );
        task_event(&j, &id, START + 1, resolution);
        let s = j.read_state().unwrap();
        assert!(s.tasks[&id].blocked.is_none());
        assert!(views::attention(&s, None).is_empty());
        assert_eq!(views::board(&s, None, START + 1, false)["open_questions"], serde_json::json!([]));
        assert!(
            s.questions["q1"].answered.is_some(),
            "resolved episodes must not leave a stale person question"
        );
        assert!(s.anomalies.is_empty(), "{:?}", s.anomalies);
    }
    for outcome in [Outcome::Question, Outcome::Blocked] {
        let (_dir, j, id, _) = fixture();
        task_event(
            &j,
            &id,
            START,
            Kind::TaskReported {
                outcome,
                summary: "Need the person".into(),
                result: None,
            },
        );
        ack(&j, START + 1);
        let s = j.read_state().unwrap();
        assert_eq!(views::attention(&s, None)[0].text, "Need the person");
        task_event(
            &j,
            &id,
            START + 2,
            Kind::TaskReported {
                outcome: Outcome::Done,
                summary: "Person unblocked me".into(),
                result: None,
            },
        );
        let s = j.read_state().unwrap();
        assert!(s.tasks[&id].blocked.is_none());
        assert!(s.anomalies.is_empty());
    }
}

#[test]
fn board_exposes_every_open_linked_question_after_one_is_answered() {
    let (_dir, j, id, _) = fixture();
    block(&j, &id);
    for qid in ["q1", "q2"] {
        task_event(&j, &id, START, Kind::QuestionAsked {
            qid: qid.into(), text: qid.into(), options: None,
        });
    }
    let s = j.read_state().unwrap();
    assert_eq!(views::attention(&s, None).len(), 1);
    let board = views::board(&s, None, START, false);
    assert_eq!(board["open_questions"].as_array().unwrap().len(), 2);
    global(&j, START + 1, Kind::QuestionAnswered { qid: "q1".into(), text: "yes".into() });
    let s = j.read_state().unwrap();
    let board = views::board(&s, None, START + 1, false);
    assert_eq!(board["open_questions"].as_array().unwrap().len(), 1);
    assert_eq!(board["open_questions"][0]["qid"], "q2");
}

#[test]
fn reminders_back_off_cap_and_replay_without_reset_or_catchup_burst() {
    let (dir, mut j, id, facts) = fixture();
    let cfg = Config::default();
    block(&j, &id);
    ack(&j, START + 1);
    let mut last = START;
    for minutes in [10, 20, 40, 80, 160, 240, 240] {
        let next = last + minutes * 60_000;
        assert!(reminder(&j, &facts, &cfg, next - 1).is_none());
        let record = reminder(&j, &facts, &cfg, next).expect("reminder is due");
        // Computing an action is not delivery: do not consume the schedule on failure.
        assert!(reminder(&j, &facts, &cfg, next).is_some());
        j.lock()
            .unwrap()
            .append(&Actor::Watcher, next, vec![(None, None, record)])
            .unwrap();
        let before = j.read_state().unwrap().tasks[&id].blocked.clone().unwrap();
        drop(j);
        let paths = CoordPaths::new(dir.path());
        std::fs::remove_file(&paths.state).unwrap();
        j = Journal::open(&paths).unwrap();
        let replayed = j.read_state().unwrap();
        let after = replayed.tasks[&id].blocked.as_ref().unwrap();
        assert_eq!(
            (after.reminders, after.last_reminded_ms),
            (before.reminders, next)
        );
        assert!(reminder(&j, &facts, &cfg, next + 1).is_none());
        assert!(
            replayed.waking.is_empty(),
            "a reminder is a delivery, not an endlessly pending inbox item"
        );
        last = next;
    }
    let much_later = last + 24 * 3_600_000;
    let record = reminder(&j, &facts, &cfg, much_later).unwrap();
    j.lock()
        .unwrap()
        .append(&Actor::Watcher, much_later, vec![(None, None, record)])
        .unwrap();
    assert!(reminder(&j, &facts, &cfg, much_later + 1).is_none());
    let mut offline = facts;
    offline
        .terms
        .get_mut("t21")
        .unwrap()
        .status
        .as_mut()
        .unwrap()
        .state = "working".into();
    assert!(reminder(&j, &offline, &cfg, much_later + 240 * 60_000).is_none());
}

#[test]
fn person_question_stops_reminders_and_answer_resolves_episode() {
    let (_dir, j, id, facts) = fixture();
    block(&j, &id);
    ack(&j, START + 1);
    task_event(
        &j,
        &id,
        START + 2,
        Kind::QuestionAsked {
            qid: "q1".into(),
            text: "Please continue in the delegate tab".into(),
            options: None,
        },
    );
    assert!(reminder(&j, &facts, &Config::default(), START + 48 * 3_600_000).is_none());
    let s = j.read_state().unwrap();
    let items = views::attention(&s, None);
    assert_eq!(
        items.len(),
        1,
        "one blocked episode must not become two badges"
    );
    assert_eq!(items[0].qid.as_deref(), Some("q1"));
    global(
        &j,
        START + 3,
        Kind::QuestionAnswered {
            qid: "q1".into(),
            text: "Continued".into(),
        },
    );
    let s = j.read_state().unwrap();
    assert!(s.tasks[&id].blocked.is_none());
    assert!(views::attention(&s, None).is_empty());
    // The still-blocked terminal is the same answered episode; no immediate reopen.
    assert!(
        !rules::evaluate(&s, &facts, &Config::default(), START + 4)
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
    task_event(
        &j,
        &id,
        START + 5,
        Kind::TaskAttention {
            state: AttentionState::Clear,
            msg: None,
        },
    );
    task_event(
        &j,
        &id,
        START + 6,
        Kind::TaskAttention {
            state: AttentionState::Blocked,
            msg: Some("A new prompt".into()),
        },
    );
    let s = j.read_state().unwrap();
    assert_eq!(s.tasks[&id].blocked.as_ref().unwrap().since_ms, START + 6);
    assert_eq!(views::attention(&s, None).len(), 1);
    assert!(s.anomalies.is_empty());
    let replayed = fold::fold(&j.read_after(0).unwrap());
    assert_eq!(views::attention(&replayed, None)[0].text, "A new prompt");
}
