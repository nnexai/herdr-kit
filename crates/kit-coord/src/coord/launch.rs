use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use super::Ctx;
use super::adapter::{RunSpec, Term};
use super::ids::{TaskId, WsId};
use super::kinds::{SessionMode, SessionRef};
use anyhow::Result;
use serde_json::json;

pub struct LaunchPlan {
    pub name: String,
    pub role_label: &'static str,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
    pub space: String,
    pub tab: Option<String>,
    pub cwd: PathBuf,
    pub argv: Vec<String>,
    pub session: Option<SessionRef>,
    pub tab_name: String,
    pub space_name: Option<String>,
}
pub struct Launched {
    pub term: Term,
    pub tab: Option<String>,
    pub session: Option<SessionRef>,
}

#[derive(Debug)]
pub struct PostLaunchError(pub anyhow::Error);
impl std::fmt::Display for PostLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "terminal started but post-launch step failed: {}",
            self.0
        )
    }
}
impl std::error::Error for PostLaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub fn launch(ctx: &Ctx, plan: &LaunchPlan) -> Result<Launched> {
    launch_with_started(ctx, plan, |_| Ok(()))
}

pub fn launch_with_started(
    ctx: &Ctx,
    plan: &LaunchPlan,
    started: impl FnOnce(&Term) -> Result<()>,
) -> Result<Launched> {
    let mut labels = vec![("herdr.space".into(), plan.space.clone())];
    if let Some(tab) = &plan.tab {
        labels.push(("herdr.tab".into(), tab.clone()));
    }
    labels.push(("coord.managed".into(), "1".into()));
    labels.push(("coord.role".into(), plan.role_label.into()));
    if let Some(ws) = &plan.ws {
        labels.push(("coord.ws".into(), ws.as_str().into()));
    }
    if let Some(task) = &plan.task {
        labels.push(("coord.task".into(), task.as_str().into()));
    }
    let term = ctx
        .hc
        .run(&RunSpec {
            name: plan.name.clone(),
            cwd: plan.cwd.clone(),
            labels,
            argv: plan.argv.clone(),
        })
        .map_err(|e| {
            if e.downcast_ref::<super::adapter::RunStartedError>()
                .is_some()
            {
                anyhow::Error::new(PostLaunchError(e))
            } else {
                e
            }
        })?;
    started(&term).map_err(PostLaunchError)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let tab = loop {
        if let Ok(snapshot) = ctx.app.ls() {
            if let Some((_, tab)) = snapshot.tab_of(&term.address) {
                break Some(tab.id.clone());
            }
        }
        if Instant::now() >= deadline {
            break plan.tab.clone();
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    if let Some(name) = &plan.space_name {
        ctx.app
            .command("space.rename", json!({"id":plan.space,"name":name}))
            .map_err(PostLaunchError)?;
    }
    if let Some(tab) = &tab {
        ctx.app
            .command("tab.rename", json!({"id":tab,"name":plan.tab_name}))
            .map_err(PostLaunchError)?;
    }
    let session = plan.session.as_ref().map(|s| {
        discover_session(ctx, s, &plan.cwd, term.created_ms, 10_000).unwrap_or_else(|| s.clone())
    });
    Ok(Launched { term, tab, session })
}

pub fn discover_session(
    _ctx: &Ctx,
    s: &SessionRef,
    cwd: &Path,
    since_ms: u64,
    wait_ms: u64,
) -> Option<SessionRef> {
    if s.id.is_some() || matches!(s.mode, SessionMode::None | SessionMode::Preset) {
        return Some(s.clone());
    }
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let found = match s.mode {
            SessionMode::Dir => s
                .dir
                .as_ref()
                .and_then(|dir| fs::read_dir(dir).ok())
                .and_then(|entries| {
                    entries
                        .flatten()
                        .filter_map(|e| {
                            let p = e.path();
                            if p.extension()?.to_str()? != "jsonl" {
                                return None;
                            }
                            let id = p.file_stem()?.to_str()?.split_once('_')?.1.to_owned();
                            Some((e.metadata().ok()?.modified().ok()?, id))
                        })
                        .max_by_key(|(modified, _)| *modified)
                        .map(|(_, id)| id)
                }),
            SessionMode::CodexRollout => {
                let root = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|p| p.join(".codex/sessions"));
                root.and_then(|root| newest_rollout(&root, cwd, since_ms))
            }
            _ => None,
        };
        if let Some(id) = found {
            let mut found = s.clone();
            found.id = Some(id);
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(
            Duration::from_millis(250).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

fn newest_rollout(root: &Path, cwd: &Path, since_ms: u64) -> Option<String> {
    fn visit(
        dir: &Path,
        cwd: &Path,
        since_ms: u64,
        best: &mut Option<(std::time::SystemTime, String)>,
        depth: u8,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 3 {
                visit(&path, cwd, since_ms, best, depth + 1);
                continue;
            }
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
            {
                continue;
            }
            let Some(mtime) = entry.metadata().ok().and_then(|m| m.modified().ok()) else {
                continue;
            };
            if mtime
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                < u128::from(since_ms)
                || best.as_ref().is_some_and(|(t, _)| *t >= mtime)
            {
                continue;
            }
            let Some(line) = fs::File::open(path)
                .ok()
                .and_then(|f| BufReader::new(f).lines().next())
                .and_then(Result::ok)
            else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value["type"] != "session_meta" || value["payload"]["cwd"].as_str() != cwd.to_str() {
                continue;
            }
            if let Some(id) = value["payload"]["id"].as_str() {
                *best = Some((mtime, id.into()));
            }
        }
    }
    let mut best = None;
    visit(root, cwd, since_ms, &mut best, 0);
    best.map(|(_, id)| id)
}

#[cfg(test)]
mod tests {
    use super::super::adapter::{App, AppSnapshot, Hc, SendOutcome};
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    struct Recorder(Rc<RefCell<Vec<RunSpec>>>);
    impl Hc for Recorder {
        fn run(&self, spec: &RunSpec) -> Result<Term> {
            self.0.borrow_mut().push(spec.clone());
            let mut t: Term = serde_json::from_str(include_str!(
                "testdata/hc-get-agent.json"
            ))?;
            t.address = "t-test".into();
            Ok(t)
        }
        fn get(&self, _: &str) -> Result<Option<Term>> {
            unreachable!()
        }
        fn list(&self, _: bool, _: Option<(&str, &str)>) -> Result<Vec<String>> {
            unreachable!()
        }
        fn send(&self, _: &str, _: &str) -> Result<SendOutcome> {
            unreachable!()
        }
        fn send_command(&self, _: &str, _: &str) -> std::process::Command {
            unreachable!()
        }
        fn label(&self, _: &str, _: &[(&str, &str)]) -> Result<()> {
            unreachable!()
        }
        fn kill(&self, _: &str) -> Result<()> {
            unreachable!()
        }
        fn rm(&self, _: &str) -> Result<()> {
            unreachable!()
        }
        fn events_command(&self) -> std::process::Command {
            unreachable!()
        }
    }
    struct AppFake(Rc<RefCell<Vec<String>>>);
    impl App for AppFake {
        fn ls(&self) -> Result<AppSnapshot> {
            Ok(serde_json::from_value(
                json!({"spaces":[{"id":"sp1","name":"x","folder":"/tmp","machine":null,"tabs":[{"id":"tab1","name":"x","terminals":[{"terminal":"t-test","pane":"p"}]}]}]}),
            )?)
        }
        fn command(&self, name: &str, args: serde_json::Value) -> Result<serde_json::Value> {
            self.0.borrow_mut().push(format!("{name}:{args}"));
            Ok(json!({}))
        }
    }
    #[test]
    fn delegate_spec_and_id_based_renames() {
        let tmp = tempfile::tempdir().unwrap();
        let specs = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new(RefCell::new(Vec::new()));
        let ctx = Ctx {
            root: tmp.path().into(),
            paths: super::super::paths::CoordPaths::new(tmp.path()),
            hc: Box::new(Recorder(specs.clone())),
            app: Box::new(AppFake(calls.clone())),
            clock: Box::new(|| 0),
            op: "test".into(),
            cause: None,
        };
        let ws = WsId::parse("ws-abcdef").unwrap();
        let task = TaskId::new(&ws, 1);
        let plan = LaunchPlan {
            name: task.as_str().into(),
            role_label: "delegate",
            ws: Some(ws),
            task: Some(task),
            space: "sp1".into(),
            tab: None,
            cwd: "/tmp".into(),
            argv: vec!["omp".into(), "hello".into()],
            session: Some(SessionRef {
                mode: SessionMode::Preset,
                id: Some("sid".into()),
                dir: None,
            }),
            tab_name: "task".into(),
            space_name: Some("space".into()),
        };
        let launched = launch(&ctx, &plan).unwrap();
        assert_eq!(launched.tab.as_deref(), Some("tab1"));
        let specs = specs.borrow();
        let s = &specs[0];
        assert_eq!(s.name, "ws-abcdef-t1");
        assert_eq!(s.cwd, Path::new("/tmp"));
        assert_eq!(s.argv, ["omp", "hello"]);
        assert_eq!(
            s.labels,
            vec![
                ("herdr.space".into(), "sp1".into()),
                ("coord.managed".into(), "1".into()),
                ("coord.role".into(), "delegate".into()),
                ("coord.ws".into(), "ws-abcdef".into()),
                ("coord.task".into(), "ws-abcdef-t1".into())
            ]
        );
        assert!(calls.borrow()[0].starts_with("space.rename:"));
        assert!(calls.borrow()[1].starts_with("tab.rename:"));
    }
    #[test]
    fn proc_launch_removes_caller_terminal_identity() {
        use super::super::adapter::ProcHc;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        const CHILD: &str = "KIT_COORD_LAUNCH_IDENTITY_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let tmp = tempfile::tempdir().unwrap();
            let exe = tmp.path().join("hc");
            let dump = tmp.path().join("hc-env");
            let fixture = tmp.path().join("terminal.json");
            fs::write(&exe, "#!/bin/sh\nenv > \"$KIT_COORD_TEST_ENV_DUMP\"\ncat \"$KIT_COORD_TEST_TERM_FIXTURE\"\n").unwrap();
            fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
            let mut term: serde_json::Value = serde_json::from_str(include_str!(
                "testdata/hc-get-agent.json"
            ))
            .unwrap();
            term["address"] = json!("t-test");
            fs::write(&fixture, serde_json::to_vec(&term).unwrap()).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("coord::launch::tests::proc_launch_removes_caller_terminal_identity")
                .arg("--nocapture")
                .env(CHILD, "1")
                .env("HC_TERMINAL_ID", "caller-terminal")
                .env("KIT_COORD_HC", &exe)
                .env("KIT_COORD_TEST_ENV_DUMP", &dump)
                .env("KIT_COORD_TEST_TERM_FIXTURE", &fixture)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "subprocess failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let env = fs::read_to_string(&dump).unwrap();
            assert!(
                env.lines()
                    .any(|line| line == "KIT_COORD_LAUNCH_IDENTITY_TEST_CHILD=1")
            );
            assert!(
                !env.lines().any(|line| line.starts_with("HC_TERMINAL_ID=")),
                "hc inherited caller identity: {env}"
            );
            return;
        }

        assert_eq!(std::env::var("HC_TERMINAL_ID").unwrap(), "caller-terminal");
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx {
            root: tmp.path().into(),
            paths: super::super::paths::CoordPaths::new(tmp.path()),
            hc: Box::new(ProcHc::from_env()),
            app: Box::new(AppFake(Rc::new(RefCell::new(Vec::new())))),
            clock: Box::new(|| 0),
            op: "test".into(),
            cause: None,
        };
        let plan = LaunchPlan {
            name: "identity-test".into(),
            role_label: "delegate",
            ws: None,
            task: None,
            space: "sp1".into(),
            tab: None,
            cwd: tmp.path().into(),
            argv: vec!["omp".into()],
            session: None,
            tab_name: "task".into(),
            space_name: None,
        };
        let launched = launch(&ctx, &plan).unwrap();
        assert_eq!(launched.term.address, "t-test");
        assert_eq!(launched.tab.as_deref(), Some("tab1"));
    }
    #[test]
    fn discovers_directory_session_and_preserves_none() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(tmp.path()).unwrap();
        fs::write(tmp.path().join("2026-01-01_abc.jsonl"), "").unwrap();
        let s = SessionRef {
            mode: SessionMode::Dir,
            id: None,
            dir: Some(tmp.path().into()),
        };
        assert_eq!(
            discover_session(&ctx, &s, tmp.path(), 0, 0)
                .unwrap()
                .id
                .as_deref(),
            Some("abc")
        );
        let none = SessionRef {
            mode: SessionMode::None,
            id: None,
            dir: None,
        };
        assert_eq!(discover_session(&ctx, &none, tmp.path(), 0, 0), Some(none));
    }
}
