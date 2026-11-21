use crate::model::{
    CoordinatorSummary, DeckSnapshot, EventEffect, HcState, LimitsSummary, RawTerm, build_snapshot,
    parse_coordinator_status, parse_limits,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, Notify, watch};

const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 16_384;

#[derive(Clone, Debug)]
pub struct SourceConfig {
    pub data_dir: PathBuf,
    pub hc: PathBuf,
    pub herdr_app: PathBuf,
    pub kit_coord: PathBuf,
    pub kit_usage: PathBuf,
}

#[derive(Deserialize)]
struct BundleConfig {
    data_dir: Option<PathBuf>,
}

impl SourceConfig {
    pub fn resolve(cli_data_dir: Option<PathBuf>) -> Result<Self> {
        let env_data_dir = std::env::var_os("KIT_DECK_DATA_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let plugin_dir = std::env::var_os("KIT_DECK_PLUGIN_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let exe_path = std::env::current_exe().ok();
        Self::resolve_with(
            cli_data_dir,
            env_data_dir,
            plugin_dir.as_deref(),
            exe_path.as_deref(),
        )
    }

    pub(crate) fn resolve_with(
        cli_data_dir: Option<PathBuf>,
        env_data_dir: Option<PathBuf>,
        plugin_dir: Option<&Path>,
        exe_path: Option<&Path>,
    ) -> Result<Self> {
        let data_dir = match cli_data_dir.or(env_data_dir) {
            Some(dir) => kit_core::paths::data_root(Some(dir))?,
            None => match bundle_data_dir(plugin_dir, exe_path)? {
                Some(dir) => kit_core::paths::data_root(Some(dir))?,
                None => kit_core::paths::data_root(None)?,
            },
        };
        Ok(Self {
            hc: discover_binary("KIT_DECK_HC", "hc", &data_dir),
            herdr_app: discover_binary("KIT_DECK_HERDR_APP", "herdr-app", &data_dir),
            kit_coord: discover_binary("KIT_DECK_COORD", "kit-coord", &data_dir),
            kit_usage: discover_binary("KIT_DECK_USAGE", "kit-usage", &data_dir),
            data_dir,
        })
    }
}

fn bundle_data_dir(
    plugin_dir: Option<&Path>,
    exe_path: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let cfg_path = match plugin_dir {
        Some(dir) => dir.join("config.json"),
        None => match exe_path.and_then(|exe| exe.parent()).and_then(Path::parent) {
            Some(dir) => dir.join("config.json"),
            None => return Ok(None),
        },
    };
    let bytes = match std::fs::read(&cfg_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", cfg_path.display()));
        }
    };
    let parsed: BundleConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid plugin config {}", cfg_path.display()))?;
    Ok(parsed.data_dir.filter(|p| !p.as_os_str().is_empty()))
}

fn discover_binary(env_name: &str, name: &str, data_dir: &Path) -> PathBuf {
    if let Some(override_val) = std::env::var_os(env_name).filter(|s| !s.is_empty()) {
        return PathBuf::from(override_val);
    }
    let mut dirs = vec![data_dir.join("bin")];
    if let Some(path_var) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path_var).filter(|p| !p.as_os_str().is_empty()));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.extend([home.join(".local/bin"), home.join(".cargo/bin")]);
    }
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/home/linuxbrew/.linuxbrew/bin",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    #[cfg(unix)]
    let found = {
        use std::os::unix::fs::PermissionsExt;
        dirs.into_iter().map(|d| d.join(name)).find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    };
    #[cfg(not(unix))]
    let found = dirs
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file()));
    found
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_else(|| PathBuf::from(name))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub async fn run_bounded(
    command: Command,
    timeout: Duration,
    max_stdout: u64,
) -> Result<Vec<u8>> {
    let output =
        kit_core::process::run_bounded(command, timeout, max_stdout, MAX_STDERR_BYTES).await?;
    if !output.status.success() {
        bail!(
            "command exited {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

async fn fetch_hc_term(cfg: &SourceConfig, terminal: &str) -> Option<RawTerm> {
    let mut cmd = Command::new(&cfg.hc);
    cmd.args(["get", "--json", terminal]);
    let bytes = run_bounded(cmd, Duration::from_secs(4), MAX_JSON_BYTES)
        .await
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn fetch_hc_snapshot_once(cfg: &SourceConfig) -> HcState {
    let mut state = HcState::default();
    let mut events_cmd = Command::new(&cfg.hc);
    events_cmd
        .args(["events", "--json"])
        .env_remove("HC_TERMINAL_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Ok(mut child) = events_cmd.spawn() {
        if let Some(stdout) = child.stdout.take() {
            let mut reader = BufReader::new(stdout).lines();
            if let Ok(Ok(Some(line))) =
                tokio::time::timeout(Duration::from_secs(3), reader.next_line()).await
            {
                let _ = state.apply_event_line(&line);
            }
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
        if state.connected {
            return state;
        }
    }

    let mut ls_cmd = Command::new(&cfg.hc);
    ls_cmd.args(["ls", "--json"]);
    if let Ok(bytes) = run_bounded(ls_cmd, Duration::from_secs(4), MAX_JSON_BYTES).await {
        let _ = state.apply_event_line(&String::from_utf8_lossy(&bytes));
    }
    state
}

async fn fetch_coord(cfg: &SourceConfig) -> (Option<CoordinatorSummary>, Option<Vec<u8>>) {
    let mut status_cmd = Command::new(&cfg.kit_coord);
    status_cmd
        .arg("--data-dir")
        .arg(&cfg.data_dir)
        .args(["status", "--json"]);
    let mut board_cmd = Command::new(&cfg.kit_coord);
    board_cmd
        .arg("--data-dir")
        .arg(&cfg.data_dir)
        .args(["board", "--json"]);
    let (status_res, board_res) = tokio::join!(
        run_bounded(status_cmd, Duration::from_secs(10), MAX_JSON_BYTES),
        run_bounded(board_cmd, Duration::from_secs(10), MAX_JSON_BYTES),
    );
    let summary = status_res.ok().and_then(|b| parse_coordinator_status(&b));
    let board = board_res.ok();
    (summary, board)
}

async fn fetch_usage(cfg: &SourceConfig, agents_working: bool) -> Option<LimitsSummary> {
    let mut cmd = Command::new(&cfg.kit_usage);
    cmd.arg("--data-dir")
        .arg(&cfg.data_dir)
        .args(["snapshot", "--json"]);
    if agents_working {
        cmd.arg("--agents-working");
    }
    let bytes = run_bounded(cmd, Duration::from_secs(45), MAX_JSON_BYTES)
        .await
        .ok()?;
    parse_limits(&bytes)
}

pub async fn collect_once(cfg: &SourceConfig) -> DeckSnapshot {
    let hc = fetch_hc_snapshot_once(cfg).await;
    let working = hc
        .agents()
        .iter()
        .any(|a| a.status == crate::model::AgentStateKind::Working);
    let ((coord, board), limits) = tokio::join!(fetch_coord(cfg), fetch_usage(cfg, working));
    build_snapshot(now_ms(), &hc, coord.as_ref(), board.as_deref(), limits)
}

pub(crate) struct SharedRawState {
    hc: HcState,
    coordinator: Option<CoordinatorSummary>,
    coord_board: Option<Vec<u8>>,
    pub(crate) limits: Option<LimitsSummary>,
}

impl Default for SharedRawState {
    fn default() -> Self {
        Self {
            hc: HcState::default(),
            coordinator: None,
            coord_board: None,
            limits: None,
        }
    }
}

impl SharedRawState {
    pub(crate) fn apply_usage_poll(&mut self, fetched: Option<LimitsSummary>) -> bool {
        match fetched {
            Some(limits) => {
                self.limits = Some(limits);
                true
            }
            None => {
                if let Some(limits) = &mut self.limits {
                    limits.mark_stale();
                    true
                } else {
                    false
                }
            }
        }
    }

    pub(crate) fn snapshot(&self) -> DeckSnapshot {
        build_snapshot(
            now_ms(),
            &self.hc,
            self.coordinator.as_ref(),
            self.coord_board.as_deref(),
            self.limits.clone(),
        )
    }
}

pub fn start_background(cfg: SourceConfig) -> (watch::Receiver<Arc<DeckSnapshot>>, Arc<Notify>) {
    let shared = Arc::new(Mutex::new(SharedRawState::default()));
    let (tx, rx) = watch::channel(Arc::new(DeckSnapshot {
        schema_version: 1,
        ..DeckSnapshot::default()
    }));
    let manual_refresh = Arc::new(Notify::new());
    let coord_wake = Arc::new(Notify::new());
    let usage_wake = Arc::new(Notify::new());

    // Fan-out manual refresh to coordinator and usage pollers.
    {
        let manual = manual_refresh.clone();
        let coord_w = coord_wake.clone();
        let usage_w = usage_wake.clone();
        tokio::spawn(async move {
            loop {
                manual.notified().await;
                coord_w.notify_one();
                usage_w.notify_one();
            }
        });
    }

    // 1) Persistent hc events --json stream.
    {
        let cfg = cfg.clone();
        let shared = shared.clone();
        let tx = tx.clone();
        let coord_wake = coord_wake.clone();
        let usage_wake = usage_wake.clone();
        tokio::spawn(async move {
            let mut backoff_secs = 1_u64;
            loop {
                let mut cmd = Command::new(&cfg.hc);
                cmd.args(["events", "--json"])
                    .env_remove("HC_TERMINAL_ID")
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                match cmd.spawn() {
                    Ok(mut child) => {
                        if let Some(stdout) = child.stdout.take() {
                            let mut lines = BufReader::new(stdout).lines();
                            let mut had_line = false;
                            let mut prev_working = false;
                            while let Ok(Some(line)) = lines.next_line().await {
                                had_line = true;
                                backoff_secs = 1;
                                let effect = {
                                    let mut guard = shared.lock().await;
                                    let effect = guard.hc.apply_event_line(&line);
                                    if effect != EventEffect::Ignored {
                                        let snap = guard.snapshot();
                                        let now_working = snap.counts.working > 0;
                                        if !prev_working && now_working {
                                            usage_wake.notify_one();
                                        }
                                        prev_working = now_working;
                                        let _ = tx.send(Arc::new(snap));
                                    }
                                    effect
                                };
                                if let EventEffect::Updated {
                                    need_get,
                                    wake_coord,
                                } = effect
                                {
                                    if let Some(addr) = need_get {
                                        if let Some(term) = fetch_hc_term(&cfg, &addr).await {
                                            let mut guard = shared.lock().await;
                                            guard.hc.upsert_term(term);
                                            let snap = guard.snapshot();
                                            let _ = tx.send(Arc::new(snap));
                                        }
                                    }
                                    if wake_coord {
                                        coord_wake.notify_one();
                                    }
                                }
                            }
                            if had_line {
                                let mut guard = shared.lock().await;
                                guard.hc.clear_disconnected();
                                let _ = tx.send(Arc::new(guard.snapshot()));
                            }
                        }
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                    Err(_) => {
                        let mut guard = shared.lock().await;
                        guard.hc.clear_disconnected();
                        let _ = tx.send(Arc::new(guard.snapshot()));
                    }
                }
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(15);
            }
        });
    }

    // 2) Coordinator status & board poller (woken on hc task/coord events + 15s fallback).
    {
        let cfg = cfg.clone();
        let shared = shared.clone();
        let tx = tx.clone();
        let coord_wake = coord_wake.clone();
        tokio::spawn(async move {
            loop {
                let (summary, board) = fetch_coord(&cfg).await;
                {
                    let mut guard = shared.lock().await;
                    guard.coordinator = summary;
                    guard.coord_board = board;
                    let _ = tx.send(Arc::new(guard.snapshot()));
                }
                tokio::select! {
                    _ = coord_wake.notified() => {
                        // Coalesce bursty events slightly.
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(15)) => {}
                }
            }
        });
    }

    // 3) Subscription limits poller (reuses kit-usage cache/lock; faster follow-up while collecting).
    {
        tokio::spawn(async move {
            let mut collecting_since: Option<tokio::time::Instant> = None;
            loop {
                let working = {
                    let guard = shared.lock().await;
                    guard
                        .hc
                        .agents()
                        .iter()
                        .any(|a| a.status == crate::model::AgentStateKind::Working)
                };
                let fetched = fetch_usage(&cfg, working).await;
                if let Some(ref limits) = fetched {
                    if limits.collecting {
                        collecting_since.get_or_insert_with(tokio::time::Instant::now);
                    } else {
                        collecting_since = None;
                    }
                } else {
                    collecting_since = None;
                }
                let mut guard = shared.lock().await;
                if guard.apply_usage_poll(fetched) {
                    let _ = tx.send(Arc::new(guard.snapshot()));
                }
                let followup =
                    collecting_since.is_some_and(|t| t.elapsed() < Duration::from_secs(45));
                let wait = Duration::from_secs(if followup { 3 } else { 60 });
                tokio::select! {
                    _ = usage_wake.notified() => {}
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        });
    }

    (rx, manual_refresh)
}
