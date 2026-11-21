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
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kit-coord"));
        c.env("HOME", self.root())
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
    let coord = h.root().join("data/coord");
    fs::create_dir_all(coord.join("roles")).unwrap();
    fs::write(coord.join("roles/test.md"),"---\nharness: omp\nstart: runner {session_id} {brief}\nsession: preset\n---\nCoordinator\n").unwrap();
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
    let roles = h.ok(&["roles"])["roles"].as_array().unwrap().clone();
    let agy_role = roles.iter().find(|r| r["name"] == "agy-default").unwrap();
    assert_eq!(agy_role["harness"], "agy");
    assert_eq!(agy_role["model"], "gemini-4-neon-high");
    assert!(agy_role["effort"].is_null());
    assert_eq!(agy_role["session"], "agy-conversation");
    assert_eq!(agy_role["resumable"], true);
    assert_eq!(agy_role["steering"], false);

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
        &argv[sep + 1..sep + 5],
        &["agy", "--model", "gemini-4-neon-high", "-i"]
    );
    assert!(argv[sep + 5].starts_with("You are a delegate working on one task"));
    assert!(calls.contains("Implement harness support"));

    // Startup (turn 0): refused as agent_not_ready.
    rail_error(
        h.run(&["task", "send", &task, "--text", "too early"]),
        "agent_not_ready",
        4,
    );
    // Working after turn 1 on antigravity: refused explicitly as steering_unsupported.
    term["turn"] = json!(1);
    h.queue("hc", "get", &term);
    rail_error(
        h.run(&["task", "send", &task, "--text", "mid-turn steer"]),
        "steering_unsupported",
        4,
    );
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
        &resume_argv[rsep + 1..rsep + 5],
        &["agy", "--conversation", conv_id, "-i"]
    );
    assert!(!resume_argv[rsep + 1..rsep + 5].contains(&"--model"));
}

#[test]
fn claude_roles_expand_model_effort_and_resume_cleanly() {
    let h = Harness::new();
    let roles_dir = h.root().join("data/coord/roles");
    fs::create_dir_all(&roles_dir).unwrap();
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
    let claude_default = roles
        .iter()
        .find(|r| r["name"] == "claude-default")
        .unwrap();
    assert_eq!(claude_default["model"], "haiku");
    let claude_sonnet = roles.iter().find(|r| r["name"] == "claude-sonnet").unwrap();
    assert_eq!(claude_sonnet["model"], "sonnet");
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
fn task_trust_confirms_detected_startup_trust_dialog_and_refuses_mid_task_prompts() {
    let h = Harness::new();
    let ws = h.open();
    let task = format!("{ws}-t1");
    let brief = h.root().join("brief");
    fs::write(&brief, "Work in new worktree").unwrap();
    h.queue("herdr-app", "ls", &json!({"spaces":[{"id":"existing","name":"Existing","folder":h.root(),"machine":null,"tabs":[{"id":"tab1","name":"Tab","terminals":[{"terminal":"t7","pane":"p1"}]}]}]}));
    let mut term: Value =
        serde_json::from_str(include_str!("../src/coord/testdata/hc-get-agent.json")).unwrap();
    term["address"] = json!("t7");
    term["state"] = json!("live");
    term["turn"] = json!(0);
    term["reported"] = json!(false);
    term["status"] = Value::Null;
    term["labels"] = json!({"herdr.space":"existing","coord.managed":"1","coord.role":"delegate","coord.ws":ws,"coord.task":task});
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
        "agy-default",
        "--space",
        "existing",
        "--reject",
        "1=no",
        "--reject",
        "2=no",
    ]);

    // Before watcher records task.start_blocked: refused with exit 4 (not_startup_dialog).
    rail_error(h.run(&["task", "trust", &task]), "not_startup_dialog", 4);
    assert!(!h.calls("hc").contains("\tkeys\t"));

    // Delegate callers are forbidden from confirming trust prompts.
    rail_error(
        h.command(&["task", "trust", &task])
            .env("HC_TERMINAL_ID", "t7")
            .output()
            .unwrap(),
        "forbidden_caller",
        3,
    );

    // Append watcher-detected task.start_blocked to the journal and invalidate state cache.
    let journal_dir = h.root().join("data/coord/journal");
    let journal_file = fs::read_dir(&journal_dir)
        .unwrap()
        .flatten()
        .find(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .unwrap()
        .path();
    let entries = h.journal();
    let sb_seq = entries.last().unwrap()["seq"].as_u64().unwrap() + 1;
    let sb_entry = json!({
        "v": 1,
        "seq": sb_seq,
        "ts_ms": 999999,
        "actor": "watcher",
        "kind": "task.start_blocked",
        "ws": ws,
        "task": task,
        "data": {"terminal": "t7", "after_secs": 30}
    });
    let mut existing = fs::read_to_string(&journal_file).unwrap();
    existing.push_str(&format!("{sb_entry}\n"));
    fs::write(&journal_file, existing).unwrap();
    let _ = fs::remove_file(h.root().join("data/coord/state.json"));

    // Non-trust startup dialog (codex update offer) is refused.
    h.queue(
        "hc",
        "read",
        &json!({"text":"Update available! 0.168 -> 0.169. Press enter to update.","complete":true,"how":"screen","why":null}),
    );
    rail_error(h.run(&["task", "trust", &task]), "not_trust_prompt", 3);
    assert!(!h.calls("hc").contains("\tkeys\t"));

    // Invalid key name is refused before touching hc.
    rail_error(
        h.run(&["task", "trust", &task, "--key", "ctrl+c"]),
        "invalid_keys",
        3,
    );

    // Real agy folder trust prompt is confirmed with default `enter` and recorded in journal.
    h.queue(
        "hc",
        "read",
        &json!({"text":"Trust this folder?\n> Yes, trust this folder\n  No, exit","complete":true,"how":"screen","why":null}),
    );
    h.queue("hc", "keys", &json!({"turn":0,"status":null}));
    let confirmed = h.ok(&["task", "trust", &task]);
    assert_eq!(confirmed["task"], task);
    assert_eq!(confirmed["confirmed"], true);
    assert_eq!(confirmed["terminal"], "t7");
    assert_eq!(confirmed["keys"], json!(["enter"]));
    assert_eq!(confirmed["start_blocked_seq"], sb_seq);

    let calls = h.calls("hc");
    assert!(
        calls.contains("\tread\t--json\t--lines\t0\tt7\n"),
        "expected visible-screen read: {calls}"
    );
    assert!(
        calls.contains("\tkeys\t--json\tt7\tenter\n"),
        "expected enter keystroke: {calls}"
    );
    let journal = h.journal();
    let last = journal.last().unwrap();
    assert_eq!(last["kind"], "task.trust_confirmed");
    assert_eq!(last["task"], task);
    assert_eq!(last["data"]["terminal"], "t7");
    assert_eq!(last["data"]["keys"], json!(["enter"]));
    assert_eq!(last["data"]["start_blocked_seq"], sb_seq);

    // Custom allowlisted keys (e.g. `down enter`) also work on claude/copilot folder trust.
    h.queue(
        "hc",
        "read",
        &json!({"text":"Do you trust the files in this folder?\n1. Yes, I trust this folder","complete":true,"how":"screen","why":null}),
    );
    let multi = h.ok(&["task", "trust", &task, "--key", "down", "--key", "enter"]);
    assert_eq!(multi["keys"], json!(["down", "enter"]));
    assert!(h.calls("hc").contains("\tkeys\t--json\tt7\tdown\tenter\n"));

    // Mid-task permission prompt (status: blocked) is strictly refused even if start_blocked was set.
    let keys_count_before = h
        .calls("hc")
        .lines()
        .filter(|l| l.contains("\tkeys\t"))
        .count();
    term["status"] = json!({"app":"antigravity","state":"blocked","msg":"Run rm -rf?"});
    h.queue("hc", "get", &term);
    rail_error(h.run(&["task", "trust", &task]), "agent_blocked", 3);

    // Already-turned delegate (turn >= 1) is strictly refused.
    term["status"] = json!({"app":"antigravity","state":"idle"});
    term["turn"] = json!(1);
    term["reported"] = json!(true);
    h.queue("hc", "get", &term);
    rail_error(h.run(&["task", "trust", &task]), "not_startup_dialog", 3);

    // Working delegate (turn 0, status working) is refused.
    term["turn"] = json!(0);
    term["status"] = json!({"app":"antigravity","state":"working"});
    h.queue("hc", "get", &term);
    rail_error(h.run(&["task", "trust", &task]), "not_startup_dialog", 3);

    assert_eq!(
        h.calls("hc")
            .lines()
            .filter(|l| l.contains("\tkeys\t"))
            .count(),
        keys_count_before
    );
}
