//! Standard journal-derived coordinator measurements.
use super::Ctx;
use super::ids::{TaskId, WsId};
use super::kinds::{Entry, Kind, NudgeTarget, Relation, TurnOrigin};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct TaskMetric {
    placed: u64,
    role: String,
    role_hash: String,
    rung: u8,
    release: String,
    first_report: Option<u64>,
    turns: u64,
    person_turns: u64,
    nudges: u64,
    stalls: u64,
    start_blocked: u64,
    outcome: Option<String>,
    relation: Option<Relation>,
}
#[derive(Default)]
struct WsMetric {
    title: String,
    opened: u64,
    closed: Option<u64>,
    tasks: u64,
    retained_since: Option<u64>,
    retained_ms: u64,
    outcome: Option<String>,
    rating: Option<String>,
}
fn label<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
fn quantile(values: &mut [u64], percentile: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    if percentile == 50 && values.len() % 2 == 0 {
        let a = values[values.len() / 2 - 1];
        let b = values[values.len() / 2];
        return Some(a + (b - a) / 2);
    }
    Some(values[(values.len() * percentile).div_ceil(100).saturating_sub(1)])
}
fn group(name: &str, key: Value, tasks: &[&TaskMetric]) -> Value {
    let n = tasks.len();
    let mut times: Vec<_> = tasks
        .iter()
        .filter_map(|t| t.first_report.map(|r| r.saturating_sub(t.placed)))
        .collect();
    let median = quantile(&mut times, 50);
    let p90 = quantile(&mut times, 90);
    let mut outcomes = BTreeMap::<String, u64>::new();
    for t in tasks {
        if let Some(o) = &t.outcome {
            *outcomes.entry(o.clone()).or_default() += 1;
        }
    }
    let mean = |f: fn(&TaskMetric) -> u64| {
        if n == 0 {
            0.0
        } else {
            tasks.iter().map(|t| f(t)).sum::<u64>() as f64 / n as f64
        }
    };
    let rate = |r| {
        if n == 0 {
            0.0
        } else {
            tasks.iter().filter(|t| t.relation == Some(r)).count() as f64 / n as f64
        }
    };
    let mut v = json!({"tasks":n,"median_placed_to_first_report_ms":median,"p90_placed_to_first_report_ms":p90,
        "mean_turns":mean(|t|t.turns),"mean_person_turns":mean(|t|t.person_turns),
        "nudges":tasks.iter().map(|t|t.nudges).sum::<u64>(),"stalls":tasks.iter().map(|t|t.stalls).sum::<u64>(),
        "start_blocked":tasks.iter().map(|t|t.start_blocked).sum::<u64>(),"outcomes":outcomes,
        "rework_rate":rate(Relation::Rework),"escalation_rate":rate(Relation::Escalation)});
    v[name] = key;
    v
}
pub fn parse_since(value: &str, now_ms: u64) -> Result<u64> {
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let n: u64 = value[..split].parse()?;
    let scale: u64 = match &value[split..] {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => bail!("invalid duration"),
    };
    Ok(now_ms.saturating_sub(
        n.checked_mul(scale)
            .ok_or_else(|| anyhow::anyhow!("duration overflow"))?,
    ))
}
pub fn stats(ctx: &Ctx, since: Option<&str>) -> Result<Value> {
    let now = ctx.now();
    let since_ms = since.map(|s| parse_since(s, now)).transpose()?;
    let entries = ctx.journal()?.read_after(0)?;
    Ok(compute(&entries, now, since_ms))
}
pub fn compute(entries: &[Entry], now: u64, since_ms: Option<u64>) -> Value {
    let since = since_ms.unwrap_or(0);
    let mut tasks = BTreeMap::<TaskId, TaskMetric>::new();
    let mut streams = BTreeMap::<WsId, WsMetric>::new();
    let mut release = String::new();
    let mut turns = BTreeMap::<String, u64>::new();
    let mut work = 0u64;
    let mut questions = BTreeMap::<String, u64>::new();
    let mut question_times = Vec::new();
    let mut waking = BTreeMap::<u64, u64>::new();
    let mut wake_times = Vec::new();
    let mut corrections = BTreeMap::<String, u64>::new();
    let mut restarts = BTreeMap::<String, u64>::new();
    let mut sessions = 0u64;
    for e in entries {
        let kind = e.typed();
        if kind.is_waking(&e.actor) {
            waking.insert(e.seq, e.ts_ms);
        }
        match kind {
            Kind::WsOpened { title, .. } => {
                if let Some(id) = &e.ws {
                    streams.insert(
                        id.clone(),
                        WsMetric {
                            title,
                            opened: e.ts_ms,
                            ..Default::default()
                        },
                    );
                }
            }
            Kind::WsClosed { outcome, .. } => {
                if let Some(w) = e.ws.as_ref().and_then(|id| streams.get_mut(id)) {
                    w.closed = Some(e.ts_ms);
                    w.outcome = outcome.as_ref().map(label);
                }
            }
            Kind::WsRetained { .. } => {
                if let Some(w) = e.ws.as_ref().and_then(|id| streams.get_mut(id)) {
                    if w.retained_since.is_none() {
                        w.retained_since = Some(e.ts_ms);
                    }
                }
            }
            Kind::WsCleaned {} => {
                if let Some(w) = e.ws.as_ref().and_then(|id| streams.get_mut(id)) {
                    if let Some(start) = w.retained_since.take() {
                        w.retained_ms = w.retained_ms.saturating_add(e.ts_ms.saturating_sub(start));
                    }
                }
            }
            Kind::TaskPlaced {
                role,
                role_hash,
                rung,
                follows,
                ..
            } => {
                if let Some(id) = &e.task {
                    tasks.insert(
                        id.clone(),
                        TaskMetric {
                            placed: e.ts_ms,
                            role,
                            role_hash,
                            rung,
                            release: release.clone(),
                            relation: follows.map(|f| f.relation),
                            ..Default::default()
                        },
                    );
                    if let Some(w) = e.ws.as_ref().and_then(|id| streams.get_mut(id)) {
                        w.tasks += 1;
                    }
                }
            }
            Kind::TaskReported { .. } => {
                if let Some(t) = e.task.as_ref().and_then(|id| tasks.get_mut(id)) {
                    t.first_report.get_or_insert(e.ts_ms);
                }
            }
            Kind::TaskTurn { origin, .. } => {
                if let Some(t) = e.task.as_ref().and_then(|id| tasks.get_mut(id)) {
                    t.turns += 1;
                    t.person_turns += u64::from(origin == TurnOrigin::Person);
                }
            }
            Kind::TaskStartBlocked { .. } => {
                if let Some(t) = e.task.as_ref().and_then(|id| tasks.get_mut(id)) {
                    t.start_blocked += 1;
                }
            }
            Kind::TaskStalled { .. } => {
                if let Some(t) = e.task.as_ref().and_then(|id| tasks.get_mut(id)) {
                    t.stalls += 1;
                }
            }
            Kind::NudgeSent {
                target: NudgeTarget::Task,
                ..
            } => {
                if let Some(t) = e.task.as_ref().and_then(|id| tasks.get_mut(id)) {
                    t.nudges += 1;
                }
            }
            Kind::TaskClosed { outcome, .. } => {
                if let Some(t) = e.task.as_ref().and_then(|id| tasks.get_mut(id)) {
                    t.outcome = outcome.as_ref().map(label);
                }
            }
            Kind::Rating { target, value, .. } => {
                if let Ok(id) = WsId::parse(&target) {
                    if let Some(w) = streams.get_mut(&id) {
                        w.rating = Some(label(&value));
                    }
                }
            }
            Kind::CoordSessionStarted {
                release: r, reason, ..
            } => {
                release = r;
                if e.ts_ms >= since {
                    sessions += 1;
                    *restarts.entry(reason).or_default() += 1;
                }
            }
            Kind::CoordTurn {
                origin, working_ms, ..
            } if e.ts_ms >= since => {
                *turns.entry(label(&origin)).or_default() += 1;
                work = work.saturating_add(working_ms.unwrap_or(0));
            }
            Kind::InboxDrained { cursor, .. } => {
                let drained: Vec<_> = waking
                    .range(..=cursor)
                    .map(|(&seq, &ts)| (seq, ts))
                    .collect();
                for (seq, ts) in drained {
                    if e.ts_ms >= since {
                        wake_times.push(e.ts_ms.saturating_sub(ts));
                    }
                    waking.remove(&seq);
                }
            }
            Kind::QuestionAsked { qid, .. } => {
                questions.insert(qid, e.ts_ms);
            }
            Kind::QuestionAnswered { qid, .. } => {
                if let Some(asked) = questions.get(&qid) {
                    if e.ts_ms >= since {
                        question_times.push(e.ts_ms.saturating_sub(*asked));
                    }
                }
            }
            Kind::Correction { what, .. } if e.ts_ms >= since => {
                *corrections.entry(label(&what)).or_default() += 1;
            }
            _ => {}
        }
    }
    let selected: Vec<_> = tasks.values().filter(|t| t.placed >= since).collect();
    let mut by_role = BTreeMap::<String, Vec<&TaskMetric>>::new();
    let mut by_rung = BTreeMap::<u8, Vec<&TaskMetric>>::new();
    let mut by_hash = BTreeMap::<String, Vec<&TaskMetric>>::new();
    let mut by_release = BTreeMap::<String, Vec<&TaskMetric>>::new();
    for t in selected {
        by_role.entry(t.role.clone()).or_default().push(t);
        by_rung.entry(t.rung).or_default().push(t);
        by_hash.entry(t.role_hash.clone()).or_default().push(t);
        by_release.entry(t.release.clone()).or_default().push(t);
    }
    let workstreams:Vec<_>=streams.iter().filter(|(_,w)|w.opened>=since).map(|(id,w)|json!({"id":id,"title":w.title,"lead_time_ms":w.closed.map(|t|t.saturating_sub(w.opened)),"tasks":w.tasks,"retained_ms":w.retained_ms+w.retained_since.map_or(0,|t|now.saturating_sub(t)),"outcome":w.outcome,"rating":w.rating})).collect();
    json!({"schema_version":1,"generated_at_ms":now,"since_ms":since_ms,
        "by_role":by_role.iter().map(|(k,v)|group("role",json!(k),v)).collect::<Vec<_>>(),
        "by_rung":by_rung.iter().map(|(k,v)|group("rung",json!(k),v)).collect::<Vec<_>>(),
        "by_role_hash":by_hash.iter().map(|(k,v)|group("role_hash",json!(k),v)).collect::<Vec<_>>(),
        "by_release":by_release.iter().map(|(k,v)|group("release",json!(k),v)).collect::<Vec<_>>(),"workstreams":workstreams,
        "coordinator":{"turns_by_origin":turns,"working_ms":work,"median_wake_to_drain_ms":quantile(&mut wake_times,50),"median_question_to_answer_ms":quantile(&mut question_times,50),"corrections":corrections,"restarts":restarts,"sessions":sessions}})
}

#[cfg(test)]
mod tests {
    use super::super::kinds::Actor;
    use super::*;
    fn event(
        seq: u64,
        ts: u64,
        ws: Option<&str>,
        task: Option<&str>,
        kind: &str,
        data: Value,
    ) -> Entry {
        Entry {
            v: 1,
            seq,
            ts_ms: ts,
            actor: Actor::Cli,
            kind: kind.into(),
            ws: ws.map(|s| WsId::parse(s).unwrap()),
            task: task.map(|s| TaskId::parse(s).unwrap()),
            op: None,
            cause: None,
            data: data.as_object().unwrap().clone(),
        }
    }
    #[test]
    fn scripted_metrics_and_since() {
        let w = "ws-abcdef";
        let a = "ws-abcdef-t1";
        let b = "ws-abcdef-t2";
        let entries = vec![
            event(
                1,
                100,
                None,
                None,
                "coord.session_started",
                json!({"role":"coord-main","terminal":"t","space":"sp","release":"v1","reason":"up","brief":"/b"}),
            ),
            event(2, 110, Some(w), None, "ws.opened", json!({"title":"work"})),
            event(
                3,
                120,
                Some(w),
                Some(a),
                "task.placed",
                json!({"rung":3,"reason":"x","rejected":[],"role":"light","harness":"omp","title":"a","brief":"/a","role_hash":"aaa"}),
            ),
            event(
                4,
                125,
                Some(w),
                Some(a),
                "task.turn",
                json!({"turn":1,"origin":"start","working_ms":10}),
            ),
            event(
                5,
                130,
                Some(w),
                Some(a),
                "task.reported",
                json!({"outcome":"done","summary":"x","result":null}),
            ),
            event(
                6,
                140,
                Some(w),
                Some(b),
                "task.placed",
                json!({"rung":2,"reason":"x","rejected":[],"role":"light","harness":"omp","title":"b","brief":"/b","role_hash":"aaa","follows":{"task":a,"relation":"rework"}}),
            ),
            event(
                7,
                150,
                Some(w),
                Some(b),
                "task.turn",
                json!({"turn":1,"origin":"person","working_ms":15}),
            ),
            event(
                8,
                170,
                Some(w),
                Some(b),
                "task.reported",
                json!({"outcome":"done","summary":"x","result":null}),
            ),
            event(
                9,
                180,
                None,
                None,
                "inbox.drained",
                json!({"cursor":8,"count":2}),
            ),
            event(
                10,
                190,
                None,
                None,
                "question.asked",
                json!({"qid":"q1","text":"?"}),
            ),
            event(
                11,
                220,
                None,
                None,
                "question.answered",
                json!({"qid":"q1","text":"yes"}),
            ),
            event(
                12,
                230,
                None,
                None,
                "coord.turn",
                json!({"turn":1,"origin":"person","working_ms":7}),
            ),
        ];
        let out = compute(&entries, 240, None);
        assert_eq!(out["by_role"][0]["tasks"], 2);
        assert_eq!(out["by_role"][0]["median_placed_to_first_report_ms"], 20);
        assert_eq!(out["by_role"][0]["p90_placed_to_first_report_ms"], 30);
        assert_eq!(out["by_role"][0]["mean_turns"], 1.0);
        assert_eq!(out["by_role"][0]["mean_person_turns"], 0.5);
        assert_eq!(out["by_role"][0]["rework_rate"], 0.5);
        assert_eq!(out["by_role_hash"][0]["role_hash"], "aaa");
        assert_eq!(out["by_release"][0]["release"], "v1");
        assert_eq!(out["coordinator"]["median_wake_to_drain_ms"], 30);
        assert_eq!(out["coordinator"]["median_question_to_answer_ms"], 30);
        assert_eq!(out["coordinator"]["turns_by_origin"]["person"], 1);
        let filtered = compute(&entries, 240, Some(135));
        assert_eq!(filtered["by_role"][0]["tasks"], 1);
        assert_eq!(filtered["by_rung"][0]["rung"], 2);
        assert_eq!(parse_since("2s", 3000).unwrap(), 1000);
    }
}
