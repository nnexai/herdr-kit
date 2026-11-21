use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub const PROVIDER_ORDER: [&str; 4] = ["codex", "claude", "copilot", "antigravity"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStateKind {
    Blocked,
    Error,
    Working,
    Done,
    Idle,
    Unknown,
}

impl AgentStateKind {
    pub fn from_word(word: Option<&str>) -> Self {
        match word {
            Some("blocked") => Self::Blocked,
            Some("error") => Self::Error,
            Some("working") => Self::Working,
            Some("done") => Self::Done,
            Some("idle") => Self::Idle,
            _ => Self::Unknown,
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Blocked => "WAIT",
            Self::Error => "ERR",
            Self::Working => "WORK",
            Self::Done => "DONE",
            Self::Idle => "IDLE",
            Self::Unknown => "LIVE",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentEntry {
    pub address: String,
    pub name: Option<String>,
    pub space: String,
    pub tab: String,
    pub where_label: String,
    pub agent: String,
    pub agent_short: String,
    pub status: AgentStateKind,
    pub status_msg: Option<String>,
    pub title: String,
    pub turn: u64,
    pub status_ms: u64,
    pub unseen_done: bool,
    pub is_coordinator: bool,
}

impl AgentEntry {
    fn sort_key(&self) -> (u8, std::cmp::Reverse<u64>, &str) {
        let rank = match self.status {
            AgentStateKind::Blocked => 0,
            AgentStateKind::Error => 1,
            AgentStateKind::Working => 2,
            AgentStateKind::Done if self.unseen_done => 3,
            AgentStateKind::Done => 4,
            AgentStateKind::Idle => 5,
            AgentStateKind::Unknown => 6,
        };
        (rank, std::cmp::Reverse(self.status_ms), &self.address)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Question,
    Blocked,
    Error,
    Done,
}

impl InboxKind {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Question => "QUESTION",
            Self::Blocked => "BLOCKED",
            Self::Error => "ERROR",
            Self::Done => "DONE",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub kind: InboxKind,
    pub title: String,
    pub text: String,
    pub source: String,
    pub terminal: Option<String>,
    pub qid: Option<String>,
    pub options: Vec<String>,
    pub since_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FleetCounts {
    pub total: usize,
    pub working: usize,
    pub blocked: usize,
    pub error: usize,
    pub unseen_done: usize,
    pub needs_you: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoordinatorSummary {
    pub state: String,
    pub enabled: bool,
    pub terminal: Option<String>,
    pub running_tasks: usize,
    pub needs_you: usize,
    pub retained: usize,
    pub open_workstreams: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BucketSummary {
    pub id: String,
    pub label: String,
    pub full_label: String,
    pub window: Option<String>,
    pub used_fraction: Option<f64>,
    pub unlimited: bool,
    pub stale: bool,
    pub resets_at_ms: Option<u64>,
    pub credits_text: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    pub short_name: String,
    pub state: String,
    pub error: Option<String>,
    pub stale: bool,
    pub worst_fraction: Option<f64>,
    pub worst_reset_ms: Option<u64>,
    pub unlimited: bool,
    pub credits_text: Option<String>,
    pub buckets: Vec<BucketSummary>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LimitsSummary {
    pub available: bool,
    pub collecting: bool,
    pub generated_at_ms: Option<u64>,
    pub worst_provider: Option<String>,
    pub providers: Vec<ProviderSummary>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeckSnapshot {
    pub schema_version: u32,
    pub generated_at_ms: u64,
    pub core_connected: bool,
    pub agents: Vec<AgentEntry>,
    pub counts: FleetCounts,
    pub coordinator: Option<CoordinatorSummary>,
    pub inbox: Vec<InboxItem>,
    pub limits: LimitsSummary,
}

impl DeckSnapshot {
    pub fn agent_for_slot(
        &self,
        slot: usize,
        filter: &str,
        pin_space: Option<&str>,
    ) -> Option<&AgentEntry> {
        let idx = slot.saturating_sub(1);
        let pin = pin_space.map(str::trim).filter(|s| !s.is_empty());
        self.agents
            .iter()
            .filter(|a| {
                if let Some(target) = pin {
                    if !a.space.eq_ignore_ascii_case(target) {
                        return false;
                    }
                }
                match filter {
                    "attention" => {
                        matches!(a.status, AgentStateKind::Blocked | AgentStateKind::Error)
                            || a.unseen_done
                    }
                    "working" => a.status == AgentStateKind::Working,
                    "done" => a.status == AgentStateKind::Done,
                    _ => true,
                }
            })
            .nth(idx)
    }

    pub fn inbox_item(&self, index: usize) -> Option<(&InboxItem, usize)> {
        if self.inbox.is_empty() {
            return None;
        }
        let actual = index % self.inbox.len();
        Some((&self.inbox[actual], actual))
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderSummary> {
        self.limits.providers.iter().find(|p| p.id == id)
    }

    pub fn resolve_provider(&self, setting: &str, selected: &str) -> Option<&ProviderSummary> {
        let target = match setting {
            "" | "worst" => self
                .limits
                .worst_provider
                .as_deref()
                .unwrap_or("antigravity"),
            "selected" => selected,
            other => other,
        };
        self.provider(target).or_else(|| {
            self.limits
                .worst_provider
                .as_deref()
                .and_then(|w| self.provider(w))
                .or_else(|| self.limits.providers.first())
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum StatusField {
    Obj(RawAgentStatus),
    Word(String),
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawAgentStatus {
    pub state: String,
    pub app: Option<String>,
    pub msg: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawForeground {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawTerm {
    pub address: String,
    pub state: String,
    #[serde(default)]
    pub status: Option<StatusField>,
    #[serde(default)]
    pub status_ms: Option<u64>,
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default, rename = "where")]
    pub where_label: Option<String>,
    #[serde(default)]
    pub foreground: Option<RawForeground>,
}

impl RawTerm {
    fn status_word(&self) -> Option<&str> {
        match self.status.as_ref()? {
            StatusField::Obj(obj) => Some(obj.state.as_str()),
            StatusField::Word(word) => Some(word.as_str()),
        }
    }

    fn status_app(&self) -> Option<&str> {
        match self.status.as_ref()? {
            StatusField::Obj(obj) => obj.app.as_deref(),
            StatusField::Word(_) => None,
        }
    }

    fn status_msg(&self) -> Option<&str> {
        match self.status.as_ref()? {
            StatusField::Obj(obj) => obj.msg.as_deref(),
            StatusField::Word(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventEffect {
    Ignored,
    Updated {
        need_get: Option<String>,
        wake_coord: bool,
    },
}

#[derive(Default)]
pub struct HcState {
    pub connected: bool,
    pub terms: BTreeMap<String, RawTerm>,
}

impl HcState {
    pub fn clear_disconnected(&mut self) {
        self.connected = false;
    }

    pub fn replace_snapshot(&mut self, terms: Vec<RawTerm>) {
        self.connected = true;
        self.terms.clear();
        for term in terms {
            self.terms.insert(term.address.clone(), term);
        }
    }

    pub fn upsert_term(&mut self, term: RawTerm) {
        self.connected = true;
        self.terms.insert(term.address.clone(), term);
    }

    pub fn apply_event_line(&mut self, line: &str) -> EventEffect {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return EventEffect::Ignored;
        };
        if let Some(terms_val) = value.get("terminals") {
            if let Ok(terms) = serde_json::from_value::<Vec<RawTerm>>(terms_val.clone()) {
                self.replace_snapshot(terms);
                return EventEffect::Updated {
                    need_get: None,
                    wake_coord: true,
                };
            }
            return EventEffect::Ignored;
        }
        let Some(terminal) = value.pointer("/params/terminal").and_then(Value::as_str) else {
            return EventEffect::Ignored;
        };
        let Some(kind) = value.pointer("/params/type").and_then(Value::as_str) else {
            return EventEffect::Ignored;
        };
        self.connected = true;
        let data = value.pointer("/params/data");
        match kind {
            "removed" | "remove" => {
                self.terms.remove(terminal);
                EventEffect::Updated {
                    need_get: None,
                    wake_coord: true,
                }
            }
            "title" => {
                if let (Some(term), Some(title)) = (
                    self.terms.get_mut(terminal),
                    data.and_then(|d| d.get("title")).and_then(Value::as_str),
                ) {
                    term.title = Some(title.to_owned());
                    EventEffect::Updated {
                        need_get: None,
                        wake_coord: false,
                    }
                } else {
                    EventEffect::Ignored
                }
            }
            "status" => {
                let mut wake_coord = false;
                if let Some(term) = self.terms.get_mut(terminal) {
                    if term.labels.get("coord.managed").map(String::as_str) == Some("1") {
                        wake_coord = true;
                    }
                    if let Some(status_val) = data.and_then(|d| d.get("status").or(Some(d))) {
                        if let Ok(parsed) =
                            serde_json::from_value::<StatusField>(status_val.clone())
                        {
                            let word = match &parsed {
                                StatusField::Obj(o) => o.state.as_str(),
                                StatusField::Word(w) => w.as_str(),
                            };
                            if matches!(word, "blocked" | "error" | "done") {
                                wake_coord = true;
                            }
                            term.status = Some(parsed);
                        }
                    }
                    if let Some(ms) = data
                        .and_then(|d| d.get("status_ms"))
                        .and_then(Value::as_u64)
                    {
                        term.status_ms = Some(ms);
                    }
                    if let Some(turn) = data.and_then(|d| d.get("turn")).and_then(Value::as_u64) {
                        term.turn = turn;
                    }
                    EventEffect::Updated {
                        need_get: Some(terminal.to_owned()),
                        wake_coord,
                    }
                } else {
                    EventEffect::Updated {
                        need_get: Some(terminal.to_owned()),
                        wake_coord: true,
                    }
                }
            }
            "turn" => {
                if let (Some(term), Some(turn)) = (
                    self.terms.get_mut(terminal),
                    data.and_then(|d| d.get("turn")).and_then(Value::as_u64),
                ) {
                    term.turn = turn;
                    EventEffect::Updated {
                        need_get: None,
                        wake_coord: false,
                    }
                } else {
                    EventEffect::Updated {
                        need_get: Some(terminal.to_owned()),
                        wake_coord: false,
                    }
                }
            }
            "label" | "labels" => {
                if let Some(term) = self.terms.get_mut(terminal) {
                    if let Some(map) = data.and_then(|d| d.as_object()) {
                        for (k, v) in map {
                            if let Some(s) = v.as_str() {
                                term.labels.insert(k.clone(), s.to_owned());
                            }
                        }
                    }
                }
                EventEffect::Updated {
                    need_get: Some(terminal.to_owned()),
                    wake_coord: true,
                }
            }
            "exited" | "finished" => {
                if let Some(term) = self.terms.get_mut(terminal) {
                    term.state = "finished".into();
                    term.status = None;
                }
                EventEffect::Updated {
                    need_get: None,
                    wake_coord: true,
                }
            }
            _ => EventEffect::Updated {
                need_get: Some(terminal.to_owned()),
                wake_coord: kind == "created" || kind == "create",
            },
        }
    }

    pub fn agents(&self) -> Vec<AgentEntry> {
        let mut list: Vec<AgentEntry> = self
            .terms
            .values()
            .filter(|t| t.state == "live")
            .filter(|t| {
                let role = t.labels.get("coord.role").map(String::as_str);
                role != Some("watcher") && t.name.as_deref() != Some("coord-watcher")
            })
            .filter(|t| t.status.is_some())
            .map(|t| {
                let where_raw = t
                    .labels
                    .get("where")
                    .map(String::as_str)
                    .or(t.where_label.as_deref())
                    .unwrap_or("");
                let (space, tab) = split_where(where_raw, &t.cwd);
                let agent = t
                    .status_app()
                    .or(t.agent.as_deref())
                    .or_else(|| t.foreground.as_ref().map(|f| f.name.as_str()))
                    .unwrap_or("agent")
                    .to_owned();
                let status = AgentStateKind::from_word(t.status_word());
                let seen_turn = t
                    .labels
                    .get("herdr.seen")
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                let unseen_done = status == AgentStateKind::Done && seen_turn < t.turn.max(1);
                let is_coordinator =
                    t.labels.get("coord.role").map(String::as_str) == Some("coordinator");
                AgentEntry {
                    address: t.address.clone(),
                    name: t.name.clone(),
                    space,
                    tab,
                    where_label: where_raw.to_owned(),
                    agent_short: short_agent(&agent),
                    agent,
                    status,
                    status_msg: t.status_msg().map(str::to_owned),
                    title: t.title.clone().unwrap_or_default(),
                    turn: t.turn,
                    status_ms: t.status_ms.unwrap_or(0),
                    unseen_done,
                    is_coordinator,
                }
            })
            .collect();
        list.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        list
    }
}

fn split_where(where_label: &str, cwd: &str) -> (String, String) {
    if let Some((space, tab)) = where_label.split_once(" / ") {
        return (space.trim().to_owned(), tab.trim().to_owned());
    }
    let space = std::path::Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("workspace")
        .to_owned();
    (space, "1".into())
}

fn short_agent(agent: &str) -> String {
    match agent.to_ascii_lowercase().as_str() {
        "antigravity" | "agy" => "AGY".into(),
        "omp" => "OMP".into(),
        "claude" | "claude-code" => "CLD".into(),
        "codex" => "CDX".into(),
        "copilot" => "CPL".into(),
        other => other.chars().take(3).collect::<String>().to_uppercase(),
    }
}

#[derive(Deserialize)]
struct CoordStatusJson {
    state: String,
    enabled: bool,
    coordinator: CoordProcJson,
    counts: CoordCountsJson,
}

#[derive(Deserialize)]
struct CoordProcJson {
    terminal: Option<String>,
}

#[derive(Deserialize)]
struct CoordCountsJson {
    running: usize,
    needs_you: usize,
    retained: usize,
    open_workstreams: usize,
}

#[derive(Deserialize)]
struct CoordBoardJson {
    #[serde(default)]
    attention: Vec<CoordAttentionJson>,
    #[serde(default)]
    open_questions: Vec<CoordQuestionJson>,
    #[serde(default)]
    workstreams: Vec<CoordWorkstreamJson>,
}

#[derive(Deserialize)]
struct CoordAttentionJson {
    id: String,
    kind: String,
    ws: Option<String>,
    task: Option<String>,
    since_ms: u64,
    text: String,
    #[serde(default)]
    options: Option<Vec<String>>,
    qid: Option<String>,
}

#[derive(Deserialize)]
struct CoordQuestionJson {
    qid: String,
    text: String,
    #[serde(default)]
    options: Option<Vec<String>>,
    ws: Option<String>,
    task: Option<String>,
    since_ms: u64,
}

#[derive(Deserialize)]
struct CoordWorkstreamJson {
    id: String,
    title: String,
    space: Option<CoordSpaceJson>,
    repo: Option<CoordRepoJson>,
    #[serde(default)]
    tasks: Vec<CoordTaskJson>,
}

#[derive(Deserialize)]
struct CoordSpaceJson {
    name: String,
}

#[derive(Deserialize)]
struct CoordRepoJson {
    name: String,
}

#[derive(Deserialize)]
struct CoordTaskJson {
    id: String,
    title: String,
    terminal: Option<String>,
    harness: Option<String>,
}

#[derive(Deserialize)]
struct UsageSnapshotJson {
    generated_at_ms: u64,
    collecting: bool,
    providers: Vec<UsageProviderJson>,
}

#[derive(Deserialize)]
struct UsageProviderJson {
    provider: String,
    state: String,
    error: Option<String>,
    stale: bool,
    #[serde(default)]
    accounts: Vec<UsageAccountJson>,
}

#[derive(Deserialize)]
struct UsageAccountJson {
    limits: Vec<UsageLimitJson>,
}

#[derive(Deserialize)]
struct UsageLimitJson {
    id: String,
    window: Option<String>,
    tier: Option<String>,
    unit: String,
    used_fraction: Option<f64>,
    used: Option<f64>,
    limit: Option<f64>,
    remaining: Option<f64>,
    unlimited: bool,
    resets_at_ms: Option<u64>,
}

pub fn parse_coordinator_status(bytes: &[u8]) -> Option<CoordinatorSummary> {
    let parsed: CoordStatusJson = serde_json::from_slice(bytes).ok()?;
    Some(CoordinatorSummary {
        state: parsed.state,
        enabled: parsed.enabled,
        terminal: parsed.coordinator.terminal,
        running_tasks: parsed.counts.running,
        needs_you: parsed.counts.needs_you,
        retained: parsed.counts.retained,
        open_workstreams: parsed.counts.open_workstreams,
    })
}

pub fn parse_limits(bytes: &[u8]) -> Option<LimitsSummary> {
    let parsed: UsageSnapshotJson = serde_json::from_slice(bytes).ok()?;
    let mut providers = Vec::with_capacity(PROVIDER_ORDER.len());
    for id in PROVIDER_ORDER {
        let Some(p) = parsed.providers.iter().find(|p| p.provider == id) else {
            continue;
        };
        let mut buckets = Vec::new();
        for account in &p.accounts {
            for limit in &account.limits {
                let used_fraction = compute_used_fraction(limit);
                let credits_text = format_credits_short(limit);
                let (label, full_label) = format_bucket_labels(id, limit);
                buckets.push(BucketSummary {
                    id: limit.id.clone(),
                    label,
                    full_label,
                    window: limit.window.clone(),
                    used_fraction,
                    unlimited: limit.unlimited,
                    stale: p.stale,
                    resets_at_ms: limit.resets_at_ms,
                    credits_text,
                });
            }
        }
        let worst = buckets
            .iter()
            .filter(|b| !b.unlimited && b.used_fraction.is_some())
            .max_by(|a, b| {
                a.used_fraction
                    .unwrap_or(-1.0)
                    .total_cmp(&b.used_fraction.unwrap_or(-1.0))
                    .then_with(|| {
                        b.resets_at_ms
                            .unwrap_or(u64::MAX)
                            .cmp(&a.resets_at_ms.unwrap_or(u64::MAX))
                    })
            });
        let unlimited = buckets.iter().any(|b| b.unlimited)
            && buckets.iter().all(|b| b.used_fraction.is_none());
        let credits_text = worst
            .and_then(|b| b.credits_text.clone())
            .or_else(|| buckets.iter().find_map(|b| b.credits_text.clone()));
        let (name, short_name) = provider_names(id);
        providers.push(ProviderSummary {
            id: id.to_owned(),
            name: name.to_owned(),
            short_name: short_name.to_owned(),
            state: p.state.clone(),
            error: p.error.clone(),
            stale: p.stale,
            worst_fraction: worst.and_then(|b| b.used_fraction),
            worst_reset_ms: worst
                .and_then(|b| b.resets_at_ms)
                .or_else(|| buckets.iter().filter_map(|b| b.resets_at_ms).min()),
            unlimited,
            credits_text,
            buckets,
        });
    }
    let visible: Vec<&ProviderSummary> = providers
        .iter()
        .filter(|p| p.state != "not_signed_in")
        .collect();
    let fresh: Vec<&&ProviderSummary> = visible
        .iter()
        .filter(|p| !p.stale && p.worst_fraction.is_some())
        .collect();
    let worst_provider = if !fresh.is_empty() {
        fresh
            .into_iter()
            .max_by(|a, b| {
                a.worst_fraction
                    .unwrap_or(-1.0)
                    .total_cmp(&b.worst_fraction.unwrap_or(-1.0))
            })
            .map(|p| p.id.clone())
    } else {
        visible
            .into_iter()
            .max_by(|a, b| {
                a.worst_fraction
                    .unwrap_or(-1.0)
                    .total_cmp(&b.worst_fraction.unwrap_or(-1.0))
            })
            .map(|p| p.id.clone())
    };
    Some(LimitsSummary {
        available: true,
        collecting: parsed.collecting,
        generated_at_ms: Some(parsed.generated_at_ms),
        worst_provider,
        providers,
    })
}

fn provider_names(id: &str) -> (&'static str, &'static str) {
    match id {
        "codex" => ("Codex", "CODEX"),
        "claude" => ("Claude", "CLAUDE"),
        "copilot" => ("Copilot", "COPILOT"),
        "antigravity" => ("Antigravity", "ANTIGRAV"),
        _ => ("Limits", "LIMITS"),
    }
}

fn compute_used_fraction(l: &UsageLimitJson) -> Option<f64> {
    if l.unlimited {
        return None;
    }
    if let Some(f) = l.used_fraction.filter(|v| v.is_finite()) {
        return Some(f.clamp(0.0, 1.0));
    }
    if l.unit == "credits" {
        if let Some(limit) = l.limit.filter(|v| v.is_finite() && *v > 0.0) {
            if let Some(rem) = l.remaining.filter(|v| v.is_finite()) {
                return Some(((limit - rem) / limit).clamp(0.0, 1.0));
            }
            if let Some(used) = l.used.filter(|v| v.is_finite()) {
                return Some((used / limit).clamp(0.0, 1.0));
            }
        }
    }
    None
}

fn format_compact_num(n: f64) -> String {
    if n >= 10_000.0 {
        format!("{:.0}k", n / 1000.0)
    } else if n >= 1000.0 {
        let tenths = (n / 100.0).round() / 10.0;
        if (tenths - tenths.round()).abs() < 0.05 {
            format!("{:.0}k", tenths)
        } else {
            format!("{tenths:.1}k")
        }
    } else {
        format!("{:.0}", n.round())
    }
}

fn format_credits_short(l: &UsageLimitJson) -> Option<String> {
    if l.unit != "credits" || l.unlimited {
        return None;
    }
    let used = l.used.or_else(|| {
        l.limit
            .zip(l.remaining)
            .map(|(lim, rem)| (lim - rem).max(0.0))
    })?;
    Some(match l.limit {
        Some(limit) => format!("{}/{}", format_compact_num(used), format_compact_num(limit)),
        None => format!("{} AIC", format_compact_num(used)),
    })
}

fn format_bucket_labels(provider: &str, l: &UsageLimitJson) -> (String, String) {
    let win_short = match l.window.as_deref() {
        Some("weekly") => "WK".to_owned(),
        Some("monthly") => "MO".to_owned(),
        Some(w) => w.to_uppercase(),
        None => String::new(),
    };
    let win_full = match l.window.as_deref() {
        Some("weekly") => "Weekly".to_owned(),
        Some("monthly") => "Monthly".to_owned(),
        Some(w) => w.to_owned(),
        None => String::new(),
    };
    match (provider, l.tier.as_deref()) {
        (_, Some("gemini_argon")) => (
            "ARGON".into(),
            format!("Gemini Argon {win_full}").trim().into(),
        ),
        (_, Some("gemini_pro")) => (
            format!("PRO {win_short}").trim().into(),
            format!("Gemini Pro {win_full}").trim().into(),
        ),
        (_, Some("gemini_flash")) => (
            format!("FLSH {win_short}").trim().into(),
            format!("Gemini Flash {win_full}").trim().into(),
        ),
        (_, Some("gemini")) => (
            format!("GEM {win_short}").trim().into(),
            format!("Gemini {win_full}").trim().into(),
        ),
        (_, Some("claude_gpt")) => (
            format!("3P {win_short}").trim().into(),
            format!("Claude & GPT {win_full}").trim().into(),
        ),
        (_, Some(tier)) => {
            let t_short = tier.chars().take(4).collect::<String>().to_uppercase();
            let mut chars = tier.chars();
            let t_full = match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            };
            (
                format!("{t_short} {win_short}").trim().into(),
                format!("{t_full} {win_full}").trim().into(),
            )
        }
        ("copilot", None) => (
            if win_short.is_empty() {
                "AIC".into()
            } else {
                format!("AIC {win_short}")
            },
            if win_full.is_empty() {
                "Premium Credits".into()
            } else {
                format!("Premium {win_full}")
            },
        ),
        (_, None) => (
            if win_short.is_empty() {
                "LIMIT".into()
            } else {
                win_short
            },
            if win_full.is_empty() {
                "Limit".into()
            } else {
                win_full
            },
        ),
    }
}

pub fn build_snapshot(
    now_ms: u64,
    hc: &HcState,
    coordinator: Option<&CoordinatorSummary>,
    coord_board_bytes: Option<&[u8]>,
    limits: Option<LimitsSummary>,
) -> DeckSnapshot {
    let agents = hc.agents();
    let board: Option<CoordBoardJson> =
        coord_board_bytes.and_then(|b| serde_json::from_slice(b).ok());

    let mut inbox = Vec::new();
    let mut covered_qids = HashSet::new();
    let mut covered_tasks = HashSet::new();
    let mut covered_terminals = HashSet::new();

    if let Some(board) = &board {
        fn find_ws<'a>(
            board: &'a CoordBoardJson,
            ws_id: Option<&str>,
        ) -> Option<&'a CoordWorkstreamJson> {
            ws_id.and_then(|id| board.workstreams.iter().find(|w| w.id == id))
        }
        fn find_task<'a>(
            board: &'a CoordBoardJson,
            ws: Option<&'a CoordWorkstreamJson>,
            task_id: Option<&str>,
        ) -> Option<&'a CoordTaskJson> {
            task_id.and_then(|id| {
                ws.into_iter()
                    .flat_map(|w| w.tasks.iter())
                    .chain(board.workstreams.iter().flat_map(|w| w.tasks.iter()))
                    .find(|t| t.id == id)
            })
        }

        for q in &board.open_questions {
            let ws = find_ws(board, q.ws.as_deref());
            let task = find_task(board, ws, q.task.as_deref());
            let terminal = task.and_then(|t| t.terminal.clone());
            if let Some(term) = &terminal {
                covered_terminals.insert(term.clone());
            }
            if let Some(tid) = &q.task {
                covered_tasks.insert(tid.clone());
            }
            covered_qids.insert(q.qid.clone());
            let title = ws
                .map(|w| w.title.clone())
                .or_else(|| task.map(|t| t.title.clone()))
                .unwrap_or_else(|| "Coordinator".into());
            let source = ws
                .and_then(|w| {
                    w.space
                        .as_ref()
                        .map(|s| s.name.clone())
                        .or_else(|| w.repo.as_ref().map(|r| r.name.clone()))
                })
                .unwrap_or_else(|| "coord".into());
            inbox.push(InboxItem {
                id: format!("q:{}", q.qid),
                kind: InboxKind::Question,
                title,
                text: q.text.clone(),
                source,
                terminal,
                qid: Some(q.qid.clone()),
                options: q.options.clone().unwrap_or_default(),
                since_ms: q.since_ms,
            });
        }

        for a in &board.attention {
            if a.kind == "question" {
                continue;
            }
            if a.qid.as_ref().is_some_and(|id| covered_qids.contains(id)) {
                continue;
            }
            if a.kind == "task_question"
                && a.task
                    .as_ref()
                    .is_some_and(|tid| covered_tasks.contains(tid))
            {
                continue;
            }
            let ws = find_ws(board, a.ws.as_deref());
            let task = find_task(board, ws, a.task.as_deref());
            let terminal = task.and_then(|t| t.terminal.clone()).or_else(|| {
                if a.kind == "coordinator_blocked" {
                    coordinator.and_then(|c| c.terminal.clone())
                } else {
                    None
                }
            });
            if let Some(term) = &terminal {
                covered_terminals.insert(term.clone());
            }
            let title = ws
                .map(|w| w.title.clone())
                .or_else(|| task.map(|t| t.title.clone()))
                .unwrap_or_else(|| "Coordinator".into());
            let source = task
                .and_then(|t| t.harness.clone())
                .or_else(|| ws.and_then(|w| w.repo.as_ref().map(|r| r.name.clone())))
                .unwrap_or_else(|| "coord".into());
            inbox.push(InboxItem {
                id: format!("att:{}", a.id),
                kind: if a.kind.contains("question") {
                    InboxKind::Question
                } else {
                    InboxKind::Blocked
                },
                title,
                text: a.text.clone(),
                source,
                terminal,
                qid: a.qid.clone(),
                options: a.options.clone().unwrap_or_default(),
                since_ms: a.since_ms,
            });
        }
    }

    for agent in &agents {
        if matches!(
            agent.status,
            AgentStateKind::Blocked | AgentStateKind::Error
        ) && !covered_terminals.contains(&agent.address)
        {
            covered_terminals.insert(agent.address.clone());
            let kind = if agent.status == AgentStateKind::Blocked {
                InboxKind::Blocked
            } else {
                InboxKind::Error
            };
            let text = agent
                .status_msg
                .clone()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| (!agent.title.trim().is_empty()).then(|| agent.title.clone()))
                .unwrap_or_else(|| {
                    format!("{} {}", agent.space, kind.short_label().to_lowercase())
                });
            inbox.push(InboxItem {
                id: format!("agent:{}", agent.address),
                kind,
                title: agent.space.clone(),
                text,
                source: format!("{} · {}", agent.agent_short, agent.address),
                terminal: Some(agent.address.clone()),
                qid: None,
                options: Vec::new(),
                since_ms: agent.status_ms,
            });
        }
    }

    for agent in &agents {
        if agent.unseen_done && !covered_terminals.contains(&agent.address) {
            let text = (!agent.title.trim().is_empty())
                .then(|| agent.title.clone())
                .unwrap_or_else(|| format!("{} finished turn {}", agent.space, agent.turn));
            inbox.push(InboxItem {
                id: format!("done:{}", agent.address),
                kind: InboxKind::Done,
                title: agent.space.clone(),
                text,
                source: format!("{} · {}", agent.agent_short, agent.address),
                terminal: Some(agent.address.clone()),
                qid: None,
                options: Vec::new(),
                since_ms: agent.status_ms,
            });
        }
    }

    inbox.sort_by(|a, b| (a.kind, a.since_ms, &a.id).cmp(&(b.kind, b.since_ms, &b.id)));

    let counts = FleetCounts {
        total: agents.len(),
        working: agents
            .iter()
            .filter(|a| a.status == AgentStateKind::Working)
            .count(),
        blocked: agents
            .iter()
            .filter(|a| a.status == AgentStateKind::Blocked)
            .count(),
        error: agents
            .iter()
            .filter(|a| a.status == AgentStateKind::Error)
            .count(),
        unseen_done: agents.iter().filter(|a| a.unseen_done).count(),
        needs_you: inbox.iter().filter(|i| i.kind != InboxKind::Done).count(),
    };

    DeckSnapshot {
        schema_version: 1,
        generated_at_ms: now_ms,
        core_connected: hc.connected,
        agents,
        counts,
        coordinator: coordinator.cloned(),
        inbox,
        limits: limits.unwrap_or_default(),
    }
}

pub fn format_span_short(ms: u64) -> String {
    let mins = ms.div_ceil(60_000).max(1);
    if mins >= 1440 {
        let days = mins / 1440;
        let hours = (mins % 1440) / 60;
        if hours > 0 {
            format!("{days}d{hours}h")
        } else {
            format!("{days}d")
        }
    } else if mins >= 60 {
        let hours = mins / 60;
        let rem = mins % 60;
        if rem > 0 {
            format!("{hours}h{rem}m")
        } else {
            format!("{hours}h")
        }
    } else {
        format!("{mins}m")
    }
}

pub fn format_reset_short(resets_at_ms: Option<u64>, now_ms: u64) -> String {
    let Some(reset) = resets_at_ms else {
        return String::new();
    };
    if reset <= now_ms {
        return "RESET".into();
    }
    format_span_short(reset - now_ms)
}

pub fn format_percent(fraction: Option<f64>, unlimited: bool) -> String {
    if unlimited {
        return "UNLIM".into();
    }
    match fraction {
        Some(f) => format!("{:.0}%", (f.clamp(0.0, 1.0) * 100.0).round()),
        None => "—".into(),
    }
}

pub fn fixture_snapshot(now_ms: u64) -> DeckSnapshot {
    let mut hc = HcState::default();
    hc.replace_snapshot(vec![
        RawTerm {
            address: "t12".into(),
            state: "live".into(),
            status: Some(StatusField::Obj(RawAgentStatus {
                state: "blocked".into(),
                app: Some("omp".into()),
                msg: Some("Confirm schema migration before applying".into()),
            })),
            status_ms: Some(now_ms.saturating_sub(120_000)),
            turn: 4,
            labels: BTreeMap::from([
                ("where".into(), "api-service / 1".into()),
                ("herdr.seen".into(), "3".into()),
            ]),
            cwd: "/home/user/projects/api-service".into(),
            name: None,
            title: Some("Review schema migration".into()),
            agent: None,
            where_label: None,
            foreground: None,
        },
        RawTerm {
            address: "t60".into(),
            state: "live".into(),
            status: Some(StatusField::Obj(RawAgentStatus {
                state: "working".into(),
                app: Some("antigravity".into()),
                msg: None,
            })),
            status_ms: Some(now_ms.saturating_sub(45_000)),
            turn: 2,
            labels: BTreeMap::from([
                ("where".into(), "herdr-kit / Antigravity".into()),
                ("herdr.seen".into(), "2".into()),
            ]),
            cwd: "/home/user/projects/herdr-kit".into(),
            name: None,
            title: Some("Build kit-deck OpenDeck extension".into()),
            agent: None,
            where_label: None,
            foreground: None,
        },
        RawTerm {
            address: "t48".into(),
            state: "live".into(),
            status: Some(StatusField::Obj(RawAgentStatus {
                state: "done".into(),
                app: Some("omp".into()),
                msg: None,
            })),
            status_ms: Some(now_ms.saturating_sub(180_000)),
            turn: 8,
            labels: BTreeMap::from([
                ("where".into(), "web-app / 1".into()),
                ("herdr.seen".into(), "7".into()),
            ]),
            cwd: "/home/user/projects/web-app".into(),
            name: None,
            title: Some("Verify UI accessibility tree".into()),
            agent: None,
            where_label: None,
            foreground: None,
        },
        RawTerm {
            address: "t5h".into(),
            state: "live".into(),
            status: Some(StatusField::Obj(RawAgentStatus {
                state: "idle".into(),
                app: Some("omp".into()),
                msg: None,
            })),
            status_ms: Some(now_ms.saturating_sub(900_000)),
            turn: 0,
            labels: BTreeMap::from([("where".into(), "sample-app / 1".into())]),
            cwd: "/home/user/projects/sample-app".into(),
            name: None,
            title: Some("Update release notes".into()),
            agent: None,
            where_label: None,
            foreground: None,
        },
    ]);
    let coord = CoordinatorSummary {
        state: "running".into(),
        enabled: true,
        terminal: Some("t4y".into()),
        running_tasks: 1,
        needs_you: 1,
        retained: 0,
        open_workstreams: 2,
    };
    let board = serde_json::json!({
        "open_questions": [{
            "qid": "q-1",
            "text": "Run release build on host or in isolated worktree?",
            "options": ["Worktree", "Host"],
            "ws": "ws-demo",
            "task": "ws-demo-t1",
            "since_ms": now_ms.saturating_sub(60_000)
        }],
        "attention": [],
        "workstreams": [{
            "id": "ws-demo",
            "title": "Stream Deck plugin",
            "space": { "name": "herdr-kit" },
            "tasks": [{ "id": "ws-demo-t1", "title": "Build kit-deck", "terminal": "t12", "harness": "omp" }]
        }]
    });
    let usage = serde_json::json!({
        "schema_version": 1,
        "generated_at_ms": now_ms,
        "collecting": false,
        "providers": [
            {
                "provider": "codex",
                "state": "available",
                "error": null,
                "stale": false,
                "accounts": [{ "fetched_at_ms": now_ms, "limits": [{ "id": "codex:7d:0", "window": "7d", "tier": null, "unit": "percent", "used_fraction": 0.12, "used": null, "limit": null, "remaining": null, "unlimited": false, "level": "ok", "resets_at_ms": now_ms + 360_000_000 }] }]
            },
            {
                "provider": "claude",
                "state": "available",
                "error": null,
                "stale": false,
                "accounts": [{ "fetched_at_ms": now_ms, "limits": [
                    { "id": "claude:5h:0", "window": "5h", "tier": null, "unit": "percent", "used_fraction": 0.24, "used": null, "limit": null, "remaining": null, "unlimited": false, "level": "ok", "resets_at_ms": now_ms + 11_000_000 },
                    { "id": "claude:7d:1", "window": "7d", "tier": null, "unit": "percent", "used_fraction": 0.35, "used": null, "limit": null, "remaining": null, "unlimited": false, "level": "ok", "resets_at_ms": now_ms + 240_000_000 }
                ] }]
            },
            {
                "provider": "copilot",
                "state": "available",
                "error": null,
                "stale": false,
                "accounts": [{ "fetched_at_ms": now_ms, "limits": [{ "id": "copilot:monthly:0", "window": "monthly", "tier": null, "unit": "credits", "used_fraction": 0.0227, "used": 34.0, "limit": 1500.0, "remaining": 1466.0, "unlimited": false, "level": "ok", "resets_at_ms": now_ms + 780_000_000 }] }]
            },
            {
                "provider": "antigravity",
                "state": "available",
                "error": null,
                "stale": false,
                "accounts": [{ "fetched_at_ms": now_ms, "limits": [
                    { "id": "antigravity:weekly:0", "window": "weekly", "tier": "gemini", "unit": "percent", "used_fraction": 0.18, "used": null, "limit": null, "remaining": null, "unlimited": false, "level": "ok", "resets_at_ms": now_ms + 590_000_000 },
                    { "id": "antigravity:5h:1", "window": "5h", "tier": "gemini", "unit": "percent", "used_fraction": 0.58, "used": null, "limit": null, "remaining": null, "unlimited": false, "level": "ok", "resets_at_ms": now_ms + 13_380_000 },
                    { "id": "antigravity:weekly:2", "window": "weekly", "tier": "claude_gpt", "unit": "percent", "used_fraction": 0.0, "used": null, "limit": null, "remaining": null, "unlimited": false, "level": "ok", "resets_at_ms": now_ms + 600_000_000 },
                    { "id": "antigravity:5h:3", "window": "5h", "tier": "claude_gpt", "unit": "percent", "used_fraction": 0.0, "used": null, "limit": null, "remaining": null, "unlimited": false, "level": "ok", "resets_at_ms": now_ms + 18_000_000 },
                    { "id": "antigravity:weekly:4", "window": "weekly", "tier": "gemini_argon", "unit": "percent", "used_fraction": 0.84, "used": null, "limit": null, "remaining": null, "unlimited": false, "level": "warning", "resets_at_ms": now_ms + 580_000_000 }
                ] }]
            }
        ]
    });
    let limits = parse_limits(&serde_json::to_vec(&usage).unwrap());
    build_snapshot(
        now_ms,
        &hc,
        Some(&coord),
        Some(&serde_json::to_vec(&board).unwrap()),
        limits,
    )
}
