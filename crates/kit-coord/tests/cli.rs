use kit_core::git_env;

use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Harness {
    tmp: tempfile::TempDir,
    hc: PathBuf,
    app: PathBuf,
    /// A repository exported to kit-coord the way a git hook exports its own.
    inherited_git: Option<PathBuf>,
}
/// Programs the fake `hc` script and kit-coord itself run, linked into the
/// isolated test `PATH`.
const TOOLS: [&str; 9] = [
    "basename", "mkdir", "ls", "grep", "sort", "head", "cat", "rm", "git",
];
/// Agent programs the tests pretend are installed: the known harnesses and
/// the `runner` used by test role files.
const PROGRAMS: [&str; 6] = ["omp", "agy", "claude", "codex", "copilot", "runner"];

impl Harness {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let hc = tmp.path().join("hc");
        let app = tmp.path().join("herdr-app");
        for path in [&hc, &app] {
            fs::copy(
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake/cli.sh"),
                path,
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let search = std::env::var_os("PATH").unwrap();
        for tool in TOOLS {
            let found = std::env::split_paths(&search)
                .map(|dir| dir.join(tool))
                .find(|p| p.is_file())
                .unwrap_or_else(|| panic!("{tool} not found on PATH"));
            std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
        }
        for program in PROGRAMS {
            fs::write(bin.join(program), "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(bin.join(program), fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            tmp,
            hc,
            app,
            inherited_git: None,
        }
    }
    fn root(&self) -> &Path {
        self.tmp.path()
    }
    /// Copy the multi-harness test role set into the data dir's local roles.
    fn install_roles(&self) {
        install_roles(
            self,
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/roles"),
        );
    }
    /// Make `program` uninstalled on the test `PATH`.
    fn uninstall(&self, program: &str) {
        fs::remove_file(self.root().join("bin").join(program)).unwrap();
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kit-coord"));
        c.env("HOME", self.root())
            .env("PATH", self.root().join("bin"))
            .env("FAKE_DIR", self.root().join("fake"))
            .env("KIT_COORD_HC", &self.hc)
            .env("KIT_COORD_HERDR_APP", &self.app)
            .env_remove("HC_TERMINAL_ID")
            .arg("--data-dir")
            .arg(self.root().join("data"))
            .args(args);
        if let Some(repo) = &self.inherited_git {
            c.env("GIT_DIR", repo.join(".git"))
                .env("GIT_WORK_TREE", repo)
                .env("GIT_INDEX_FILE", repo.join(".git/index"));
        }
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn queue(&self, name: &str, key: &str, value: &Value) {
        let dir = self.root().join("fake").join(name).join("responses");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{key}.default.out")), value.to_string()).unwrap();
    }
    fn calls(&self, name: &str) -> String {
        fs::read_to_string(self.root().join("fake").join(name).join("calls.tsv"))
            .unwrap_or_default()
    }
    fn queued(&self, name: &str, key: &str, n: usize, value: &Value, code: i32) {
        let dir = self
            .root()
            .join("fake")
            .join(name)
            .join("responses")
            .join(key);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{n:03}.out")), value.to_string()).unwrap();
        if code != 0 {
            fs::write(dir.join(format!("{n:03}.code")), code.to_string()).unwrap();
        }
    }
    fn journal(&self) -> Vec<Value> {
        let p = self.root().join("data/coord/journal");
        let mut entries = Vec::new();
        if let Ok(files) = fs::read_dir(p) {
            for f in files.flatten() {
                if f.path().extension().is_some_and(|e| e == "jsonl") {
                    for line in fs::read_to_string(f.path()).unwrap().lines() {
                        entries.push(serde_json::from_str::<Value>(line).unwrap());
                    }
                }
            }
        }
        entries.sort_by_key(|v| v["seq"].as_u64());
        entries
    }
    fn open(&self) -> String {
        self.ok(&["ws", "open", "--title", "Work", "--target", "delivery"])["ws"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

fn install_roles(h: &Harness, from: &str) {
    let roles = h.root().join("data/coord/roles");
    fs::create_dir_all(&roles).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        fs::copy(&path, roles.join(path.file_name().unwrap())).unwrap();
    }
}

#[test]
fn workstream_views_insights_and_cause() {
    let h = Harness::new();
    let ws = h.open();
    let entries = h.journal();
    assert_eq!(entries[0]["kind"], "ws.opened");
    assert_eq!(entries[0]["actor"], "cli");
    h.ok(&[
        "--cause", "1", "decide", "--what", "choice", "--why", "reason", "--tier", "digest",
        "--ws", &ws,
    ]);
    let board = h.ok(&["board", "--json"]);
    assert_eq!(board["schema_version"], 1);
    assert_eq!(board["workstreams"].as_array().unwrap().len(), 1);
    let log = h.ok(&["log", "--json", "--limit", "2", "--before-seq", "5"]);
    assert_eq!(log["entries"].as_array().unwrap().len(), 2);
    let explain = h.ok(&["explain", &ws, "--json"]);
    assert_eq!(explain["kind"], "ws");
    let inbox = h.ok(&["inbox"]);
    let last = inbox["last_seq"].as_u64().unwrap();
    h.ok(&["inbox", "ack", &last.to_string()]);
    assert_eq!(h.ok(&["inbox"])["cursor"], last);
    h.ok(&["rate", &ws, "good"]);
    let stats = h.ok(&["stats", "--json"]);
    assert_eq!(stats["schema_version"], 1);
    assert!(stats["workstreams"].is_array());
    assert!(stats["by_role"].is_array());
    assert_eq!(h.journal()[1]["cause"], 1);
    h.ok(&["ws", "close", &ws, "--summary", "done"]);
    let released = h.ok(&["release", &ws]);
    assert_eq!(released["cleaned"], true);
    assert!(
        h.ok(&["board", "--json"])["workstreams"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        h.ok(&["board", "--json", "--all"])["workstreams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn refusals_and_delegate_permissions() {
    let h = Harness::new();
    let ws = h.open();
    let o = h.run(&[
        "task",
        "start",
        "--ws",
        &ws,
        "--rung",
        "3",
        "--reason",
        "work",
        "--brief",
        "-",
        "--role",
        "coord-default",
        "--space",
        "x",
    ]);
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stderr).unwrap()["error"]["code"],
        "ladder_reason_missing"
    );
    assert!(h.calls("hc").is_empty());
    assert_eq!(h.journal().len(), 1);
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["labels"] =
        json!({"coord.managed":"1","coord.role":"delegate","coord.task":format!("{ws}-t1")});
    h.queue("hc", "get", &term);
    let o = h
        .command(&["rate", &ws, "good"])
        .env("HC_TERMINAL_ID", "t9")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stderr).unwrap()["error"]["code"],
        "forbidden_caller"
    );
    term["labels"] = json!({});
    h.queue("hc", "get", &term);
    let o = h
        .command(&["report", "done", "--summary", "done"])
        .env("HC_TERMINAL_ID", "t9")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(
        serde_json::from_slice::<Value>(&o.stderr).unwrap()["error"]["code"],
        "not_a_delegate"
    );
}

#[test]
fn concurrent_appends_have_unique_contiguous_sequences() {
    let h = Harness::new();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..20 {
                    let status = h
                        .command(&["decide", "--what", "x", "--why", "y", "--tier", "silent"])
                        .stdout(std::process::Stdio::null())
                        .status()
                        .unwrap();
                    assert!(status.success());
                }
            });
        }
    });
    let entries = h.journal();
    assert_eq!(entries.len(), 160);
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e["seq"], (i + 1) as u64);
        assert_eq!(e["op"].as_str().unwrap().len(), 8);
    }
}

#[test]
fn delegate_start_report_and_send_readiness() {
    let h = Harness::new();
    let ws = h.open();
    let task = format!("{ws}-t1");
    let brief = h.root().join("brief");
    fs::write(&brief, "Complete the task").unwrap();
    let roles = h.root().join("data/coord/roles");
    fs::create_dir_all(&roles).unwrap();
    fs::write(roles.join("test.md"),"---\nharness: omp\nstart: runner {session_id} {brief}\nsession: preset\n---\nInstructions\n").unwrap();
    let space = json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]});
    h.queue("herdr-app", "ls", &space);
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t7");
    term["state"] = json!("live");
    term["turn"] = json!(0);
    term["labels"] = json!({"herdr.space":"existing","coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task});
    h.queue("hc","run",&json!({"address":"t7","agent":"omp","cwd":h.root(),"name":"delegate","state":"live","turn":0}));
    h.queue("hc", "get", &term);
    let mut person = term.clone();
    person["labels"] = json!({});
    h.queued("hc", "get", 1, &person, 0);
    let output = h
        .command(&[
            "--cause",
            "1",
            "task",
            "start",
            "--ws",
            &ws,
            "--rung",
            "3",
            "--reason",
            "work",
            "--brief",
            brief.to_str().unwrap(),
            "--role",
            "test",
            "--space",
            "existing",
            "--reject",
            "1=no",
            "--reject",
            "2=no",
        ])
        .env("HC_TERMINAL_ID", "person")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let started: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(started["task"], task);
    assert_eq!(h.ok(&["explain", &task, "--json"])["kind"], "task");
    let entries = h.journal();
    let kinds: Vec<_> = entries[1..]
        .iter()
        .map(|v| v["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "task.placed",
            "res.adopted",
            "res.intent",
            "res.created",
            "task.started",
            "task.launch_details"
        ]
    );
    let op = &entries[1]["op"];
    assert!(
        entries[1..]
            .iter()
            .all(|e| &e["op"] == op && e["cause"] == 1)
    );
    assert_eq!(entries[1]["data"]["session"]["id"], started["session"]);
    let calls = h.calls("hc");
    let run = calls.lines().find(|s| s.contains("\trun\t")).unwrap();
    assert!(
        calls
            .lines()
            .filter(|line| line.starts_with("HC_TERMINAL_ID="))
            .all(|line| line.starts_with("HC_TERMINAL_ID=<unset>\t"))
    );
    let argv: Vec<_> = run.split('\t').collect();
    assert_eq!(
        &argv[..21],
        &[
            "HC_TERMINAL_ID=<unset>",
            "run",
            "--json",
            "--name",
            &task,
            "--cwd",
            h.root().to_str().unwrap(),
            "--no-parent",
            "--keep",
            "--bg",
            "--label",
            "herdr.space=existing",
            "--label",
            "coord.managed=1",
            "--label",
            "coord.role=delegate",
            "--label",
            &format!("coord.ws={ws}"),
            "--label",
            &format!("coord.task={task}"),
            "--"
        ]
    );
    assert_eq!(argv[21], "runner");
    assert_eq!(argv[22], started["session"].as_str().unwrap());
    let report = h
        .command(&["report", "done", "--summary", "done"])
        .env("HC_TERMINAL_ID", "t7")
        .output()
        .unwrap();
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let last = h.journal().pop().unwrap();
    assert_eq!(last["kind"], "task.reported");
    assert_eq!(last["actor"], format!("task:{task}"));
    let send = h.run(&["task", "send", &task, "--text", "hello"]);
    assert_eq!(send.status.code(), Some(4));
    assert_eq!(
        serde_json::from_slice::<Value>(&send.stderr).unwrap()["error"]["code"],
        "agent_not_ready"
    );
    assert!(!h.calls("hc").lines().any(|l| l.contains("\tsend\t")));
}

#[test]
fn task_send_steers_working_omp_after_readiness_but_not_startup_or_permission_prompts() {
    let h = Harness::new();
    let ws = h.open();
    let task = format!("{ws}-t1");
    let brief = h.root().join("brief");
    fs::write(&brief, "Complete the task").unwrap();
    let roles = h.root().join("data/coord/roles");
    fs::create_dir_all(&roles).unwrap();
    fs::write(roles.join("test.md"), "---\nharness: omp\nstart: runner {session_id} {brief}\nsession: preset\n---\nInstructions\n").unwrap();
    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]}));
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t7");
    term["labels"] = json!({"herdr.space":"existing","coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task});
    term["status"]["state"] = json!("working");
    term["turn"] = json!(0);
    h.queue("hc", "run", &json!({"address":"t7"}));
    h.queue("hc", "get", &term);
    h.ok(&[
        "task",
        "start",
        "--ws",
        &ws,
        "--rung",
        "3",
        "--reason",
        "work",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "test",
        "--space",
        "existing",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    let startup = h.run(&["task", "send", &task, "--text", "not yet"]);
    assert_eq!(startup.status.code(), Some(4));
    assert!(!h.calls("hc").lines().any(|line| line.contains("\tsend\t")));
    term["turn"] = json!(1);
    h.queue("hc", "get", &term);
    h.queue(
        "hc",
        "send",
        &json!({"turn":1,"started":false,"status":"working"}),
    );
    let steered = h.ok(&["task", "send", &task, "--text", "also check the tests"]);
    assert_eq!(steered["delivery"], "steering");
    assert_eq!(steered["steered"], true);
    let calls = h.calls("hc");
    let send = calls
        .lines()
        .find(|line| line.contains("\tsend\t"))
        .unwrap();
    assert!(send.contains("also check the tests"));
    assert!(calls.contains("\t--if\tworking\n"), "{calls}");
    assert!(!calls.contains("--force"));
    term["status"]["state"] = json!("blocked");
    h.queue("hc", "get", &term);
    let blocked = h.run(&["task", "send", &task, "--text", "never answer this prompt"]);
    assert_eq!(blocked.status.code(), Some(3));
    assert_eq!(
        serde_json::from_slice::<Value>(&blocked.stderr).unwrap()["error"]["code"],
        "agent_blocked"
    );
    assert_eq!(
        h.calls("hc")
            .lines()
            .filter(|line| line.contains("\tsend\t"))
            .count(),
        1
    );
    term["status"]["state"] = json!("idle");
    h.queue("hc", "get", &term);
    let next = h.ok(&["task", "send", &task, "--text", "next task"]);
    assert_eq!(next["delivery"], "new_turn");
    assert_eq!(next["steered"], false);
}

#[test]
fn compact_run_with_failed_get_keeps_recovery_intents() {
    let h = Harness::new();
    let ws = h.open();
    let roles = h.root().join("data/coord/roles");
    fs::create_dir_all(&roles).unwrap();
    fs::write(roles.join("test.md"),"---\nharness: omp\nstart: runner {session_id} {brief}\nsession: preset\n---\nInstructions\n").unwrap();
    let brief = h.root().join("brief");
    fs::write(&brief, "work").unwrap();
    h.queue("herdr-app","ls",&json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[]}]}));
    h.queue("hc", "run", &json!({"address":"t7","state":"live"}));
    h.queued("hc", "get", 1, &json!({"error":{"kind":"not_found"}}), 3);
    let out = h.run(&[
        "task",
        "start",
        "--ws",
        &ws,
        "--rung",
        "3",
        "--reason",
        "work",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "test",
        "--space",
        "existing",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("t7"));
    let entries = h.journal();
    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "res.intent" && e["data"]["type"] == "tab")
    );
    assert!(
        entries.iter().find(|e| e["kind"] == "task.placed").unwrap()["data"]["session"]["id"]
            .as_str()
            .is_some()
    );
    assert!(
        !entries
            .iter()
            .any(|e| e["kind"] == "res.released" || e["kind"] == "task.closed")
    );
}

#[test]
fn empty_status_is_schema_versioned() {
    let h = Harness::new();
    let status = h.ok(&["status", "--json"]);
    assert_eq!(status["schema_version"], 1);
    assert_eq!(status["state"], "not_running");
    for key in [
        "generated_at_ms",
        "release",
        "degraded",
        "watcher",
        "coordinator",
        "counts",
        "journal_dir",
        "journal_seq",
    ] {
        assert!(status.get(key).is_some(), "{key}");
    }
}

#[test]
fn up_is_idempotent_with_live_managed_terminals() {
    let h = Harness::new();
    h.install_roles();
    h.uninstall("agy");
    let coord = h.root().join("data/coord");
    fs::create_dir_all(coord.join("roles")).unwrap();
    fs::write(coord.join("roles/test.md"),"---\nharness: omp\nstart: runner {session_id} {brief}\nsession: preset\n---\nCoordinator\n").unwrap();
    let helper = h.root().join("quiet-hours");
    fs::write(&helper, "#!/bin/sh\necho quiet\n").unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        coord.join("extensions.json"),
        json!({"quiet-hours":{"description":"Prints quiet during quiet hours.","command":[helper],"startup":true}}).to_string(),
    )
    .unwrap();
    fs::write(
        coord.join("config.json"),
        json!({"coordinator_role":"test"}).to_string(),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    fs::write(coord.join("heartbeat.json"),json!({"pid":1,"terminal":"tw","release":env!("KIT_COORD_BUILD_REVISION"),"started_ms":now,"beat_ms":now,"events_ok":true}).to_string()).unwrap();
    let mut watcher: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    watcher["address"] = json!("tw");
    watcher["state"] = json!("live");
    watcher["pid"] = json!(1);
    watcher["labels"] = json!({"coord.managed":"1","coord.role":"watcher"});
    let mut main = watcher.clone();
    main["address"] = json!("tc");
    main["labels"] = json!({"coord.managed":"1","coord.role":"coordinator"});
    h.queued("hc", "get", 1, &json!({"error":{"kind":"not_found"}}), 3);
    for (n, t) in [
        (2, &watcher),
        (3, &main),
        (4, &watcher),
        (5, &main),
        (6, &watcher),
        (7, &main),
        (8, &watcher),
        (9, &main),
    ] {
        h.queued("hc", "get", n, t, 0);
    }
    h.queue("hc", "ls", &json!({"terminals":[]}));
    h.queued("hc", "run", 1, &json!({"address":"tw"}), 0);
    h.queued("hc", "run", 2, &json!({"address":"tc"}), 0);
    h.queue("herdr-app","ls",&json!({"spaces":[{"id":"fixture","name":"fixture","folder":h.root(),"machine":null,"tabs":[{"id":"wt","name":"watcher","terminals":[{"terminal":"tw","pane":"p1"}]},{"id":"ct","name":"coordinator","terminals":[{"terminal":"tc","pane":"p2"}]}]}]}));
    h.ok(&["up"]);
    h.ok(&["up"]);
    let calls = h.calls("hc");
    let runs: Vec<_> = calls.lines().filter(|l| l.contains("\trun\t")).collect();
    assert_eq!(runs.len(), 2);
    assert!(runs[0].contains("coord.role=watcher"));
    assert!(runs[1].contains("coord.role=coordinator"));
    // The first message: product instructions, role guidance derived from this
    // machine's role files, the coordinator role body as local notes, and the
    // startup read ending with the local extensions.
    let first = fs::read_to_string(coord.join("briefs/coordinator-1.md")).unwrap();
    let at = |text: &str| {
        first
            .find(text)
            .unwrap_or_else(|| panic!("{text}\n{first}"))
    };
    assert!(at("You are the coordinator") < at("## Roles on this machine"));
    assert!(
        at("- `coord-light`: Quick, mechanical")
            < at("## Local coordinator notes\n\nCoordinator\n")
    );
    assert!(
        at("## Startup read")
            < at(
                "## Local extensions\n\n- `quiet-hours` (`kit ext quiet-hours`): Prints quiet during quiet hours.\n  Now: quiet\n"
            )
    );
    assert!(!first.contains("agy-default"));
    assert!(!first.contains("- `test`"));
    // The board lists extensions without running them: any caller may read it.
    let board = h.run(&["board"]);
    assert!(String::from_utf8_lossy(&board.stdout).ends_with(
        "## Local extensions\n\n- `quiet-hours` (`kit ext quiet-hours`): Prints quiet during quiet hours.\n"
    ));
    assert_eq!(h.ok(&["ext", "quiet-hours"])["output"], "quiet");
}

fn init_repo(h: &Harness, name: &str) -> PathBuf {
    let repo = h.root().join(name);
    fs::create_dir_all(&repo).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ][..],
    ] {
        assert!(
            git_env::git()
                .env("HOME", h.root())
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    repo
}

/// Opens a workstream on `repo` and starts its first task in a new worktree on
/// branch `feature`. Returns the workstream, the task and the start reply.
fn start_worktree_task(h: &Harness, repo: &Path) -> (String, String, Value) {
    let ws = h.ok(&[
        "ws",
        "open",
        "--title",
        "Work",
        "--target",
        "delivery",
        "--repo",
        repo.to_str().unwrap(),
    ])["ws"]
        .as_str()
        .unwrap()
        .to_owned();
    let coord = h.root().join("data/coord");
    let roles = coord.join("roles");
    fs::create_dir_all(&roles).unwrap();
    fs::write(
        coord.join("config.json"),
        json!({"worktree_root":h.root().join("trees")}).to_string(),
    )
    .unwrap();
    fs::write(roles.join("test.md"),"---\nharness: omp\nstart: runner {session_id} {brief}\nsession: preset\n---\nInstructions\n").unwrap();
    let brief = h.root().join("brief");
    fs::write(&brief, "Complete work").unwrap();
    let task = format!("{ws}-t1");
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t7");
    term["state"] = json!("live");
    term["turn"] = json!(1);
    term["labels"] =
        json!({"coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task});
    term["foreground"]["pid"] = term["pid"].clone();
    h.queue("hc", "run", &json!({"address":"t7"}));
    h.queue("hc", "get", &term);
    h.queue("hc", "ls", &json!({"terminals":[]}));
    h.queue("herdr-app","ls",&json!({"spaces":[{"id":"fixture","name":"fixture","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"tab","terminals":[{"terminal":"t7","pane":"p"}]}]}]}));
    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws,
        "--rung",
        "4",
        "--reason",
        "work",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "test",
        "--branch",
        "feature",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
        "--reject",
        "3=no",
    ]);
    (ws, task, started)
}

/// Every file below `dir`, by path.
fn files(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut found = std::collections::BTreeMap::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    found
}

/// Git hooks and `git rebase --exec` export their own repository. Work on a
/// workstream's repository must still land there.
#[test]
fn inherited_git_repository_is_left_alone() {
    let mut h = Harness::new();
    let repo = init_repo(&h, "repo");
    let decoy = init_repo(&h, "decoy");
    let before = files(&decoy);
    h.inherited_git = Some(decoy.clone());
    start_worktree_task(&h, &repo);
    assert!(h.root().join("trees/repo/feature").exists());
    let branches = git_env::git()
        .arg("-C")
        .arg(&repo)
        .args(["branch", "--list", "feature"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&branches.stdout).contains("feature"));
    assert_eq!(files(&decoy), before);
}

#[test]
fn worktree_release_clean_and_dirty() {
    for dirty in [false, true] {
        let h = Harness::new();
        let repo = init_repo(&h, "repo");
        let (ws, task, started) = start_worktree_task(&h, &repo);
        let space = started["space"].as_str().unwrap();
        let tree = h.root().join("trees/repo/feature");
        assert!(tree.exists());
        let entries = h.journal();
        let intent = entries
            .iter()
            .find(|e| e["kind"] == "res.intent" && e["data"]["type"] == "branch")
            .unwrap();
        assert!(
            intent["data"]["base_sha"]
                .as_str()
                .is_some_and(|s| s.len() == 40)
        );
        assert!(
            intent["seq"].as_u64().unwrap()
                < entries
                    .iter()
                    .find(|e| e["kind"] == "res.created" && e["data"]["type"] == "branch")
                    .unwrap()["seq"]
                    .as_u64()
                    .unwrap()
        );
        assert!(h.calls("hc").contains(&format!("herdr.space={space}")));
        assert!(h.calls("herdr-app").contains("repo · Work"));
        h.queue("herdr-app","ls",&json!({"spaces":[{"id":space,"name":"repo · Work","folder":tree,"machine":null,"tabs":[{"id":"tab1","name":"tab","terminals":[{"terminal":"t7","pane":"p"}]}]}]}));
        let report = h
            .command(&["report", "done", "--summary", "done"])
            .env("HC_TERMINAL_ID", "t7")
            .output()
            .unwrap();
        assert!(
            report.status.success(),
            "{}",
            String::from_utf8_lossy(&report.stderr)
        );
        h.ok(&["task", "close", &task, "--reason", "accepted"]);
        h.ok(&["ws", "close", &ws, "--summary", "done"]);
        if dirty {
            fs::write(tree.join("untracked"), "data").unwrap();
        }
        let result = h.ok(&["release", &ws]);
        if dirty {
            assert_eq!(result["retained"]["reason_code"], "uncommitted_files");
            assert!(tree.exists());
        } else {
            assert_eq!(result["cleaned"], true);
            assert!(!tree.exists());
            assert!(h.calls("hc").contains("\tkill\t"));
            assert!(h.calls("hc").contains("\trm\t"));
            assert!(h.calls("herdr-app").contains("\tdo\tspace.close\t"));
        }
    }
}

fn rail_fixture() -> (Harness, Value, PathBuf) {
    let h = Harness::new();
    let dir = h.root().join("data/coord/sessions/coordinator-1");
    fs::create_dir_all(&dir).unwrap();
    let journal = h.root().join("data/coord/journal");
    fs::create_dir_all(&journal).unwrap();
    let entry = json!({"v":1,"seq":1,"ts_ms":1,"actor":"cli","kind":"coord.session_started","ws":null,"task":null,
        "data":{"role":"coord-main","model":"fixture","terminal":"c1","pid":42,"created_ms":123,
        "space":"sp","session":{"mode":"dir","id":"session-id","dir":dir},"release":"fixture","reason":"start","brief":"/brief"}});
    fs::write(journal.join("1970-01.jsonl"), format!("{entry}\n")).unwrap();
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("c1");
    term["state"] = json!("live");
    term["pid"] = json!(42);
    term["created_ms"] = json!(123);
    term["turn"] = json!(2);
    term["reported"] = json!(true);
    term["status"] = json!({"state":"idle","app":"omp","kind":null,"msg":null});
    term["status_ms"] = json!(555);
    term["labels"] = json!({"coord.managed":"1","coord.role":"coordinator"});
    h.queue("hc", "get", &term);
    h.queue(
        "hc",
        "send",
        &json!({"turn":3,"started":true,"status":"working"}),
    );
    (h, term, dir)
}

fn rail_error(output: Output, code: &str, exit: i32) {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["error"]["code"],
        code
    );
}

#[test]
fn rail_send_validates_delivery_identity_and_question() {
    let (h, mut term, _) = rail_fixture();
    let mut delegate = term.clone();
    delegate["labels"] =
        json!({"coord.managed":"1","coord.role":"delegate","coord.task":"ws-aaaaaa-t1"});
    h.queue("hc", "get", &delegate);
    rail_error(
        h.command(&["say", "--text", "hi"])
            .env("HC_TERMINAL_ID", "t9")
            .output()
            .unwrap(),
        "forbidden_caller",
        3,
    );
    h.queue("hc", "get", &term);
    rail_error(h.run(&["say", "--text", "\u{1b}x"]), "invalid_text", 3);
    rail_error(h.run(&["say", "--text", " \n"]), "empty_message", 3);
    rail_error(
        h.run(&["say", "--text", "hi", "--question", "q99"]),
        "unknown_question",
        3,
    );
    term["status"]["state"] = json!("working");
    h.queue("hc", "get", &term);
    rail_error(h.run(&["say", "--text", "hi"]), "agent_not_ready", 4);
    term["status"]["state"] = json!("idle");
    term["pid"] = json!(43);
    h.queue("hc", "get", &term);
    rail_error(h.run(&["say", "--text", "hi"]), "coordinator_changed", 4);
    assert!(!h.calls("hc").contains("\tsend\t"));
    term["pid"] = json!(42);
    h.queue("hc", "get", &term);
    let question = h.ok(&["ask", "Choose", "--option", "yes"]);
    let qid = question["qid"].as_str().unwrap();
    let board = h.ok(&["board", "--json"]);
    assert!(board.to_string().contains(&format!("\"qid\":\"{qid}\"")));
    let before = h.journal().len();
    h.queued("hc", "send", 1, &json!({"error":"failed"}), 1);
    rail_error(
        h.run(&["say", "--text", "a  b\n\tc", "--question", qid]),
        "operation_failed",
        1,
    );
    assert_eq!(h.journal().len(), before);
    let sent = h.ok(&["say", "--text", "a  b\n\tc", "--question", qid]);
    assert_eq!(sent["state"], "sent");
    assert_eq!(sent["answered"], true);
    let entries = h.journal();
    assert_eq!(entries[before]["kind"], "coord.sent");
    assert_eq!(entries[before]["data"]["text"], "a  b\n\tc");
    assert_eq!(entries[before + 1]["kind"], "question.answered");
    assert_eq!(entries[before + 1]["data"]["text"], "a  b\n\tc");
    assert!(h.calls("hc").contains("\tsend\t--json\tc1\ta  b\n\tc\n"));
    rail_error(
        h.run(&["say", "--text", "again", "--question", qid]),
        "question_answered",
        3,
    );
}

#[test]
fn rail_conversation_projects_active_branch_and_real_status() {
    let (h, mut term, dir) = rail_fixture();
    term["status"]["state"] = json!("working");
    h.queue("hc", "get", &term);
    let path = dir.join("2026_session-id.jsonl");
    let lines = [
        json!({"type":"session","id":"session-id"}),
        json!({"type":"message","id":"u0","parentId":null,"message":{"role":"user","timestamp":1,"content":[{"type":"text","text":"brief"}]}}),
        json!({"type":"message","id":"old","parentId":"u0","message":{"role":"user","timestamp":2,"content":"old branch"}}),
        json!({"type":"message","id":"u1","parentId":"u0","message":{"role":"user","timestamp":3,"content":[{"type":"text","text":"hello"}]}}),
        json!({"type":"message","id":"a1","parentId":"u1","message":{"role":"assistant","timestamp":4,"stopReason":"toolUse","content":[{"type":"thinking","thinking":"secret"},{"type":"text","text":"interim"},{"type":"toolCall","name":"bash","arguments":{"command":"herdr-kit coord task start --ws private"}}]}}),
        json!({"type":"message","id":"t1","parentId":"a1","message":{"role":"toolResult","content":[{"type":"text","text":"private"}]}}),
        json!({"type":"message","id":"a2","parentId":"t1","message":{"role":"assistant","timestamp":5,"stopReason":"stop","content":[{"type":"thinking","thinking":"secret"},{"type":"text","text":"answer"}]}}),
        json!({"type":"message","id":"u2","parentId":"a2","message":{"role":"user","timestamp":6,"content":"inbox: 2 new"}}),
        json!({"type":"message","id":"u3","parentId":"u2","message":{"role":"user","timestamp":7,"content":"<hc from=worker>hidden"}}),
        json!({"type":"message","id":"u4","parentId":"u3","message":{"role":"user","timestamp":8,"content":"next"}}),
        json!({"type":"message","id":"a3","parentId":"u4","message":{"role":"assistant","timestamp":9,"stopReason":"toolUse","content":[{"type":"toolCall","name":"bash","arguments":{"command":"herdr-kit coord task start --ws secret"}}]}}),
    ];
    fs::write(
        &path,
        format!(
            "{}\n{{\"type\":",
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();
    let result = h.ok(&["conversation", "--json"]);
    assert_eq!(result["source"]["state"], "ready");
    assert_eq!(result["coordinator"]["working_since_ms"], 555);
    assert_eq!(result["coordinator"]["step"]["label"], "coord task start");
    assert_eq!(result["turn"]["steps"], 1);
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|m| m["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["hello", "answer", "next"]
    );
    assert!(!result.to_string().contains("secret"));
    term["status"]["state"] = json!("idle");
    h.queue("hc", "get", &term);
    assert!(h.ok(&["conversation"])["coordinator"]["working_since_ms"].is_null());
    fs::write(&path, "{\"type\":\"session\",\"id\":\"wrong\"}\n").unwrap();
    assert_eq!(h.ok(&["conversation"])["source"]["state"], "mismatch");
    fs::write(
        &path,
        "{\"type\":\"session\",\"id\":\"session-id\"}\nnot-json\n",
    )
    .unwrap();
    assert_eq!(h.ok(&["conversation"])["source"]["state"], "malformed");
    let empty = Harness::new();
    assert_eq!(
        empty.ok(&["conversation"])["source"]["state"],
        "not_running"
    );
}

#[test]
fn rail_projection_matches_sent_and_reports_unsafe_or_ambiguous_source() {
    use std::os::unix::fs::symlink;
    let (h, _, dir) = rail_fixture();
    let sent = h.ok(&["say", "--text", "exact message"]);
    let nudge = h.ok(&["say", "--text", "inbox: 2 new"]);
    let wrapped = h.ok(&["say", "--text", "<hc from=worker>hello"]);
    let brief_text = "## Commands\n`kit` is: example\n## Startup read\nhello";
    let brief = h.ok(&["say", "--text", brief_text]);
    let lines = [
        json!({"type":"session","id":"session-id"}),
        json!({"type":"message","id":"u0","parentId":null,"message":{"role":"user","timestamp":10,"content":"brief"}}),
        json!({"type":"message","id":"u1","parentId":"u0","timestamp":"2026-01-01T00:00:00.123Z","message":{"role":"user","content":"exact message"}}),
        json!({"type":"message","id":"u2","parentId":"u1","message":{"role":"user","content":"inbox: 2 new"}}),
        json!({"type":"message","id":"u3","parentId":"u2","message":{"role":"user","content":"<hc from=worker>hello"}}),
        json!({"type":"message","id":"u4","parentId":"u3","message":{"role":"user","content":brief_text}}),
        json!({"type":"message","id":"u5","parentId":"u4","message":{"role":"user","content":"inbox: 3 new"}}),
        json!({"type":"message","id":"u6","parentId":"u5","message":{"role":"user","content":"<hc from=worker>operational"}}),
    ];
    fs::write(
        dir.join("2026_session-id.jsonl"),
        format!(
            "{}\n",
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();
    let result = h.ok(&["conversation"]);
    let m = &result["messages"][0];
    assert_eq!(m["via"], "ui");
    assert_eq!(m["sent_seq"], sent["seq"]);
    assert_eq!(m["ts_ms"], 1767225600123u64);
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    for (message, expected, seq) in [
        (&messages[1], "inbox: 2 new", &nudge["seq"]),
        (&messages[2], "<hc from=worker>hello", &wrapped["seq"]),
        (&messages[3], brief_text, &brief["seq"]),
    ] {
        assert_eq!(message["text"], expected);
        assert_eq!(message["via"], "ui");
        assert_eq!(&message["sent_seq"], seq);
    }

    // Remove the recorded id to exercise the ambiguous-discovery refusal.
    let journal = h.root().join("data/coord/journal/1970-01.jsonl");
    let mut start: Value = serde_json::from_str(
        fs::read_to_string(&journal)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    start["data"]["session"]["id"] = Value::Null;
    fs::write(&journal, format!("{start}\n")).unwrap();
    let _ = fs::remove_file(h.root().join("data/coord/state.json"));
    fs::write(dir.join("another.jsonl"), "{}\n").unwrap();
    assert_eq!(h.ok(&["conversation"])["source"]["state"], "ambiguous");
    fs::remove_file(dir.join("another.jsonl")).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    let external = h.root().join("outside");
    fs::create_dir_all(&external).unwrap();
    symlink(&external, &dir).unwrap();
    assert_eq!(h.ok(&["conversation"])["source"]["state"], "unreadable");
}

#[test]
fn rail_send_obeys_terminal_lock_and_size_bound() {
    use fs2::FileExt;
    let (h, _, _) = rail_fixture();
    let lockdir = h.root().join("data/coord/locks");
    fs::create_dir_all(&lockdir).unwrap();
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .open(lockdir.join("6331"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    rail_error(h.run(&["say", "--text", "message"]), "terminal_busy", 4);
    rail_error(
        h.run(&["say", "--text", &"x".repeat(16_385)]),
        "message_too_long",
        3,
    );
    assert!(!h.calls("hc").contains("\tsend\t"));
}

#[test]
fn agy_delegate_discovers_conversation_reports_and_resumes() {
    let h = Harness::new();
    h.install_roles();
    let roles = h.ok(&["roles"])["roles"].as_array().unwrap().clone();
    let agy_role = roles.iter().find(|r| r["name"] == "agy-default").unwrap();
    assert_eq!(agy_role["harness"], "agy");
    assert_eq!(agy_role["model"], "gemini-4-neon-high");
    assert!(agy_role["effort"].is_null());
    assert_eq!(agy_role["session"], "agy-conversation");
    assert_eq!(agy_role["resumable"], true);
    assert_eq!(agy_role["steering"], true);

    let ws = h.open();
    let task = format!("{ws}-t1");
    let brief = h.root().join("brief");
    fs::write(&brief, "Implement harness support").unwrap();

    // Pre-populate agy brain dir inside isolated HOME with a /usage decoy,
    // a sibling task whose brief cites this task's result path, and this task's
    // own conversation transcript (matching the trailing fixed reporting footer).
    let agy_brain = h.root().join(".gemini/antigravity-cli/brain");
    let decoy_logs = agy_brain.join("00000000-0000-4000-8000-000000000001/.system_generated/logs");
    fs::create_dir_all(&decoy_logs).unwrap();
    fs::write(
        decoy_logs.join("transcript_full.jsonl"),
        format!(
            "{}\n",
            json!({"step_index":0,"type":"USER_INPUT","content":"/usage"})
        ),
    )
    .unwrap();
    let result_file = h.root().join(format!("data/coord/results/{task}.md"));
    let other_result = h.root().join(format!("data/coord/results/{ws}-t2.md"));
    let xref_logs = agy_brain.join("00000000-0000-4000-8000-000000000002/.system_generated/logs");
    fs::create_dir_all(&xref_logs).unwrap();
    fs::write(
        xref_logs.join("transcript_full.jsonl"),
        format!(
            "{}\n",
            json!({
                "step_index": 0,
                "source": "USER_EXPLICIT",
                "type": "USER_INPUT",
                "status": "DONE",
                "content": format!(
                    "<USER_REQUEST>\nContext: previous task result: {}\n\n---\nReporting (fixed by the coordinator system):\n\n- Long results go into {}; add --result {} to the report.\n- After reporting, stop and wait: a follow-up may arrive in this session.\n</USER_REQUEST>",
                    result_file.display(),
                    other_result.display(),
                    other_result.display()
                )
            })
        ),
    )
    .unwrap();
    let conv_id = "720d8fe2-d9d4-40b4-af0d-fb7a29307ff0";
    let target_logs = agy_brain.join(conv_id).join(".system_generated/logs");
    fs::create_dir_all(&target_logs).unwrap();
    fs::write(
        target_logs.join("transcript_full.jsonl"),
        format!(
            "{}\n",
            json!({
                "step_index": 0,
                "source": "USER_EXPLICIT",
                "type": "USER_INPUT",
                "status": "DONE",
                "content": format!(
                    "<USER_REQUEST>\nImplement harness support\n\n---\nReporting (fixed by the coordinator system):\n\n- Long results go into {}; add --result {} to the report.\n- After reporting, stop and wait: a follow-up may arrive in this session.\n</USER_REQUEST>",
                    result_file.display(),
                    result_file.display()
                )
            })
        ),
    )
    .unwrap();

    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]}));
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t7");
    term["state"] = json!("live");
    term["created_ms"] = json!(0);
    term["turn"] = json!(0);
    term["reported"] = json!(true);
    term["status"] = json!({"app":"antigravity","state":"working"});
    term["labels"] = json!({"herdr.space":"existing","coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task});
    h.queue("hc", "run", &json!({"address":"t7"}));
    h.queue("hc", "get", &term);

    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws,
        "--rung",
        "3",
        "--reason",
        "work",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "agy-default",
        "--space",
        "existing",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    assert_eq!(started["session"], conv_id);
    let calls = h.calls("hc");
    let run_line = calls.lines().find(|l| l.contains("\trun\t")).unwrap();
    let argv: Vec<_> = run_line.split('\t').collect();
    let sep = argv.iter().position(|a| *a == "--").unwrap();
    assert_eq!(
        &argv[sep + 1..sep + 7],
        &[
            "agy",
            "--settings",
            r#"{"queuedMessages":"send-immediately"}"#,
            "--model",
            "gemini-4-neon-high",
            "-i"
        ]
    );
    assert!(argv[sep + 7].starts_with("You are an Antigravity CLI delegate"));
    assert!(calls.contains("Implement harness support"));

    // Startup (turn 0): refused as agent_not_ready.
    rail_error(
        h.run(&["task", "send", &task, "--text", "too early"]),
        "agent_not_ready",
        4,
    );
    // Working after turn 1 on steering-enabled antigravity: steers mid-turn!
    term["turn"] = json!(1);
    h.queue("hc", "get", &term);
    h.queue(
        "hc",
        "send",
        &json!({"turn":1,"started":false,"status":"working"}),
    );
    let steered = h.ok(&["task", "send", &task, "--text", "mid-turn steer"]);
    assert_eq!(steered["delivery"], "steering");
    assert_eq!(steered["steered"], true);
    // Blocked (permission prompt): refused as agent_blocked.
    term["status"]["state"] = json!("blocked");
    h.queue("hc", "get", &term);
    rail_error(
        h.run(&["task", "send", &task, "--text", "answer prompt"]),
        "agent_blocked",
        3,
    );
    // Idle after turn 1: delivers a new turn.
    term["status"]["state"] = json!("idle");
    h.queue("hc", "get", &term);
    h.queue(
        "hc",
        "send",
        &json!({"turn":2,"started":true,"status":"working"}),
    );
    let sent = h.ok(&["task", "send", &task, "--text", "check edge cases"]);
    assert_eq!(sent["delivery"], "new_turn");
    assert_eq!(sent["steered"], false);

    // Delegate reporting contract with --result works for agy delegates.
    fs::write(&result_file, "# Findings\nAll good.\n").unwrap();
    let rep = h
        .command(&[
            "report",
            "done",
            "--summary",
            "implemented",
            "--result",
            result_file.to_str().unwrap(),
        ])
        .env("HC_TERMINAL_ID", "t7")
        .output()
        .unwrap();
    assert!(
        rep.status.success(),
        "{}",
        String::from_utf8_lossy(&rep.stderr)
    );
    let reported = h.journal().pop().unwrap();
    assert_eq!(reported["kind"], "task.reported");
    assert_eq!(reported["data"]["outcome"], "done");
    assert!(
        reported["data"]["result"]
            .as_str()
            .unwrap()
            .ends_with(&format!("{task}.md"))
    );

    // Cold resume when terminal is gone uses `agy --conversation <id> -i <msg>` without `--model`.
    let mut resumed_term = term.clone();
    resumed_term["address"] = json!("t8");
    resumed_term["turn"] = json!(0);
    h.queued("hc", "get", 1, &json!({"error":{"kind":"not_found"}}), 3);
    h.queued("hc", "run", 1, &json!({"address":"t8"}), 0);
    h.queued("hc", "get", 2, &resumed_term, 0);
    let resumed = h.ok(&["task", "send", &task, "--text", "one more tweak"]);
    assert_eq!(resumed["delivery"], "new_turn");
    assert_eq!(resumed["steered"], false);
    assert_eq!(resumed["session"], conv_id);
    let runs: Vec<_> = h
        .calls("hc")
        .lines()
        .filter(|l| l.contains("\trun\t"))
        .map(str::to_owned)
        .collect();
    assert_eq!(runs.len(), 2);
    let resume_argv: Vec<_> = runs[1].split('\t').collect();
    let rsep = resume_argv.iter().position(|a| *a == "--").unwrap();
    assert_eq!(
        &resume_argv[rsep + 1..rsep + 7],
        &[
            "agy",
            "--settings",
            r#"{"queuedMessages":"send-immediately"}"#,
            "--conversation",
            conv_id,
            "-i"
        ]
    );
    assert!(!resume_argv[rsep + 1..rsep + 7].contains(&"--model"));
}

#[test]
fn claude_roles_expand_model_effort_and_resume_cleanly() {
    let h = Harness::new();
    h.ok(&[
        "roles",
        "create",
        "claude-opus",
        "--harness",
        "claude",
        "--model",
        "opus",
        "--effort",
        "high",
        "--prompt",
        "Claude opus delegate.",
    ]);
    let roles_dir = h.root().join("data/coord/roles");
    fs::write(
        roles_dir.join("claude-haiku-low.md"),
        "---\nharness: claude\nmodel: haiku\neffort: low\nstart: claude --session-id {session_id} {brief}\nresume: claude --resume {session_id} {brief}\nsession: preset\n---\nFast delegate\n",
    )
    .unwrap();
    let roles = h.ok(&["roles"])["roles"].as_array().unwrap().clone();
    let opus = roles.iter().find(|r| r["name"] == "claude-opus").unwrap();
    assert_eq!(opus["harness"], "claude");
    assert_eq!(opus["model"], "opus");
    assert_eq!(opus["effort"], "high");
    assert_eq!(opus["session"], "preset");
    assert_eq!(opus["resumable"], true);
    assert_eq!(opus["steering"], false);
    let haiku = roles
        .iter()
        .find(|r| r["name"] == "claude-haiku-low")
        .unwrap();
    assert_eq!(haiku["source"], "file");
    assert_eq!(haiku["model"], "haiku");
    assert_eq!(haiku["effort"], "low");

    let ws = h.open();
    let task = format!("{ws}-t1");
    let brief = h.root().join("brief");
    fs::write(&brief, "Investigate race condition").unwrap();
    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]}));
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t7");
    term["state"] = json!("live");
    term["turn"] = json!(1);
    term["reported"] = json!(true);
    term["status"] = json!({"app":"claude","state":"working"});
    term["labels"] = json!({"herdr.space":"existing","coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task});
    h.queue("hc", "run", &json!({"address":"t7"}));
    h.queue("hc", "get", &term);

    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws,
        "--rung",
        "3",
        "--reason",
        "deep work",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "claude-opus",
        "--space",
        "existing",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    let session_id = started["session"].as_str().unwrap().to_owned();
    let run_line = h
        .calls("hc")
        .lines()
        .find(|l| l.contains("\trun\t"))
        .unwrap()
        .to_owned();
    let argv: Vec<_> = run_line.split('\t').collect();
    let sep = argv.iter().position(|a| *a == "--").unwrap();
    assert_eq!(
        &argv[sep + 1..sep + 8],
        &[
            "claude",
            "--model",
            "opus",
            "--effort",
            "high",
            "--session-id",
            session_id.as_str()
        ]
    );
    let board = h.ok(&["board", "--json"]);
    let task_view = &board["workstreams"][0]["tasks"][0];
    assert_eq!(task_view["harness"], "claude");
    assert_eq!(task_view["model"], "opus");
    assert_eq!(task_view["effort"], "high");

    // Working claude refuses mid-turn steering explicitly.
    rail_error(
        h.run(&["task", "send", &task, "--text", "steer"]),
        "steering_unsupported",
        4,
    );
    // Done claude accepts new_turn.
    term["status"]["state"] = json!("done");
    h.queue("hc", "get", &term);
    h.queue(
        "hc",
        "send",
        &json!({"turn":2,"started":true,"status":"working"}),
    );
    let sent = h.ok(&["task", "send", &task, "--text", "follow up"]);
    assert_eq!(sent["delivery"], "new_turn");
    assert_eq!(sent["steered"], false);

    // Cold resume keeps the session model without re-passing --model or --effort.
    let mut resumed_term = term.clone();
    resumed_term["address"] = json!("t8");
    h.queued("hc", "get", 1, &json!({"error":{"kind":"not_found"}}), 3);
    h.queued("hc", "run", 1, &json!({"address":"t8"}), 0);
    h.queued("hc", "get", 2, &resumed_term, 0);
    let resumed = h.ok(&["task", "send", &task, "--text", "after restart"]);
    assert_eq!(resumed["session"], session_id);
    let runs: Vec<_> = h
        .calls("hc")
        .lines()
        .filter(|l| l.contains("\trun\t"))
        .map(str::to_owned)
        .collect();
    let rargv: Vec<_> = runs[1].split('\t').collect();
    let rsep = rargv.iter().position(|a| *a == "--").unwrap();
    assert_eq!(
        &rargv[rsep + 1..rsep + 4],
        &["claude", "--resume", session_id.as_str()]
    );
    assert!(!rargv[rsep + 1..rsep + 4].contains(&"--model"));
    assert!(!rargv[rsep + 1..rsep + 4].contains(&"--effort"));
}

#[test]
fn agy_pretrusts_owned_worktree_only_for_trusted_repo_and_task_trust_is_removed() {
    let h = Harness::new();
    h.install_roles();
    let worktree_root = h.root().join("worktrees");
    fs::create_dir_all(h.root().join("data/coord")).unwrap();
    fs::write(
        h.root().join("data/coord/config.json"),
        json!({"worktree_root": worktree_root}).to_string(),
    )
    .unwrap();

    // `task trust` subcommand is removed completely (CLI parse error exit 2).
    let removed = h.run(&["task", "trust", "ws-abcdef-t1"]);
    assert_eq!(removed.status.code(), Some(2));

    let init_repo = |name: &str| {
        let repo = h.root().join(name);
        fs::create_dir_all(&repo).unwrap();
        for args in [
            &["init", "-q"][..],
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ][..],
        ] {
            assert!(
                kit_core::git_env::git()
                    .env("HOME", h.root())
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        repo
    };

    let trusted_repo = init_repo("trusted-repo");
    let untrusted_repo = init_repo("untrusted-repo");
    let brief = h.root().join("brief");
    fs::write(&brief, "Work in owned worktree").unwrap();

    // 1. Trusted repo (person has it open in an unowned Herdr space):
    //    `agy-default` receives `--settings '{"trustedWorkspaces":["<worktree cwd>"]}'`
    //    on both initial launch and cold resume, scoped only to its launch directory.
    let ws_trusted = h.ok(&[
        "ws",
        "open",
        "--title",
        "Trusted",
        "--target",
        "main",
        "--repo",
        trusted_repo.to_str().unwrap(),
    ])["ws"]
        .as_str()
        .unwrap()
        .to_owned();
    let task_trusted = format!("{ws_trusted}-t1");
    let conv_id = "22222222-3333-4444-8555-666666666666";
    let result_file = h
        .root()
        .join(format!("data/coord/results/{task_trusted}.md"));
    let logs = h.root().join(format!(
        ".gemini/antigravity-cli/brain/{conv_id}/.system_generated/logs"
    ));
    fs::create_dir_all(&logs).unwrap();
    let transcript_line = json!({
        "step_index": 0,
        "source": "USER_EXPLICIT",
        "type": "USER_INPUT",
        "status": "DONE",
        "content": format!(
            "<USER_REQUEST>\nWork in owned worktree\n\n---\nReporting (fixed by the coordinator system):\n\n- Long results go into {}; add --result {} to the report.\n- After reporting, stop and wait: a follow-up may arrive in this session.\n</USER_REQUEST>",
            result_file.display(),
            result_file.display()
        )
    });
    fs::write(
        logs.join("transcript_full.jsonl"),
        format!("{transcript_line}\n"),
    )
    .unwrap();

    h.queue(
        "herdr-app",
        "ls",
        &json!({"spaces":[{"id":"person-sp","name":"Person Repo","folder":trusted_repo,"machine":null,"tabs":[]}]}),
    );
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t10");
    term["state"] = json!("live");
    term["created_ms"] = json!(0);
    term["turn"] = json!(1);
    term["reported"] = json!(true);
    term["status"] = json!({"app":"antigravity","state":"idle"});
    term["labels"] = json!({"herdr.space":"sp","coord.managed":"1","coord.role":"delegate","coord.ws":ws_trusted,"coord.task":task_trusted});
    h.queue("hc", "run", &json!({"address":"t10"}));
    h.queue("hc", "get", &term);

    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws_trusted,
        "--rung",
        "4",
        "--reason",
        "isolated worktree",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "agy-default",
        "--branch",
        "feat/agy-trust",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
        "--reject",
        "3=no",
    ]);
    assert_eq!(started["session"], conv_id);
    let expected_cwd = worktree_root.join("trusted-repo/feat-agy-trust");
    let expected_settings = json!({
        "queuedMessages": "send-immediately",
        "trustedWorkspaces": [expected_cwd]
    });

    let start_run = h
        .calls("hc")
        .lines()
        .find(|l| l.contains("\trun\t"))
        .unwrap()
        .to_owned();
    let start_argv: Vec<_> = start_run.split('\t').collect();
    let sep = start_argv.iter().position(|a| *a == "--").unwrap();
    assert_eq!(start_argv[sep + 1], "agy");
    assert_eq!(start_argv[sep + 2], "--settings");
    assert_eq!(
        serde_json::from_str::<Value>(start_argv[sep + 3]).unwrap(),
        expected_settings
    );
    assert_eq!(
        &start_argv[sep + 4..sep + 7],
        &["--model", "gemini-4-neon-high", "-i"]
    );

    // Cold resume also passes merged `--settings` for the worktree cwd (since `--settings` is
    // session-only and never persisted) while still omitting `--model`.
    let mut resumed_term = term.clone();
    resumed_term["address"] = json!("t11");
    resumed_term["turn"] = json!(0);
    h.queued("hc", "get", 1, &json!({"error":{"kind":"not_found"}}), 3);
    h.queue(
        "herdr-app",
        "ls",
        &json!({"spaces":[{"id":"person-sp","name":"Person Repo","folder":trusted_repo,"machine":null,"tabs":[]}]}),
    );
    h.queued("hc", "run", 1, &json!({"address":"t11"}), 0);
    h.queued("hc", "get", 2, &resumed_term, 0);
    let resumed = h.ok(&["task", "send", &task_trusted, "--text", "continue"]);
    assert_eq!(resumed["session"], conv_id);
    let runs: Vec<_> = h
        .calls("hc")
        .lines()
        .filter(|l| l.contains("\trun\t"))
        .map(str::to_owned)
        .collect();
    assert_eq!(runs.len(), 2);
    let resume_argv: Vec<_> = runs[1].split('\t').collect();
    let rsep = resume_argv.iter().position(|a| *a == "--").unwrap();
    assert_eq!(resume_argv[rsep + 1], "agy");
    assert_eq!(resume_argv[rsep + 2], "--settings");
    assert_eq!(
        serde_json::from_str::<Value>(resume_argv[rsep + 3]).unwrap(),
        expected_settings
    );
    assert_eq!(
        &resume_argv[rsep + 4..rsep + 7],
        &["--conversation", conv_id, "-i"]
    );
    assert!(!resume_argv[rsep + 1..rsep + 7].contains(&"--model"));

    // 2. Untrusted repo (not open in any person-owned Herdr space):
    //    `agy-default` receives steering settings only (MUST NOT receive `trustedWorkspaces`).
    let ws_untrusted = h.ok(&[
        "ws",
        "open",
        "--title",
        "Untrusted",
        "--target",
        "main",
        "--repo",
        untrusted_repo.to_str().unwrap(),
    ])["ws"]
        .as_str()
        .unwrap()
        .to_owned();
    let task_untrusted = format!("{ws_untrusted}-t1");
    h.queue(
        "herdr-app",
        "ls",
        &json!({"spaces":[{"id":"person-sp","name":"Person Repo","folder":trusted_repo,"machine":null,"tabs":[]}]}),
    );
    let mut uterm = term.clone();
    uterm["address"] = json!("t12");
    uterm["labels"] = json!({"herdr.space":"sp2","coord.managed":"1","coord.role":"delegate","coord.ws":ws_untrusted,"coord.task":task_untrusted});
    h.queue("hc", "run", &json!({"address":"t12"}));
    h.queue("hc", "get", &uterm);
    h.ok(&[
        "task",
        "start",
        "--ws",
        &ws_untrusted,
        "--rung",
        "4",
        "--reason",
        "isolated worktree",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "agy-default",
        "--branch",
        "feat/untrusted",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
        "--reject",
        "3=no",
    ]);
    let all_runs: Vec<_> = h
        .calls("hc")
        .lines()
        .filter(|l| l.contains("\trun\t"))
        .map(str::to_owned)
        .collect();
    assert_eq!(all_runs.len(), 3);
    let untrusted_argv: Vec<_> = all_runs[2].split('\t').collect();
    let usep = untrusted_argv.iter().position(|a| *a == "--").unwrap();
    assert_eq!(
        &untrusted_argv[usep + 1..usep + 7],
        &[
            "agy",
            "--settings",
            r#"{"queuedMessages":"send-immediately"}"#,
            "--model",
            "gemini-4-neon-high",
            "-i"
        ]
    );
    assert!(!untrusted_argv[usep + 3].contains("trustedWorkspaces"));

    // No persistent settings.json was created or modified.
    assert!(
        !h.root()
            .join(".gemini/antigravity-cli/settings.json")
            .exists()
    );
}

#[test]
fn codex_compact_model_option_conflict_is_rejected_and_standalone_compact_model_is_recognized() {
    let h = Harness::new();
    let ws = h.open();
    let brief = h.root().join("brief");
    fs::write(&brief, "Check compact codex model flag").unwrap();
    let roles_dir = h.root().join("data/coord/roles");
    fs::create_dir_all(&roles_dir).unwrap();
    fs::write(
        roles_dir.join("codex-conflict.md"),
        "---\nharness: codex\nmodel: gpt-6-luna\nstart: codex -mother-model {brief}\nresume: codex resume {session_id} {brief}\nsession: codex-rollout\n---\nBody\n",
    )
    .unwrap();
    fs::write(
        roles_dir.join("codex-compact.md"),
        "---\nharness: codex\nstart: codex -mgpt-6-luna {brief}\nresume: codex resume {session_id} {brief}\nsession: codex-rollout\n---\nBody\n",
    )
    .unwrap();

    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[]}]}));
    rail_error(
        h.run(&[
            "task",
            "start",
            "--ws",
            &ws,
            "--rung",
            "3",
            "--reason",
            "work",
            "--brief",
            brief.to_str().unwrap(),
            "--role",
            "codex-conflict",
            "--space",
            "existing",
            "--reject",
            "1=no",
            "--reject",
            "2=no",
        ]),
        "role_invalid",
        3,
    );
    assert!(!h.calls("hc").contains("\trun\t"));

    // Remove the invalid role file so `roles` can list all valid roles, and verify
    // standalone compact `-mVALUE` is recognized as the effective model.
    fs::remove_file(roles_dir.join("codex-conflict.md")).unwrap();
    let roles = h.ok(&["roles"]);
    let compact = roles["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "codex-compact")
        .unwrap();
    assert_eq!(compact["harness"], "codex");
    assert_eq!(compact["model"], "gpt-6-luna");
}

#[test]
fn trust_override_is_confined_to_workstream_repo_checkouts() {
    let h = Harness::new();
    h.install_roles();
    let worktree_root = h.root().join("worktrees");
    fs::create_dir_all(h.root().join("data/coord")).unwrap();
    fs::write(
        h.root().join("data/coord/config.json"),
        json!({"worktree_root": worktree_root}).to_string(),
    )
    .unwrap();

    let init_repo = |name: &str| {
        let repo = h.root().join(name);
        fs::create_dir_all(&repo).unwrap();
        for args in [
            &["init", "-q"][..],
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ][..],
        ] {
            assert!(
                kit_core::git_env::git()
                    .env("HOME", h.root())
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        repo
    };

    let repo_a = init_repo("repo-a");
    let repo_b = init_repo("repo-b");
    let wt_a = h.root().join("wt-a");
    assert!(
        kit_core::git_env::git()
            .env("HOME", h.root())
            .arg("-C")
            .arg(&repo_a)
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "existing-wt",
                wt_a.to_str().unwrap(),
                "HEAD",
            ])
            .status()
            .unwrap()
            .success()
    );

    let brief = h.root().join("brief");
    fs::write(&brief, "Verify trust confinement").unwrap();

    let spaces_snapshot = |tab_term: Option<(&str, &str)>| {
        let make_tabs = |space_id: &str| match tab_term {
            Some((sid, tid)) if sid == space_id => {
                json!([{"id":"tab1","name":"Tab","terminals":[{"terminal":tid,"pane":"p1"}]}])
            }
            _ => json!([]),
        };
        json!({
            "spaces": [
                {"id": "person-a", "name": "Repo A", "folder": repo_a, "machine": null, "tabs": make_tabs("person-a")},
                {"id": "space-b", "name": "Repo B", "folder": repo_b, "machine": null, "tabs": make_tabs("space-b")},
                {"id": "space-wt-a", "name": "WT A", "folder": wt_a, "machine": null, "tabs": make_tabs("space-wt-a")}
            ]
        })
    };

    let write_agy_transcript = |conv_id: &str, task_id: &str| {
        let result_file = h.root().join(format!("data/coord/results/{task_id}.md"));
        let logs = h.root().join(format!(
            ".gemini/antigravity-cli/brain/{conv_id}/.system_generated/logs"
        ));
        fs::create_dir_all(&logs).unwrap();
        let line = json!({
            "step_index": 0,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "content": format!(
                "<USER_REQUEST>\nVerify trust confinement\n\n---\nReporting (fixed by the coordinator system):\n\n- Long results go into {}; add --result {} to the report.\n- After reporting, stop and wait: a follow-up may arrive in this session.\n</USER_REQUEST>",
                result_file.display(),
                result_file.display()
            )
        });
        fs::write(logs.join("transcript_full.jsonl"), format!("{line}\n")).unwrap();
    };

    let write_codex_rollout = |file_name: &str, session_id: &str, cwd: &Path| {
        let dir = h.root().join(".codex/sessions");
        fs::create_dir_all(&dir).unwrap();
        let line = json!({
            "type": "session_meta",
            "payload": {
                "id": session_id,
                "cwd": cwd
            }
        });
        fs::write(dir.join(file_name), format!("{line}\n")).unwrap();
    };

    let make_term = |addr: &str, space: &str, ws: &str, task: &str, app: &str| {
        let mut term: Value =
            serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
        term["address"] = json!(addr);
        term["state"] = json!("live");
        term["created_ms"] = json!(0);
        term["turn"] = json!(1);
        term["reported"] = json!(true);
        term["status"] = json!({"app": app, "state": "idle"});
        term["labels"] = json!({
            "herdr.space": space,
            "coord.managed": "1",
            "coord.role": "delegate",
            "coord.ws": ws,
            "coord.task": task
        });
        term
    };

    let open_ws = |title: &str| {
        h.ok(&[
            "ws",
            "open",
            "--title",
            title,
            "--target",
            "main",
            "--repo",
            repo_a.to_str().unwrap(),
        ])["ws"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let run_argv = |idx: usize| -> Vec<String> {
        let line = h
            .calls("hc")
            .lines()
            .filter(|l| l.contains("\trun\t"))
            .nth(idx)
            .unwrap()
            .to_owned();
        let parts: Vec<_> = line.split('\t').map(str::to_owned).collect();
        let sep = parts.iter().position(|a| a == "--").unwrap();
        parts[sep + 1..].to_vec()
    };

    // 1. Rung-3 into unrelated repo B with agy-default: MUST omit --settings on start AND cold send.
    let ws_agy_b = open_ws("Agy unrelated B");
    let task_agy_b = format!("{ws_agy_b}-t1");
    let conv_agy_b = "33333333-1111-4111-8111-111111111111";
    write_agy_transcript(conv_agy_b, &task_agy_b);
    h.queue(
        "herdr-app",
        "ls",
        &spaces_snapshot(Some(("space-b", "t20"))),
    );
    h.queue("hc", "run", &json!({"address": "t20"}));
    h.queue(
        "hc",
        "get",
        &make_term("t20", "space-b", &ws_agy_b, &task_agy_b, "antigravity"),
    );
    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws_agy_b,
        "--rung",
        "3",
        "--reason",
        "existing space",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "agy-default",
        "--space",
        "space-b",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    assert_eq!(started["session"], conv_agy_b);
    let argv0 = run_argv(0);
    assert_eq!(
        &argv0[..6],
        &[
            "agy",
            "--settings",
            r#"{"queuedMessages":"send-immediately"}"#,
            "--model",
            "gemini-4-neon-high",
            "-i"
        ]
    );
    assert!(!argv0[2].contains("trustedWorkspaces"));

    h.queued("hc", "get", 1, &json!({"error": {"kind": "not_found"}}), 3);
    h.queue(
        "herdr-app",
        "ls",
        &spaces_snapshot(Some(("space-b", "t21"))),
    );
    h.queued("hc", "run", 1, &json!({"address": "t21"}), 0);
    h.queued(
        "hc",
        "get",
        2,
        &make_term("t21", "space-b", &ws_agy_b, &task_agy_b, "antigravity"),
        0,
    );
    h.ok(&["task", "send", &task_agy_b, "--text", "cold resume in B"]);
    let argv1 = run_argv(1);
    assert_eq!(
        &argv1[..6],
        &[
            "agy",
            "--settings",
            r#"{"queuedMessages":"send-immediately"}"#,
            "--conversation",
            conv_agy_b,
            "-i"
        ]
    );
    assert!(!argv1[2].contains("trustedWorkspaces"));

    // 2. Rung-3 into unrelated repo B with custom codex role: MUST omit -c trust override on start AND cold send.
    h.ok(&[
        "roles",
        "create",
        "codex-default",
        "--harness",
        "codex",
        "--model",
        "gpt-6-luna",
    ]);
    let ws_codex_b = open_ws("Codex unrelated B");
    let task_codex_b = format!("{ws_codex_b}-t1");
    let sess_codex_b = "codex-session-b";
    write_codex_rollout("rollout-b.jsonl", sess_codex_b, &repo_b);
    h.queue(
        "herdr-app",
        "ls",
        &spaces_snapshot(Some(("space-b", "t22"))),
    );
    h.queue("hc", "run", &json!({"address": "t22"}));
    h.queue(
        "hc",
        "get",
        &make_term("t22", "space-b", &ws_codex_b, &task_codex_b, "codex"),
    );
    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws_codex_b,
        "--rung",
        "3",
        "--reason",
        "existing space",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "codex-default",
        "--space",
        "space-b",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    assert_eq!(started["session"], sess_codex_b);
    let argv2 = run_argv(2);
    assert_eq!(&argv2[..3], &["codex", "-m", "gpt-6-luna"]);
    assert!(!argv2.iter().any(|a| a == "-c"));

    h.queued("hc", "get", 1, &json!({"error": {"kind": "not_found"}}), 3);
    h.queue(
        "herdr-app",
        "ls",
        &spaces_snapshot(Some(("space-b", "t23"))),
    );
    h.queued("hc", "run", 1, &json!({"address": "t23"}), 0);
    h.queued(
        "hc",
        "get",
        2,
        &make_term("t23", "space-b", &ws_codex_b, &task_codex_b, "codex"),
        0,
    );
    h.ok(&[
        "task",
        "send",
        &task_codex_b,
        "--text",
        "cold resume codex in B",
    ]);
    let argv3 = run_argv(3);
    assert_eq!(&argv3[..3], &["codex", "resume", sess_codex_b]);
    assert!(!argv3.iter().any(|a| a == "-c"));

    // 3. Rung-3 into a same-repo worktree of repo A with agy-default: MUST apply --settings for wt_a.
    let ws_agy_wt = open_ws("Agy same-repo wt");
    let task_agy_wt = format!("{ws_agy_wt}-t1");
    let conv_agy_wt = "44444444-2222-4222-8222-222222222222";
    write_agy_transcript(conv_agy_wt, &task_agy_wt);
    h.queue(
        "herdr-app",
        "ls",
        &spaces_snapshot(Some(("space-wt-a", "t24"))),
    );
    h.queue("hc", "run", &json!({"address": "t24"}));
    h.queue(
        "hc",
        "get",
        &make_term("t24", "space-wt-a", &ws_agy_wt, &task_agy_wt, "antigravity"),
    );
    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws_agy_wt,
        "--rung",
        "3",
        "--reason",
        "same-repo worktree",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "agy-default",
        "--space",
        "space-wt-a",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    assert_eq!(started["session"], conv_agy_wt);
    let argv4 = run_argv(4);
    assert_eq!(argv4[0], "agy");
    assert_eq!(argv4[1], "--settings");
    assert_eq!(
        serde_json::from_str::<Value>(&argv4[2]).unwrap(),
        json!({"queuedMessages": "send-immediately", "trustedWorkspaces": [wt_a]})
    );
    assert_eq!(&argv4[3..6], &["--model", "gemini-4-neon-high", "-i"]);

    h.queued("hc", "get", 1, &json!({"error": {"kind": "not_found"}}), 3);
    h.queue(
        "herdr-app",
        "ls",
        &spaces_snapshot(Some(("space-wt-a", "t25"))),
    );
    h.queued("hc", "run", 1, &json!({"address": "t25"}), 0);
    h.queued(
        "hc",
        "get",
        2,
        &make_term("t25", "space-wt-a", &ws_agy_wt, &task_agy_wt, "antigravity"),
        0,
    );
    h.ok(&[
        "task",
        "send",
        &task_agy_wt,
        "--text",
        "cold resume in wt_a",
    ]);
    let argv5 = run_argv(5);
    assert_eq!(argv5[0], "agy");
    assert_eq!(argv5[1], "--settings");
    assert_eq!(
        serde_json::from_str::<Value>(&argv5[2]).unwrap(),
        json!({"queuedMessages": "send-immediately", "trustedWorkspaces": [wt_a]})
    );
    assert_eq!(&argv5[3..6], &["--conversation", conv_agy_wt, "-i"]);

    // 4. Rung-3 into a same-repo worktree of repo A with codex-default: MUST apply -c trust override.
    let ws_codex_wt = open_ws("Codex same-repo wt");
    let task_codex_wt = format!("{ws_codex_wt}-t1");
    let sess_codex_wt = "codex-session-wt";
    write_codex_rollout("rollout-wt.jsonl", sess_codex_wt, &wt_a);
    h.queue(
        "herdr-app",
        "ls",
        &spaces_snapshot(Some(("space-wt-a", "t26"))),
    );
    h.queue("hc", "run", &json!({"address": "t26"}));
    h.queue(
        "hc",
        "get",
        &make_term("t26", "space-wt-a", &ws_codex_wt, &task_codex_wt, "codex"),
    );
    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws_codex_wt,
        "--rung",
        "3",
        "--reason",
        "same-repo worktree",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "codex-default",
        "--space",
        "space-wt-a",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    assert_eq!(started["session"], sess_codex_wt);
    let expected_codex_trust = format!(
        r#"projects={{ "{}" = {{ trust_level = "trusted" }} }}"#,
        repo_a.display()
    );
    let argv6 = run_argv(6);
    assert_eq!(
        &argv6[..5],
        &[
            "codex",
            "-c",
            expected_codex_trust.as_str(),
            "-m",
            "gpt-6-luna"
        ]
    );

    // 5. Rung-4 coordinator-created worktree of repo A with agy-default: MUST apply --settings.
    let ws_agy_r4 = open_ws("Agy coord wt");
    let task_agy_r4 = format!("{ws_agy_r4}-t1");
    let conv_agy_r4 = "55555555-3333-4333-8333-333333333333";
    write_agy_transcript(conv_agy_r4, &task_agy_r4);
    h.queue("herdr-app", "ls", &spaces_snapshot(None));
    h.queue("hc", "run", &json!({"address": "t27"}));
    h.queue(
        "hc",
        "get",
        &make_term("t27", "sp-r4", &ws_agy_r4, &task_agy_r4, "antigravity"),
    );
    let started = h.ok(&[
        "task",
        "start",
        "--ws",
        &ws_agy_r4,
        "--rung",
        "4",
        "--reason",
        "coord worktree",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        "agy-default",
        "--branch",
        "feat/coord-wt",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
        "--reject",
        "3=no",
    ]);
    assert_eq!(started["session"], conv_agy_r4);
    let coord_wt = worktree_root.join("repo-a/feat-coord-wt");
    let argv7 = run_argv(7);
    assert_eq!(argv7[0], "agy");
    assert_eq!(argv7[1], "--settings");
    assert_eq!(
        serde_json::from_str::<Value>(&argv7[2]).unwrap(),
        json!({"queuedMessages": "send-immediately", "trustedWorkspaces": [coord_wt]})
    );
    assert_eq!(&argv7[3..6], &["--model", "gemini-4-neon-high", "-i"]);
}

#[test]
fn roles_cli_view_create_edit_override_reset_delete_and_validation() {
    use std::io::Write;
    use std::process::Stdio;

    let h = Harness::new();
    h.install_roles();
    let roles_dir = h.root().join("data/coord/roles");

    // 1. View list and a single local role as JSON with source, resolved fields, and prompt body.
    let list = h.ok(&["roles"]);
    let agy_item = list["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "agy-default")
        .unwrap();
    assert_eq!(agy_item["source"], "file");
    assert_eq!(agy_item["overrides_builtin"], false);
    assert_eq!(agy_item["harness"], "agy");
    assert_eq!(agy_item["model"], "gemini-4-neon-high");
    assert_eq!(agy_item["available"], true);
    assert!(agy_item["unavailable_reason"].is_null());
    assert_eq!(agy_item["fallback"], json!(["coord-deep"]));
    assert!(
        agy_item["description"]
            .as_str()
            .unwrap()
            .contains("second opinions")
    );
    assert!(
        agy_item["body"]
            .as_str()
            .unwrap()
            .contains("Antigravity CLI")
    );

    let agy_one = h.ok(&["roles", "agy-default"]);
    assert_eq!(agy_one["name"], "agy-default");
    assert_eq!(agy_one["source"], "file");
    assert_eq!(agy_one["start"], "agy -i {brief}");
    assert_eq!(
        agy_one["resume"],
        "agy --conversation {session_id} -i {brief}"
    );
    assert!(agy_one["trust"].as_str().unwrap().contains("{cwd_json}"));
    assert!(agy_one["body"].as_str().unwrap().contains("Antigravity CLI"));

    // 2. Create copied from agy-default with overrides.
    let created = h.ok(&[
        "roles",
        "create",
        "agy-fast",
        "--from",
        "agy-default",
        "--model",
        "gemini-4-neon-medium",
        "--effort",
        "low",
        "--prompt",
        "Fast delegate for quick checks.",
    ]);
    assert_eq!(created["action"], "created");
    assert_eq!(created["role"]["name"], "agy-fast");
    assert_eq!(created["role"]["source"], "file");
    assert_eq!(created["role"]["harness"], "agy");
    assert_eq!(created["role"]["model"], "gemini-4-neon-medium");
    assert_eq!(created["role"]["effort"], "low");
    assert_eq!(created["role"]["body"], "Fast delegate for quick checks.");
    assert!(roles_dir.join("agy-fast.md").is_file());

    // 3. Create from scratch using harness defaults + stdin prompt.
    let mut child = h
        .command(&[
            "roles",
            "create",
            "claude-reviewer",
            "--harness",
            "claude",
            "--model",
            "sonnet",
            "--effort",
            "medium",
            "--prompt-file",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"Review changes carefully.\nReport only concrete findings.\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reviewer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reviewer["role"]["source"], "file");
    assert_eq!(reviewer["role"]["session"], "preset");
    assert_eq!(
        reviewer["role"]["start"],
        "claude --session-id {session_id} {brief}"
    );
    assert!(
        reviewer["role"]["body"]
            .as_str()
            .unwrap()
            .contains("Review changes carefully.")
    );

    // 4. Edit a custom file role (including prompt from file).
    let prompt_file = h.root().join("new-prompt.md");
    fs::write(&prompt_file, "Updated fast prompt.\n").unwrap();
    let edited = h.ok(&[
        "roles",
        "edit",
        "agy-fast",
        "--model",
        "gemini-4-neon-high",
        "--clear-effort",
        "--prompt-file",
        prompt_file.to_str().unwrap(),
    ]);
    assert_eq!(edited["action"], "updated");
    assert_eq!(edited["role"]["source"], "file");
    assert_eq!(edited["role"]["model"], "gemini-4-neon-high");
    assert!(edited["role"]["effort"].is_null());
    assert_eq!(edited["role"]["body"], "Updated fast prompt.");

    // 5. Edit an inline-model file role (like existing coord-fable) without conflict.
    fs::write(
        roles_dir.join("legacy-inline.md"),
        "---\nharness: omp\nstart: omp --model @old --session-dir {session_dir} {brief}\nresume: omp --session-dir {session_dir} --resume {session_id} {brief}\nsession: dir\n---\nOld prompt\n",
    )
    .unwrap();
    let inline_edited = h.ok(&["roles", "edit", "legacy-inline", "--model", "@slow"]);
    assert_eq!(inline_edited["role"]["model"], "@slow");
    assert_eq!(
        inline_edited["role"]["start"],
        "omp --session-dir {session_dir} {brief}"
    );

    // 6. Edit a built-in role -> creates override file, source becomes "override", --defaults still shows original.
    let overridden = h.ok(&[
        "roles",
        "edit",
        "coord-default",
        "--model",
        "@task",
        "--description",
        "Everyday work; generous subscription.",
        "--fallback",
        "coord-light,agy-default",
    ]);
    assert_eq!(overridden["action"], "updated");
    assert_eq!(overridden["role"]["source"], "override");
    assert_eq!(overridden["role"]["overrides_builtin"], true);
    assert_eq!(overridden["role"]["model"], "@task");
    assert_eq!(
        overridden["role"]["description"],
        "Everyday work; generous subscription."
    );
    assert_eq!(
        overridden["role"]["fallback"],
        json!(["coord-light", "agy-default"])
    );
    assert!(roles_dir.join("coord-default.md").is_file());
    rail_error(
        h.run(&[
            "roles",
            "edit",
            "coord-default",
            "--fallback",
            "coord-default",
        ]),
        "role_invalid",
        3,
    );
    let cleared = h.ok(&["roles", "edit", "coord-default", "--clear-fallback"]);
    assert_eq!(cleared["role"]["fallback"], json!([]));
    assert_eq!(cleared["role"]["model"], "@task");

    let listed_override = h.ok(&["roles"])["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "coord-default")
        .unwrap()
        .clone();
    assert_eq!(listed_override["source"], "override");
    assert_eq!(listed_override["model"], "@task");

    let default_view = h.ok(&["roles", "--defaults", "coord-default"]);
    assert_eq!(default_view["source"], "builtin");
    assert!(default_view["model"].is_null());

    // Delete on built-in override is refused with guidance to use reset.
    rail_error(
        h.run(&["roles", "delete", "coord-default"]),
        "cannot_delete_builtin",
        3,
    );

    // 7. Reset built-in override -> removes override file, source returns to "builtin".
    let reset = h.ok(&["roles", "reset", "coord-default"]);
    assert_eq!(reset["action"], "reset");
    assert_eq!(reset["role"]["source"], "builtin");
    assert!(reset["role"]["model"].is_null());
    assert!(!roles_dir.join("coord-default.md").exists());

    // Reset again when not overridden -> refused.
    rail_error(
        h.run(&["roles", "reset", "coord-default"]),
        "role_not_overridden",
        3,
    );
    // Reset on non-builtin -> refused.
    rail_error(h.run(&["roles", "reset", "agy-fast"]), "not_a_builtin", 3);

    // 8. Delete custom file role -> removes file, subsequent view fails with role_not_found.
    let deleted = h.ok(&["roles", "delete", "agy-fast"]);
    assert_eq!(deleted["action"], "deleted");
    assert_eq!(deleted["name"], "agy-fast");
    assert!(!roles_dir.join("agy-fast.md").exists());
    rail_error(h.run(&["roles", "agy-fast"]), "role_not_found", 3);
    rail_error(
        h.run(&["roles", "delete", "coord-default"]),
        "cannot_delete_builtin",
        3,
    );

    // 9. Rejection of invalid roles: never writes or corrupts files.
    // Unknown harness without --start/--session
    rail_error(
        h.run(&["roles", "create", "bad-harness", "--harness", "unknown-tool"]),
        "role_invalid",
        3,
    );
    assert!(!roles_dir.join("bad-harness.md").exists());

    // Unknown harness with model and no {model} placeholder
    rail_error(
        h.run(&[
            "roles",
            "create",
            "bad-harness-model",
            "--harness",
            "custom",
            "--model",
            "m1",
            "--start",
            "custom {brief}",
            "--session",
            "none",
        ]),
        "role_invalid",
        3,
    );
    assert!(!roles_dir.join("bad-harness-model.md").exists());

    // Missing {brief} placeholder
    rail_error(
        h.run(&[
            "roles",
            "create",
            "missing-brief",
            "--harness",
            "agy",
            "--start",
            "agy --no-brief",
            "--session",
            "none",
        ]),
        "role_invalid",
        3,
    );
    assert!(!roles_dir.join("missing-brief.md").exists());

    // Missing {session_id} for preset mode
    rail_error(
        h.run(&[
            "roles",
            "create",
            "missing-sid",
            "--harness",
            "claude",
            "--start",
            "claude {brief}",
            "--session",
            "preset",
        ]),
        "role_invalid",
        3,
    );
    assert!(!roles_dir.join("missing-sid.md").exists());

    // Bad frontmatter / unterminated quote in start
    rail_error(
        h.run(&[
            "roles",
            "create",
            "bad-quote",
            "--harness",
            "agy",
            "--start",
            "agy 'unterminated {brief}",
            "--session",
            "none",
        ]),
        "role_invalid",
        3,
    );
    assert!(!roles_dir.join("bad-quote.md").exists());

    // Bad effort on edit does not overwrite existing valid role
    let before = fs::read_to_string(roles_dir.join("claude-reviewer.md")).unwrap();
    rail_error(
        h.run(&["roles", "edit", "claude-reviewer", "--effort", "turbo"]),
        "role_invalid",
        3,
    );
    assert_eq!(
        fs::read_to_string(roles_dir.join("claude-reviewer.md")).unwrap(),
        before
    );

    // Reserved subcommand names and duplicate names rejected
    rail_error(
        h.run(&["roles", "create", "edit", "--harness", "omp"]),
        "invalid_role_name",
        3,
    );
    rail_error(
        h.run(&["roles", "create", "agy-default", "--harness", "agy"]),
        "role_exists",
        3,
    );
    rail_error(
        h.run(&["roles", "create", "claude-reviewer", "--harness", "claude"]),
        "role_exists",
        3,
    );

    // 10. Caller permissions: coordinator allowed, delegate forbidden from mutating but allowed to read.
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["labels"] = json!({"coord.managed":"1","coord.role":"coordinator"});
    h.queue("hc", "get", &term);
    let coord_create = h
        .command(&[
            "roles",
            "create",
            "coord-made",
            "--from",
            "coord-light",
            "--prompt",
            "Created by coordinator.",
        ])
        .env("HC_TERMINAL_ID", "tc")
        .output()
        .unwrap();
    assert!(
        coord_create.status.success(),
        "{}",
        String::from_utf8_lossy(&coord_create.stderr)
    );

    term["labels"] = json!({"coord.managed":"1","coord.role":"delegate","coord.task":"ws-abcdef-t1"});
    h.queue("hc", "get", &term);
    rail_error(
        h.command(&["roles", "delete", "coord-made"])
            .env("HC_TERMINAL_ID", "td")
            .output()
            .unwrap(),
        "forbidden_caller",
        3,
    );
    // Delegate can still read roles.
    let delegate_read = h
        .command(&["roles", "coord-made"])
        .env("HC_TERMINAL_ID", "td")
        .output()
        .unwrap();
    assert!(delegate_read.status.success());
}

#[test]
fn roles_usage_task_start_exhausted_refusal_override_and_computer_overlay() {
    use std::os::unix::fs::PermissionsExt;

    let h = Harness::new();
    h.install_roles();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // Seed isolated OMP config so @plan/@task/@advisor/@design resolve deterministically.
    let omp_cfg = h.root().join("omp-config.yml");
    fs::write(
        &omp_cfg,
        "modelRoles:\n  plan: anthropic/claude-opus-5-5:high\n  task: openai-codex/gpt-6.1-terra:medium\n  advisor: openai-codex/gpt-6.2-sol:medium\n  design: anthropic/claude-sonnet-5-5:medium\n",
    )
    .unwrap();

    // Seed isolated usage/snapshot.json (schema 3): Claude exhausted (resets in 10m),
    // Codex ok (3% used), Copilot not signed in, Antigravity gemini ok (40% used).
    let usage_dir = h.root().join("data/usage");
    fs::create_dir_all(&usage_dir).unwrap();
    fs::set_permissions(&usage_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let snap = json!({
        "schema": 3,
        "omp": {
            "attempted_at_ms": now_ms,
            "succeeded_at_ms": now_ms,
            "next_attempt_at_ms": now_ms + 300_000,
            "failures": 0,
            "providers": [
                {
                    "provider": "codex",
                    "state": "available",
                    "error": null,
                    "accounts": [{
                        "fetched_at_ms": now_ms,
                        "limits": [{
                            "id": "codex:7d:0",
                            "window": "7d",
                            "tier": null,
                            "unit": "percent",
                            "used_fraction": 0.03,
                            "used": null,
                            "limit": null,
                            "remaining": null,
                            "unlimited": false,
                            "level": "ok",
                            "resets_at_ms": now_ms + 300_000_000
                        }]
                    }]
                },
                {
                    "provider": "claude",
                    "state": "available",
                    "error": null,
                    "accounts": [{
                        "fetched_at_ms": now_ms,
                        "limits": [{
                            "id": "claude:5h:0",
                            "window": "5h",
                            "tier": null,
                            "unit": "percent",
                            "used_fraction": 1.0,
                            "used": null,
                            "limit": null,
                            "remaining": null,
                            "unlimited": false,
                            "level": "exhausted",
                            "resets_at_ms": now_ms + 600_000
                        }]
                    }]
                },
                {
                    "provider": "copilot",
                    "state": "not_signed_in",
                    "error": "not_signed_in",
                    "accounts": []
                }
            ]
        },
        "antigravity": {
            "attempted_at_ms": now_ms,
            "succeeded_at_ms": now_ms,
            "next_attempt_at_ms": now_ms + 300_000,
            "failures": 0,
            "providers": [{
                "provider": "antigravity",
                "state": "available",
                "error": null,
                "accounts": [{
                    "fetched_at_ms": now_ms,
                    "limits": [{
                        "id": "antigravity:5h:0",
                        "window": "5h",
                        "tier": "gemini",
                        "unit": "percent",
                        "used_fraction": 0.40,
                        "used": null,
                        "limit": null,
                        "remaining": null,
                        "unlimited": false,
                        "level": "ok",
                        "resets_at_ms": now_ms + 3_600_000
                    }]
                }]
            }]
        }
    });
    let snap_path = usage_dir.join("snapshot.json");
    fs::write(&snap_path, serde_json::to_vec(&snap).unwrap()).unwrap();
    fs::set_permissions(&snap_path, fs::Permissions::from_mode(0o600)).unwrap();

    // 1. `kit-coord roles` exposes resolved_model and live usage per role.
    let roles_out = h
        .command(&["roles"])
        .env("KIT_COORD_OMP_CONFIG", &omp_cfg)
        .output()
        .unwrap();
    assert!(roles_out.status.success());
    let roles: Value = serde_json::from_slice(&roles_out.stdout).unwrap();
    let list = roles["roles"].as_array().unwrap();
    assert_eq!(list.len(), 7, "two built-ins and five local test roles");
    let deep = list.iter().find(|r| r["name"] == "coord-deep").unwrap();
    assert_eq!(deep["resolved_model"], "anthropic/claude-opus-5-5:high");
    assert_eq!(deep["usage"]["provider"], "claude");
    assert_eq!(deep["usage"]["status"], "exhausted");
    assert_eq!(deep["usage"]["blocking_window"], "5h");
    assert!(deep["usage"]["resets_in_s"].as_u64().unwrap() > 0);

    let light = list.iter().find(|r| r["name"] == "coord-light").unwrap();
    assert_eq!(light["resolved_model"], "openai-codex/gpt-6-luna:medium");
    assert_eq!(light["usage"]["provider"], "codex");
    assert_eq!(light["usage"]["status"], "ok");

    let design = list.iter().find(|r| r["name"] == "coord-design").unwrap();
    assert_eq!(design["harness"], "agy");
    assert_eq!(design["steering"], true);
    assert_eq!(design["usage"]["provider"], "antigravity");
    assert_eq!(design["usage"]["tier"], "gemini");
    assert_eq!(design["usage"]["status"], "ok");

    // 2. `task start` refuses exhausted role before allocating anything.
    let ws = h.open();
    let brief = h.root().join("brief");
    fs::write(&brief, "Build feature").unwrap();
    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]}));
    let refused = h
        .command(&[
            "task",
            "start",
            "--ws",
            &ws,
            "--rung",
            "3",
            "--reason",
            "work",
            "--brief",
            brief.to_str().unwrap(),
            "--role",
            "coord-deep",
            "--space",
            "existing",
            "--reject",
            "1=no",
            "--reject",
            "2=no",
        ])
        .env("KIT_COORD_OMP_CONFIG", &omp_cfg)
        .output()
        .unwrap();
    rail_error(refused, "subscription_exhausted", 3);
    assert!(h.calls("hc").is_empty());

    // 3. `task start` refuses not_signed_in subscription as well.
    h.ok(&[
        "roles",
        "create",
        "cop-test",
        "--harness",
        "copilot",
        "--model",
        "gpt-6-luna",
    ]);
    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]}));
    let not_signed = h
        .command(&[
            "task",
            "start",
            "--ws",
            &ws,
            "--rung",
            "3",
            "--reason",
            "work",
            "--brief",
            brief.to_str().unwrap(),
            "--role",
            "cop-test",
            "--space",
            "existing",
            "--reject",
            "1=no",
            "--reject",
            "2=no",
        ])
        .env("KIT_COORD_OMP_CONFIG", &omp_cfg)
        .output()
        .unwrap();
    rail_error(not_signed, "subscription_not_signed_in", 3);

    // 4. `--allow-exhausted` overrides refusal on `coord-computer` and materializes `omp-computer.yml`.
    let task = format!("{ws}-t1");
    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]}));
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t7");
    term["state"] = json!("live");
    term["turn"] = json!(1);
    term["reported"] = json!(true);
    term["status"] = json!({"app":"omp","state":"working"});
    term["labels"] = json!({"herdr.space":"existing","coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task});
    h.queue("hc", "run", &json!({"address":"t7"}));
    h.queue("hc", "get", &term);
    let started_out = h
        .command(&[
            "task",
            "start",
            "--ws",
            &ws,
            "--rung",
            "3",
            "--reason",
            "computer task",
            "--brief",
            brief.to_str().unwrap(),
            "--role",
            "coord-computer",
            "--space",
            "existing",
            "--reject",
            "1=no",
            "--reject",
            "2=no",
            "--allow-exhausted",
        ])
        .env("KIT_COORD_OMP_CONFIG", &omp_cfg)
        .output()
        .unwrap();
    assert!(
        started_out.status.success(),
        "{}",
        String::from_utf8_lossy(&started_out.stderr)
    );
    let started: Value = serde_json::from_slice(&started_out.stdout).unwrap();
    assert_eq!(started["usage"]["status"], "exhausted");
    assert_eq!(started["usage"]["overridden"], true);
    let overlay = h.root().join("data/coord/omp-computer.yml");
    let overlay_text = fs::read_to_string(&overlay).unwrap();
    assert!(overlay_text.contains("computer:\n  enabled: true"));
    assert!(h.calls("hc").contains("--config"));
    assert!(h.calls("hc").contains(overlay.to_str().unwrap()));

    // 5. Role-level `steering: false` on an agy role refuses mid-turn steering.
    h.ok(&[
        "roles",
        "create",
        "agy-no-steer",
        "--from",
        "agy-default",
        "--steering",
        "false",
    ]);
    let no_steer_role = h.ok(&["roles", "agy-no-steer"]);
    assert_eq!(no_steer_role["steering"], false);
    let task2 = format!("{ws}-t2");
    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t9","pane":"p2"}]}]}]}));
    let mut term2 = term.clone();
    term2["address"] = json!("t9");
    term2["status"] = json!({"app":"antigravity","state":"working"});
    term2["labels"] = json!({"herdr.space":"existing","coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task2});
    h.queue("hc", "run", &json!({"address":"t9"}));
    h.queue("hc", "get", &term2);
    let started2 = h
        .command(&[
            "task",
            "start",
            "--ws",
            &ws,
            "--rung",
            "3",
            "--reason",
            "non-steering agy",
            "--brief",
            brief.to_str().unwrap(),
            "--role",
            "agy-no-steer",
            "--space",
            "existing",
            "--reject",
            "1=no",
            "--reject",
            "2=no",
        ])
        .env("KIT_COORD_OMP_CONFIG", &omp_cfg)
        .output()
        .unwrap();
    assert!(started2.status.success());
    h.queue("hc", "get", &term2);
    rail_error(
        h.run(&["task", "send", &task2, "--text", "steer blocked"]),
        "steering_unsupported",
        4,
    );
}

/// Start `role` as a rung-3 task in a fake existing space and return the
/// command output and the launched argv (the brief is the last element).
fn start_role(h: &Harness, ws: &str, role: &str) -> (Output, Vec<String>) {
    let n = h
        .journal()
        .iter()
        .filter(|e| e["kind"] == "task.placed")
        .count()
        + 1;
    let task = format!("{ws}-t{n}");
    let brief = h.root().join("brief");
    fs::write(&brief, "Do the work").unwrap();
    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]}));
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t7");
    term["state"] = json!("live");
    term["labels"] = json!({"herdr.space":"existing","coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task});
    h.queue("hc", "run", &json!({"address":"t7"}));
    h.queue("hc", "get", &term);
    let out = h.run(&[
        "task",
        "start",
        "--ws",
        ws,
        "--rung",
        "3",
        "--reason",
        "work",
        "--brief",
        brief.to_str().unwrap(),
        "--role",
        role,
        "--space",
        "existing",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);
    let calls = h.calls("hc");
    let argv = calls
        .split("\trun\t")
        .nth(n)
        .and_then(|run| run.split_once("\t--\t"))
        .map(|(_, argv)| argv.split('\t').map(str::to_owned).collect())
        .unwrap_or_default();
    (out, argv)
}

fn guidance(h: &Harness) -> String {
    let out = h.run(&["roles", "--guidance"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn roles_of_a_missing_harness_are_not_startable_and_left_out_of_guidance() {
    let h = Harness::new();
    h.install_roles();
    h.uninstall("agy");
    let roles = h.ok(&["roles"]);
    let role = |name: &str| {
        roles["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone()
    };
    for name in ["agy-default", "coord-design"] {
        assert_eq!(role(name)["available"], false);
        assert_eq!(
            role(name)["unavailable_reason"],
            "agy is not installed (not found on PATH)"
        );
    }
    assert_eq!(role("coord-light")["available"], true);

    let text = guidance(&h);
    assert!(!text.contains("agy"), "{text}");
    assert!(!text.contains("coord-design"), "{text}");
    assert!(!text.contains("coord-main"), "{text}");
    assert!(
        text.contains("- `coord-light`: Quick, mechanical"),
        "{text}"
    );
    assert!(text.contains("- `coord-computer`: Computer use"), "{text}");
    assert!(text.contains("Fallback: `coord-default`."), "{text}");
    assert!(text.contains("2 more role(s) cannot start here"), "{text}");

    let ws = h.open();
    let (out, argv) = start_role(&h, &ws, "agy-default");
    rail_error(out, "harness_unavailable", 3);
    assert!(argv.is_empty());
    assert!(!h.calls("hc").contains("\trun\t"));
    assert!(h.journal().iter().all(|e| e["kind"] != "task.placed"));

    let (out, argv) = start_role(&h, &ws, "coord-light");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        argv[..3],
        ["omp", "--model", "openai-codex/gpt-6-luna:medium"]
    );
}

#[test]
fn copilot_only_role_set_lists_guides_and_expands_start_commands() {
    let h = Harness::new();
    for program in ["agy", "claude", "codex"] {
        h.uninstall(program);
    }
    install_roles(
        &h,
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/coordinator/examples/roles/copilot-only"
        ),
    );
    let roles = h.ok(&["roles"])["roles"].as_array().unwrap().clone();
    let names: Vec<_> = roles.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "coord-deep",
            "coord-default",
            "coord-design",
            "coord-main",
            "coord-review",
            "copilot-cli"
        ]
    );
    for role in &roles {
        assert_eq!(role["available"], true, "{role}");
        assert_eq!(role["usage"]["provider"], "copilot", "{role}");
    }
    let design = roles.iter().find(|r| r["name"] == "coord-design").unwrap();
    assert_eq!(design["model"], "github-copilot/gemini-4-neon:high");
    assert_eq!(design["steering"], true);

    let text = guidance(&h);
    assert!(!text.contains("coord-main"), "{text}");
    assert!(!text.contains("cannot start here"), "{text}");
    assert!(text.contains(
        "- `coord-design`: UI/UX, interaction and visual design on Gemini Neon through the Copilot provider.\n  `omp`, model `github-copilot/gemini-4-neon:high`, `copilot` subscription, steering. Fallback: `coord-deep`.\n"
    ), "{text}");
    assert!(text.contains(
        "- `copilot-cli`: GitHub Copilot CLI delegate for a second opinion from another harness; no mid-turn steering.\n  `copilot`, model `auto`, `copilot` subscription. Fallback: `coord-default`.\n"
    ), "{text}");

    let ws = h.open();
    let (out, argv) = start_role(&h, &ws, "coord-design");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        argv[..4],
        [
            "omp",
            "--model",
            "github-copilot/gemini-4-neon:high",
            "--session-dir"
        ]
    );
    assert!(argv[5].starts_with("You are a UI/UX and visual design delegate"));
    let (out, argv) = start_role(&h, &ws, "copilot-cli");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(argv[..4], ["copilot", "--model", "auto", "--session-id"]);
    assert_eq!(argv[5], "-i");
    assert!(argv[6].starts_with("You are a delegate working on one task"));
}
