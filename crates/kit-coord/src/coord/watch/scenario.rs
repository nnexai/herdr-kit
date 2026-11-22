//! One scripted watcher run against a simulated terminal world and a stepped
//! clock: rule evaluation, ticks, reconcile passes and releases. The transcript
//! (rule actions, adapter calls, journal entries and read models) must match
//! `testdata/watch-scenario.txt`.
//!
//! To record a new transcript after an intended behaviour change, run the test
//! with `KIT_COORD_SCENARIO_OUT=<file>` and replace the testdata file with it.
use super::*;
use crate::coord::{
    adapter::{
        AgentStatus, App, AppSnapshot, AppSpace, AppTab, AppTerm, Foreground, RunSpec, SendOutcome,
    },
    paths::CoordPaths,
    release::{self, Mode},
    views,
};
use std::cell::{Cell, RefCell};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

const T0: u64 = 1_800_000_000_000;

fn git(dir: &Path, args: &[&str]) {
    let out = kit_core::git_env::git()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn reconcile_pass(ctx: &Ctx, j: &Journal, facts: &mut Facts) -> Result<Vec<Entry>> {
    reconcile::reconcile(ctx, j, facts)
}

/// The terminals and spaces the fake `hc` and `herdr-app` report and change.
#[derive(Default)]
struct World {
    terms: BTreeMap<String, Term>,
    spaces: Vec<AppSpace>,
    calls: Vec<String>,
    failing_sends: u32,
}
impl World {
    fn term(&mut self, address: &str) -> &mut Term {
        self.terms.get_mut(address).unwrap()
    }
    fn set(&mut self, address: &str, word: &str, turn: u64, at_secs: u64) {
        let term = self.term(address);
        term.status.as_mut().unwrap().state = word.into();
        term.turn = turn;
        term.status_ms = Some(T0 + at_secs * 1000);
    }
    fn remove(&mut self, address: &str) {
        self.terms.remove(address);
        for tab in self.spaces.iter_mut().flat_map(|s| &mut s.tabs) {
            tab.terminals.retain(|t| t.terminal != address);
        }
    }
    fn space(&mut self, id: &str, name: &str, folder: &Path, tabs: &[(&str, &str)]) {
        self.spaces.push(AppSpace {
            id: id.into(),
            name: name.into(),
            folder: folder.to_string_lossy().into_owned(),
            machine: None,
            tabs: tabs
                .iter()
                .map(|(tab, terminal)| AppTab {
                    id: (*tab).into(),
                    name: (*tab).into(),
                    terminals: vec![AppTerm {
                        terminal: (*terminal).into(),
                        pane: format!("pane-{terminal}"),
                    }],
                })
                .collect(),
        });
    }
}
struct WorldHc(Rc<RefCell<World>>);
impl Hc for WorldHc {
    fn run(&self, spec: &RunSpec) -> Result<Term> {
        anyhow::bail!("the scenario does not launch {}", spec.name)
    }
    fn get(&self, terminal: &str) -> Result<Option<Term>> {
        let mut world = self.0.borrow_mut();
        world.calls.push(format!("hc get {terminal}"));
        Ok(world.terms.get(terminal).cloned())
    }
    fn list(&self, all: bool, label: Option<(&str, &str)>) -> Result<Vec<String>> {
        let mut world = self.0.borrow_mut();
        world
            .calls
            .push(format!("hc list all={all} label={label:?}"));
        Ok(world
            .terms
            .values()
            .filter(|t| all || t.live())
            .filter(|t| label.is_none_or(|(k, v)| t.label(k) == Some(v)))
            .map(|t| t.address.clone())
            .collect())
    }
    fn send(&self, terminal: &str, _text: &str) -> Result<SendOutcome> {
        anyhow::bail!("the scenario does not send synchronously to {terminal}")
    }
    fn send_command(&self, terminal: &str, text: &str) -> Command {
        let mut world = self.0.borrow_mut();
        world.calls.push(format!("hc send {terminal} {text}"));
        let mut command = Command::new("sh");
        if world.failing_sends > 0 {
            world.failing_sends -= 1;
            command.args(["-c", "printf 'draft rejected' >&2; exit 1"]);
        } else {
            command.args(["-c", r#"printf '{"turn":1,"started":true,"status":null}'"#]);
        }
        command
    }
    fn label(&self, terminal: &str, labels: &[(&str, &str)]) -> Result<()> {
        self.0
            .borrow_mut()
            .calls
            .push(format!("hc label {terminal} {labels:?}"));
        Ok(())
    }
    fn kill(&self, terminal: &str) -> Result<()> {
        let mut world = self.0.borrow_mut();
        world.calls.push(format!("hc kill {terminal}"));
        if let Some(term) = world.terms.get_mut(terminal) {
            term.state = "exited".into();
        }
        Ok(())
    }
    fn rm(&self, terminal: &str) -> Result<()> {
        let mut world = self.0.borrow_mut();
        world.calls.push(format!("hc rm {terminal}"));
        world.remove(terminal);
        Ok(())
    }
    fn events_command(&self) -> Command {
        Command::new("false")
    }
}
struct WorldApp(Rc<RefCell<World>>);
impl App for WorldApp {
    fn ls(&self) -> Result<AppSnapshot> {
        let mut world = self.0.borrow_mut();
        world.calls.push("app ls".into());
        Ok(AppSnapshot {
            spaces: world.spaces.clone(),
        })
    }
    fn command(&self, name: &str, args: Value) -> Result<Value> {
        let mut world = self.0.borrow_mut();
        world.calls.push(format!("app {name} {args}"));
        let id = args["id"].as_str().unwrap_or_default();
        let new_name = args["name"].as_str().unwrap_or_default();
        match name {
            "tab.close" => {
                for space in &mut world.spaces {
                    space.tabs.retain(|t| t.id != id);
                }
            }
            "space.close" => world.spaces.retain(|s| s.id != id),
            "tab.rename" => {
                for tab in world.spaces.iter_mut().flat_map(|s| &mut s.tabs) {
                    if tab.id == id {
                        tab.name = new_name.into();
                    }
                }
            }
            "space.rename" => {
                for space in &mut world.spaces {
                    if space.id == id {
                        space.name = new_name.into();
                    }
                }
            }
            _ => {}
        }
        Ok(json!({}))
    }
}

fn agent(
    address: &str,
    role: &str,
    pid: u32,
    word: &str,
    turn: u64,
    task: Option<&TaskId>,
) -> Term {
    let mut labels = BTreeMap::from([
        ("coord.managed".to_owned(), "1".to_owned()),
        ("coord.role".to_owned(), role.to_owned()),
    ]);
    if let Some(task) = task {
        labels.insert("coord.task".into(), task.as_str().into());
        labels.insert("coord.ws".into(), task.ws().as_str().into());
    }
    Term {
        address: address.into(),
        state: "live".into(),
        status: Some(AgentStatus {
            state: word.into(),
            app: None,
            kind: None,
            msg: None,
        }),
        status_ms: Some(T0),
        turn,
        reported: true,
        labels,
        pid: Some(pid),
        created_ms: T0 - 1000,
        cwd: "/".into(),
        foreground: Some(Foreground {
            pid: pid as i32,
            name: "agent".into(),
            cwd: "/".into(),
        }),
        reason: None,
        name: None,
    }
}
fn person(address: &str, cwd: &Path) -> Term {
    Term {
        labels: BTreeMap::new(),
        cwd: cwd.to_string_lossy().into_owned(),
        ..agent(address, "", 9000, "idle", 0, None)
    }
}

struct Scenario {
    root: PathBuf,
    ctx: Ctx,
    j: Journal,
    core: Core,
    cfg: Config,
    world: Rc<RefCell<World>>,
    clock: Rc<Cell<u64>>,
    seen_seq: u64,
    out: String,
}
impl Scenario {
    fn new(root: &Path) -> Self {
        let world = Rc::new(RefCell::new(World::default()));
        let clock = Rc::new(Cell::new(T0));
        let now = clock.clone();
        let ctx = Ctx {
            root: root.into(),
            paths: CoordPaths::new(root),
            hc: Box::new(WorldHc(world.clone())),
            app: Box::new(WorldApp(world.clone())),
            clock: Box::new(move || now.get()),
            op: "scenario".into(),
            cause: None,
            path: crate::coord::stub_path(root),
        };
        ctx.paths.ensure().unwrap();
        let j = ctx.journal().unwrap();
        Self {
            root: root.into(),
            ctx,
            j,
            core: Core {
                facts: Facts::default(),
                release: "r1".into(),
                started_ms: T0,
                loaded_seq: 0,
                pending: BTreeMap::new(),
                retry_after: BTreeMap::new(),
            },
            cfg: Config::default(),
            world,
            clock,
            seen_seq: 0,
            out: String::new(),
        }
    }
    fn step(&mut self, secs: u64, title: &str) {
        self.clock.set(T0 + secs * 1000);
        writeln!(self.out, "\n== +{secs}s {title}").unwrap();
    }
    /// Appends what a command of `actor` would have journaled.
    fn put(&mut self, actor: Actor, task: Option<&TaskId>, ws: Option<&WsId>, kind: Value) {
        let kind: Kind = serde_json::from_value(kind).unwrap();
        assert!(!matches!(kind, Kind::Unknown));
        self.j
            .lock()
            .unwrap()
            .append(
                &actor,
                self.clock.get(),
                vec![(
                    ws.cloned().or_else(|| task.map(TaskId::ws)),
                    task.cloned(),
                    kind,
                )],
            )
            .unwrap();
    }
    /// Delivers an `hc events` line of type `kind` for a terminal.
    fn event_of(&mut self, terminal: &str, kind: &str) {
        self.core
            .on_line(
                Line::Event {
                    terminal: terminal.into(),
                    kind: kind.into(),
                },
                self.ctx.hc.as_ref(),
            )
            .unwrap();
    }
    /// Delivers an `hc events` line for a terminal whose state changed.
    fn event(&mut self, terminal: &str) {
        self.event_of(terminal, "status");
    }
    /// Runs the watcher tick; like the watcher loop, a failed tick is only logged.
    fn core_tick(&mut self) {
        if let Err(e) = self.core.tick(&self.ctx, &self.j, &self.cfg) {
            writeln!(self.out, "tick: error: {e:#}").unwrap();
        }
    }
    fn rules(&mut self) {
        let state = self.j.read_state().unwrap();
        let actions = rules::evaluate(&state, &self.core.facts, &self.cfg, self.ctx.now());
        if actions.is_empty() {
            writeln!(self.out, "rule: none").unwrap();
        }
        for action in actions {
            writeln!(self.out, "rule: {action:?}").unwrap();
        }
    }
    /// One watcher tick, followed by further ticks until its deliveries are recorded.
    ///
    /// While deliveries are in flight the watcher ticks once more: nothing can have
    /// finished yet, because the delivery tasks only run when this test awaits. The
    /// next tick then runs once every started delivery has finished, so what a tick
    /// sees never depends on how fast the delivery process was.
    async fn tick(&mut self) {
        self.flush();
        self.rules();
        self.core_tick();
        while !self.core.pending.is_empty() {
            self.core_tick();
            for pending in self.core.pending.values_mut() {
                let result = loop {
                    match pending.result.try_recv() {
                        Ok(result) => break result,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            tokio::time::sleep(Duration::from_millis(1)).await
                        }
                        Err(e) => panic!("delivery task ended without a result: {e}"),
                    }
                };
                let (tx, rx) = std::sync::mpsc::channel();
                tx.send(result).unwrap();
                pending.result = rx;
            }
            self.core_tick();
        }
        self.flush();
    }
    fn reconcile(&mut self) {
        match reconcile_pass(&self.ctx, &self.j, &mut self.core.facts) {
            Ok(_) => writeln!(self.out, "reconcile: ok").unwrap(),
            Err(e) => writeln!(self.out, "reconcile: error: {e:#}").unwrap(),
        }
        self.flush();
    }
    fn release(&mut self, ws: &WsId) {
        match release::release_ws(&self.ctx, &self.j, &Actor::Coordinator, ws, Mode::Command) {
            Ok(outcome) => writeln!(
                self.out,
                "release {}: cleaned={} released={:?} retained={:?}",
                ws.as_str(),
                outcome.cleaned,
                outcome
                    .released
                    .iter()
                    .map(|r| r.name.as_str())
                    .collect::<Vec<_>>(),
                outcome.retained.map(|r| r.reason_code),
            )
            .unwrap(),
            Err(e) => writeln!(self.out, "release {}: error: {e:#}", ws.as_str()).unwrap(),
        }
        self.flush();
    }
    /// Writes the adapter calls and journal entries since the previous flush.
    fn flush(&mut self) {
        for call in self.world.borrow_mut().calls.drain(..) {
            writeln!(self.out, "call: {call}").unwrap();
        }
        for e in self.j.read_after(self.seen_seq).unwrap() {
            self.seen_seq = e.seq;
            writeln!(
                self.out,
                "#{} +{}ms {} {} {} {} {}",
                e.seq,
                e.ts_ms - T0,
                serde_json::to_string(&e.actor).unwrap(),
                e.kind,
                e.ws.as_ref().map_or("-", |id| id.as_str()),
                e.task.as_ref().map_or("-", |id| id.as_str()),
                serde_json::to_string(&e.data).unwrap(),
            )
            .unwrap();
        }
    }
    fn board(&mut self) {
        let state = self.j.read_state().unwrap();
        let word = state.coordinator.terminal.as_ref().and_then(|t| {
            self.world
                .borrow()
                .terms
                .get(t)
                .and_then(|t| t.word().map(str::to_owned))
        });
        let now = self.ctx.now();
        writeln!(
            self.out,
            "board: {}",
            serde_json::to_string_pretty(&views::board(&state, word.as_deref(), now, true))
                .unwrap()
        )
        .unwrap();
        writeln!(
            self.out,
            "board text:\n{}",
            views::board_text(&state, now, true)
        )
        .unwrap();
    }
    fn state(&mut self) {
        let state = self.j.read_state().unwrap();
        let waking: Vec<Entry> = self
            .j
            .read_after(state.cursor)
            .unwrap()
            .into_iter()
            .filter(|e| state.waking.contains(&e.seq))
            .collect();
        writeln!(
            self.out,
            "startup read:\n{}",
            views::startup_read(&state, &waking, self.ctx.now())
        )
        .unwrap();
        writeln!(
            self.out,
            "state: {}",
            serde_json::to_string_pretty(&state).unwrap()
        )
        .unwrap();
    }
    /// The transcript with the temporary directory, commit ids and errno values made stable.
    fn transcript(&self) -> String {
        let text = self.out.replace(&*self.root.to_string_lossy(), "<root>");
        let mut stable = String::with_capacity(text.len());
        let mut run = String::new();
        for c in text.chars().chain(['\n']) {
            if c.is_ascii_digit() || ('a'..='f').contains(&c) {
                run.push(c);
                continue;
            }
            if run.len() == 40 {
                stable.push_str("<sha>");
            } else if c == ')' && stable.ends_with("(os error ") {
                stable.push_str("<errno>");
            } else {
                stable.push_str(&run);
            }
            run.clear();
            stable.push(c);
        }
        stable.pop();
        stable
    }
}

#[tokio::test]
async fn scripted_watcher_run_matches_recorded_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut s = Scenario::new(&root);
    let world = s.world.clone();

    // A repository with a delegate worktree on branch `feature`.
    let repo = root.join("repo");
    let tree = root.join("trees/feature");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            tree.to_str().unwrap(),
        ],
    );
    let person_dir = root.join("person");
    std::fs::create_dir_all(&person_dir).unwrap();

    let a = WsId::parse("ws-aaaaaa").unwrap();
    let a1 = TaskId::new(&a, 1);
    let b = WsId::parse("ws-bbbbbb").unwrap();
    let (b1, b2, b3) = (TaskId::new(&b, 1), TaskId::new(&b, 2), TaskId::new(&b, 3));
    let b4 = TaskId::new(&b, 4);
    let home = s.ctx.paths.home.clone();
    let session = json!({"mode":"dir","id":"session-a1","dir":root.join("sessions/a1")});
    let placed = |title: &str, session: Value| {
        json!({"kind":"task.placed","data":{"rung":2,"reason":"scenario","rejected":[{"rung":1,"reason":"no session"}],
            "role":"omp-default","harness":"omp","title":title,"brief":root.join("brief"),"session":session,"brief_bytes":12}})
    };
    let started = |terminal: &str, tab: &str, space: &str, cwd: &Path, pid: u32, session: Value| {
        json!({"kind":"task.started","data":{"terminal":terminal,"tab":tab,"tab_name":"task","space":space,"cwd":cwd,
            "pid":pid,"created_ms":T0 - 1000,"session":session,"resumed":false}})
    };

    s.step(0, "the watcher starts; the coordinator and four delegates are running, a fifth task never launched");
    s.put(
        Actor::Watcher,
        None,
        None,
        json!({"kind":"watcher.started","data":{"release":"r0","terminal":"w0","pid":4000}}),
    );
    s.put(
        Actor::Watcher,
        None,
        None,
        json!({"kind":"watcher.stopped","data":{"reason":"release_changed"}}),
    );
    s.put(
        Actor::Watcher,
        None,
        None,
        json!({"kind":"watcher.started","data":{"release":"r1","pid":4001}}),
    );
    s.put(Actor::Cli, None, None, json!({"kind":"res.created","data":{"type":"space","name":"sp-coord","folder":home,"label":"coordinator"}}));
    s.put(Actor::Cli, None, None, json!({"kind":"res.created","data":{"type":"tab","name":"c1","space":"sp-coord","tab":"tab-c1"}}));
    s.put(Actor::Cli, None, None, json!({"kind":"coord.session_started","data":{"role":"coord-main","terminal":"c1","pid":100,
        "created_ms":T0 - 1000,"space":"sp-coord","tab":"tab-c1","release":"r1","reason":"start","brief":root.join("brief-c")}}));
    s.put(Actor::Coordinator, None, Some(&a), json!({"kind":"ws.opened","data":{"title":"Ship feature","repo":repo,
        "delivery_target":"merged into main","delivery_branch":"main","purpose":"Scenario repository","request":"Ship it"}}));
    s.put(
        Actor::Coordinator,
        Some(&a1),
        None,
        placed("Implement feature", session.clone()),
    );
    s.put(Actor::Coordinator, Some(&a1), None, json!({"kind":"res.created","data":{"type":"branch","name":"feature","repo":repo,"base":"main"}}));
    s.put(Actor::Coordinator, Some(&a1), None, json!({"kind":"res.created","data":{"type":"worktree","name":tree,"repo":repo,"branch":"feature"}}));
    s.put(Actor::Coordinator, Some(&a1), None, json!({"kind":"res.created","data":{"type":"space","name":"sp-a","folder":tree,"label":"repo · Ship feature"}}));
    s.put(Actor::Coordinator, Some(&a1), None, json!({"kind":"res.created","data":{"type":"tab","name":"d1","space":"sp-a","tab":"tab-d1"}}));
    s.put(
        Actor::Coordinator,
        Some(&a1),
        None,
        started("d1", "tab-d1", "sp-a", &tree, 201, session),
    );
    s.put(
        Actor::Coordinator,
        None,
        Some(&b),
        json!({"kind":"ws.opened","data":{"title":"Research"}}),
    );
    s.put(Actor::Coordinator, None, Some(&b), json!({"kind":"res.adopted","data":{"type":"space","name":"sp-person","folder":person_dir}}));
    let resumable = json!({"mode":"dir","id":"session-b4"});
    for (task, terminal, tab, pid, title, session) in [
        (&b1, "d2", "tab-d2", 202, "Survey options", Value::Null),
        (&b2, "d3", "tab-d3", 203, "Measure baseline", Value::Null),
        (&b4, "d4", "tab-d4", 204, "Write summary", resumable),
    ] {
        s.put(
            Actor::Coordinator,
            Some(task),
            None,
            placed(title, session.clone()),
        );
        s.put(Actor::Coordinator, Some(task), None, json!({"kind":"res.created","data":{"type":"tab","name":terminal,"space":"sp-person","tab":tab}}));
        s.put(
            Actor::Coordinator,
            Some(task),
            None,
            started(terminal, tab, "sp-person", &person_dir, pid, session),
        );
    }
    s.put(
        Actor::Coordinator,
        Some(&b3),
        None,
        placed("Never launched", Value::Null),
    );
    s.put(
        Actor::Coordinator,
        Some(&b3),
        None,
        json!({"kind":"res.intent","data":{"type":"tab","name":b3.as_str(),"space":"sp-person"}}),
    );
    {
        let mut w = world.borrow_mut();
        for term in [
            agent("c1", "coordinator", 100, "done", 1, None),
            agent("d1", "delegate", 201, "working", 1, Some(&a1)),
            agent("d2", "delegate", 202, "idle", 0, Some(&b1)),
            agent("d3", "delegate", 203, "working", 1, Some(&b2)),
            agent("d4", "delegate", 204, "working", 1, Some(&b4)),
            person("p9", &person_dir),
        ] {
            w.terms.insert(term.address.clone(), term);
        }
        w.space("sp-coord", "coordinator", &home, &[("tab-c1", "c1")]);
        w.space("sp-a", "feature", &tree, &[("tab-d1", "d1")]);
        w.space(
            "sp-person",
            "person",
            &person_dir,
            &[
                ("tab-d2", "d2"),
                ("tab-d3", "d3"),
                ("tab-d4", "d4"),
                ("tab-p9", "p9"),
            ],
        );
    }
    s.flush();
    let managed: Vec<Term> = world
        .borrow()
        .terms
        .values()
        .filter(|t| t.managed())
        .cloned()
        .collect();
    s.core
        .on_line(Line::Snapshot(managed), s.ctx.hc.as_ref())
        .unwrap();

    s.step(1, "first tick records the turns already taken");
    s.tick().await;

    s.step(
        31,
        "a delegate that never started its turn is start-blocked",
    );
    s.tick().await;
    s.step(
        32,
        "a command holds the coordinator terminal; the inbox nudge waits",
    );
    {
        let _busy = s.ctx.terminal_lock("c1").unwrap();
        s.tick().await;
    }
    s.step(33, "the coordinator is told about its inbox");
    s.tick().await;

    s.step(45, "the coordinator reads its inbox and finishes a turn");
    world.borrow_mut().set("c1", "working", 1, 41);
    s.event("c1");
    world.borrow_mut().set("c1", "done", 2, 44);
    s.event("c1");
    let state = s.j.read_state().unwrap();
    s.put(
        Actor::Coordinator,
        None,
        None,
        json!({"kind":"inbox.drained","data":{"cursor":state.last_seq,"count":state.waking.len()}}),
    );
    s.tick().await;

    s.step(50, "the start-blocked delegate begins working");
    world.borrow_mut().set("d2", "working", 1, 50);
    s.event("d2");
    s.tick().await;

    s.step(
        114,
        "an idle delegate is due a nudge, but its terminal now runs another process",
    );
    world.borrow_mut().set("d1", "done", 1, 50);
    s.event("d1");
    world.borrow_mut().term("d1").pid = Some(777);
    s.tick().await;
    s.step(115, "the delegate is nudged; the first delivery fails");
    world.borrow_mut().term("d1").pid = Some(201);
    world.borrow_mut().failing_sends = 1;
    s.tick().await;
    s.step(144, "one second before the delivery backoff ends: no retry");
    s.tick().await;
    s.step(145, "the nudge is retried when the backoff ends");
    s.tick().await;
    s.step(
        444,
        "one second before the nudged delegate counts as stalled",
    );
    s.tick().await;
    s.step(445, "the nudged delegate stayed silent and is stalled");
    s.tick().await;
    s.tick().await;

    s.step(500, "the delegate reports that it is blocked");
    s.put(Actor::Task(a1.clone()), Some(&a1), None, json!({"kind":"task.reported","data":{"outcome":"blocked","summary":"Need deploy credentials","result":null}}));
    s.put(
        Actor::Task(a1.clone()),
        Some(&a1),
        None,
        json!({"kind":"task.progress","data":{"summary":"Waiting for credentials"}}),
    );
    s.tick().await;
    s.step(520, "the coordinator acknowledges; the blocked item stays");
    world.borrow_mut().set("c1", "done", 3, 519);
    s.event("c1");
    let state = s.j.read_state().unwrap();
    s.put(
        Actor::Coordinator,
        None,
        None,
        json!({"kind":"inbox.drained","data":{"cursor":state.last_seq,"count":state.waking.len()}}),
    );
    s.tick().await;
    s.board();

    s.step(1099, "one second before the first blocked reminder");
    s.tick().await;
    s.step(1100, "first blocked reminder");
    s.tick().await;
    s.step(2299, "the second reminder waits twice as long");
    s.tick().await;
    s.step(2300, "second blocked reminder");
    s.tick().await;
    s.step(2400, "the coordinator asks the person; reminders pause");
    s.put(Actor::Coordinator, Some(&a1), None, json!({"kind":"question.asked","data":{"qid":"q1","text":"Which credentials?","options":["staging","production"]}}));
    s.flush();
    s.step(9000, "no reminder while the person is asked");
    s.tick().await;
    s.board();
    s.step(9100, "the person answers; the blocked item is resolved");
    s.put(
        Actor::User,
        None,
        None,
        json!({"kind":"question.answered","data":{"qid":"q1","text":"staging"}}),
    );
    s.tick().await;

    s.step(
        9200,
        "a terminal asks for attention while a question is open, then continues",
    );
    world.borrow_mut().set("d2", "blocked", 1, 9190);
    world.borrow_mut().term("d2").status.as_mut().unwrap().msg = Some("Approve the plan?".into());
    s.event("d2");
    s.tick().await;
    s.put(
        Actor::Coordinator,
        Some(&b1),
        None,
        json!({"kind":"question.asked","data":{"qid":"q2","text":"Approve the plan?"}}),
    );
    s.tick().await;
    s.step(
        9256,
        "the waiting terminal sits idle; a blocked delegate is not nudged",
    );
    world.borrow_mut().set("d2", "done", 1, 9195);
    s.event("d2");
    s.tick().await;
    s.step(9260, "the terminal continues on its own");
    world.borrow_mut().set("d2", "working", 2, 9255);
    s.event("d2");
    s.tick().await;
    s.tick().await;
    s.step(9300, "a terminal reports an error");
    world.borrow_mut().set("d3", "error", 1, 9290);
    s.event("d3");
    s.tick().await;
    s.tick().await;

    s.step(9400, "one terminal is replaced, one exits, one is removed");
    world.borrow_mut().term("d2").pid = Some(999);
    s.event("d2");
    world.borrow_mut().term("d3").state = "exited".into();
    s.event("d3");
    world.borrow_mut().remove("d4");
    s.event_of("d4", "removed");
    s.tick().await;
    s.tick().await;

    s.step(
        9500,
        "reconcile settles the stale intent and finds a person terminal in an owned space",
    );
    world.borrow_mut().remove("d3");
    {
        let mut w = world.borrow_mut();
        let p8 = person("p8", &tree);
        w.terms.insert("p8".into(), p8);
        w.spaces[1].tabs.push(AppTab {
            id: "tab-p8".into(),
            name: "shell".into(),
            terminals: vec![AppTerm {
                terminal: "p8".into(),
                pane: "pane-p8".into(),
            }],
        });
    }
    s.reconcile();
    s.step(9620, "a second reconcile pass repeats nothing");
    s.reconcile();
    s.step(
        9621,
        "the coordinator began a turn the watcher has not heard of; no nudge",
    );
    world.borrow_mut().term("c1").status.as_mut().unwrap().state = "working".into();
    s.tick().await;
    s.step(9622, "the coordinator is idle again and is nudged");
    world.borrow_mut().term("c1").status.as_mut().unwrap().state = "done".into();
    s.tick().await;

    s.step(
        9700,
        "the coordinator records the remaining kinds of entries",
    );
    s.put(Actor::Coordinator, Some(&a1), None, json!({"kind":"task.sent","data":{"message":root.join("message"),"via":"send","terminal":"d1","started":true}}));
    s.put(
        Actor::Coordinator,
        Some(&a1),
        None,
        json!({"kind":"task.note","data":{"text":"Credentials are in the vault"}}),
    );
    s.put(Actor::Coordinator, None, Some(&a), json!({"kind":"decision","data":{"what":"Use staging","why":"The person said so","tier":"digest","qid":"q1"}}));
    s.put(
        Actor::User,
        None,
        None,
        json!({"kind":"instruction.set","data":{"id":"i1","text":"Prefer small commits"}}),
    );
    s.put(
        Actor::User,
        None,
        None,
        json!({"kind":"instruction.set","data":{"id":"i2","text":"Temporary"}}),
    );
    s.put(
        Actor::User,
        None,
        None,
        json!({"kind":"instruction.set","data":{"id":"i2"}}),
    );
    s.put(
        Actor::Coordinator,
        None,
        None,
        json!({"kind":"thread.opened","data":{"id":"th1","title":"Release plan","note":"draft"}}),
    );
    s.put(
        Actor::Coordinator,
        None,
        None,
        json!({"kind":"thread.opened","data":{"id":"th2","title":"Closed thread"}}),
    );
    s.put(
        Actor::Coordinator,
        None,
        None,
        json!({"kind":"thread.closed","data":{"id":"th2","title":"Closed thread"}}),
    );
    s.put(
        Actor::User,
        None,
        None,
        json!({"kind":"rating","data":{"target":a1.as_str(),"value":"good","note":"quick"}}),
    );
    s.put(
        Actor::User,
        None,
        None,
        json!({"kind":"rating","data":{"target":b.as_str(),"value":"bad"}}),
    );
    s.put(Actor::User, None, None, json!({"kind":"correction","data":{"what":"placement","why":"wrong space","ref":a1.as_str()}}));
    s.put(
        Actor::Coordinator,
        None,
        None,
        json!({"kind":"scout.result","data":{"question":"Where is auth?","summary":"src/auth"}}),
    );
    s.put(
        Actor::Cli,
        None,
        None,
        json!({"kind":"config.changed","data":{"key":"stall_secs","old":300,"new":600}}),
    );
    s.put(
        Actor::User,
        None,
        None,
        json!({"kind":"coord.sent","data":{"text":"hello","terminal":"c1","qid":null}}),
    );
    s.put(Actor::Coordinator, Some(&a1), None, json!({"kind":"task.launch_details","data":{"terminal":"d1","tab":"tab-d1b","session":{"mode":"dir","id":"session-a1b"}}}));
    s.put(
        Actor::Watcher,
        Some(&a1),
        None,
        json!({"kind":"task.session","data":{"session":{"mode":"dir","id":"session-a1c"}}}),
    );
    // Entries the fold must reject without changing state.
    s.put(
        Actor::Coordinator,
        None,
        Some(&a),
        json!({"kind":"ws.opened","data":{"title":"Duplicate"}}),
    );
    s.put(
        Actor::Coordinator,
        Some(&TaskId::new(&a, 9)),
        None,
        json!({"kind":"task.progress","data":{"summary":"unknown task"}}),
    );
    s.put(
        Actor::User,
        None,
        None,
        json!({"kind":"question.answered","data":{"qid":"q9","text":"unknown question"}}),
    );
    s.put(
        Actor::Coordinator,
        None,
        Some(&a),
        json!({"kind":"ws.cleaned","data":{}}),
    );
    s.put(Actor::Coordinator, Some(&b3), None, json!({"kind":"task.reported","data":{"outcome":"done","summary":"closed task","result":null}}));
    s.flush();
    s.release(&b);
    world.borrow_mut().set("d1", "working", 2, 9700);
    s.event("d1");
    s.tick().await;
    s.tick().await;

    s.step(
        9800,
        "the delegate reports done and the workstream is closed while the agent is busy",
    );
    s.put(Actor::Task(a1.clone()), Some(&a1), None, json!({"kind":"task.reported","data":{"outcome":"done","summary":"Feature implemented","result":root.join("result.md")}}));
    s.put(
        Actor::Coordinator,
        Some(&a1),
        None,
        json!({"kind":"task.closed","data":{"reason":"accepted","outcome":"accepted"}}),
    );
    s.put(
        Actor::Coordinator,
        None,
        Some(&a),
        json!({"kind":"ws.closed","data":{"summary":"Shipped","outcome":"delivered"}}),
    );
    world.borrow_mut().set("d1", "working", 3, 9800);
    s.event("d1");
    let closed_seq = s.j.last_seq().unwrap();
    world
        .borrow_mut()
        .term("d1")
        .foreground
        .as_mut()
        .unwrap()
        .pid = 4242;
    s.tick().await;
    s.tick().await;
    assert!(!s.j.read_after(closed_seq).unwrap().iter().any(|entry| {
        entry.task.as_ref() == Some(&a1) && matches!(entry.typed(), Kind::TaskTurn { .. })
    }));
    {
        let _busy = s.ctx.terminal_lock("d1").unwrap();
        s.release(&a);
    }
    s.release(&a);
    s.board();

    s.step(9920, "the watcher rechecks the retained workstream");
    s.reconcile();
    s.step(10040, "an unchanged recheck does not repeat retention");
    s.reconcile();
    s.tick().await;
    for secs in [10060, 10080, 10100, 10120, 10140] {
        let before = s.j.last_seq().unwrap();
        s.step(secs, "another unchanged reconcile pass");
        s.reconcile();
        s.tick().await;
        assert!(!s.j.read_after(before).unwrap().iter().any(|entry| {
            matches!(
                entry.typed(),
                Kind::WsRetained { .. } | Kind::NudgeSent { .. }
            )
        }));
    }

    s.step(
        10160,
        "the agent is idle; its tab is released but a person is in the space",
    );
    world.borrow_mut().set("d1", "done", 2, 10100);
    world
        .borrow_mut()
        .term("d1")
        .foreground
        .as_mut()
        .unwrap()
        .pid = 201;
    s.reconcile();

    s.step(
        10280,
        "the person leaves; the space closes but the worktree is dirty and occupied",
    );
    {
        let mut w = world.borrow_mut();
        w.remove("p8");
        let p7 = person("p7", &tree.join("src"));
        w.terms.insert("p7".into(), p7);
    }
    std::fs::write(tree.join("notes.txt"), "work in progress\n").unwrap();
    s.reconcile();

    s.step(10400, "the work is committed but not merged");
    git(&tree, &["add", "-A"]);
    git(&tree, &["commit", "-q", "-m", "work"]);
    s.release(&a);
    s.step(10460, "an unchanged recheck journals nothing");
    s.reconcile();

    s.step(
        10520,
        "the branch is merged; a terminal is still inside the worktree",
    );
    git(&repo, &["merge", "-q", "--ff-only", "feature"]);
    s.reconcile();
    s.board();

    s.step(
        10640,
        "the terminal leaves; worktree and branch are removed",
    );
    world.borrow_mut().remove("p7");
    s.reconcile();
    s.release(&a);

    s.step(
        10700,
        "a workstream whose tab now belongs to another program is retained, then cleaned",
    );
    s.put(
        Actor::Coordinator,
        Some(&b1),
        None,
        json!({"kind":"task.closed","data":{"reason":"abandoned","outcome":"abandoned"}}),
    );
    s.put(
        Actor::Coordinator,
        Some(&b2),
        None,
        json!({"kind":"task.closed","data":{"reason":"superseded","outcome":"superseded"}}),
    );
    s.put(
        Actor::Coordinator,
        None,
        Some(&b),
        json!({"kind":"ws.closed","data":{"summary":"Stopped","outcome":"abandoned"}}),
    );
    s.release(&b);
    world.borrow_mut().remove("d2");
    s.step(10820, "the replaced terminal is gone");
    s.reconcile();
    s.release(&b);

    s.step(
        10880,
        "an event says the coordinator terminal is gone while hc still has it",
    );
    s.event_of("c1", "removed");
    s.tick().await;
    s.step(10900, "everything is acknowledged; the coordinator is idle");
    world.borrow_mut().set("c1", "done", 4, 10890);
    s.event("c1");
    let state = s.j.read_state().unwrap();
    s.put(
        Actor::Coordinator,
        None,
        None,
        json!({"kind":"inbox.drained","data":{"cursor":state.last_seq,"count":state.waking.len()}}),
    );
    s.tick().await;
    s.tick().await;
    s.step(
        39700,
        "eight hours later an idle coordinator would be restarted",
    );
    s.rules();
    s.step(39750, "a requested handoff would restart it at once");
    s.put(Actor::Coordinator, None, None, json!({"kind":"coord.handoff_requested","data":{"by":"coordinator","note":"Continue with the release"}}));
    s.rules();
    s.flush();

    s.step(
        39800,
        "the coordinator terminal disappears before the watcher hears of it",
    );
    world.borrow_mut().remove("c1");
    s.tick().await;
    s.step(39801, "the watcher learns that the terminal is gone");
    s.event_of("c1", "removed");
    s.tick().await;
    s.tick().await;
    s.board();
    s.state();

    let transcript = s.transcript();
    if let Some(path) = std::env::var_os("KIT_COORD_SCENARIO_OUT") {
        std::fs::write(path, &transcript).unwrap();
        return;
    }
    let expected = include_str!("../testdata/watch-scenario.txt");
    if transcript != expected {
        let line = transcript
            .lines()
            .zip(expected.lines())
            .position(|(got, want)| got != want)
            .unwrap_or_else(|| transcript.lines().count().min(expected.lines().count()));
        panic!(
            "transcript differs from testdata/watch-scenario.txt at line {}:\n got: {}\nwant: {}",
            line + 1,
            transcript.lines().nth(line).unwrap_or("<end>"),
            expected.lines().nth(line).unwrap_or("<end>"),
        );
    }
}
