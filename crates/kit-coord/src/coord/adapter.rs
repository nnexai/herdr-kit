use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::exec::{self, Captured};

#[derive(Clone, Debug)]
pub struct RunSpec {
    pub name: String,
    pub cwd: PathBuf,
    pub labels: Vec<(String, String)>,
    pub argv: Vec<String>,
}

#[derive(Debug)]
pub struct RunStartedError {
    pub address: String,
    pub source: anyhow::Error,
}
impl std::fmt::Display for RunStartedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "terminal {} started but could not be inspected: {}",
            self.address, self.source
        )
    }
}
impl std::error::Error for RunStartedError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Term {
    pub address: String,
    pub state: String,
    pub status: Option<AgentStatus>,
    pub status_ms: Option<u64>,
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub reported: bool,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub pid: Option<u32>,
    pub created_ms: u64,
    pub cwd: String,
    pub foreground: Option<Foreground>,
    pub reason: Option<String>,
    pub name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentStatus {
    pub state: String,
    pub app: Option<String>,
    pub kind: Option<String>,
    pub msg: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Foreground {
    pub pid: i32,
    pub name: String,
    pub cwd: String,
}
impl Term {
    pub fn live(&self) -> bool {
        self.state == "live"
    }
    pub fn word(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.state.as_str())
    }
    pub fn settled(&self) -> bool {
        matches!(self.word(), Some("idle" | "done"))
    }
    pub fn sendable(&self) -> bool {
        self.live() && self.settled() && self.turn >= 1 && self.reported
    }
    pub fn label(&self, k: &str) -> Option<&str> {
        self.labels.get(k).map(String::as_str)
    }
    pub fn managed(&self) -> bool {
        self.label("coord.managed") == Some("1")
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendOutcome {
    pub turn: u64,
    pub started: Option<bool>,
    pub status: Option<String>,
}
pub trait Hc {
    fn run(&self, spec: &RunSpec) -> Result<Term>;
    fn get(&self, terminal: &str) -> Result<Option<Term>>;
    fn list(&self, all: bool, label: Option<(&str, &str)>) -> Result<Vec<String>>;
    fn send(&self, terminal: &str, text: &str) -> Result<SendOutcome>;
    fn steer(&self, terminal: &str, text: &str) -> Result<SendOutcome> {
        let mut command = self.send_command(terminal, text);
        command.args(["--if", "working"]);
        json_reply(command, 90)
    }
    fn send_command(&self, terminal: &str, text: &str) -> Command;
    fn label(&self, terminal: &str, labels: &[(&str, &str)]) -> Result<()>;
    fn kill(&self, terminal: &str) -> Result<()>;
    fn rm(&self, terminal: &str) -> Result<()>;
    fn events_command(&self) -> Command;
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSnapshot {
    pub spaces: Vec<AppSpace>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSpace {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub machine: Option<String>,
    pub tabs: Vec<AppTab>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppTab {
    pub id: String,
    pub name: String,
    pub terminals: Vec<AppTerm>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppTerm {
    pub terminal: String,
    pub pane: String,
}
pub trait App {
    fn ls(&self) -> Result<AppSnapshot>;
    fn command(&self, name: &str, args: Value) -> Result<Value>;
}
impl AppSnapshot {
    pub fn tab_of(&self, terminal: &str) -> Option<(&AppSpace, &AppTab)> {
        self.spaces.iter().find_map(|s| {
            s.tabs
                .iter()
                .find(|t| t.terminals.iter().any(|p| p.terminal == terminal))
                .map(|t| (s, t))
        })
    }
    pub fn space(&self, id: &str) -> Option<&AppSpace> {
        self.spaces.iter().find(|s| s.id == id)
    }
}

fn execute(cmd: Command, seconds: u64) -> Result<Captured> {
    exec::run(cmd, "command", seconds)
}
/// Standard output of a command that must have succeeded.
fn checked(result: Captured) -> Result<Vec<u8>> {
    ensure!(
        result.status.success(),
        "command failed: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(result.stdout)
}
fn succeed(cmd: Command, seconds: u64) -> Result<()> {
    checked(execute(cmd, seconds)?).map(drop)
}
/// The JSON a successful command printed.
fn json_reply<T: serde::de::DeserializeOwned>(cmd: Command, seconds: u64) -> Result<T> {
    json(&checked(execute(cmd, seconds)?)?)
}
fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).context("invalid command JSON")
}

pub struct ProcHc {
    exe: PathBuf,
}
impl ProcHc {
    pub fn from_env() -> Self {
        Self {
            exe: std::env::var_os("KIT_COORD_HC")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| "hc".into()),
        }
    }
    fn cmd(&self, verb: &str) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.arg(verb).env_remove("HC_TERMINAL_ID");
        cmd
    }
}
impl Hc for ProcHc {
    fn run(&self, spec: &RunSpec) -> Result<Term> {
        ensure!(!spec.argv.is_empty(), "empty run argv");
        let mut cmd = self.cmd("run");
        cmd.args(["--json", "--name", &spec.name, "--cwd"])
            .arg(&spec.cwd)
            .args(["--no-parent", "--keep", "--bg"]);
        for (key, value) in &spec.labels {
            cmd.arg("--label").arg(format!("{key}={value}"));
        }
        cmd.arg("--").args(&spec.argv);
        #[derive(Deserialize)]
        struct RunReply {
            address: String,
        }
        let reply: RunReply = json_reply(cmd, 30)?;
        let term = self
            .get(&reply.address)
            .and_then(|term| term.ok_or_else(|| anyhow::anyhow!("terminal not found")));
        term.map_err(|source| {
            RunStartedError {
                address: reply.address,
                source,
            }
            .into()
        })
    }
    fn get(&self, terminal: &str) -> Result<Option<Term>> {
        let mut cmd = self.cmd("get");
        cmd.args(["--json", terminal]);
        let result = execute(cmd, 15)?;
        let mentions = |bytes: &[u8]| String::from_utf8_lossy(bytes).contains("not_found");
        if !result.status.success() && (mentions(&result.stdout) || mentions(&result.stderr)) {
            return Ok(None);
        }
        Ok(Some(json(&checked(result)?)?))
    }
    fn list(&self, all: bool, label: Option<(&str, &str)>) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct List {
            terminals: Vec<Address>,
        }
        #[derive(Deserialize)]
        struct Address {
            address: String,
        }
        let mut cmd = self.cmd("ls");
        cmd.arg("--json");
        if all {
            cmd.arg("--all");
        }
        if let Some((key, value)) = label {
            cmd.arg("--label").arg(format!("{key}={value}"));
        }
        let list: List = json_reply(cmd, 15)?;
        Ok(list.terminals.into_iter().map(|t| t.address).collect())
    }
    fn send(&self, terminal: &str, text: &str) -> Result<SendOutcome> {
        json_reply(self.send_command(terminal, text), 90)
    }
    fn send_command(&self, terminal: &str, text: &str) -> Command {
        let mut cmd = self.cmd("send");
        cmd.args(["--json", terminal, text]);
        cmd
    }
    fn label(&self, terminal: &str, labels: &[(&str, &str)]) -> Result<()> {
        let mut cmd = self.cmd("label");
        cmd.args(["--json", terminal]);
        for (key, value) in labels {
            cmd.arg(format!("{key}={value}"));
        }
        succeed(cmd, 15)
    }
    fn kill(&self, terminal: &str) -> Result<()> {
        let mut cmd = self.cmd("kill");
        cmd.args(["--json", terminal]);
        succeed(cmd, 15)
    }
    fn rm(&self, terminal: &str) -> Result<()> {
        let mut cmd = self.cmd("rm");
        cmd.args(["--json", terminal]);
        succeed(cmd, 15)
    }
    fn events_command(&self) -> Command {
        let mut cmd = self.cmd("events");
        cmd.arg("--json");
        cmd
    }
}
pub struct ProcApp {
    exe: PathBuf,
}
impl ProcApp {
    pub fn from_env() -> Self {
        Self {
            exe: std::env::var_os("KIT_COORD_HERDR_APP")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| "herdr-app".into()),
        }
    }
}
impl App for ProcApp {
    fn ls(&self) -> Result<AppSnapshot> {
        let mut cmd = Command::new(&self.exe);
        cmd.arg("ls");
        json_reply(cmd, 15)
    }
    fn command(&self, name: &str, args: Value) -> Result<Value> {
        ensure!(args.is_object(), "app command arguments must be an object");
        let mut cmd = Command::new(&self.exe);
        cmd.arg("do").arg(name).arg(serde_json::to_string(&args)?);
        let value: Value = json_reply(cmd, 15)?;
        ensure!(value.get("error").is_none(), "app command failed: {value}");
        Ok(value)
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    #[derive(Default)]
    pub struct FakeHc {
        pub calls: RefCell<Vec<String>>,
        pub runs: RefCell<VecDeque<Result<Term>>>,
        pub gets: RefCell<VecDeque<Result<Option<Term>>>>,
        pub lists: RefCell<VecDeque<Result<Vec<String>>>>,
        pub sends: RefCell<VecDeque<Result<SendOutcome>>>,
        pub mutations: RefCell<VecDeque<Result<()>>>,
    }
    impl FakeHc {
        fn mutation(&self, call: String) -> Result<()> {
            self.calls.borrow_mut().push(call);
            self.mutations.borrow_mut().pop_front().unwrap_or(Ok(()))
        }
    }
    impl Hc for FakeHc {
        fn run(&self, spec: &RunSpec) -> Result<Term> {
            self.calls.borrow_mut().push(format!("run {}", spec.name));
            self.runs
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("unscripted run")))
        }
        fn get(&self, terminal: &str) -> Result<Option<Term>> {
            self.calls.borrow_mut().push(format!("get {terminal}"));
            self.gets
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("unscripted get")))
        }
        fn list(&self, all: bool, label: Option<(&str, &str)>) -> Result<Vec<String>> {
            self.calls
                .borrow_mut()
                .push(format!("list {all} {label:?}"));
            self.lists
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("unscripted list")))
        }
        fn send(&self, terminal: &str, text: &str) -> Result<SendOutcome> {
            self.calls
                .borrow_mut()
                .push(format!("send {terminal} {text}"));
            self.sends
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("unscripted send")))
        }
        fn send_command(&self, terminal: &str, text: &str) -> Command {
            self.calls
                .borrow_mut()
                .push(format!("send {terminal} {text}"));
            let result = self
                .sends
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("unscripted send")));
            let mut cmd = Command::new("sh");
            match result {
                Ok(outcome) => {
                    cmd.args([
                        "-c",
                        "printf '%s' \"$1\"",
                        "sh",
                        &serde_json::to_string(&outcome).unwrap(),
                    ]);
                }
                Err(err) => {
                    cmd.args([
                        "-c",
                        "printf '%s' \"$1\" >&2; exit 1",
                        "sh",
                        &format!("{err:#}"),
                    ]);
                }
            }
            cmd
        }
        fn label(&self, terminal: &str, labels: &[(&str, &str)]) -> Result<()> {
            self.mutation(format!("label {terminal} {labels:?}"))
        }
        fn kill(&self, terminal: &str) -> Result<()> {
            self.mutation(format!("kill {terminal}"))
        }
        fn rm(&self, terminal: &str) -> Result<()> {
            self.mutation(format!("rm {terminal}"))
        }
        fn events_command(&self) -> Command {
            Command::new("false")
        }
    }
    #[derive(Default)]
    pub struct FakeApp {
        pub calls: RefCell<Vec<String>>,
        pub snapshots: RefCell<VecDeque<Result<AppSnapshot>>>,
        pub responses: RefCell<VecDeque<Result<Value>>>,
    }
    impl App for FakeApp {
        fn ls(&self) -> Result<AppSnapshot> {
            self.calls.borrow_mut().push("ls".into());
            self.snapshots
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("unscripted ls")))
        }
        fn command(&self, name: &str, args: Value) -> Result<Value> {
            self.calls.borrow_mut().push(format!("{name} {args}"));
            self.responses
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("unscripted command")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn send_uses_nonblocking_unconditional_command() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("hc");
        std::fs::write(&exe, "#!/bin/sh\n[ \"$#\" = 4 ] && [ \"$1\" = send ] && [ \"$2\" = --json ] && [ \"$3\" = t1 ] && [ \"$4\" = 'hello world' ] || exit 1\nprintf '%s' '{\"turn\":3,\"started\":true,\"status\":\"working\"}'\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let reply = ProcHc { exe }.send("t1", "hello world").unwrap();
        assert_eq!(reply.turn, 3);
        assert_eq!(reply.started, Some(true));
    }
    #[test]
    fn terminal_readiness_and_labels() {
        let mut t: Term = serde_json::from_str(include_str!("testdata/hc-get-agent.json")).unwrap();
        t.state = "live".into();
        t.status = Some(AgentStatus {
            state: "idle".into(),
            app: None,
            kind: None,
            msg: None,
        });
        t.turn = 1;
        t.reported = true;
        assert!(t.sendable());
        t.reported = false;
        assert!(!t.sendable());
        t.reported = true;
        t.turn = 0;
        assert!(!t.sendable());
        t.turn = 1;
        t.status.as_mut().unwrap().state = "done".into();
        assert!(t.sendable());
        t.state = "finished".into();
        assert!(!t.sendable());
        t.labels.insert("coord.managed".into(), "1".into());
        assert!(t.managed());
    }
    #[test]
    fn snapshot_lookup_uses_nested_terminals() {
        let s: AppSnapshot =
            serde_json::from_str(include_str!("testdata/herdr-app-ls.json")).unwrap();
        let (space, tab) = s.tab_of("t7").unwrap();
        assert_eq!(s.space(&space.id).unwrap().id, space.id);
        assert_eq!(tab.terminals[0].terminal, "t7");
        assert!(s.tab_of("missing").is_none());
    }
    #[test]
    fn events_command_never_inherits_caller_identity() {
        let cmd = ProcHc::from_env().events_command();
        assert!(
            cmd.get_envs()
                .any(|(k, v)| k == "HC_TERMINAL_ID" && v.is_none())
        );
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["events", "--json"]);
    }
}
