//! Journal-backed read models for the coordinator.
use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

use super::ids::{TaskId, WsId};
use super::journal::{Journal, LogPage, LogQuery};
use super::kinds::{Actor, Entry, Outcome};
use super::state::{State, Task, TaskState, Ws, WsState};

#[derive(Clone, Debug, Serialize)]
pub struct Attention {
    pub id: String,
    pub kind: String,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
    pub since_ms: u64,
    pub text: String,
    pub options: Option<Vec<String>>,
    pub seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qid: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Counts {
    pub running: usize,
    pub needs_you: usize,
    pub retained: usize,
    pub open_workstreams: usize,
}

pub fn attention(state: &State, coordinator_word: Option<&str>) -> Vec<Attention> {
    let mut items = Vec::new();
    for (qid, q) in state.questions.iter().filter(|(_, q)| q.answered.is_none()) {
        if q.task
            .as_ref()
            .and_then(|id| state.tasks.get(id))
            .and_then(|t| t.blocked.as_ref())
            .is_some_and(|item| q.asked_seq >= item.seq)
        {
            continue;
        }
        items.push(Attention {
            id: format!("q-{}", q.asked_seq),
            kind: "question".into(),
            ws: q.ws.clone(),
            task: q.task.clone(),
            since_ms: q.asked_ms,
            text: q.text.clone(),
            options: q.options.clone(),
            seq: Some(q.asked_seq),
            qid: Some(qid.clone()),
        });
    }
    for t in state.tasks.values() {
        if let Some(item) = &t.blocked {
            let question = state.questions.iter().find(|(_, q)| {
                q.answered.is_none() && q.task.as_ref() == Some(&t.id) && q.asked_seq >= item.seq
            });
            items.push(Attention {
                id: format!("blocked-{}", item.seq),
                kind: if item.outcome == Outcome::Question {
                    "task_question"
                } else {
                    "task_blocked"
                }
                .into(),
                ws: Some(t.ws.clone()),
                task: Some(t.id.clone()),
                since_ms: item.since_ms,
                text: item.text.clone(),
                options: question.and_then(|(_, q)| q.options.clone()),
                seq: Some(item.seq),
                qid: question.map(|(id, _)| id.clone()),
            });
            continue;
        }
        if t.state == TaskState::Running || t.state == TaskState::Placed {
            for (seq, since, prefix, kind, text) in [
                (
                    t.start_blocked,
                    t.start_blocked_ms,
                    "sb",
                    "start_blocked",
                    "Task did not start; it may be waiting at a start-up dialog.",
                ),
                (
                    t.stalled,
                    t.stalled_ms,
                    "st",
                    "stalled",
                    "Task has not reported after a nudge.",
                ),
            ] {
                if let Some(seq) = seq {
                    items.push(Attention {
                        id: format!("{prefix}-{seq}"),
                        kind: kind.into(),
                        ws: Some(t.ws.clone()),
                        task: Some(t.id.clone()),
                        since_ms: since.unwrap_or(t.state_since_ms),
                        text: text.into(),
                        options: None,
                        seq: Some(seq),
                        qid: None,
                    });
                }
            }
        }
    }
    if coordinator_word == Some("blocked") {
        if let Some(terminal) = &state.coordinator.terminal {
            items.push(Attention {
                id: format!("cb-{terminal}"),
                kind: "coordinator_blocked".into(),
                ws: None,
                task: None,
                since_ms: state.coordinator.started_ms.unwrap_or(0),
                text: "Coordinator is blocked.".into(),
                options: None,
                seq: None,
                qid: None,
            });
        }
    }
    items.sort_by(|a, b| (a.since_ms, &a.id).cmp(&(b.since_ms, &b.id)));
    items
}

pub fn counts(state: &State, attention_len: usize) -> Counts {
    Counts {
        running: state
            .tasks
            .values()
            .filter(|t| {
                t.blocked.is_none()
                    && (t.state == TaskState::Placed
                        || (t.state == TaskState::Running && t.stalled.is_none()))
            })
            .count(),
        needs_you: attention_len,
        retained: state
            .workstreams
            .values()
            .filter(|w| w.retained.is_some())
            .count(),
        open_workstreams: state
            .workstreams
            .values()
            .filter(|w| w.state == WsState::Open)
            .count(),
    }
}

fn task_json(t: &Task) -> Value {
    let mut obj = json!({
        "id":t.id, "number":t.n, "title":t.title, "state":t.state,
        "report": if t.state == TaskState::Reported { t.report } else { None },
        "needs_you": t.blocked.is_some(),
        "start_blocked": matches!(t.state, TaskState::Running | TaskState::Placed) && t.start_blocked.is_some(),
        "stalled": t.state == TaskState::Running && t.stalled.is_some(),
        "state_since_ms":t.state_since_ms, "placed_ms":t.placed_ms, "started_ms":t.started_ms,
        "last_event_ms":t.last_event_ms, "rung":t.rung, "placement_reason":t.reason,
        "rejected_rungs":t.rejected, "role":t.role, "harness":t.harness, "model":t.model,
        "summary":t.summary,
        "terminal": if matches!(t.state, TaskState::Dormant | TaskState::Closed) { None } else { t.terminal.as_ref() },
        "space_id":t.space, "tab_id":t.tab, "brief_path":t.brief, "result_path":t.result, "nudges":t.nudges
    });
    if let Some(effort) = &t.effort {
        obj["effort"] = json!(effort);
    }
    obj
}

fn ws_json(state: &State, w: &Ws) -> Value {
    let repo = w
        .repo
        .as_ref()
        .map(|p| json!({"path":p, "name":p.file_name().unwrap_or_default().to_string_lossy()}));
    let space = w
        .space
        .as_ref()
        .map(|s| json!({"id":s.id,"name":s.name,"owned":s.owned}));
    let retained = w.retained.as_ref().map(|r| json!({"reason_code":r.reason_code,"reason":r.reason,
        "since_ms":r.since_ms,"checked_ms":r.checked_ms,"auto_clears":true,
        "checks":r.checks.iter().map(|c| json!({"name":c.name,"ok":c.ok,"detail":c.detail})).collect::<Vec<_>>() }));
    json!({"id":w.id,"title":w.title,"state":w.state,"repo":repo,"delivery_target":w.delivery_target,
        "summary":w.summary,"opened_ms":w.opened_ms,"closed_ms":w.closed_ms,"last_event_ms":w.last_event_ms,
        "space":space,"retained":retained,"tasks":w.tasks.iter().filter_map(|id| state.tasks.get(id)).map(task_json).collect::<Vec<_>>()})
}

pub fn board(state: &State, coordinator_word: Option<&str>, now_ms: u64, all: bool) -> Value {
    json!({"schema_version":1,"generated_at_ms":now_ms,"journal_seq":state.last_seq,
        "workstreams":state.workstreams.values().filter(|w| all || w.state != WsState::Cleaned).map(|w| ws_json(state,w)).collect::<Vec<_>>(),
        "attention":attention(state,coordinator_word),
        "open_questions":state.questions.iter().filter(|(_,q)| q.answered.is_none()).map(|(qid,q)|
            json!({"qid":qid,"text":q.text,"options":q.options,"ws":q.ws,"task":q.task,
                "seq":q.asked_seq,"since_ms":q.asked_ms})).collect::<Vec<_>>()})
}

pub fn board_text(state: &State, now_ms: u64, all: bool) -> String {
    let mut out = format!(
        "# Coordinator board\n\nJournal sequence: {} (at {now_ms})\n",
        state.last_seq
    );
    out.push_str("\n## Needs you\n");
    let items = attention(state, None);
    if items.is_empty() {
        out.push_str("\nNone\n");
    }
    for item in items {
        let task = item.task.as_ref().and_then(|id| state.tasks.get(id));
        out.push_str(&format!(
            "- {}: {} — tab {}, terminal {}, since {} ({} min ago)\n",
            item.task
                .as_ref()
                .map(|id| id.as_str())
                .unwrap_or("Coordinator"),
            item.text,
            task.and_then(|t| t.tab.as_deref()).unwrap_or("unknown"),
            task.and_then(|t| t.terminal.as_deref())
                .unwrap_or("unavailable"),
            item.since_ms,
            now_ms.saturating_sub(item.since_ms) / 60_000,
        ));
    }
    for w in state
        .workstreams
        .values()
        .filter(|w| all || w.state != WsState::Cleaned)
    {
        out.push_str(&format!("\n## {} — {}\n", w.id.as_str(), w.title));
        if let Some(summary) = &w.summary {
            out.push_str(&format!("{summary}\n"));
        }
        if let Some(r) = &w.retained {
            out.push_str(&format!("Retained: {}\n", r.reason));
        }
        for t in w.tasks.iter().filter_map(|id| state.tasks.get(id)) {
            let label = if t.blocked.is_some() {
                "Needs you".to_owned()
            } else {
                format!("{:?}", t.state)
            };
            out.push_str(&format!("- {}: {} ({label})", t.id.as_str(), t.title));
            if let Some(summary) = &t.summary {
                out.push_str(&format!(" — {summary}"));
            }
            out.push('\n');
        }
    }
    out
}

pub fn log(j: &Journal, q: &LogQuery, now_ms: u64) -> Result<Value> {
    let page = j.page(q)?;
    Ok(json!({"schema_version":1,"generated_at_ms":now_ms,
        "newest_seq":page.entries.first().and_then(|e| e["seq"].as_u64()),
        "oldest_seq":page.entries.last().and_then(|e| e["seq"].as_u64()),
        "entries":page.entries,"has_more":page.has_more}))
}

pub fn log_text(page: &LogPage) -> String {
    let mut out = String::from("# Journal\n");
    for e in &page.entries {
        out.push_str(&format!("\n- {}\n", e));
    }
    if page.has_more {
        out.push_str("\nMore older entries are available.\n");
    }
    out
}

pub fn inbox(j: &Journal, state: &State) -> Result<Value> {
    let entries: Vec<_> = j
        .read_after(state.cursor)?
        .into_iter()
        .filter(|e| state.waking.contains(&e.seq) && e.actor != Actor::Coordinator)
        .collect();
    Ok(
        json!({"schema_version":1,"cursor":state.cursor,"last_seq":state.last_seq,"pending":entries.len(),"entries":entries}),
    )
}

pub fn explain(j: &Journal, state: &State, id: &str, _now_ms: u64) -> Result<Value> {
    let (kind, record, ws, task) = if let Ok(task) = TaskId::parse(id) {
        let Some(record) = state.tasks.get(&task) else {
            bail!("unknown task: {id}")
        };
        ("task", task_json(record), None, Some(task))
    } else if let Ok(ws) = WsId::parse(id) {
        let Some(record) = state.workstreams.get(&ws) else {
            bail!("unknown workstream: {id}")
        };
        ("ws", ws_json(state, record), Some(ws), None)
    } else {
        bail!("invalid workstream or task id: {id}")
    };
    let mut entries: Vec<_> = j
        .read_after(0)?
        .into_iter()
        .filter(|e| {
            task.as_ref().map_or_else(
                || e.ws.as_ref() == ws.as_ref(),
                |t| e.task.as_ref() == Some(t),
            )
        })
        .collect();
    entries.sort_by_key(|e| e.seq);
    let checks: Vec<_> = entries
        .iter()
        .filter(|e| e.kind == "res.release_checked")
        .map(|e| {
            json!({
        "resource":e.data.get("resource"),"name":e.data.get("check"),"ok":e.data.get("ok"),
        "detail":e.data.get("detail"),"seq":e.seq})
        })
        .collect();
    let seqs: std::collections::BTreeSet<_> = entries.iter().map(|e| e.seq).collect();
    let anomalies: Vec<_> = state
        .anomalies
        .iter()
        .filter(|a| seqs.contains(&a.seq))
        .collect();
    Ok(
        json!({"schema_version":1,"kind":kind,"id":id,"record":record,"entries":entries,"checks":checks,"anomalies":anomalies}),
    )
}

pub fn explain_text(v: &Value) -> String {
    format!(
        "# Explanation\n\n```json\n{}\n```\n",
        serde_json::to_string_pretty(v).unwrap_or_default()
    )
}

pub fn startup_read(state: &State, waking: &[Entry], now_ms: u64) -> String {
    let mut out = String::new();
    if let Some(note) = &state.coordinator.handoff_note {
        out.push_str(&format!("## Handoff note\n\n{note}\n\n"));
    }
    out.push_str(&board_text(state, now_ms, false));
    out.push_str(&format!(
        "\n## Inbox\n\n{} waking entries pending\n",
        state.waking.len()
    ));
    for e in waking
        .iter()
        .filter(|e| state.waking.contains(&e.seq) && e.actor != Actor::Coordinator)
        .rev()
        .take(20)
    {
        out.push_str(&format!(
            "- {}\n",
            serde_json::to_string(e).unwrap_or_default()
        ));
    }
    out.push_str("\n## Recent decisions\n");
    for d in state.decisions.iter().rev().take(10) {
        out.push_str(&format!("- {} — {}\n", d.what, d.why));
    }
    out.push_str("\n## Standing instructions\n");
    for (id, text) in &state.instructions {
        out.push_str(&format!("- {id}: {text}\n"));
    }
    out.push_str("\n## Open threads\n");
    for t in state.threads.values() {
        out.push_str(&format!("- {}: {}\n", t.id, t.title));
    }
    out.push_str("\n## Open questions\n");
    for q in state.questions.values().filter(|q| q.answered.is_none()) {
        out.push_str(&format!("- {}\n", q.text));
    }
    const CAP: usize = 32 * 1024;
    const NOTICE: &str =
        "\n\nTruncated. Run coord board and coord inbox for the full current state.\n";
    if out.len() > CAP {
        let mut end = CAP - NOTICE.len();
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push_str(NOTICE);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{fold, paths::CoordPaths};
    use super::*;

    fn sample() -> Vec<Entry> {
        include_str!("testdata/journal-sample.jsonl")
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    fn query() -> LogQuery {
        LogQuery {
            limit: 2,
            before_seq: None,
            after_seq: None,
            since_ms: None,
            ws: None,
            task: None,
        }
    }
    #[test]
    fn sample_board_and_attention_resolutions() {
        let entries = sample();
        let mut s = fold::fold(&entries);
        let b = board(&s, None, 123, false);
        assert_eq!(b["schema_version"], 1);
        assert_eq!(b["workstreams"][0]["repo"]["name"], "app");
        assert_eq!(
            b["workstreams"][0]["tasks"][0]["placement_reason"],
            "two delegates will edit auth.rs in parallel"
        );
        assert_eq!(b["attention"][0]["kind"], "task_question");
        let id = TaskId::parse("ws-k3m2ab-t1").unwrap();
        let t = s.tasks.get_mut(&id).unwrap();
        t.state = TaskState::Running;
        t.blocked = None;
        t.start_blocked = Some(30);
        t.start_blocked_ms = Some(300);
        t.stalled = Some(31);
        t.stalled_ms = Some(400);
        let a = attention(&s, None);
        assert_eq!((a[0].kind.as_str(), a[0].since_ms), ("start_blocked", 300));
        assert_eq!((a[1].kind.as_str(), a[1].since_ms), ("stalled", 400));
        assert_eq!(counts(&s, a.len()).needs_you, a.len());
        s.tasks.get_mut(&id).unwrap().state = TaskState::Closed;
        assert!(attention(&s, None).is_empty());
        assert_eq!(
            attention(&s, Some("blocked"))[0].kind,
            "coordinator_blocked"
        );
    }
    #[test]
    fn board_all_includes_cleaned_workstreams() {
        let mut s = fold::fold(&sample());
        s.workstreams.values_mut().next().unwrap().state = WsState::Cleaned;
        assert!(
            board(&s, None, 0, false)["workstreams"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            board(&s, None, 0, true)["workstreams"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(!board_text(&s, 0, false).contains("## ws-k3m2ab"));
        assert!(board_text(&s, 0, true).contains("ws-k3m2ab"));
    }
    #[test]
    fn page_inbox_explain_and_cap() {
        let dir = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(dir.path());
        let j = Journal::open(&paths).unwrap();
        std::fs::write(
            paths.month_file(1794500000000),
            include_str!("testdata/journal-sample.jsonl"),
        )
        .unwrap();
        let s = j.read_state().unwrap();
        let page = log(&j, &query(), 4).unwrap();
        assert_eq!(page["entries"][0]["seq"], 12);
        assert_eq!(page["oldest_seq"], 11);
        assert_eq!(page["has_more"], true);
        let mut q = query();
        q.before_seq = Some(3);
        assert_eq!(log(&j, &q, 0).unwrap()["has_more"], false);
        q.before_seq = None;
        q.after_seq = Some(10);
        q.ws = Some(WsId::parse("ws-k3m2ab").unwrap());
        assert_eq!(
            log(&j, &q, 0).unwrap()["entries"].as_array().unwrap().len(),
            1
        );
        q.since_ms = Some(u64::MAX);
        assert!(
            log(&j, &q, 0).unwrap()["entries"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let inbox = inbox(&j, &s).unwrap();
        assert_eq!(inbox["pending"], 1);
        assert_eq!(inbox["entries"][0]["seq"], 11);
        assert_eq!(explain(&j, &s, "ws-k3m2ab-t1", 0).unwrap()["kind"], "task");
        let mut big = s;
        big.instructions.insert("i1".into(), "💡".repeat(10000));
        assert!(startup_read(&big, &j.read_after(0).unwrap(), 0).len() <= 32768);
    }
}
