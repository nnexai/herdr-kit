//! Person-to-coordinator delivery and the current OMP session projection.
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

use super::{
    Ctx,
    caller::Caller,
    error,
    journal::LogQuery,
    kinds::{Kind, SessionMode},
    lifecycle, place,
};
use anyhow::Result;
use kit_core::paths::{SymlinkMessage, no_links};
use serde_json::{Value, json};

const MAX_TRANSCRIPT: u64 = 8 * 1024 * 1024;

pub fn say(ctx: &Ctx, caller: &Caller, text: &str, question: Option<&str>) -> Result<Value> {
    if !matches!(caller, Caller::Person { .. }) {
        return Err(error::refused(
            "forbidden_caller",
            "only the person may send to the coordinator",
        ));
    }
    if text.trim().is_empty() {
        return Err(error::refused("empty_message", "message is empty"));
    }
    if text.len() > 16_384 {
        return Err(error::refused(
            "message_too_long",
            "message exceeds 16384 bytes",
        ));
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(error::refused(
            "invalid_text",
            "message contains a control character",
        ));
    }
    let j = ctx.journal()?;
    let initial = j.read_state()?;
    let addr = initial
        .coordinator
        .terminal
        .as_deref()
        .ok_or_else(|| error::not_ready("coordinator_not_running", "no coordinator terminal"))?
        .to_owned();
    if let Some(qid) = question {
        place::answer_entry(&initial, qid, text)?;
    }
    let _terminal = ctx.terminal_lock(&addr)?;
    // The journal can change while waiting for the terminal lock.
    let current = j.read_state()?;
    if current.coordinator.terminal.as_deref() != Some(&addr)
        || current.coordinator.pid != initial.coordinator.pid
        || current.coordinator.created_ms != initial.coordinator.created_ms
        || current.coordinator.session != initial.coordinator.session
    {
        return Err(error::not_ready(
            "coordinator_changed",
            "coordinator changed before delivery",
        ));
    }
    if let Some(qid) = question {
        place::answer_entry(&current, qid, text)?;
    }
    let term = ctx.hc.get(&addr)?.ok_or_else(|| {
        error::not_ready(
            "coordinator_not_running",
            "coordinator terminal is unavailable",
        )
    })?;
    if term.address != addr || !lifecycle::same_coordinator(&term, &current.coordinator) {
        return Err(error::not_ready(
            "coordinator_changed",
            "coordinator identity changed",
        ));
    }
    if current.coordinator.pid.is_none() || current.coordinator.created_ms.is_none() {
        return Err(error::not_ready(
            "coordinator_changed",
            "coordinator identity is incomplete",
        ));
    }
    place::send_guard(&term)?;
    let sent = ctx.hc.send(&addr, text)?;
    let mut lock = j.lock()?;
    let after = &lock.state().coordinator;
    let changed = after.terminal.as_deref() != Some(&addr)
        || after.pid != current.coordinator.pid
        || after.created_ms != current.coordinator.created_ms
        || after.session != current.coordinator.session;
    let mut entries = vec![(
        None,
        None,
        Kind::CoordSent {
            text: text.to_owned(),
            terminal: addr.clone(),
            qid: question.map(str::to_owned),
        },
    )];
    let answer = if changed {
        None
    } else {
        question.and_then(|qid| place::answer_entry(lock.state(), qid, text).ok())
    };
    let answered = answer.is_some();
    if let Some(answer) = answer {
        entries.push(answer);
    }
    let entries = lock.append(&caller.actor(), ctx.now(), entries)?;
    Ok(
        json!({"state":"sent", "seq":entries[0].seq, "terminal":addr, "turn":sent.turn, "started":sent.started, "status":sent.status, "qid":question, "answered":answered, "coordinator_changed":changed}),
    )
}

fn session_file(
    ctx: &Ctx,
    rec: &super::state::CoordRec,
) -> std::result::Result<std::path::PathBuf, &'static str> {
    let session = rec.session.as_ref().ok_or("no_session")?;
    if session.mode != SessionMode::Dir {
        return Err("unsupported_harness");
    }
    let dir = session.dir.as_ref().ok_or("no_session")?;
    let rel = dir
        .strip_prefix(&ctx.paths.sessions)
        .map_err(|_| "unreadable")?;
    if rel.components().count() != 1
        || !matches!(rel.components().next(), Some(Component::Normal(_)))
    {
        return Err("unreadable");
    }
    if !rel
        .as_os_str()
        .to_str()
        .and_then(|s| s.strip_prefix("coordinator-"))
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("unreadable");
    }
    no_links(dir, SymlinkMessage::WithPath).map_err(|_| "unreadable")?;
    let mut files = Vec::new();
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.map_err(|_| "unreadable")?.path();
                if path.extension().is_some_and(|x| x == "jsonl") {
                    files.push(path);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("pending"),
        Err(_) => return Err("unreadable"),
    }
    if let Some(id) = &session.id {
        files.retain(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.split_once('_'))
                .is_some_and(|(_, found)| found == id)
        });
    }
    if files.is_empty() {
        return Err("pending");
    }
    if files.len() != 1 {
        return Err("ambiguous");
    }
    let file = files.remove(0);
    no_links(&file, SymlinkMessage::WithPath).map_err(|_| "unreadable")?;
    Ok(file)
}

// Validate the session header separately, since it may precede the bounded tail.
fn read_entries(
    path: &Path,
    expected: Option<&str>,
) -> std::result::Result<(Vec<Value>, bool, usize), &'static str> {
    let mut file = File::open(path).map_err(|_| "unreadable")?;
    let size = file.metadata().map_err(|_| "unreadable")?.len();
    let mut head = Vec::new();
    (&mut file)
        .take(64 * 1024)
        .read_to_end(&mut head)
        .map_err(|_| "unreadable")?;
    let header = head
        .split(|b| *b == b'\n')
        .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
        .find(|v| v["type"] == "session")
        .ok_or("malformed")?;
    let id = header["id"].as_str().ok_or("malformed")?;
    if expected.is_some_and(|e| e != id) {
        return Err("mismatch");
    }
    let truncated = size > MAX_TRANSCRIPT;
    file.seek(SeekFrom::Start(size.saturating_sub(MAX_TRANSCRIPT)))
        .map_err(|_| "unreadable")?;
    let mut data = Vec::new();
    file.take(MAX_TRANSCRIPT)
        .read_to_end(&mut data)
        .map_err(|_| "unreadable")?;
    let start = if truncated {
        data.iter()
            .position(|b| *b == b'\n')
            .map_or(data.len(), |i| i + 1)
    } else {
        0
    };
    let mut entries = Vec::new();
    let mut invalid = 0;
    for line in data[start..].split_inclusive(|b| *b == b'\n') {
        if !line.ends_with(b"\n") {
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(v)
                if v.is_object()
                    && v["type"].is_string()
                    && (v["type"] != "message"
                        || (v["id"].is_string()
                            && v["message"]["role"].is_string()
                            && !v["message"]["content"].is_null())) =>
            {
                entries.push(v)
            }
            _ => invalid += 1,
        }
    }
    if invalid > 0 {
        return Err("malformed");
    }
    Ok((entries, truncated, invalid))
}

fn branch(entries: &[Value]) -> (Vec<&Value>, bool) {
    let mut index = HashMap::new();
    for entry in entries {
        if let Some(id) = entry["id"].as_str() {
            index.insert(id, entry);
        }
    }
    let Some(mut item) = entries
        .iter()
        .rev()
        .find(|e| e.get("parentId").is_some() && e["id"].is_string())
    else {
        return (
            entries.iter().filter(|v| v["type"] == "message").collect(),
            false,
        );
    };
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut cut = false;
    loop {
        let Some(id) = item["id"].as_str() else {
            cut = true;
            break;
        };
        if !seen.insert(id) {
            cut = true;
            break;
        }
        result.push(item);
        match item["parentId"].as_str() {
            Some(parent) => match index.get(parent) {
                Some(next) => item = next,
                None => {
                    cut = true;
                    break;
                }
            },
            None => break,
        }
    }
    result.reverse();
    (result, cut)
}
fn text_of(message: &Value) -> String {
    match &message["content"] {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            let mut text = String::new();
            let mut first = true;
            for part in blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
            {
                if !first {
                    text.push_str("\n\n");
                }
                first = false;
                text.push_str(part);
            }
            text
        }
        _ => String::new(),
    }
}
// OMP normally supplies milliseconds directly; its entry timestamp is UTC.
fn timestamp(entry: &Value) -> Option<u64> {
    if let Some(ms) = entry["message"]["timestamp"].as_u64() {
        return Some(ms);
    }
    let s = entry["timestamp"].as_str()?.as_bytes();
    if s.len() < 20
        || s[4] != b'-'
        || s[7] != b'-'
        || s[10] != b'T'
        || s[13] != b':'
        || s[16] != b':'
        || *s.last()? != b'Z'
    {
        return None;
    }
    let num = |a: usize, b: usize| -> Option<i64> {
        std::str::from_utf8(s.get(a..b)?).ok()?.parse().ok()
    };
    let (y, m, d, h, min, sec) = (
        num(0, 4)?,
        num(5, 7)?,
        num(8, 10)?,
        num(11, 13)?,
        num(14, 16)?,
        num(17, 19)?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || min > 59 || sec > 59 {
        return None;
    }
    let millis = if s.len() == 20 {
        0
    } else {
        if s[19] != b'.' || s.len() < 22 {
            return None;
        }
        let fraction = &s[20..s.len() - 1];
        if fraction.is_empty() || !fraction.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let mut n = 0i64;
        for i in 0..3 {
            n = n * 10 + fraction.get(i).map_or(0, |c| i64::from(c - b'0'));
        }
        n
    };
    let year = y - i64::from(m <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    u64::try_from(((days * 24 + h) * 60 + min) * 60_000 + sec * 1000 + millis).ok()
}
fn nudge(s: &str) -> bool {
    s.strip_prefix("inbox: ")
        .and_then(|s| s.strip_suffix(" new"))
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}
fn safe_tool(s: &str) -> Option<&str> {
    (!s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'))
    .then_some(s)
}
fn step_of(block: &Value) -> Option<Value> {
    let name = safe_tool(block["name"].as_str()?)?;
    let mut label = name.to_owned();
    if name == "bash" {
        if let Some(cmd) = block["arguments"]["command"].as_str() {
            let mut words = cmd.split_whitespace();
            if words.any(|word| word == "coord") {
                if let Some(verb) = words.next().filter(|v| {
                    matches!(
                        *v,
                        "board"
                            | "log"
                            | "inbox"
                            | "explain"
                            | "ws"
                            | "task"
                            | "thread"
                            | "config"
                            | "decide"
                            | "ask"
                            | "scout"
                            | "status"
                            | "report"
                            | "note"
                            | "instruct"
                            | "correct"
                            | "release"
                    )
                }) {
                    label = format!("coord {verb}");
                    if matches!(verb, "ws" | "task" | "inbox" | "thread" | "config") {
                        if let Some(sub) = words.next().filter(|v| {
                            matches!(
                                *v,
                                "open" | "close" | "list" | "start" | "send" | "ack" | "set"
                            )
                        }) {
                            label.push(' ');
                            label.push_str(sub);
                        }
                    }
                }
            }
        }
    }
    Some(json!({"tool":name,"label":label}))
}
pub fn conversation(ctx: &Ctx, limit: usize) -> Result<Value> {
    let j = ctx.journal()?;
    let state = j.read_state()?;
    let rec = &state.coordinator;
    let term = rec
        .terminal
        .as_deref()
        .and_then(|addr| ctx.hc.get(addr).ok().flatten())
        .filter(|t| lifecycle::same_coordinator(t, rec));
    let word = term.as_ref().and_then(|t| t.word());
    let working_since = term
        .as_ref()
        .filter(|t| t.word() == Some("working"))
        .and_then(|t| t.status_ms);
    let mut source_state = "ready";
    let mut truncated = false;
    let mut invalid = 0;
    let mut messages = Vec::new();
    let mut steps = 0usize;
    let mut step: Option<Value> = None;
    let mut saw_user = false;
    let mut sent = j
        .page(&LogQuery {
            limit: 500,
            before_seq: None,
            after_seq: None,
            since_ms: rec.started_ms,
            ws: None,
            task: None,
        })?
        .entries;
    sent.reverse();
    let mut sent_pos = 0;
    let loaded = if rec.terminal.is_none() {
        Err("not_running")
    } else {
        session_file(ctx, rec)
            .and_then(|p| read_entries(&p, rec.session.as_ref().and_then(|s| s.id.as_deref())))
    };
    match loaded {
        Err(reason) => source_state = reason,
        Ok((entries, tail, bad)) => {
            invalid = bad;
            let (branch, cut) = branch(&entries);
            truncated = tail || cut;
            for entry in branch {
                if entry["type"] != "message" {
                    continue;
                }
                let msg = &entry["message"];
                let role = msg["role"].as_str().unwrap_or("");
                if role == "user" {
                    steps = 0;
                    step = None;
                    let text = text_of(msg);
                    let first = !saw_user;
                    saw_user = true;
                    let matched = sent[sent_pos..].iter().enumerate().find(|(_, e)| {
                        e["kind"] == "coord.sent"
                            && e["data"]["terminal"].as_str() == rec.terminal.as_deref()
                            && e["data"]["text"].as_str() == Some(text.as_str())
                    });
                    if matched.is_none()
                        && ((first && !truncated)
                            || (text.contains("## Commands\n`kit` is:")
                                && text.contains("## Startup read\n"))
                            || nudge(&text)
                            || text.starts_with("<hc from="))
                    {
                        continue;
                    }
                    if text.is_empty() {
                        continue;
                    }
                    let mut item = json!({"id":entry["id"],"ts_ms":timestamp(entry),"role":"user","text":text,"via":"terminal"});
                    if let Some((offset, e)) = matched {
                        sent_pos += offset + 1;
                        item["via"] = json!("ui");
                        item["sent_seq"] = e["seq"].clone();
                        item["qid"] = e["data"]["qid"].clone();
                    }
                    messages.push(item);
                } else if role == "assistant" {
                    if let Some(blocks) = msg["content"].as_array() {
                        for block in blocks {
                            if block["type"] == "toolCall" {
                                steps += 1;
                                step = step_of(block);
                            }
                        }
                    }
                    if msg["stopReason"] == "stop" {
                        let text = text_of(msg);
                        if !text.trim().is_empty() {
                            messages.push(json!({"id":entry["id"],"ts_ms":timestamp(entry),"role":"assistant","text":text}));
                        }
                    }
                }
            }
        }
    }
    let has_more = messages.len() > limit;
    if has_more {
        messages.drain(..messages.len() - limit);
    }
    Ok(
        json!({"schema_version":1,"generated_at_ms":ctx.now(),"coordinator":{"terminal":rec.terminal,"reachable":term.is_some(),"status":word,"status_ms":term.as_ref().and_then(|t| t.status_ms),"working_since_ms":working_since,"step":if word == Some("working") { step.clone() } else { None },"model":rec.model,"restart_pending":rec.handoff.is_some()},"turn":{"steps":steps,"step":step},"messages":messages,"has_more":has_more,"source":{"state":source_state,"detail":if source_state == "ready" { None } else { Some(source_state) },"session_id":rec.session.as_ref().and_then(|s| s.id.as_ref()),"watch_dir":rec.session.as_ref().and_then(|s| s.dir.as_ref()),"truncated":truncated,"invalid_lines":invalid}}),
    )
}
