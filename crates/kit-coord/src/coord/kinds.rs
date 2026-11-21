use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::ids::{TaskId, WsId};

pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub v: u32,
    pub seq: u64,
    pub ts_ms: u64,
    pub actor: Actor,
    pub kind: String,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<u64>,
    pub data: Map<String, Value>,
}

impl Entry {
    pub fn typed(&self) -> Kind {
        let mut envelope = Map::new();
        envelope.insert("kind".into(), Value::String(self.kind.clone()));
        envelope.insert("data".into(), Value::Object(self.data.clone()));
        serde_json::from_value(Value::Object(envelope)).unwrap_or(Kind::Unknown)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    Coordinator,
    Watcher,
    User,
    Cli,
    Task(TaskId),
}

impl Serialize for Actor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Coordinator => serializer.serialize_str("coordinator"),
            Self::Watcher => serializer.serialize_str("watcher"),
            Self::User => serializer.serialize_str("user"),
            Self::Cli => serializer.serialize_str("cli"),
            Self::Task(id) => serializer.serialize_str(&format!("task:{}", id.as_str())),
        }
    }
}
impl<'de> Deserialize<'de> for Actor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "coordinator" => Ok(Self::Coordinator),
            "watcher" => Ok(Self::Watcher),
            "user" => Ok(Self::User),
            "cli" => Ok(Self::Cli),
            _ => value
                .strip_prefix("task:")
                .ok_or_else(|| serde::de::Error::custom("invalid actor"))
                .and_then(|id| {
                    TaskId::parse(id)
                        .map(Self::Task)
                        .map_err(serde::de::Error::custom)
                }),
        }
    }
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum $name { $($variant),+ }
    };
}
string_enum!(SendVia { Send, Resume });
string_enum!(Outcome {
    Done,
    Blocked,
    Question
});
string_enum!(AttentionState {
    Blocked,
    Error,
    Clear
});
string_enum!(ResType {
    Space,
    Worktree,
    Branch,
    Tab
});
string_enum!(NudgeTarget { Coordinator, Task });
string_enum!(Tier {
    Silent,
    Digest,
    Ask
});
string_enum!(CorrectionWhat {
    Placement,
    Model,
    Cleanup,
    Other
});
string_enum!(SessionMode {
    Dir,
    Preset,
    CodexRollout,
    AgyConversation,
    None
});
string_enum!(WsOutcome {
    Delivered,
    Partial,
    Abandoned
});
string_enum!(TaskOutcome {
    Accepted,
    Rejected,
    Superseded,
    Abandoned
});
string_enum!(RatingValue { Good, Bad });
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    FollowUp,
    Rework,
    Escalation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOrigin {
    Start,
    Send,
    Nudge,
    Person,
    Inbox,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Follows {
    pub task: TaskId,
    pub relation: Relation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRef {
    pub r#type: ResType,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_branch: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
}
impl ResourceRef {
    /// A resource identified only by type and name; callers fill in what applies.
    pub fn new(r#type: ResType, name: impl Into<String>) -> Self {
        Self {
            r#type,
            name: name.into(),
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
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub mode: SessionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejected {
    pub rung: u8,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum Kind {
    #[serde(rename = "ws.opened")]
    WsOpened {
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery_target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery_branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        purpose: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<String>,
    },
    #[serde(rename = "ws.closed")]
    WsClosed {
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<WsOutcome>,
    },
    #[serde(rename = "ws.cleaned")]
    WsCleaned {},
    #[serde(rename = "ws.retained")]
    WsRetained {
        reason_code: String,
        reason: String,
        checks: Vec<Check>,
    },
    #[serde(rename = "task.placed")]
    TaskPlaced {
        rung: u8,
        reason: String,
        rejected: Vec<Rejected>,
        role: String,
        harness: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        title: String,
        brief: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_of: Option<TaskId>,
        #[serde(default)]
        session: Option<SessionRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follows: Option<Follows>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        role_hash: String,
        #[serde(default)]
        brief_bytes: u64,
    },
    #[serde(rename = "task.started")]
    TaskStarted {
        terminal: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<String>,
        tab_name: String,
        space: String,
        cwd: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        created_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionRef>,
        resumed: bool,
    },
    #[serde(rename = "task.launch_details")]
    TaskLaunchDetails {
        terminal: String,
        tab: Option<String>,
        session: Option<SessionRef>,
    },
    #[serde(rename = "task.session")]
    TaskSession { session: SessionRef },
    #[serde(rename = "task.sent")]
    TaskSent {
        message: PathBuf,
        via: SendVia,
        terminal: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started: Option<bool>,
    },
    #[serde(rename = "task.reported")]
    TaskReported {
        outcome: Outcome,
        summary: String,
        #[serde(default)]
        result: Option<PathBuf>,
    },
    #[serde(rename = "task.progress")]
    TaskProgress { summary: String },
    #[serde(rename = "task.note")]
    TaskNote { text: String },
    #[serde(rename = "task.start_blocked")]
    TaskStartBlocked { terminal: String, after_secs: u64 },
    #[serde(rename = "task.trust_confirmed")]
    TaskTrustConfirmed {
        terminal: String,
        keys: Vec<String>,
        start_blocked_seq: u64,
    },
    #[serde(rename = "task.attention")]
    TaskAttention {
        state: AttentionState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        msg: Option<String>,
    },
    #[serde(rename = "task.blocked_reminder")]
    TaskBlockedReminder { task: TaskId, episode_seq: u64 },
    #[serde(rename = "task.stalled")]
    TaskStalled { silent_secs: u64, nudge_seq: u64 },
    #[serde(rename = "task.dormant")]
    TaskDormant { reason: String },
    #[serde(rename = "task.closed")]
    TaskClosed {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<TaskOutcome>,
    },
    #[serde(rename = "task.turn")]
    TaskTurn {
        turn: u64,
        origin: TurnOrigin,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_ms: Option<u64>,
    },
    #[serde(rename = "coord.turn")]
    CoordTurn {
        turn: u64,
        origin: TurnOrigin,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_ms: Option<u64>,
    },
    #[serde(rename = "coord.sent")]
    CoordSent {
        text: String,
        terminal: String,
        qid: Option<String>,
    },
    #[serde(rename = "rating")]
    Rating {
        target: String,
        value: RatingValue,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    #[serde(rename = "config.changed")]
    ConfigChanged { key: String, old: Value, new: Value },
    #[serde(rename = "res.intent")]
    ResIntent(ResourceRef),
    #[serde(rename = "res.created")]
    ResCreated(ResourceRef),
    #[serde(rename = "res.adopted")]
    ResAdopted(ResourceRef),
    #[serde(rename = "res.release_checked")]
    ResReleaseChecked {
        resource: String,
        r#type: ResType,
        name: String,
        check: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    #[serde(rename = "res.released")]
    ResReleased {
        r#type: ResType,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    #[serde(rename = "nudge.sent")]
    NudgeSent {
        target: NudgeTarget,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upto_seq: Option<u64>,
    },
    #[serde(rename = "reconcile.finding")]
    ReconcileFinding { code: String, summary: String },
    #[serde(rename = "watcher.started")]
    WatcherStarted {
        release: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<String>,
        pid: u32,
    },
    #[serde(rename = "watcher.stopped")]
    WatcherStopped { reason: String },
    #[serde(rename = "coord.session_started")]
    CoordSessionStarted {
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        terminal: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_ms: Option<u64>,
        space: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionRef>,
        release: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        role_hash: String,
        reason: String,
        brief: PathBuf,
    },
    #[serde(rename = "coord.session_ended")]
    CoordSessionEnded { reason: String, terminal: String },
    #[serde(rename = "coord.handoff_requested")]
    CoordHandoffRequested {
        by: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    #[serde(rename = "inbox.drained")]
    InboxDrained { cursor: u64, count: u64 },
    #[serde(rename = "decision")]
    Decision {
        what: String,
        why: String,
        tier: Tier,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        qid: Option<String>,
    },
    #[serde(rename = "question.asked")]
    QuestionAsked {
        qid: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<Vec<String>>,
    },
    #[serde(rename = "question.answered")]
    QuestionAnswered { qid: String, text: String },
    #[serde(rename = "scout.result")]
    ScoutResult { question: String, summary: String },
    #[serde(rename = "instruction.set")]
    InstructionSet {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    #[serde(rename = "thread.opened")]
    ThreadOpened {
        id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    #[serde(rename = "thread.closed")]
    ThreadClosed {
        id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    #[serde(rename = "correction")]
    Correction {
        what: CorrectionWhat,
        why: String,
        r#ref: String,
    },
    #[serde(other)]
    Unknown,
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::WsOpened { .. } => "ws.opened",
            Self::WsClosed { .. } => "ws.closed",
            Self::WsCleaned { .. } => "ws.cleaned",
            Self::WsRetained { .. } => "ws.retained",
            Self::TaskPlaced { .. } => "task.placed",
            Self::TaskStarted { .. } => "task.started",
            Self::TaskLaunchDetails { .. } => "task.launch_details",
            Self::TaskSession { .. } => "task.session",
            Self::TaskSent { .. } => "task.sent",
            Self::TaskReported { .. } => "task.reported",
            Self::TaskProgress { .. } => "task.progress",
            Self::TaskNote { .. } => "task.note",
            Self::TaskStartBlocked { .. } => "task.start_blocked",
            Self::TaskTrustConfirmed { .. } => "task.trust_confirmed",
            Self::TaskAttention { .. } => "task.attention",
            Self::TaskBlockedReminder { .. } => "task.blocked_reminder",
            Self::TaskStalled { .. } => "task.stalled",
            Self::TaskDormant { .. } => "task.dormant",
            Self::TaskClosed { .. } => "task.closed",
            Self::TaskTurn { .. } => "task.turn",
            Self::CoordTurn { .. } => "coord.turn",
            Self::CoordSent { .. } => "coord.sent",
            Self::Rating { .. } => "rating",
            Self::ConfigChanged { .. } => "config.changed",
            Self::ResIntent(_) => "res.intent",
            Self::ResCreated(_) => "res.created",
            Self::ResAdopted(_) => "res.adopted",
            Self::ResReleaseChecked { .. } => "res.release_checked",
            Self::ResReleased { .. } => "res.released",
            Self::NudgeSent { .. } => "nudge.sent",
            Self::ReconcileFinding { .. } => "reconcile.finding",
            Self::WatcherStarted { .. } => "watcher.started",
            Self::WatcherStopped { .. } => "watcher.stopped",
            Self::CoordSessionStarted { .. } => "coord.session_started",
            Self::CoordSessionEnded { .. } => "coord.session_ended",
            Self::CoordHandoffRequested { .. } => "coord.handoff_requested",
            Self::InboxDrained { .. } => "inbox.drained",
            Self::Decision { .. } => "decision",
            Self::QuestionAsked { .. } => "question.asked",
            Self::QuestionAnswered { .. } => "question.answered",
            Self::ScoutResult { .. } => "scout.result",
            Self::InstructionSet { .. } => "instruction.set",
            Self::ThreadOpened { .. } => "thread.opened",
            Self::ThreadClosed { .. } => "thread.closed",
            Self::Correction { .. } => "correction",
            Self::Unknown => "unknown",
        }
    }
    pub fn is_waking(&self, actor: &Actor) -> bool {
        if matches!(actor, Actor::Coordinator) {
            return false;
        }
        match self {
            Self::TaskReported { .. }
            | Self::TaskNote { .. }
            | Self::TaskStalled { .. }
            | Self::TaskStartBlocked { .. }
            | Self::TaskDormant { .. }
            | Self::ReconcileFinding { .. }
            | Self::WsRetained { .. } => true,
            Self::TaskAttention { state, .. } => *state != AttentionState::Clear,
            Self::InstructionSet { .. }
            | Self::Correction { .. }
            | Self::ThreadOpened { .. }
            | Self::ThreadClosed { .. } => matches!(actor, Actor::User | Actor::Cli),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn actors_are_strings_and_validate_task_ids() {
        let actor: Actor = serde_json::from_value(json!("task:ws-k3m2ab-t1")).unwrap();
        assert_eq!(
            serde_json::to_value(actor).unwrap(),
            json!("task:ws-k3m2ab-t1")
        );
        assert!(serde_json::from_value::<Actor>(json!("task:bad")).is_err());
        assert!(serde_json::from_value::<Actor>(json!("stranger")).is_err());
    }
    #[test]
    fn typed_falls_back_on_unknown_and_invalid_data() {
        let mut entry: Entry = serde_json::from_value(json!({"v":1,"seq":1,"ts_ms":0,"actor":"cli","kind":"task.progress","ws":null,"task":null,"data":{"summary":"ok"}})).unwrap();
        assert!(matches!(entry.typed(), Kind::TaskProgress { .. }));
        entry.data.clear();
        assert!(matches!(entry.typed(), Kind::Unknown));
        entry.kind = "future.kind".into();
        assert!(matches!(entry.typed(), Kind::Unknown));
    }
    #[test]
    fn serialization_and_waking_contract() {
        let report = Kind::TaskReported {
            outcome: Outcome::Done,
            summary: "done".into(),
            result: None,
        };
        assert_eq!(
            serde_json::to_value(&report).unwrap()["data"]["result"],
            Value::Null
        );
        assert!(report.is_waking(&Actor::Watcher));
        assert!(!report.is_waking(&Actor::Coordinator));
        assert!(
            !Kind::TaskAttention {
                state: AttentionState::Clear,
                msg: None
            }
            .is_waking(&Actor::Watcher)
        );
        assert!(
            !Kind::InstructionSet {
                id: "i1".into(),
                text: None
            }
            .is_waking(&Actor::Watcher)
        );
        assert!(
            Kind::InstructionSet {
                id: "i1".into(),
                text: None
            }
            .is_waking(&Actor::Cli)
        );
        assert_eq!(
            serde_json::to_value(SessionMode::CodexRollout).unwrap(),
            json!("codex-rollout")
        );
        assert_eq!(
            serde_json::to_value(SessionMode::AgyConversation).unwrap(),
            json!("agy-conversation")
        );
    }
    #[test]
    fn all_kinds_round_trip_through_entry_json() {
        let cases = [
            ("ws.opened", json!({"title":"work"})),
            ("ws.closed", json!({"summary":"closed"})),
            ("ws.cleaned", json!({})),
            (
                "ws.retained",
                json!({"reason_code":"dirty","reason":"dirty files","checks":[]}),
            ),
            (
                "task.placed",
                json!({"rung":2,"reason":"reuse","rejected":[],"role":"coord-default","harness":"omp","title":"task","brief":"/brief"}),
            ),
            (
                "task.started",
                json!({"terminal":"t1","tab_name":"task","space":"sp1","cwd":"/tmp","created_ms":1,"resumed":false}),
            ),
            (
                "task.session",
                json!({"session":{"mode":"dir","id":"session","dir":"/sessions"}}),
            ),
            (
                "task.sent",
                json!({"message":"/message","via":"send","terminal":"t1"}),
            ),
            (
                "task.reported",
                json!({"outcome":"done","summary":"done","result":null}),
            ),
            ("task.progress", json!({"summary":"progress"})),
            ("task.note", json!({"text":"note"})),
            (
                "task.start_blocked",
                json!({"terminal":"t1","after_secs":10}),
            ),
            (
                "task.trust_confirmed",
                json!({"terminal":"t1","keys":["enter"],"start_blocked_seq":9}),
            ),
            ("task.attention", json!({"state":"blocked"})),
            ("task.stalled", json!({"silent_secs":20,"nudge_seq":4})),
            ("task.dormant", json!({"reason":"terminal_finished"})),
            ("task.closed", json!({"reason":"done"})),
            (
                "res.intent",
                json!({"type":"worktree","name":"/tree","repo":"/repo","branch":"topic","base":"main","create_branch":true}),
            ),
            (
                "res.created",
                json!({"type":"space","name":"sp1","folder":"/tree","label":"space"}),
            ),
            (
                "res.adopted",
                json!({"type":"tab","name":"t1","space":"sp1","tab":"tb1"}),
            ),
            (
                "res.release_checked",
                json!({"resource":"branch:topic","type":"branch","name":"topic","check":"merged","ok":true}),
            ),
            ("res.released", json!({"type":"branch","name":"topic"})),
            (
                "nudge.sent",
                json!({"target":"coordinator","text":"inbox","upto_seq":5}),
            ),
            (
                "reconcile.finding",
                json!({"code":"missing","summary":"missing resource"}),
            ),
            ("watcher.started", json!({"release":"dev","pid":42})),
            ("watcher.stopped", json!({"reason":"down"})),
            (
                "coord.session_started",
                json!({"role":"coord-main","terminal":"t1","space":"sp1","release":"dev","reason":"up","brief":"/brief"}),
            ),
            (
                "coord.session_ended",
                json!({"reason":"down","terminal":"t1"}),
            ),
            ("coord.handoff_requested", json!({"by":"user"})),
            ("inbox.drained", json!({"cursor":5,"count":2})),
            (
                "decision",
                json!({"what":"reuse","why":"safe","tier":"digest"}),
            ),
            ("question.asked", json!({"qid":"q1","text":"which?"})),
            ("question.answered", json!({"qid":"q1","text":"this"})),
            (
                "scout.result",
                json!({"question":"where?","summary":"here"}),
            ),
            ("instruction.set", json!({"id":"i1"})),
            ("thread.opened", json!({"id":"th1","title":"discussion"})),
            ("thread.closed", json!({"id":"th1","title":"discussion"})),
            (
                "correction",
                json!({"what":"placement","why":"wrong","ref":"ws-k3m2ab-t1"}),
            ),
        ];
        for (name, data) in cases {
            let original = json!({"v":1,"seq":1,"ts_ms":123,"actor":"cli","kind":name,"ws":null,"task":null,"data":data});
            let entry: Entry = serde_json::from_value(original.clone()).unwrap();
            let encoded = serde_json::to_string(&entry).unwrap();
            let decoded: Entry = serde_json::from_str(&encoded).unwrap();
            let kind = decoded.typed();
            assert_eq!(kind.name(), name);
            let mut expected = json!({"kind":name,"data":data});
            if name == "task.placed" {
                expected["data"]["brief_bytes"] = json!(0);
                expected["data"]["session"] = json!(null);
            }
            let canonical = serde_json::to_value(kind).unwrap();
            assert_eq!(canonical, expected);
            assert_eq!(
                serde_json::to_value(serde_json::from_value::<Kind>(canonical.clone()).unwrap())
                    .unwrap(),
                canonical
            );
            assert_eq!(serde_json::to_value(decoded).unwrap(), original);
        }
    }

    #[test]
    fn future_versions_and_unknown_kinds_preserve_entry_data() {
        let original = json!({"v":2,"seq":7,"ts_ms":8,"actor":"watcher","kind":"future.kind","ws":null,"task":null,"data":{"new_field":[1,2]}});
        let entry: Entry = serde_json::from_value(original.clone()).unwrap();
        assert!(matches!(entry.typed(), Kind::Unknown));
        let encoded = serde_json::to_string(&entry).unwrap();
        let decoded: Entry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), original);
    }
}

#[cfg(test)]
mod insight_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn new_and_old_entries_round_trip() {
        let old = json!({"v":1,"seq":1,"ts_ms":1,"actor":"cli","kind":"task.placed","ws":"ws-abcdef","task":"ws-abcdef-t1","data":{"rung":2,"reason":"x","rejected":[],"role":"x","harness":"omp","title":"x","brief":"/b"}});
        let entry: Entry = serde_json::from_value(old).unwrap();
        assert_eq!(entry.op, None);
        assert_eq!(entry.cause, None);
        assert!(matches!(
            entry.typed(),
            Kind::TaskPlaced {
                request: None,
                follows: None,
                brief_bytes: 0,
                ..
            }
        ));
        assert!(
            serde_json::to_value(entry.typed()).unwrap()["data"]
                .get("session")
                .unwrap()
                .is_null()
        );
        for value in [
            json!({"kind":"task.turn","data":{"turn":2,"origin":"person","working_ms":123}}),
            json!({"kind":"coord.turn","data":{"turn":3,"origin":"inbox"}}),
            json!({"kind":"task.closed","data":{"reason":"done","outcome":"accepted"}}),
            json!({"kind":"ws.closed","data":{"summary":"done","outcome":"delivered"}}),
            json!({"kind":"rating","data":{"target":"ws-abcdef","value":"good","note":"yes"}}),
            json!({"kind":"config.changed","data":{"key":"stall_secs","old":300,"new":600}}),
            json!({"kind":"task.placed","data":{"rung":1,"reason":"x","rejected":[],"role":"r","harness":"omp","title":"x","brief":"/b","request":"please","follows":{"task":"ws-abcdef-t1","relation":"follow_up"},"role_hash":"123456789abc","brief_bytes":99}}),
        ] {
            let kind: Kind = serde_json::from_value(value.clone()).unwrap();
            assert!(!matches!(kind, Kind::Unknown));
            let mut expected = value;
            if expected["kind"] == "task.placed" {
                assert!(matches!(
                    &kind,
                    Kind::TaskPlaced {
                        session: None,
                        follows: Some(_),
                        ..
                    }
                ));
                expected["data"]["session"] = json!(null);
            }
            let canonical = serde_json::to_value(kind).unwrap();
            assert_eq!(canonical, expected);
            assert_eq!(
                serde_json::to_value(serde_json::from_value::<Kind>(canonical.clone()).unwrap())
                    .unwrap(),
                canonical
            );
        }
    }
}
