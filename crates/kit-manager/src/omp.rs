//! `herdr-kit omp`: find and restart eligible live personal OMP sessions in place,
//! including current images when explicitly requested.
//!
//! The restart itself is OMP's own `/restart`, submitted from inside the session by the
//! herdr-kit OMP plugin (`omp-plugin/`). The plugin alone can see whether the composer is
//! empty and focused and whether anything is running or queued, so it makes the final
//! decision in one synchronous step; this side chooses the sessions, asks each exact
//! process incarnation, and proves the result afterwards. Nothing is typed into terminals.

mod system;
#[cfg(test)]
mod tests;

pub use system::{System, target};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The plugin protocol this side speaks (`omp-plugin/extension.ts`).
pub const PROTOCOL: u32 = 1;
/// A request the plugin has not acted on by then is refused by the plugin itself.
const REQUEST_TTL_MS: u64 = 10_000;
const REPLY_GRACE_MS: u64 = 1_000;
/// From the plugin's go-ahead until the restarted image announces itself.
const RESTART_TIMEOUT_MS: u64 = 60_000;
/// Quiet time after the new image appears, long enough to see `--restore` start work.
const SETTLE_MS: u64 = 3_000;
const POLL_MS: u64 = 200;

/// `omp --version` of the releases whose `--restore` was checked to go on with nothing for
/// a session the plugin admits. `/restart` always relaunches with `--restore`, and the new
/// image runs it, so the plugin's guard is only a proof against these releases' rules
/// (`hasRestorableTurn`, `restoreSubagents`). Any other target gets the census only.
pub const RESTORE_VERIFIED: &[&str] = &["omp/19.1.1"];

fn restore_verified(target: &Target) -> bool {
    RESTORE_VERIFIED.contains(&target.version.as_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileId {
    pub dev: u64,
    pub ino: u64,
}

/// The executable sessions should end up running.
#[derive(Clone, Debug, Serialize)]
pub struct Target {
    pub path: PathBuf,
    pub resolved: PathBuf,
    #[serde(skip)]
    pub id: FileId,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Terminal {
    pub address: String,
    #[serde(default)]
    pub name: Option<String>,
    pub state: String,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub reported: bool,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub foreground: Option<Foreground>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Status {
    pub state: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Foreground {
    pub pid: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub argv: Vec<String>,
}

/// The image a live process runs, read from the kernel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub path: String,
    pub id: FileId,
    pub deleted: bool,
    /// Process start time; exec keeps it, a new process with a reused pid does not.
    pub start_time: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launcher {
    pub path: String,
    pub dev: u64,
    pub ino: u64,
}

/// What a session's plugin publishes about itself (`<pid>.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub protocol: u32,
    pub pid: i32,
    pub start_time: Option<String>,
    pub instance: String,
    pub session_id: Option<String>,
    pub session_file: Option<String>,
    pub session_saved: bool,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub idle: bool,
    pub queued: bool,
    pub running_jobs: u64,
    pub unprompted_runs: u64,
    pub composer_empty: bool,
    pub launcher: Option<Launcher>,
    /// What `--restore` would go on with, as of the session's last settled moment.
    #[serde(default)]
    pub replay_risk: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub protocol: u32,
    pub nonce: String,
    pub pid: i32,
    pub instance: String,
    pub session_file: String,
    pub target: FileId,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Reply {
    pub nonce: Option<String>,
    pub outcome: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub before: Option<Presence>,
}

/// Everything `herdr-kit omp` observes or changes, so the sequencing is testable.
pub trait Host {
    fn terminals(&self) -> Result<Vec<Terminal>>;
    fn terminal(&self, address: &str) -> Result<Option<Terminal>>;
    fn image(&self, pid: i32) -> Option<Image>;
    fn presence(&self, pid: i32) -> Option<Presence>;
    fn send(&self, request: &Request) -> Result<()>;
    fn reply(&self, pid: i32) -> Option<Reply>;
    /// Remove an unanswered request; it has expired by then anyway.
    fn withdraw(&self, pid: i32);
    fn file_id(&self, path: &Path) -> Option<FileId>;
    fn now_ms(&self) -> u64;
    fn sleep_ms(&self, ms: u64);
    fn nonce(&self) -> String;
}

/// One terminal's standing, as `status` shows it and `restart` acts on it.
#[derive(Clone, Debug, Serialize)]
pub struct Row {
    pub terminal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    /// `current`, `old`, `unknown`, or absent for terminals that are not OMP.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `restart`, `skip`, or `none` (current or not OMP).
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip)]
    presence: Option<Presence>,
}

fn is_omp(fg: &Foreground) -> bool {
    let base = |s: &str| Path::new(s).file_name().is_some_and(|n| n == "omp");
    fg.name == "omp" || fg.argv.first().is_some_and(|a| base(a))
}

/// What a run knows about the whole core when it judges one terminal.
pub struct Census<'a> {
    pub target: &'a Target,
    /// The caller's own terminal (`HC_TERMINAL_ID`), never restarted.
    pub caller: Option<&'a str>,
    /// Sessions open in more than one process: a restart there could fork the transcript.
    pub shared: BTreeSet<String>,
    /// Include current images in restart eligibility, without changing safety checks.
    force: bool,
}

impl<'a> Census<'a> {
    pub fn new(
        host: &dyn Host,
        terms: &[Terminal],
        target: &'a Target,
        caller: Option<&'a str>,
    ) -> Self {
        let mut seen = BTreeMap::<String, usize>::new();
        for term in terms.iter().filter(|t| !t.address.contains('/')) {
            let session = term
                .foreground
                .as_ref()
                .and_then(|fg| host.presence(fg.pid));
            if let Some(id) = session.and_then(|p| p.session_id) {
                *seen.entry(id).or_default() += 1;
            }
        }
        let shared = seen
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(id, _)| id)
            .collect();
        Self {
            target,
            caller,
            shared,
            force: false,
        }
    }
}

/// Decide what `restart` may do with `term` now. Fails closed: anything unproven is a skip.
pub fn assess(host: &dyn Host, term: &Terminal, census: &Census) -> Row {
    let (target, caller) = (census.target, census.caller);
    let mut row = Row {
        terminal: term.address.clone(),
        name: term.name.clone(),
        pid: None,
        image: None,
        exe: None,
        model: None,
        action: "skip",
        reason: None,
        presence: None,
    };
    let skip = |mut row: Row, reason: &str| {
        row.action = "skip";
        row.reason = Some(reason.to_owned());
        row
    };
    if term.address.contains('/') {
        return skip(row, "remote");
    }
    if term.state != "live" {
        return skip(row, "not_live");
    }
    let Some(fg) = term.foreground.as_ref().filter(|fg| is_omp(fg)) else {
        row.action = "none";
        row.reason = Some("not_omp".into());
        return row;
    };
    row.pid = Some(fg.pid);
    if term.labels.get("coord.managed").map(String::as_str) == Some("1") {
        return skip(row, "managed");
    }
    if caller == Some(term.address.as_str()) {
        return skip(row, "self");
    }
    let Some(image) = host.image(fg.pid) else {
        row.image = Some("unknown");
        return skip(row, "exe_unreadable");
    };
    row.exe = Some(image.path.clone());
    let current = image.id == target.id && !image.deleted;
    row.image = Some(if current { "current" } else { "old" });
    if current && !census.force {
        row.action = "none";
        row.reason = Some("current".into());
        return row;
    }
    let Some(presence) = host
        .presence(fg.pid)
        .filter(|p| p.protocol == PROTOCOL && p.pid == fg.pid)
        .filter(|p| p.start_time.as_deref() == Some(image.start_time.as_str()))
    else {
        return skip(row, "no_bridge");
    };
    row.model = presence.model.clone();
    let word = term.status.as_ref().map(|s| s.state.as_str());
    let reason = match word {
        _ if !term.reported => Some("status_unknown"),
        Some("idle" | "done") => None,
        Some("working") => Some("busy"),
        Some("blocked") => Some("blocked"),
        _ => Some("status_unknown"),
    }
    .or_else(|| {
        let launcher = presence.launcher.as_ref();
        [
            (!presence.session_saved, "session_not_saved"),
            (!presence.idle, "busy"),
            (presence.queued, "queued_messages"),
            (presence.running_jobs > 0, "background_jobs"),
            (!presence.composer_empty, "draft"),
            (
                presence
                    .session_id
                    .as_ref()
                    .is_some_and(|id| census.shared.contains(id)),
                "session_shared",
            ),
            (launcher.is_none(), "launcher_unresolved"),
            (
                launcher.is_some_and(|l| {
                    (FileId {
                        dev: l.dev,
                        ino: l.ino,
                    }) != target.id
                }),
                "launcher_not_target",
            ),
            (
                presence.replay_risk.as_deref() == Some("restorable_tail"),
                "restorable_tail",
            ),
            (presence.replay_risk.is_some(), "restorable_subagent"),
        ]
        .into_iter()
        .find_map(|(hit, reason)| hit.then_some(reason))
    });
    row.presence = Some(presence);
    match reason {
        Some(reason) => skip(row, reason),
        None => {
            row.action = "restart";
            row
        }
    }
}

fn summary<'a>(words: impl Iterator<Item = &'a str>) -> BTreeMap<&'a str, usize> {
    let mut counts = BTreeMap::new();
    for word in words {
        *counts.entry(word).or_default() += 1;
    }
    counts
}

/// The caller's own terminal; refuses callers the coordinator manages.
fn caller(terms: &[Terminal], caller: Option<&str>) -> Result<()> {
    let Some(address) = caller else { return Ok(()) };
    match terms.iter().find(|t| t.address == address) {
        Some(t) if t.labels.get("coord.managed").map(String::as_str) == Some("1") => {
            bail!("forbidden_caller: coordinator-managed terminals may not restart sessions")
        }
        Some(_) => Ok(()),
        None => bail!("caller_unknown: HC_TERMINAL_ID {address} is not a terminal of this core"),
    }
}

/// The named terminals (address or hc name), each once, in the order given.
fn named<'a>(terms: &'a [Terminal], wanted: &[String]) -> Result<Vec<&'a Terminal>> {
    let mut found: Vec<&Terminal> = Vec::new();
    for want in wanted {
        let Some(term) = terms
            .iter()
            .find(|t| &t.address == want || t.name.as_ref() == Some(want))
        else {
            bail!("terminal_not_found: {want}");
        };
        if !found.iter().any(|t| t.address == term.address) {
            found.push(term);
        }
    }
    Ok(found)
}

/// Read-only census: every terminal with what `restart` would do with it.
pub fn status(
    host: &dyn Host,
    target: &Target,
    only: &[String],
    me: Option<&str>,
) -> Result<Value> {
    let terms = host.terminals()?;
    let shown = if only.is_empty() {
        terms.iter().collect()
    } else {
        named(&terms, only)?
    };
    let census = Census::new(host, &terms, target, me);
    let rows: Vec<Row> = shown
        .into_iter()
        .map(|t| assess(host, t, &census))
        .collect();
    let counts = summary(rows.iter().map(|r| r.action));
    Ok(json!({
        "target": target,
        "restart_available": restore_verified(target),
        "summary": counts,
        "terminals": rows,
    }))
}

/// Which terminals a restart run may touch.
pub enum Scope {
    All,
    Only(Vec<String>),
}

#[derive(Serialize)]
struct Outcome {
    terminal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// `restarted`, `failed`, `skipped`, `current` or `not_attempted`.
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Value::is_null")]
    detail: Value,
}

/// Restart the selected sessions serially; stops at the first failure, on cancel, or when
/// the target executable changes. Without force, a rerun only touches what is still old.
pub fn restart(
    host: &dyn Host,
    target: &Target,
    scope: &Scope,
    me: Option<&str>,
    cancel: &AtomicBool,
    force: bool,
) -> Result<Value> {
    let terms = host.terminals()?;
    caller(&terms, me)?;
    if !restore_verified(target) {
        bail!(
            "target_unverified: {} relaunches sessions with a --restore not checked for replay \
             (checked: {}); `herdr-kit omp status` still works",
            target.version,
            RESTORE_VERIFIED.join(", ")
        );
    }
    let mut census = Census::new(host, &terms, target, me);
    census.force = force;
    let order: Vec<&Terminal> = match scope {
        // Terminals that are not OMP are only reported when named explicitly.
        Scope::All => terms
            .iter()
            .filter(|t| t.foreground.as_ref().is_some_and(is_omp))
            .collect(),
        Scope::Only(list) => named(&terms, list)?,
    };
    let mut results = Vec::new();
    let mut stopped: Option<&'static str> = None;
    for listed in order {
        let address = &listed.address;
        let outcome = |outcome, reason: Option<String>, detail| Outcome {
            terminal: address.clone(),
            name: listed.name.clone(),
            outcome,
            reason,
            detail,
        };
        if let Some(why) = stopped {
            results.push(outcome("not_attempted", Some(why.into()), Value::Null));
            continue;
        }
        if cancel.load(Ordering::SeqCst) {
            stopped = Some("cancelled");
            results.push(outcome(
                "not_attempted",
                Some("cancelled".into()),
                Value::Null,
            ));
            continue;
        }
        if host.file_id(&target.path) != Some(target.id) {
            stopped = Some("target_changed");
            results.push(outcome(
                "not_attempted",
                Some("target_changed".into()),
                Value::Null,
            ));
            continue;
        }
        let Some(term) = host.terminal(address)? else {
            results.push(outcome("skipped", Some("gone".into()), Value::Null));
            continue;
        };
        let row = assess(host, &term, &census);
        let mut result = match (row.action, row.reason.as_deref()) {
            ("restart", _) => one(host, target, &term, row.presence.clone().expect("assessed")),
            (_, Some("current")) => outcome("current", None, Value::Null),
            _ => outcome("skipped", row.reason.clone(), Value::Null),
        };
        result.name = term.name.clone().or_else(|| listed.name.clone());
        if result.outcome == "failed" {
            stopped = Some("failure");
        }
        results.push(result);
    }
    let counts = summary(results.iter().map(|r| r.outcome));
    Ok(json!({
        "target": target,
        "summary": counts,
        "complete": stopped.is_none(),
        "stopped": stopped,
        "results": results,
    }))
}

/// Ask one session to restart and prove what happened.
fn one(host: &dyn Host, target: &Target, term: &Terminal, before: Presence) -> Outcome {
    let done = |outcome, reason: &str, detail| Outcome {
        terminal: term.address.clone(),
        name: None,
        outcome,
        reason: Some(reason.to_owned()),
        detail,
    };
    let pid = before.pid;
    let nonce = host.nonce();
    let expires = host.now_ms() + REQUEST_TTL_MS;
    let request = Request {
        protocol: PROTOCOL,
        nonce: nonce.clone(),
        pid,
        instance: before.instance.clone(),
        session_file: before.session_file.clone().unwrap_or_default(),
        target: target.id,
        expires_at_ms: expires,
    };
    if let Err(e) = host.send(&request) {
        return done("skipped", "request_failed", json!(format!("{e:#}")));
    }
    let answer = loop {
        if let Some(reply) = host
            .reply(pid)
            .filter(|r| r.nonce.as_deref() == Some(&nonce))
        {
            break reply;
        }
        if host.now_ms() > expires + REPLY_GRACE_MS {
            host.withdraw(pid);
            return done("skipped", "no_answer", Value::Null);
        }
        host.sleep_ms(POLL_MS);
    };
    if answer.outcome != "restarting" {
        let reason = answer.reason.unwrap_or_else(|| "refused".into());
        return done("skipped", &reason, Value::Null);
    }
    let before = answer.before.unwrap_or(before);
    let deadline = host.now_ms() + RESTART_TIMEOUT_MS;
    loop {
        if let Some(reply) = host
            .reply(pid)
            .filter(|r| r.nonce.as_deref() == Some(&nonce))
            && reply.outcome == "refused"
        {
            // The plugin took the request back before OMP accepted `/restart`.
            let reason = reply.reason.unwrap_or_else(|| "refused".into());
            return done("skipped", &reason, Value::Null);
        }
        if host
            .presence(pid)
            .is_some_and(|p| p.instance != before.instance && p.start_time == before.start_time)
        {
            break;
        }
        if host
            .image(pid)
            .is_none_or(|i| Some(&i.start_time) != before.start_time.as_ref())
        {
            return done("failed", "process_exited", Value::Null);
        }
        if host.now_ms() > deadline {
            return done("failed", "restart_unconfirmed", Value::Null);
        }
        host.sleep_ms(POLL_MS);
    }
    host.sleep_ms(SETTLE_MS);
    let after = host.presence(pid);
    let image = host.image(pid);
    let now = host.terminal(&term.address).ok().flatten();
    let mut problems = Vec::new();
    match &image {
        None => problems.push("process_exited"),
        Some(i) if Some(&i.start_time) != before.start_time.as_ref() => {
            problems.push("process_replaced")
        }
        Some(i) if i.id != target.id || i.deleted => problems.push("image_not_target"),
        Some(_) => (),
    }
    let terminal_ok = now
        .as_ref()
        .is_some_and(|t| t.state == "live" && t.foreground.as_ref().is_some_and(|f| f.pid == pid));
    if !terminal_ok {
        problems.push("terminal_changed");
    }
    match &after {
        None => problems.push("bridge_gone"),
        Some(a) => {
            if a.instance == before.instance || a.start_time != before.start_time {
                problems.push("bridge_changed");
            }
            if a.session_file != before.session_file || a.session_id != before.session_id {
                problems.push("session_changed");
            }
            if a.model != before.model {
                problems.push("model_changed");
            }
            if before.thinking.is_some() && a.thinking != before.thinking {
                problems.push("thinking_changed");
            }
            let working = now
                .as_ref()
                .and_then(|t| t.status.as_ref())
                .is_some_and(|s| s.state == "working");
            if a.unprompted_runs > 0 || a.running_jobs > 0 || !a.idle || working {
                problems.push("work_resumed");
            }
        }
    }
    let detail = json!({
        "pid": pid,
        "session_file": before.session_file,
        "exe": image.as_ref().map(|i| &i.path),
        "model": {"before": before.model, "after": after.as_ref().and_then(|a| a.model.clone())},
        "thinking": {"before": before.thinking, "after": after.as_ref().and_then(|a| a.thinking.clone())},
    });
    if problems.is_empty() {
        let mut ok = done("restarted", "", detail);
        ok.reason = None;
        ok
    } else {
        done("failed", &problems.join(","), detail)
    }
}
