//! Single-instance event watcher.
use super::ids::{TaskId, WsId};
use anyhow::Result;
use fs2::FileExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::{fs::OpenOptions, io, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};

struct PendingSend {
    _guard: std::fs::File,
    result: std::sync::mpsc::Receiver<Result<()>>,
    ws: Option<WsId>,
    task: Option<TaskId>,
    record: Kind,
}

async fn deliver(command: std::process::Command) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut command = tokio::process::Command::from(command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn()?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let mut out = out.take(65537);
    let mut err = err.take(65537);
    let operation = async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            out.read_to_end(&mut stdout),
            err.read_to_end(&mut stderr)
        )?;
        anyhow::ensure!(
            stdout.len() <= 65536 && stderr.len() <= 65536,
            "delivery output exceeded limit"
        );
        anyhow::ensure!(
            status.success(),
            "hc send failed: {}",
            String::from_utf8_lossy(&stderr)
        );
        let _: super::adapter::SendOutcome = serde_json::from_slice(&stdout)?;
        Ok(())
    };
    match tokio::time::timeout(Duration::from_secs(90), operation).await {
        Ok(result) => result,
        Err(_) => {
            if let Some(pid) = child
                .id()
                .and_then(|id| i32::try_from(id).ok())
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = child.kill().await;
            Err(anyhow::anyhow!("hc send timed out after 90s"))
        }
    }
}
use super::{
    Ctx,
    adapter::{Hc, Term},
    config::Config,
    journal::Journal,
    kinds::{Actor, Entry, Kind},
    lifecycle, reconcile,
    rules::{self, Action, Facts},
};

pub enum Line {
    Snapshot(Vec<Term>),
    Event { terminal: String, kind: String },
    Other,
}
pub fn parse_line(line: &str) -> Line {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Line::Other;
    };
    if let Some(terms) = v.get("terminals") {
        return serde_json::from_value(terms.clone())
            .map(Line::Snapshot)
            .unwrap_or(Line::Other);
    }
    match (
        v.pointer("/params/terminal").and_then(Value::as_str),
        v.pointer("/params/type").and_then(Value::as_str),
    ) {
        (Some(terminal), Some(kind)) => Line::Event {
            terminal: terminal.into(),
            kind: kind.into(),
        },
        _ => Line::Other,
    }
}
pub struct Core {
    pub facts: Facts,
    pub release: String,
    pub started_ms: u64,
    pub loaded_seq: u64,
    pending: BTreeMap<String, PendingSend>,
    retry_after: BTreeMap<String, u64>,
}
impl Core {
    fn observe(&mut self, term: Term) {
        if term.word() == Some("working") {
            if let Some(ms) = term.status_ms {
                self.facts.working_since.insert(term.address.clone(), ms);
            }
        }
        self.facts.terms.insert(term.address.clone(), term);
    }
    pub fn on_line(&mut self, line: Line, hc: &dyn Hc) -> Result<()> {
        match line {
            Line::Snapshot(terms) => {
                self.facts.terms.clear();
                self.facts.initialized = true;
                for term in terms {
                    self.facts.terms.insert(term.address.clone(), term);
                }
            }
            Line::Event { terminal, kind } if kind == "removed" => {
                self.facts.terms.remove(&terminal);
                self.facts.working_since.remove(&terminal);
            }
            Line::Event { terminal, kind } if kind != "title" => {
                if let Some(term) = hc.get(&terminal)? {
                    self.observe(term);
                } else {
                    self.facts.terms.remove(&terminal);
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn load(&mut self, j: &Journal) -> Result<()> {
        let entries = j.read_after(self.loaded_seq)?;
        if let Some(last) = entries.last() {
            self.loaded_seq = last.seq;
        }
        self.facts.entries.extend(entries);
        self.compact_history();
        Ok(())
    }
    fn compact_history(&mut self) {
        use super::{ids::TaskId, kinds::NudgeTarget};
        use std::collections::BTreeMap;
        let group = |entry: &Entry| -> Option<Option<TaskId>> {
            match entry.typed() {
                Kind::TaskStarted { .. } | Kind::TaskSent { .. } | Kind::TaskTurn { .. } => {
                    entry.task.clone().map(Some)
                }
                Kind::NudgeSent {
                    target: NudgeTarget::Task,
                    ..
                } => entry.task.clone().map(Some),
                Kind::CoordSessionStarted { .. }
                | Kind::CoordTurn { .. }
                | Kind::NudgeSent {
                    target: NudgeTarget::Coordinator,
                    ..
                } => Some(None),
                _ => None,
            }
        };
        let mut turns: BTreeMap<Option<TaskId>, u64> = BTreeMap::new();
        for entry in &self.facts.entries {
            if matches!(
                entry.typed(),
                Kind::TaskTurn { .. } | Kind::CoordTurn { .. }
            ) {
                if let Some(key) = group(entry) {
                    turns.insert(key, entry.seq);
                }
            }
        }
        let mut anchors: BTreeMap<Option<TaskId>, u64> = BTreeMap::new();
        for entry in &self.facts.entries {
            if matches!(
                entry.typed(),
                Kind::TaskStarted { .. } | Kind::TaskSent { .. } | Kind::CoordSessionStarted { .. }
            ) {
                if let Some(key) = group(entry) {
                    if turns.get(&key).is_some_and(|seq| entry.seq < *seq) {
                        anchors.insert(key, entry.seq);
                    }
                }
            }
        }
        self.facts.entries.retain(|entry| {
            group(entry).is_some_and(|key| {
                entry.seq >= turns.get(&key).copied().unwrap_or(0)
                    || anchors.get(&key) == Some(&entry.seq)
            })
        });
    }
    pub fn tick(&mut self, ctx: &Ctx, j: &Journal, cfg: &Config) -> Result<Vec<Entry>> {
        self.load(j)?;
        let completed: Vec<_> = self
            .pending
            .iter_mut()
            .filter_map(|(address, pending)| match pending.result.try_recv() {
                Ok(result) => Some((address.clone(), result)),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(e) => Some((address.clone(), Err(e.into()))),
            })
            .collect();
        for (address, result) in completed {
            let pending = self.pending.remove(&address).unwrap();
            match result {
                Ok(()) => {
                    j.lock()?.append(
                        &Actor::Watcher,
                        ctx.now(),
                        vec![(pending.ws, pending.task, pending.record)],
                    )?;
                    self.retry_after.remove(&address);
                }
                Err(e) => {
                    self.retry_after
                        .insert(address.clone(), ctx.now().saturating_add(30_000));
                    j.lock()?.append(
                        &Actor::Watcher,
                        ctx.now(),
                        vec![(
                            pending.ws,
                            pending.task,
                            Kind::ReconcileFinding {
                                code: "delivery_failed".into(),
                                summary: format!("Delivery to {address} failed: {e:#}"),
                            },
                        )],
                    )?;
                }
            }
        }
        self.load(j)?;
        let actions = rules::evaluate(&j.read_state()?, &self.facts, cfg, ctx.now());
        let batch = Journal::with_context(&ctx.paths, super::new_op()?, None)?;
        let mut written = Vec::new();
        for action in actions {
            let send_guard = if let Action::Send { terminal, .. } = &action {
                if self.pending.contains_key(terminal)
                    || self
                        .retry_after
                        .get(terminal)
                        .is_some_and(|until| ctx.now() < *until)
                {
                    continue;
                }
                match ctx.terminal_lock(terminal) {
                    Ok(guard) => Some(guard),
                    Err(_) => continue,
                }
            } else {
                None
            };
            let send_term = if let Action::Send { terminal, .. } = &action {
                match ctx.hc.get(terminal)? {
                    Some(term) => Some(term),
                    None => continue,
                }
            } else {
                None
            };
            if let Action::Append {
                kind: Kind::CoordSessionEnded { terminal, .. },
                ..
            } = &action
            {
                // Journal writes and hc events are independent streams. In
                // particular our synchronous restart queues events while the
                // journal already points at the replacement. Confirm loss
                // against hc before turning a stale cache miss into history.
                match ctx.hc.get(terminal)? {
                    Some(term) => self.observe(term),
                    None => {
                        self.facts.terms.remove(terminal);
                        self.facts.working_since.remove(terminal);
                    }
                }
            }
            let mut lock = batch.lock()?;
            let fresh = rules::evaluate(lock.state(), &self.facts, cfg, ctx.now());
            let key = action_key(&action);
            if !fresh.iter().any(|a| action_key(a) == key) {
                continue;
            }
            match action {
                Action::Append { ws, task, kind } => {
                    if matches!(kind, Kind::TaskTurn { .. } | Kind::CoordTurn { .. }) {
                        let address = task
                            .as_ref()
                            .and_then(|id| lock.state().tasks.get(id))
                            .and_then(|t| t.terminal.clone())
                            .or_else(|| lock.state().coordinator.terminal.clone());
                        if let Some(address) = address {
                            self.facts.working_since.remove(&address);
                        }
                    }
                    written.extend(lock.append(
                        &Actor::Watcher,
                        ctx.now(),
                        vec![(ws, task, kind)],
                    )?);
                }
                Action::Send {
                    terminal,
                    text,
                    record,
                    ws,
                    task,
                } => {
                    let term = send_term.unwrap();
                    if !term.sendable() {
                        continue;
                    }
                    if !term.managed() {
                        continue;
                    }
                    if let Some(id) = &task {
                        let Some(rec) = lock.state().tasks.get(id) else {
                            continue;
                        };
                        if term.label("coord.task") != Some(id.as_str())
                            || term.label("coord.ws") != Some(rec.ws.as_str())
                            || rec.pid.is_none()
                            || rec.pid != term.pid
                            || rec.created_ms != Some(term.created_ms)
                        {
                            continue;
                        }
                    } else if term.label("coord.role") != Some("coordinator") {
                        continue;
                    }
                    self.observe(term);
                    if !rules::evaluate(lock.state(), &self.facts, cfg, ctx.now())
                        .iter()
                        .any(|a| action_key(a) == key)
                    {
                        continue;
                    }
                    let command = ctx.hc.send_command(&terminal, &text);
                    let (tx, rx) = std::sync::mpsc::channel();
                    tokio::spawn(async move {
                        let _ = tx.send(deliver(command).await);
                    });
                    self.pending.insert(
                        terminal,
                        PendingSend {
                            _guard: send_guard.unwrap(),
                            result: rx,
                            ws,
                            task,
                            record,
                        },
                    );
                }
                Action::CloseLeftover { task, terminal } => {
                    written.extend(lock.append(&Actor::Watcher, ctx.now(), vec![(Some(task.ws()), Some(task), Kind::ReconcileFinding { code:"replaced_terminal_retained".into(), summary:format!("Retained replaced terminal {terminal}; it may contain a person's work") })])?);
                }
                Action::RestartCoordinator { reason } => {
                    drop(lock);
                    lifecycle::restart_coordinator(ctx, &batch, &reason)?;
                }
            }
        }
        self.load(j)?;
        Ok(written)
    }
}
fn action_key(a: &Action) -> String {
    match a {
        Action::Append { ws, task, kind } => format!(
            "append:{ws:?}:{task:?}:{}",
            serde_json::to_string(kind).unwrap_or_default()
        ),
        Action::Send {
            terminal,
            ws,
            task,
            record,
            ..
        } => format!(
            "send:{terminal}:{ws:?}:{task:?}:{}",
            serde_json::to_string(record).unwrap_or_default()
        ),
        Action::CloseLeftover { task, terminal } => format!("close:{task:?}:{terminal}"),
        Action::RestartCoordinator { reason } => format!("restart:{reason}"),
    }
}
fn stream_started(core: &mut Core) {
    core.facts.events_ok = true;
}
// Returns true only for the first EOF or spawn failure in an outage.
fn stream_ended(core: &mut Core, outage: &mut bool) -> bool {
    core.facts.events_ok = false;
    let first = !*outage;
    *outage = true;
    first
}
fn stream_line_received(outage: &mut bool) {
    *outage = false;
}
const STREAM_RETRY: Duration = Duration::from_secs(2);

fn replacement_command(root: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new(root.join("bin/herdr-kit"));
    command.arg("--data-dir").arg(root).args(["coord", "watch"]);
    command
}

pub async fn run(ctx: Ctx) -> Result<Value> {
    ctx.paths.ensure()?;
    super::paths::no_links(&ctx.paths.watcher_lock)?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&ctx.paths.watcher_lock)?;
    if let Err(e) = file.try_lock_exclusive() {
        if e.kind() == io::ErrorKind::WouldBlock {
            return Ok(json!({"state":"already_running"}));
        }
        return Err(e.into());
    }
    let j = ctx.journal()?;
    let started_ms = ctx.now();
    let terminal = std::env::var("HC_TERMINAL_ID").ok();
    let release = lifecycle::active_release(&ctx.root);
    j.lock()?.append(
        &Actor::Watcher,
        started_ms,
        vec![(
            None,
            None,
            Kind::WatcherStarted {
                release: release.clone(),
                terminal: terminal.clone(),
                pid: std::process::id(),
            },
        )],
    )?;
    let mut core = Core {
        facts: Facts::default(),
        release,
        started_ms,
        loaded_seq: 0,
        pending: BTreeMap::new(),
        retry_after: BTreeMap::new(),
    };
    let cfg = ctx.config()?;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let mut recon = tokio::time::interval(Duration::from_secs(cfg.reconcile_secs.max(1)));
    let mut outage = false;
    loop {
        let mut command = tokio::process::Command::from(ctx.hc.events_command());
        command
            .args(["--label", "coord.managed=1"])
            .env_remove("HC_TERMINAL_ID")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let child = command.spawn();
        if let Ok(mut child) = child {
            stream_started(&mut core);
            let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
            loop {
                tokio::select! {
                    line = lines.next_line() => match line {
                        Ok(Some(line)) => { if let Err(e) = core.on_line(parse_line(&line), ctx.hc.as_ref()) { eprintln!("watch event: {e:#}"); } stream_line_received(&mut outage); }
                        _ => break,
                    },
                    _ = tick.tick() => { if let Err(e) = core.tick(&ctx, &j, &cfg) { eprintln!("watch tick: {e:#}"); } },
                    _ = recon.tick() => { if let Err(e) = reconcile::reconcile(&ctx, &j, &mut core.facts, &cfg) { eprintln!("watch reconcile: {e:#}"); } },
                    _ = heartbeat.tick() => heartbeat_step(&ctx, &j, &core, &terminal)?,
                }
            }
        }
        if stream_ended(&mut core, &mut outage) {
            j.lock()?.append(
                &Actor::Watcher,
                ctx.now(),
                vec![(
                    None,
                    None,
                    Kind::ReconcileFinding {
                        code: "events_stream_ended".into(),
                        summary: "Managed terminal event stream ended".into(),
                    },
                )],
            )?;
        }
        let delay = tokio::time::sleep(STREAM_RETRY);
        tokio::pin!(delay);
        loop {
            tokio::select! {
                _ = &mut delay => break,
                _ = tick.tick() => { if let Err(e) = core.tick(&ctx, &j, &cfg) { eprintln!("watch tick: {e:#}"); } },
                _ = recon.tick() => { if let Err(e) = reconcile::reconcile(&ctx, &j, &mut core.facts, &cfg) { eprintln!("watch reconcile: {e:#}"); } },
                _ = heartbeat.tick() => heartbeat_step(&ctx, &j, &core, &terminal)?,
            }
        }
    }
}
fn prepare_heartbeat(
    ctx: &Ctx,
    j: &Journal,
    core: &Core,
    terminal: &Option<String>,
) -> Result<Option<std::process::Command>> {
    lifecycle::write_heartbeat(
        &ctx.paths,
        &lifecycle::Heartbeat {
            pid: std::process::id(),
            terminal: terminal.clone(),
            release: core.release.clone(),
            started_ms: core.started_ms,
            beat_ms: ctx.now(),
            events_ok: core.facts.events_ok,
        },
    )?;
    if lifecycle::active_release(&ctx.root) != core.release {
        j.lock()?.append(
            &Actor::Watcher,
            ctx.now(),
            vec![(
                None,
                None,
                Kind::WatcherStopped {
                    reason: "release_changed".into(),
                },
            )],
        )?;
        return Ok(Some(replacement_command(&ctx.root)));
    }
    Ok(None)
}
fn heartbeat_step(ctx: &Ctx, j: &Journal, core: &Core, terminal: &Option<String>) -> Result<()> {
    if let Some(mut command) = prepare_heartbeat(ctx, j, core, terminal)? {
        use std::os::unix::process::CommandExt;
        return Err(command.exec().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::{
        adapter::fake::{FakeApp, FakeHc},
        paths::CoordPaths,
    };
    fn ctx(root: &std::path::Path, hc: FakeHc) -> Ctx {
        Ctx {
            root: root.into(),
            paths: CoordPaths::new(root),
            hc: Box::new(hc),
            app: Box::new(FakeApp::default()),
            clock: Box::new(|| 42_000),
            op: "test".into(),
            cause: None,
        }
    }
    #[test]
    fn captured_lines_and_scripted_core() {
        let mut lines = include_str!("testdata/hc-events-head.jsonl").lines();
        let snapshot = parse_line(lines.next().unwrap());
        assert!(matches!(&snapshot, Line::Snapshot(terms) if terms.len() == 1));
        let title = parse_line(lines.next().unwrap());
        assert!(matches!(&title, Line::Event { kind, .. } if kind == "title"));
        let hc = FakeHc::default();
        let mut core = Core {
            facts: Facts::default(),
            release: "development".into(),
            started_ms: 0,
            loaded_seq: 0,
            pending: BTreeMap::new(),
            retry_after: BTreeMap::new(),
        };
        core.on_line(snapshot, &hc).unwrap();
        core.on_line(title, &hc).unwrap();
        assert!(hc.calls.borrow().is_empty());
        assert!(core.facts.terms.contains_key("t4"));
        core.on_line(
            Line::Event {
                terminal: "t4".into(),
                kind: "removed".into(),
            },
            &hc,
        )
        .unwrap();
        assert!(core.facts.terms.is_empty());
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx(root.path(), hc);
        let journal = ctx.journal().unwrap();
        assert!(
            core.tick(&ctx, &journal, &Config::default())
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn stream_eof_retries_once_per_outage_until_a_line_arrives() {
        let hc = FakeHc::default();
        let mut core = Core {
            facts: Facts::default(),
            release: "development".into(),
            started_ms: 0,
            loaded_seq: 0,
            pending: BTreeMap::new(),
            retry_after: BTreeMap::new(),
        };
        let mut outage = false;
        stream_started(&mut core);
        assert!(core.facts.events_ok);
        assert!(stream_ended(&mut core, &mut outage));
        assert!(!core.facts.events_ok);
        assert_eq!(STREAM_RETRY, Duration::from_secs(2));
        stream_started(&mut core);
        assert!(!stream_ended(&mut core, &mut outage));
        stream_started(&mut core);
        let line = include_str!("testdata/hc-events-head.jsonl")
            .lines()
            .next()
            .unwrap();
        core.on_line(parse_line(line), &hc).unwrap();
        stream_line_received(&mut outage);
        assert!(core.facts.terms.contains_key("t4"));
        assert!(stream_ended(&mut core, &mut outage));
    }
    #[test]
    fn release_change_records_stop_and_prepares_exact_replacement_argv() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data dir");
        let ctx = ctx(&root, FakeHc::default());
        ctx.paths.ensure().unwrap();
        let j = ctx.journal().unwrap();
        let core = Core {
            facts: Facts {
                events_ok: true,
                ..Facts::default()
            },
            release: "development".into(),
            started_ms: 123,
            loaded_seq: 0,
            pending: BTreeMap::new(),
            retry_after: BTreeMap::new(),
        };
        let terminal = Some("watcher-terminal".into());
        assert!(
            prepare_heartbeat(&ctx, &j, &core, &terminal)
                .unwrap()
                .is_none()
        );
        assert!(j.read_after(0).unwrap().is_empty());
        let hb = lifecycle::read_heartbeat(&ctx.paths).unwrap().unwrap();
        assert_eq!(
            (hb.started_ms, hb.beat_ms, hb.events_ok),
            (123, 42_000, true)
        );
        std::fs::write(root.join("state.json"), br#"{"current":"new-release"}"#).unwrap();
        let cmd = prepare_heartbeat(&ctx, &j, &core, &terminal)
            .unwrap()
            .unwrap();
        assert_eq!(cmd.get_program(), root.join("bin/herdr-kit").as_os_str());
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            [
                std::ffi::OsStr::new("--data-dir"),
                root.as_os_str(),
                std::ffi::OsStr::new("coord"),
                std::ffi::OsStr::new("watch")
            ]
        );
        let entries = j.read_after(0).unwrap();
        assert!(
            matches!(entries.as_slice(), [entry] if matches!(entry.typed(), Kind::WatcherStopped { reason } if reason == "release_changed"))
        );
    }
    #[tokio::test]
    async fn scripted_tick_rechecks_readiness_before_nudge() {
        use crate::coord::{
            adapter::SendOutcome,
            ids::{TaskId, WsId},
        };
        for fresh_ready in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let ws = WsId::parse("ws-abcdef").unwrap();
            let task = TaskId::new(&ws, 1);
            let term: Term = serde_json::from_value(json!({"address":"t1","state":"live","status":{"state":"done","app":null,"kind":null,"msg":null},"status_ms":0,"turn":1,"reported":true,"labels":{"coord.managed":"1","coord.task":task.as_str(),"coord.ws":ws.as_str()},"pid":1,"created_ms":0,"cwd":"/tmp","foreground":null,"reason":null,"name":null})).unwrap();
            let mut fresh = term.clone();
            if !fresh_ready {
                fresh.turn = 0;
            }
            let hc = FakeHc::default();
            hc.gets.borrow_mut().push_back(Ok(Some(fresh)));
            if fresh_ready {
                hc.sends.borrow_mut().push_back(Ok(SendOutcome {
                    turn: 1,
                    started: Some(true),
                    status: None,
                }));
            }
            let ctx = ctx(root.path(), hc);
            let j = ctx.journal().unwrap();
            j.lock()
                .unwrap()
                .append(
                    &Actor::Watcher,
                    0,
                    vec![
                        (
                            Some(ws.clone()),
                            None,
                            Kind::WsOpened {
                                title: "test".into(),
                                repo: None,
                                delivery_target: None,
                                delivery_branch: None,
                                purpose: None,
                                request: None,
                            },
                        ),
                        (
                            Some(ws.clone()),
                            Some(task.clone()),
                            Kind::TaskPlaced {
                                rung: 2,
                                reason: "test".into(),
                                rejected: vec![],
                                role: "r".into(),
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
                            Some(task.clone()),
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
                    ],
                )
                .unwrap();
            let mut core = Core {
                facts: Facts::default(),
                release: "development".into(),
                started_ms: 0,
                loaded_seq: 0,
                pending: BTreeMap::new(),
                retry_after: BTreeMap::new(),
            };
            core.on_line(Line::Snapshot(vec![term]), ctx.hc.as_ref())
                .unwrap();
            let cfg = Config {
                nudge_delegate_secs: 1,
                ..Config::default()
            };
            core.tick(&ctx, &j, &cfg).unwrap();
            if fresh_ready {
                assert_eq!(core.pending.len(), 1);
                while !core.pending.is_empty() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    core.tick(&ctx, &j, &cfg).unwrap();
                }
            }
            assert_eq!(
                j.read_after(0)
                    .unwrap()
                    .iter()
                    .filter(|e| e.kind == "nudge.sent")
                    .count(),
                usize::from(fresh_ready)
            );
        }
    }
    #[tokio::test]
    async fn failed_delivery_keeps_coordinator_inbox_eligible_and_backs_off() {
        use crate::coord::kinds::NudgeTarget;
        let root = tempfile::tempdir().unwrap();
        let term: Term = serde_json::from_value(json!({"address":"c1","state":"live","status":{"state":"done","app":null,"kind":null,"msg":null},"status_ms":0,"turn":1,"reported":true,"labels":{"coord.managed":"1","coord.role":"coordinator"},"pid":1,"created_ms":1,"cwd":"/tmp","foreground":null,"reason":null,"name":null})).unwrap();
        let hc = FakeHc::default();
        hc.gets.borrow_mut().push_back(Ok(Some(term.clone())));
        hc.sends
            .borrow_mut()
            .push_back(Err(anyhow::anyhow!("draft rejected")));
        let ctx = ctx(root.path(), hc);
        let j = ctx.journal().unwrap();
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                1,
                vec![
                    (
                        None,
                        None,
                        Kind::CoordSessionStarted {
                            role: "coord-main".into(),
                            model: None,
                            terminal: "c1".into(),
                            pid: Some(1),
                            created_ms: Some(1),
                            space: "sp".into(),
                            tab: None,
                            session: None,
                            release: "development".into(),
                            role_hash: String::new(),
                            reason: "up".into(),
                            brief: "/tmp/brief".into(),
                        },
                    ),
                    (
                        None,
                        None,
                        Kind::ReconcileFinding {
                            code: "test".into(),
                            summary: "wake".into(),
                        },
                    ),
                ],
            )
            .unwrap();
        let mut core = Core {
            facts: Facts::default(),
            release: "development".into(),
            started_ms: 0,
            loaded_seq: 0,
            pending: BTreeMap::new(),
            retry_after: BTreeMap::new(),
        };
        core.on_line(Line::Snapshot(vec![term]), ctx.hc.as_ref())
            .unwrap();
        let cfg = Config {
            nudge_coordinator_secs: 1,
            ..Config::default()
        };
        core.tick(&ctx, &j, &cfg).unwrap();
        assert_eq!(core.pending.len(), 1);
        core.tick(&ctx, &j, &cfg).unwrap();
        assert_eq!(core.pending.len(), 1);
        while !core.pending.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
            core.tick(&ctx, &j, &cfg).unwrap();
        }
        let entries = j.read_after(0).unwrap();
        assert!(!entries.iter().any(|e| matches!(
            e.typed(),
            Kind::NudgeSent {
                target: NudgeTarget::Coordinator,
                ..
            }
        )));
        assert!(entries.iter().any(|e| matches!(e.typed(), Kind::ReconcileFinding { code, summary } if code == "delivery_failed" && summary.contains("draft rejected") && summary.contains("c1"))));
        assert_eq!(j.read_state().unwrap().last_coordinator_nudge_upto, 0);
        assert!(
            rules::evaluate(&j.read_state().unwrap(), &core.facts, &cfg, ctx.now())
                .iter()
                .any(|a| matches!(a, Action::Send { terminal, .. } if terminal == "c1"))
        );
        assert_eq!(core.retry_after["c1"], 72_000);
    }
    #[tokio::test]
    async fn pending_delivery_allows_reports_and_heartbeat_and_excludes_release() {
        use crate::coord::{
            caller::Caller,
            cli::ReportKind,
            ids::{TaskId, WsId},
            kinds::{NudgeTarget, ResType, ResourceRef},
            place, release,
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx(root.path(), FakeHc::default());
        let j = ctx.journal().unwrap();
        let ws = WsId::parse("ws-abcdef").unwrap();
        let task = TaskId::new(&ws, 1);
        let r = ResourceRef {
            r#type: ResType::Tab,
            name: "t1".into(),
            repo: None,
            folder: None,
            label: None,
            branch: None,
            base: None,
            base_sha: None,
            create_branch: None,
            space: Some("sp".into()),
            tab: None,
        };
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                0,
                vec![
                    (
                        Some(ws.clone()),
                        None,
                        Kind::WsOpened {
                            title: "test".into(),
                            repo: None,
                            delivery_target: None,
                            delivery_branch: None,
                            purpose: None,
                            request: None,
                        },
                    ),
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
                        Kind::TaskPlaced {
                            rung: 2,
                            reason: "test".into(),
                            rejected: vec![],
                            role: "r".into(),
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
                    (Some(ws.clone()), Some(task.clone()), Kind::ResCreated(r)),
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
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
                ],
            )
            .unwrap();
        let mut core = Core {
            facts: Facts::default(),
            release: "development".into(),
            started_ms: 0,
            loaded_seq: 0,
            pending: BTreeMap::new(),
            retry_after: BTreeMap::new(),
        };
        let guard = ctx.terminal_lock("t1").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut cmd = std::process::Command::new("sh");
        cmd.args([
            "-c",
            "sleep 1; printf '{\"turn\":1,\"started\":true,\"status\":null}'",
        ]);
        tokio::spawn(async move {
            let _ = tx.send(deliver(cmd).await);
        });
        core.pending.insert(
            "t1".into(),
            PendingSend {
                _guard: guard,
                result: rx,
                ws: Some(ws.clone()),
                task: Some(task.clone()),
                record: Kind::NudgeSent {
                    target: NudgeTarget::Task,
                    text: "nudge".into(),
                    upto_seq: None,
                },
            },
        );
        let caller = Caller::Delegate {
            terminal: "t1".into(),
            task: task.clone(),
            ws: ws.clone(),
        };
        place::report(&ctx, &caller, ReportKind::Done, "done", None).unwrap();
        assert!(prepare_heartbeat(&ctx, &j, &core, &None).unwrap().is_none());
        assert!(release::release_task_tab(&ctx, &j, &Actor::Watcher, &task).is_err());
        assert_eq!(core.pending.len(), 1);
        assert!(
            j.read_after(0)
                .unwrap()
                .iter()
                .any(|e| e.kind == "task.reported")
        );
    }
    #[test]
    fn replaced_terminal_with_editor_is_retained() {
        use crate::coord::{
            ids::{TaskId, WsId},
            kinds::{ResType, ResourceRef},
        };
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx(root.path(), FakeHc::default());
        let j = ctx.journal().unwrap();
        let ws = WsId::parse("ws-abcdef").unwrap();
        let task = TaskId::new(&ws, 1);
        let r = ResourceRef {
            r#type: ResType::Tab,
            name: "t1".into(),
            repo: None,
            folder: None,
            label: None,
            branch: None,
            base: None,
            base_sha: None,
            create_branch: None,
            space: Some("sp".into()),
            tab: None,
        };
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                0,
                vec![
                    (
                        Some(ws.clone()),
                        None,
                        Kind::WsOpened {
                            title: "test".into(),
                            repo: None,
                            delivery_target: None,
                            delivery_branch: None,
                            purpose: None,
                            request: None,
                        },
                    ),
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
                        Kind::TaskPlaced {
                            rung: 2,
                            reason: "test".into(),
                            rejected: vec![],
                            role: "r".into(),
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
                    (Some(ws.clone()), Some(task.clone()), Kind::ResCreated(r)),
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
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
                ],
            )
            .unwrap();
        let term: Term = serde_json::from_value(json!({"address":"t1","state":"live","status":null,"status_ms":0,"turn":0,"reported":false,"labels":{"coord.managed":"1","coord.task":task.as_str(),"coord.ws":ws.as_str()},"pid":2,"created_ms":1,"cwd":"/tmp","foreground":{"pid":3,"name":"vim","cwd":"/tmp"},"reason":null,"name":null})).unwrap();
        let mut core = Core {
            facts: Facts::default(),
            release: "development".into(),
            started_ms: 0,
            loaded_seq: 0,
            pending: BTreeMap::new(),
            retry_after: BTreeMap::new(),
        };
        core.on_line(Line::Snapshot(vec![term]), ctx.hc.as_ref())
            .unwrap();
        core.tick(&ctx, &j, &Config::default()).unwrap();
        assert!(j.read_after(0).unwrap().iter().any(|e| matches!(e.typed(),Kind::ReconcileFinding { code, .. } if code=="replaced_terminal_retained")));
    }
    #[test]
    fn coordinator_restart_releases_watcher_lock_before_lifecycle_relocks() {
        let root = tempfile::tempdir().unwrap();
        let term: Term = serde_json::from_value(json!({"address":"c1","state":"live","status":{"state":"done","app":null,"kind":null,"msg":null},"status_ms":0,"turn":1,"reported":true,"labels":{"coord.managed":"1","coord.role":"coordinator"},"pid":1,"created_ms":1,"cwd":"/tmp","foreground":null,"reason":null,"name":null})).unwrap();
        let hc = FakeHc::default();
        hc.gets.borrow_mut().push_back(Ok(Some(term.clone())));
        hc.mutations
            .borrow_mut()
            .push_back(Err(anyhow::anyhow!("scripted kill refusal")));
        let ctx = ctx(root.path(), hc);
        let j = ctx.journal().unwrap();
        j.lock()
            .unwrap()
            .append(
                &Actor::Watcher,
                1,
                vec![
                    (
                        None,
                        None,
                        Kind::CoordSessionStarted {
                            role: "coord-main".into(),
                            model: None,
                            terminal: "c1".into(),
                            pid: Some(1),
                            created_ms: Some(1),
                            space: "sp".into(),
                            tab: None,
                            session: None,
                            release: "development".into(),
                            role_hash: String::new(),
                            reason: "up".into(),
                            brief: "/tmp/brief".into(),
                        },
                    ),
                    (
                        None,
                        None,
                        Kind::CoordHandoffRequested {
                            by: "user".into(),
                            note: None,
                        },
                    ),
                ],
            )
            .unwrap();
        let mut core = Core {
            facts: Facts::default(),
            release: "development".into(),
            started_ms: 0,
            loaded_seq: 0,
            pending: BTreeMap::new(),
            retry_after: BTreeMap::new(),
        };
        core.on_line(Line::Snapshot(vec![term]), ctx.hc.as_ref())
            .unwrap();
        let error = core.tick(&ctx, &j, &Config::default()).unwrap_err();
        assert!(format!("{error:#}").contains("scripted kill refusal"));
        assert!(
            j.read_after(0)
                .unwrap()
                .iter()
                .any(|e| e.kind == "coord.session_ended")
        );
    }
    #[test]
    fn journal_restart_precedes_terminal_events_without_ending_live_replacement() {
        // Both watcher-driven handoffs (user/R7) and external --force can publish
        // the new session before this watcher consumes its buffered hc events.
        for reason in ["user", "forced", "idle"] {
            for cached in ["missing", "before_labels", "old_identity", "finished"] {
                let root = tempfile::tempdir().unwrap();
                let live: Term = serde_json::from_value(json!({
                    "address":"c2","state":"live","status":null,"status_ms":0,
                    "turn":0,"reported":false,
                    "labels":{"coord.managed":"1","coord.role":"coordinator"},
                    "pid":2,"created_ms":2,"cwd":"/tmp"
                }))
                .unwrap();
                let hc = FakeHc::default();
                hc.gets.borrow_mut().push_back(Ok(Some(live.clone())));
                hc.gets.borrow_mut().push_back(Ok(None));
                let ctx = ctx(root.path(), hc);
                let j = ctx.journal().unwrap();
                let started = |terminal: &str, pid, reason: &str| Kind::CoordSessionStarted {
                    role: "coord-main".into(),
                    model: None,
                    terminal: terminal.into(),
                    pid: Some(pid),
                    created_ms: Some(pid as u64),
                    space: "sp".into(),
                    tab: Some("same-tab".into()),
                    session: None,
                    release: "development".into(),
                    role_hash: String::new(),
                    reason: reason.into(),
                    brief: "/tmp/brief".into(),
                };
                j.lock()
                    .unwrap()
                    .append(
                        &Actor::Cli,
                        1,
                        vec![
                            (None, None, started("c1", 1, "up")),
                            (
                                None,
                                None,
                                Kind::CoordSessionEnded {
                                    reason: reason.into(),
                                    terminal: "c1".into(),
                                },
                            ),
                            (None, None, started("c2", 2, reason)),
                        ],
                    )
                    .unwrap();
                let mut core = Core {
                    facts: Facts::default(),
                    release: "development".into(),
                    started_ms: 0,
                    loaded_seq: 0,
                    pending: BTreeMap::new(),
                    retry_after: BTreeMap::new(),
                };
                let mut stale = live.clone();
                match cached {
                    "before_labels" => stale.labels.clear(),
                    "old_identity" => stale.pid = Some(1),
                    "finished" => stale.state = "finished".into(),
                    _ => {}
                }
                core.on_line(
                    Line::Snapshot(if cached == "missing" {
                        vec![]
                    } else {
                        vec![stale]
                    }),
                    ctx.hc.as_ref(),
                )
                .unwrap();
                core.tick(&ctx, &j, &Config::default()).unwrap();
                assert_eq!(
                    j.read_state().unwrap().coordinator.terminal.as_deref(),
                    Some("c2"),
                    "{reason}/{cached}"
                );
                assert_eq!(j.read_after(3).unwrap().len(), 0);
                assert_eq!(core.facts.terms["c2"].pid, Some(2));
                // A later, genuinely missing replacement must still be detected.
                core.on_line(
                    Line::Event {
                        terminal: "c2".into(),
                        kind: "removed".into(),
                    },
                    ctx.hc.as_ref(),
                )
                .unwrap();
                core.tick(&ctx, &j, &Config::default()).unwrap();
                assert!(j.read_state().unwrap().coordinator.terminal.is_none());
                assert!(
                    matches!(j.read_after(3).unwrap().as_slice(), [e] if matches!(e.typed(), Kind::CoordSessionEnded { reason, terminal } if reason == "terminal_gone" && terminal == "c2"))
                );
            }
        }
    }
    #[tokio::test]
    async fn watcher_lock_rejects_second_instance() {
        let root = tempfile::tempdir().unwrap();
        let ctx = ctx(root.path(), FakeHc::default());
        ctx.paths.ensure().unwrap();
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&ctx.paths.watcher_lock)
            .unwrap();
        lock.try_lock_exclusive().unwrap();
        assert_eq!(run(ctx).await.unwrap()["state"], "already_running");
    }
}
