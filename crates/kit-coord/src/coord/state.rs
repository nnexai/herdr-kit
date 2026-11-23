use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ids::{TaskId, WsId};
use super::kinds::{
    AttachmentKind, AttachmentMode, AttentionState, Check, Follows, Outcome, RatingValue, Rejected,
    ResourceRef, SessionRef, TaskOutcome, Tier, WsOutcome,
};

pub const FOLD_VERSION: u32 = 9;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub fold_version: u32,
    pub last_seq: u64,
    pub last_ts_ms: u64,
    pub workstreams: BTreeMap<WsId, Ws>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub resources: Vec<ResourceRec>,
    pub intents: BTreeMap<u64, Intent>,
    pub projects: BTreeMap<PathBuf, Project>,
    pub coordinator: CoordRec,
    pub watcher: WatcherRec,
    pub cursor: u64,
    pub waking: BTreeSet<u64>,
    pub last_coordinator_nudge_upto: u64,
    pub questions: BTreeMap<String, Question>,
    pub instructions: BTreeMap<String, String>,
    pub threads: BTreeMap<String, Thread>,
    pub decisions: VecDeque<DecisionRec>,
    pub counters: Counters,
    pub anomalies: Vec<Anomaly>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            fold_version: FOLD_VERSION,
            last_seq: 0,
            last_ts_ms: 0,
            workstreams: BTreeMap::new(),
            tasks: BTreeMap::new(),
            resources: Vec::new(),
            intents: BTreeMap::new(),
            projects: BTreeMap::new(),
            coordinator: CoordRec::default(),
            watcher: WatcherRec::default(),
            cursor: 0,
            waking: BTreeSet::new(),
            last_coordinator_nudge_upto: 0,
            questions: BTreeMap::new(),
            instructions: BTreeMap::new(),
            threads: BTreeMap::new(),
            decisions: VecDeque::new(),
            counters: Counters::default(),
            anomalies: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WsState {
    Open,
    Closed,
    Cleaned,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Placed,
    Running,
    Reported,
    Dormant,
    Closed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ws {
    pub id: WsId,
    pub title: String,
    pub repo: Option<PathBuf>,
    pub delivery_target: Option<String>,
    pub delivery_branch: Option<String>,
    pub state: WsState,
    pub retained: Option<Retained>,
    pub summary: Option<String>,
    pub opened_ms: u64,
    pub closed_ms: Option<u64>,
    pub last_event_ms: u64,
    pub space: Option<SpaceRec>,
    pub cwd: Option<PathBuf>,
    pub tasks: Vec<TaskId>,
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub outcome: Option<WsOutcome>,
    #[serde(default)]
    pub rating: Option<RatingValue>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpaceRec {
    pub id: String,
    pub name: String,
    pub owned: bool,
    pub folder: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Retained {
    pub reason_code: String,
    pub reason: String,
    pub since_ms: u64,
    pub checked_ms: Option<u64>,
    pub checks: Vec<Check>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub ws: WsId,
    pub n: u32,
    pub title: String,
    pub state: TaskState,
    pub report: Option<Outcome>,
    pub start_blocked: Option<u64>,
    pub stalled: Option<u64>,
    pub attention: Option<AttentionState>,
    pub blocked: Option<BlockedEpisode>,
    pub start_blocked_ms: Option<u64>,
    pub stalled_ms: Option<u64>,
    pub state_since_ms: u64,
    pub placed_ms: u64,
    pub started_ms: Option<u64>,
    pub last_event_ms: u64,
    pub rung: u8,
    pub reason: String,
    pub rejected: Vec<Rejected>,
    pub role: String,
    pub harness: String,
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    pub summary: Option<String>,
    pub terminal: Option<String>,
    pub space: Option<String>,
    pub tab: Option<String>,
    pub cwd: Option<PathBuf>,
    pub pid: Option<u32>,
    pub created_ms: Option<u64>,
    pub session: Option<SessionRef>,
    pub brief: PathBuf,
    pub result: Option<PathBuf>,
    pub nudges: u32,
    pub last_nudge: Option<(u64, u64)>,
    pub last_activity_seq: u64,
    pub reported_seq: Option<u64>,
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub follows: Option<Follows>,
    #[serde(default)]
    pub role_hash: String,
    #[serde(default)]
    pub brief_bytes: u64,
    #[serde(default)]
    pub outcome: Option<TaskOutcome>,
    #[serde(default)]
    pub rating: Option<RatingValue>,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub person_turns: u64,
    #[serde(default)]
    pub working_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentRec>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRec {
    pub id: String,
    pub label: String,
    pub kind: AttachmentKind,
    pub mode: AttachmentMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored_path: Option<String>,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256_12: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    pub report_seq: u64,
    pub report_kind: String,
    pub ts_ms: u64,
    #[serde(default)]
    pub pruned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pruned_reason: Option<String>,
}

/// A durable open item, independent of the disposable inbox cursor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockedEpisode {
    pub seq: u64,
    pub since_ms: u64,
    pub text: String,
    pub outcome: Outcome,
    pub reminders: u32,
    pub last_reminded_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceRec {
    pub r: ResourceRef,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
    pub owned: bool,
    pub released: bool,
    pub created_seq: u64,
    pub last_checks: BTreeMap<String, (bool, Option<String>)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Intent {
    pub seq: u64,
    pub ts_ms: u64,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
    pub r: ResourceRef,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub purpose: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CoordRec {
    pub terminal: Option<String>,
    pub space: Option<String>,
    pub tab: Option<String>,
    pub session: Option<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_ms: Option<u64>,
    pub role: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    pub release: Option<String>,
    pub started_ms: Option<u64>,
    pub sessions_started: u64,
    pub handoff: Option<(u64, String)>,
    pub handoff_note: Option<String>,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub person_turns: u64,
    #[serde(default)]
    pub inbox_turns: u64,
    #[serde(default)]
    pub working_ms: u64,
    #[serde(default)]
    pub role_hash: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WatcherRec {
    pub release: Option<String>,
    pub terminal: Option<String>,
    pub pid: Option<u32>,
    pub started_ms: Option<u64>,
    pub stopped_ms: Option<u64>,
    pub stop_reason: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Question {
    pub text: String,
    pub options: Option<Vec<String>>,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
    pub asked_seq: u64,
    pub asked_ms: u64,
    pub answered: Option<Answer>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    pub seq: u64,
    pub ts_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub title: String,
    pub note: Option<String>,
    pub opened_seq: u64,
    pub opened_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionRec {
    pub seq: u64,
    pub ts_ms: u64,
    pub what: String,
    pub why: String,
    pub tier: Tier,
    pub qid: Option<String>,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Counters {
    pub instruction: u64,
    pub thread: u64,
    pub question: u64,
}
impl Default for Counters {
    fn default() -> Self {
        Self {
            instruction: 1,
            thread: 1,
            question: 1,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Anomaly {
    pub seq: u64,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_round_trips_with_current_version() {
        let state = State::default();
        assert_eq!(state.fold_version, FOLD_VERSION);
        assert_eq!(state.counters.instruction, 1);
        let decoded: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(decoded.fold_version, FOLD_VERSION);
        assert_eq!(decoded.last_seq, 0);
        assert!(decoded.workstreams.is_empty());
    }
    #[test]
    fn states_use_board_names() {
        assert_eq!(
            serde_json::to_string(&WsState::Cleaned).unwrap(),
            "\"cleaned\""
        );
        assert_eq!(
            serde_json::to_string(&TaskState::Dormant).unwrap(),
            "\"dormant\""
        );
    }
}
