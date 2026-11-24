//! Sequencing against a simulated core, kernel and plugin: which sessions get asked, in
//! what order, and what each answer turns into. The real plugin and native exec are
//! exercised in a disposable Herdr fixture instead (see the README).
use super::*;
use std::cell::{Cell, RefCell};

const OLD: FileId = FileId { dev: 1, ino: 100 };
const NEW: FileId = FileId { dev: 1, ino: 200 };

#[derive(Clone, Copy, PartialEq)]
enum Bridge {
    /// Restarts when asked.
    Obeys,
    /// Refuses with this reason at request time (the state changed since the census).
    Refuses(&'static str),
    /// Never answers.
    Silent,
    /// Restarts, but the new image comes back on another model.
    LosesModel,
    /// Restarts, and `--restore` starts work on its own.
    Resumes,
    /// The process dies instead of coming back.
    Dies,
}

#[derive(Clone)]
struct Session {
    term: Terminal,
    pid: i32,
    exe: Option<FileId>,
    deleted: bool,
    bridge: Option<Bridge>,
    presence: Presence,
    /// When the new image announces itself, if a restart is under way.
    exec_at: Option<u64>,
}

struct Fake {
    now: Cell<u64>,
    sessions: RefCell<Vec<Session>>,
    target_id: Cell<FileId>,
    sent: RefCell<Vec<(String, u64)>>,
    pending: RefCell<Vec<(Request, u64)>>,
    replies: RefCell<BTreeMap<i32, Reply>>,
    withdrawn: RefCell<Vec<i32>>,
    cancel: AtomicBool,
    cancel_on_request: Option<usize>,
    /// The launcher is repointed just before this (1-based) per-session target check.
    swap_target_on_check: Option<usize>,
    checks: Cell<usize>,
    in_flight: Cell<bool>,
}

fn presence(pid: i32) -> Presence {
    Presence {
        protocol: PROTOCOL,
        pid,
        start_time: Some(format!("start-{pid}")),
        instance: format!("{pid}-a"),
        session_id: Some(format!("session-{pid}")),
        session_file: Some(format!("/sessions/{pid}.jsonl")),
        session_saved: true,
        model: Some("openai-codex/gpt-6.2-sol".into()),
        thinking: Some("high".into()),
        idle: true,
        queued: false,
        running_jobs: 0,
        unprompted_runs: 0,
        composer_empty: true,
        launcher: Some(Launcher {
            path: "/bin/omp".into(),
            dev: NEW.dev,
            ino: NEW.ino,
        }),
        replay_risk: None,
    }
}

/// A live, idle personal OMP session on the old image with the plugin loaded.
fn old(n: i32) -> Session {
    Session {
        term: Terminal {
            address: format!("t{n}"),
            name: Some(format!("s{n}")),
            state: "live".into(),
            status: Some(Status {
                state: "idle".into(),
            }),
            reported: true,
            labels: BTreeMap::new(),
            foreground: Some(Foreground {
                pid: 1000 + n,
                name: "omp".into(),
                argv: vec!["/bin/omp".into()],
            }),
        },
        pid: 1000 + n,
        exe: Some(OLD),
        deleted: n % 2 == 0,
        bridge: Some(Bridge::Obeys),
        presence: presence(1000 + n),
        exec_at: None,
    }
}

fn with(mut s: Session, f: impl FnOnce(&mut Session)) -> Session {
    f(&mut s);
    s
}

impl Fake {
    fn new(sessions: Vec<Session>) -> Self {
        Self {
            now: Cell::new(1_000_000),
            sessions: RefCell::new(sessions),
            target_id: Cell::new(NEW),
            sent: RefCell::default(),
            pending: RefCell::default(),
            replies: RefCell::default(),
            withdrawn: RefCell::default(),
            cancel: AtomicBool::new(false),
            cancel_on_request: None,
            swap_target_on_check: None,
            checks: Cell::new(0),
            in_flight: Cell::new(false),
        }
    }
    fn target(&self) -> Target {
        Target {
            path: "/bin/omp".into(),
            resolved: "/cellar/2/omp".into(),
            id: NEW,
            version: RESTORE_VERIFIED[0].into(),
        }
    }
    fn session(&self, pid: i32) -> Option<std::cell::RefMut<'_, Session>> {
        std::cell::RefMut::filter_map(self.sessions.borrow_mut(), |all| {
            all.iter_mut().find(|s| s.pid == pid)
        })
        .ok()
    }
    fn asked(&self) -> Vec<String> {
        self.sent.borrow().iter().map(|(t, _)| t.clone()).collect()
    }
    fn run(&self, scope: Scope) -> Value {
        self.in_flight.set(false);
        restart(self, &self.target(), &scope, None, &self.cancel).unwrap()
    }
    /// The plugin and kernel catch up with the clock.
    fn advance(&self) {
        let now = self.now.get();
        let ready: Vec<(Request, u64)> = {
            let mut pending = self.pending.borrow_mut();
            let (ready, wait) = pending.drain(..).partition(|(_, at)| now >= at + 400);
            *pending = wait;
            ready
        };
        for (req, _) in ready {
            let Some(mut s) = self.session(req.pid) else {
                continue;
            };
            let reply = |outcome: &str, reason: Option<&str>, before| Reply {
                nonce: Some(req.nonce.clone()),
                outcome: outcome.into(),
                reason: reason.map(Into::into),
                before,
            };
            let answer = match s.bridge {
                None | Some(Bridge::Silent) => continue,
                Some(Bridge::Refuses(why)) => reply("refused", Some(why), None),
                Some(_) => {
                    s.exec_at = Some(now + 1_500);
                    reply("restarting", None, Some(s.presence.clone()))
                }
            };
            self.replies.borrow_mut().insert(req.pid, answer);
        }
        for s in self.sessions.borrow_mut().iter_mut() {
            if s.exec_at.is_some_and(|at| now >= at) {
                s.exec_at = None;
                if s.bridge == Some(Bridge::Dies) {
                    s.exe = None;
                    s.term.state = "finished".into();
                    continue;
                }
                s.exe = Some(self.target_id.get());
                s.deleted = false;
                s.presence.instance = format!("{}-b", s.pid);
                match s.bridge {
                    Some(Bridge::LosesModel) => s.presence.model = Some("fallback/model".into()),
                    Some(Bridge::Resumes) => s.presence.unprompted_runs = 1,
                    _ => {}
                }
                self.in_flight.set(false);
            }
        }
    }
}

impl Host for Fake {
    fn terminals(&self) -> Result<Vec<Terminal>> {
        Ok(self
            .sessions
            .borrow()
            .iter()
            .map(|s| s.term.clone())
            .collect())
    }
    fn terminal(&self, address: &str) -> Result<Option<Terminal>> {
        let all = self.sessions.borrow();
        Ok(all
            .iter()
            .find(|s| s.term.address == address)
            .map(|s| s.term.clone()))
    }
    fn image(&self, pid: i32) -> Option<Image> {
        let s = self.session(pid)?;
        Some(Image {
            path: if s.deleted {
                "/cellar/1/omp (deleted)".into()
            } else {
                "/cellar/x/omp".into()
            },
            id: s.exe?,
            deleted: s.deleted,
            start_time: format!("start-{pid}"),
        })
    }
    fn presence(&self, pid: i32) -> Option<Presence> {
        let s = self.session(pid)?;
        s.bridge.map(|_| s.presence.clone())
    }
    fn send(&self, request: &Request) -> Result<()> {
        assert!(
            !self.in_flight.get(),
            "a second request while a restart was in flight"
        );
        let address = self.session(request.pid).unwrap().term.address.clone();
        self.sent.borrow_mut().push((address, self.now.get()));
        self.pending
            .borrow_mut()
            .push((request.clone(), self.now.get()));
        self.in_flight.set(true);
        let count = self.sent.borrow().len();
        if self.cancel_on_request == Some(count) {
            self.cancel.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    fn reply(&self, pid: i32) -> Option<Reply> {
        self.replies.borrow().get(&pid).cloned()
    }
    fn withdraw(&self, pid: i32) {
        self.withdrawn.borrow_mut().push(pid);
        self.in_flight.set(false);
    }
    fn file_id(&self, _path: &Path) -> Option<FileId> {
        self.checks.set(self.checks.get() + 1);
        if self.swap_target_on_check == Some(self.checks.get()) {
            self.target_id.set(FileId { dev: 1, ino: 300 });
        }
        Some(self.target_id.get())
    }
    fn now_ms(&self) -> u64 {
        self.now.get()
    }
    fn sleep_ms(&self, ms: u64) {
        self.now.set(self.now.get() + ms);
        self.advance();
        // Refusals and silences end a request too.
        if self
            .replies
            .borrow()
            .values()
            .any(|r| r.outcome == "refused")
        {
            self.in_flight.set(false);
        }
    }
    fn nonce(&self) -> String {
        format!("n{}", self.sent.borrow().len())
    }
}

fn outcomes(report: &Value) -> Vec<(String, String, String)> {
    report["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let s = |k: &str| r[k].as_str().unwrap_or("").to_owned();
            (s("terminal"), s("outcome"), s("reason"))
        })
        .collect()
}

fn count(report: &Value, outcome: &str) -> u64 {
    report["summary"][outcome].as_u64().unwrap_or(0)
}

#[test]
fn thirty_two_old_sessions_restart_one_at_a_time_and_a_rerun_touches_none() {
    let mut sessions: Vec<Session> = (1..=32).map(old).collect();
    sessions.push(with(old(40), |s| {
        s.exe = Some(NEW);
        s.deleted = false;
    }));
    sessions.push(with(old(41), |s| {
        s.term.foreground.as_mut().unwrap().name = "fish".into();
        s.term.foreground.as_mut().unwrap().argv = vec!["fish".into()];
    }));
    let fake = Fake::new(sessions);

    let report = fake.run(Scope::All);
    assert_eq!(count(&report, "restarted"), 32, "{report:#}");
    assert_eq!(count(&report, "current"), 1);
    assert_eq!(report["complete"], true);
    assert_eq!(fake.asked().len(), 32);
    // Serial: each request starts only after the previous restart was proven.
    let times: Vec<u64> = fake.sent.borrow().iter().map(|(_, at)| *at).collect();
    assert!(times.windows(2).all(|w| w[1] - w[0] >= SETTLE_MS));
    let sessions = fake.sessions.borrow();
    let omp = sessions.iter().filter(|s| s.term.address != "t41");
    assert!(omp.into_iter().all(|s| s.exe == Some(NEW)));
    drop(sessions);

    let again = fake.run(Scope::All);
    assert_eq!(count(&again, "current"), 33);
    assert_eq!(
        fake.asked().len(),
        32,
        "a rerun asked an already restarted session"
    );
}

#[test]
fn status_classifies_every_case_and_changes_nothing() {
    let fake = Fake::new(vec![
        old(1),
        with(old(2), |s| s.deleted = true),
        with(old(3), |s| s.exe = Some(NEW)),
        with(old(4), |s| s.exe = None),
        with(old(5), |s| s.bridge = None),
        with(old(6), |s| {
            s.term.labels.insert("coord.managed".into(), "1".into());
        }),
        with(old(7), |s| s.term.address = "vps/t7".into()),
        with(old(8), |s| {
            s.term.status = Some(Status {
                state: "working".into(),
            })
        }),
        with(old(9), |s| {
            s.term.status = Some(Status {
                state: "blocked".into(),
            })
        }),
        with(old(10), |s| s.term.reported = false),
        with(old(11), |s| s.presence.composer_empty = false),
        with(old(12), |s| s.presence.queued = true),
        with(old(13), |s| s.presence.running_jobs = 2),
        with(old(14), |s| s.presence.launcher.as_mut().unwrap().ino = 999),
        with(old(15), |s| s.presence.session_saved = false),
        with(old(16), |s| {
            s.presence.start_time = Some("reused-pid".into())
        }),
        with(old(17), |s| s.term.foreground = None),
        old(18),
        with(old(19), |s| s.presence.session_id = Some("same".into())),
        with(old(20), |s| s.presence.session_id = Some("same".into())),
        with(old(21), |s| {
            s.presence.replay_risk = Some("restorable_subagent".into())
        }),
    ]);
    let report = status(&fake, &fake.target(), &[], Some("t18")).unwrap();
    let rows: Vec<(String, String, String, String)> = report["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let s = |k: &str| r[k].as_str().unwrap_or("-").to_owned();
            (s("terminal"), s("image"), s("action"), s("reason"))
        })
        .collect();
    let expect = [
        ("t1", "old", "restart", "-"),
        ("t2", "old", "restart", "-"),
        ("t3", "current", "none", "current"),
        ("t4", "unknown", "skip", "exe_unreadable"),
        ("t5", "old", "skip", "no_bridge"),
        ("t6", "-", "skip", "managed"),
        ("vps/t7", "-", "skip", "remote"),
        ("t8", "old", "skip", "busy"),
        ("t9", "old", "skip", "blocked"),
        ("t10", "old", "skip", "status_unknown"),
        ("t11", "old", "skip", "draft"),
        ("t12", "old", "skip", "queued_messages"),
        ("t13", "old", "skip", "background_jobs"),
        ("t14", "old", "skip", "launcher_not_target"),
        ("t15", "old", "skip", "session_not_saved"),
        ("t16", "old", "skip", "no_bridge"),
        ("t17", "-", "none", "not_omp"),
        ("t18", "-", "skip", "self"),
        ("t19", "old", "skip", "session_shared"),
        ("t20", "old", "skip", "session_shared"),
        ("t21", "old", "skip", "restorable_subagent"),
    ];
    let expect: Vec<(String, String, String, String)> = expect
        .iter()
        .map(|(a, b, c, d)| (a.to_string(), b.to_string(), c.to_string(), d.to_string()))
        .collect();
    assert_eq!(rows, expect);
    assert_eq!(report["summary"]["restart"], 2);
    assert!(fake.asked().is_empty() && fake.replies.borrow().is_empty());
}

#[test]
fn named_terminals_are_the_only_ones_asked() {
    let fake = Fake::new((1..=30).map(old).collect());
    let report = fake.run(Scope::Only(vec!["t7".into(), "t3".into(), "t7".into()]));
    assert_eq!(fake.asked(), ["t7", "t3"]);
    assert_eq!(count(&report, "restarted"), 2);

    let unknown = restart(
        &fake,
        &fake.target(),
        &Scope::Only(vec!["t99".into()]),
        None,
        &fake.cancel,
    );
    assert!(
        unknown
            .unwrap_err()
            .to_string()
            .contains("terminal_not_found")
    );
    assert_eq!(fake.asked().len(), 2);
}

#[test]
fn a_session_that_changed_since_the_census_is_skipped_not_forced() {
    let fake = Fake::new(vec![
        old(1),
        with(old(2), |s| s.bridge = Some(Bridge::Refuses("draft"))),
        with(old(3), |s| {
            s.bridge = Some(Bridge::Refuses("composer_not_focused"))
        }),
        with(old(4), |s| s.bridge = Some(Bridge::Silent)),
        old(5),
    ]);
    let report = fake.run(Scope::All);
    assert_eq!(
        outcomes(&report),
        [
            ("t1".into(), "restarted".into(), "".into()),
            ("t2".into(), "skipped".into(), "draft".into()),
            ("t3".into(), "skipped".into(), "composer_not_focused".into()),
            ("t4".into(), "skipped".into(), "no_answer".into()),
            ("t5".into(), "restarted".into(), "".into()),
        ]
    );
    assert_eq!(*fake.withdrawn.borrow(), [1004]);
    assert_eq!(report["complete"], true);
}

#[test]
fn a_failed_restart_stops_the_run_and_a_retry_only_does_the_rest() {
    let fake = Fake::new(vec![
        old(1),
        with(old(2), |s| s.bridge = Some(Bridge::LosesModel)),
        old(3),
        old(4),
    ]);
    let report = fake.run(Scope::All);
    assert_eq!(
        outcomes(&report),
        [
            ("t1".into(), "restarted".into(), "".into()),
            ("t2".into(), "failed".into(), "model_changed".into()),
            ("t3".into(), "not_attempted".into(), "failure".into()),
            ("t4".into(), "not_attempted".into(), "failure".into()),
        ]
    );
    assert_eq!(report["complete"], false);
    let detail = &report["results"][1]["detail"]["model"];
    assert_eq!(detail["before"], "openai-codex/gpt-6.2-sol");
    assert_eq!(detail["after"], "fallback/model");

    let retry = fake.run(Scope::All);
    assert_eq!(
        fake.asked(),
        ["t1", "t2", "t3", "t4"],
        "no session asked twice"
    );
    assert_eq!(count(&retry, "current"), 2);
    assert_eq!(count(&retry, "restarted"), 2);
}

#[test]
fn work_resumed_by_restore_or_a_dead_process_is_a_failure() {
    let fake = Fake::new(vec![with(old(1), |s| s.bridge = Some(Bridge::Resumes))]);
    let report = fake.run(Scope::All);
    assert_eq!(outcomes(&report)[0].2, "work_resumed");

    let fake = Fake::new(vec![
        with(old(1), |s| s.bridge = Some(Bridge::Dies)),
        old(2),
    ]);
    let report = fake.run(Scope::All);
    assert_eq!(outcomes(&report)[0].1, "failed");
    assert_eq!(outcomes(&report)[0].2, "process_exited");
    assert_eq!(outcomes(&report)[1].1, "not_attempted");
}

#[test]
fn cancel_finishes_the_session_in_flight_and_a_rerun_completes() {
    let mut fake = Fake::new((1..=30).map(old).collect());
    fake.cancel_on_request = Some(5);
    let report = fake.run(Scope::All);
    assert_eq!(
        count(&report, "restarted"),
        5,
        "the in-flight restart is still proven"
    );
    assert_eq!(count(&report, "not_attempted"), 25);
    assert_eq!(report["stopped"], "cancelled");

    fake.cancel.store(false, Ordering::SeqCst);
    fake.cancel_on_request = None;
    let rerun = fake.run(Scope::All);
    assert_eq!(count(&rerun, "current"), 5);
    assert_eq!(count(&rerun, "restarted"), 25);
    assert_eq!(fake.asked().len(), 30);
}

#[test]
fn a_target_replaced_mid_run_stops_before_the_next_session() {
    let mut fake = Fake::new((1..=4).map(old).collect());
    fake.swap_target_on_check = Some(3);
    let report = fake.run(Scope::All);
    assert_eq!(report["stopped"], "target_changed");
    assert_eq!(fake.asked().len(), 2);
}

#[test]
fn coordinator_managed_callers_are_refused_before_anything_is_asked() {
    let fake = Fake::new(vec![
        old(1),
        with(old(2), |s| {
            s.term.labels.insert("coord.managed".into(), "1".into());
        }),
    ]);
    let err = restart(&fake, &fake.target(), &Scope::All, Some("t2"), &fake.cancel).unwrap_err();
    assert!(err.to_string().contains("forbidden_caller"));
    let err = restart(&fake, &fake.target(), &Scope::All, Some("t9"), &fake.cancel).unwrap_err();
    assert!(err.to_string().contains("caller_unknown"));
    assert!(fake.asked().is_empty());
}

#[test]
fn a_target_whose_restore_was_not_checked_gets_the_census_but_no_restart() {
    let fake = Fake::new((1..=3).map(old).collect());
    let mut target = fake.target();
    target.version = "omp/99.0.0".into();
    let err = restart(&fake, &target, &Scope::All, None, &fake.cancel).unwrap_err();
    assert!(err.to_string().starts_with("target_unverified: omp/99.0.0"));
    assert!(fake.asked().is_empty());
    let report = status(&fake, &target, &[], None).unwrap();
    assert_eq!(report["restart_available"], false);
    assert_eq!(report["summary"]["restart"], 3);
    assert_eq!(
        status(&fake, &fake.target(), &[], None).unwrap()["restart_available"],
        true
    );
}
