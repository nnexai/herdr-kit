# Coordinator: implementation plan

Historical implementation record: source paths below use the pre-workspace tree.
P1 moves that direct implementation under `crates/kit-manager/`; deployment
ownership and subsequent extraction follow [the modular plan](standalone-mod-plan.md)
and [P1 runbook](monorepo-p1.md), rather than this original packaging inventory.

Inputs, in order of authority: `docs/coordinator-spec.md` (the spec, final),
`docs/coordinator/board-design.md` §4.4 (authoritative for the JSON of
`coord status`, `coord board`, `coord log`), this plan. Supporting files:

- `docs/coordinator/roles/*.md`: the seven built-in role files, verbatim (copy to `src/coord/roles/`).
- `docs/coordinator/examples/journal-sample.jsonl`: on-disk journal lines.
- `docs/coordinator/examples/fake-cli.sh`: recording fake for `hc` and `herdr-app` (copy to `tests/fake/cli.sh`).
- `docs/coordinator/examples/captures/`: trimmed real outputs (`hc get --json` of an omp agent, `hc ls --json`, the head of `hc events --json`, `herdr-app ls`, `git worktree list --porcelain`), the unit-test fixtures (copy to `src/coord/testdata/`).

Evidence tags: **[cap]** checked against a read-only capture in
`/tmp/coord-captures/` (help text, `hc schema`, live JSON, 2026-11-13);
**[spec]** verified live by the spec author; **[doc]** documented in
`~/.config/herdr-app/AGENTS.md`, `herdr.d.ts` or `hc --skill` but not observed;
**[src]** read in source; **[INFERENCE]** not checked. Section 11 lists every
unchecked item.

## 1. Outcome

`herdr-kit coord <verb>` in the existing binary implements the spec: an
append-only journal with a derived state, placement mechanics (spaces, tabs,
worktrees) that record ownership before acting, mechanical release checks, a
watcher run as an hc terminal, coordinator lifecycle (up/down/restart/handoff),
read models for the person, the mod and agents, seven built-in roles, and a
second managed mod `herdr-kit-coord` installed by the existing release manager.
`cargo test --locked` and `node --test tests/*.test.js` cover it without a
live Herdr.

## 2. Evidence

### 2.1 Repository

| Fact | Where |
|---|---|
| One clap-derive CLI; global `--data-dir` (env `HERDR_KIT_DATA_DIR`); every command returns `serde_json::Value`, printed as one line; errors print `{"error":{"code":"operation_failed","message":…}}` to stderr and exit 1 | `src/main.rs:9-17`, `:70-81` |
| Data root resolution (relative `--data-dir` made absolute) | `src/main.rs:83-89`, `src/paths.rs:4-20` |
| Tokio is `current_thread`; features `rt, macros, time, process, io-util` (no `signal`, no `test-util`) | `src/main.rs:70`, `Cargo.toml:17` |
| `unsafe_code = "forbid"` | `Cargo.toml:21-22` |
| Child processes: tokio `Command` with `kill_on_drop`, `process_group(0)`, bounded output, timeout | `src/usage/source.rs:37-61` |
| Sync child with deadline (`bounded`) | `src/manager.rs:157-169` |
| Atomic file write (tempfile in same dir, fsync, rename, dir fsync); symlink refusal `no_links` | `src/manager.rs:48-95` |
| flock with deadline (`try_lock_exclusive` loop, 20 s) | `src/manager.rs:202-223` |
| Single managed mod name `const MOD = "herdr-kit-usage"` | `src/manager.rs:19` |
| Generation naming `name(rev)`, `mod_path(home, rev)`, stable binary `bootstrap(data) = data/bin/herdr-kit` | `src/manager.rs:194-200` |
| Release staging copies one mod into `staged/mod`, rewrites the mod name in `.js`/`.json`, stamps `runtime.js` (`BINARY, DATA_DIR, RELEASE, MOD_NAME`), rewrites the layout, `herdr-app check`s the mod | `src/manager.rs:322-352` |
| Verification/recovery/activation all assume `releases/<rev>/mod` | `src/manager.rs:231-241`, `:242-274`, `:275-299` |
| `update` actions `check|apply|maintain|rollback`; activation inside `update`/`install` | `src/manager.rs:368-442` |
| Receipts are content trees of the whole release dir; `managed_mods` lists revisions | `src/manager.rs:21-33`, `:353-357` |
| Usage mod calls `exec(BINARY, [--data-dir?, 'usage', …])`, runs `update maintain` every 6 h | `mods/herdr-kit-usage/main.js:9`, `:27`, `:48`, `:57` |
| Status row is one block with the usage mod (`view: statusbar`) | `layouts/herdr-kit.json:170-182` |
| Usage strip draws `bg(t.chrome).border_t(1)` across its block | `mods/herdr-kit-usage/main.js:98-100` |
| JS model tests run with `node --test tests/*.test.js` | `package.json:1`, `tests/usage.test.js` |
| Herdr's worktree path convention: `~/.herdr/worktrees/<basename of main checkout>/<slug>`; `slug = branch.replace(/[^A-Za-z0-9._-]+/g,"-")` trimmed of `-`; main checkout from `git rev-parse --path-format=absolute --git-common-dir` minus `/.git` | `~/.config/herdr-app/mods/worktree/main.js:16,22,99-105,187-189` [src] |

### 2.2 External tools (checked against captures unless tagged otherwise)

`hc` (herdr-core 0.1.20, `hc schema` `info.version`) [cap]:

- `hc run [--json] [--name N] [--cwd D] [--label K=V]… [--no-parent] [--keep] [--bg] -- ARGV…`. `--name`: "unique among live terminals: 1-32 of a-z 0-9 - _". `--bg`: start and return at once. Piped output of `run` is text unless `--json`. One argv string containing spaces is run as a shell line (hc skill), so argv is always passed as separate items after `--`.
- `RunResult`/`Terminal` JSON (`hc get --json t7`, `hc schema`): `address`, `argv`, `cwd` (launch folder), `created_ms`, `state` (`live|finished`), `status` (object `{app, state, kind?, msg?}` or `null`; `state` ∈ `idle|working|done|blocked|error`), `status_ms` (when the word last changed), `turn` ("goes up each time the status leaves `working`"), `reported` (ever sent a 7501 report), `labels` (map), `pid`, `foreground` `{pid, name, argv, cwd}` or `null`, `exit_code`, `finished_ms`, `reason` (`exited|killed|server_stop|…|stopped_by_restart`), `keep`, `name`, `parent`.
- `hc ls --json [--all] [--label K=V]…` prints a compact list `{"hidden_finished":n,"terminals":[{"address","agent","cwd","state","status"(string, absent when none),"turn","where"}]}`, **without labels or pid**; details need `hc get`.
- `hc get [--json] <t>`; `hc label [--json] <t> K=V…` (`key=` removes); `hc kill [--json] <t>` (hangup then escalate; returns the terminal); `hc rm [--json] <t>` (finished only; error kind `live` otherwise).
- `hc send [--json] [--force] [--if idle|working] [--wait] <t> <text>`: pastes, presses Enter; waits for a person's draft up to 60 s, else ~3 s of quiet; refuses `blocked` unless `--force`; result `{turn, started, status}`.
- `hc events [--json] [--label K=V]…`: first line is a snapshot `{"channel":null,"seq":N,"terminals":[<full Terminal>…]}`; then one line per event `{"jsonrpc":"2.0","method":"event","params":{"data":{…},"seq":N,"terminal":"t15","type":"title"}}`. Only `title` data (`{"title":…}`) was captured; other types' data are unverified (§11) and the design does not depend on them.
- Error kinds in the schema: `not_found`, `name_taken`, `blocked`, `status_mismatch`, `live`, `finished`, `unavailable`.

`herdr-app` [cap]:

- `herdr-app ls` prints `{"spaces":[{"id","name","folder","machine","active","tabs":[{"id","name","active","settled","terminals":[{"terminal","pane","status","app","name","sub_agents"}]}]}],"recent":[…]}`; it talks to the open window (skill).
- `herdr-app do <command> '<one JSON object>'` (skill: "`do` accepts `--key value` arguments … or one JSON object"; prints created ids or `{ "error": … }` and exits 1). Commands and args from `herdr-app commands`: `space.rename {id?, name?}`, `space.close {id?, confirmed?}`, `tab.rename {id?, name?}`, `tab.close {id?, confirmed?}`, `tab.move {id, index}` ("1 = first"), `terminal.pin {terminal, pinned}`.
- `herdr-app status` prints `{zoom, font_size, ui_font_size, space, tab, focused_pane, layout, agent, floats, overlays, errors}`.

`git worktree list --porcelain` prints blocks `worktree <path>` / `HEAD <sha>` / `branch refs/heads/<b>` [cap].

Verified by the spec author [spec]: `hc run --bg --no-parent --keep --label herdr.space=…` does not move the person's view; `HC_TERMINAL_ID` set in the caller makes hc wrap messages in `<hc from … reach …>`; reusing a closed space id recreates it; `hc send --if idle` refuses `done`; omp session file `<timestamp>_<id>.jsonl` in `--session-dir`, resume with `--session-dir … --resume <id> "<msg>"`; the harness table and trust table of spec §5.

## 3. Decisions

### 3.1 Architecture

| # | Decision | Rejected alternatives |
|---|---|---|
| D1 | All code in `src/coord/` behind `Command::Coord`; one module per responsibility (§4). No new crates. Random bytes from `/dev/urandom` (Linux and macOS). | `rand`/`uuid`/`chrono` crates (avoidable); a second binary for the watcher (spec: one tool). |
| D2 | On-disk journal line == the `LogEntry` shape of board-design §4.4.3: `{"v":1,"seq","ts_ms","actor","kind","ws","task","data"}`. `coord log --json` prints stored lines unchanged. Typed access through an adjacently tagged enum `Kind` (`#[serde(tag="kind", content="data")]`) with `#[serde(other)] Unknown`, so older and newer kinds both read. | Internally tagged/flattened (differs from the mod contract; flatten+enum is fragile in serde). |
| D3 | `seq` is global and contiguous; assigned under the journal flock by reading the last line of the newest non-empty month file. Entries are capped at 64 KiB; long text lives in files (`briefs/`, `results/`). Readers never lock and ignore a trailing line without `\n`. | A separate seq counter file (two writes per append); SQLite (new dependency, not "append-only text"). |
| D4 | `state.json` is a serde dump of `State` plus `fold_version`. Load = read cache, then fold entries with `seq > last_seq` from month files ≥ the cache's month. Wrong `fold_version`, parse error or missing file → full refold. Only locked writers persist it. | Always full refold (fine for months, slows `status`, which the mod calls every 20 s). |
| D5 | One fold function `fold::apply(&mut State, &Entry)`; it never fails. Entries that do not fit (unknown task, duplicate) are counted in `state.anomalies` and listed by `explain`. All read models (`board`, `status` counts, `inbox`, watcher rules) derive from `State` only; `needs_you` and `attention` come from one function (`views::attention`). | Per-view ad-hoc journal scans (strip and board could disagree; board-design Q5). |
| D6 | Adapters: `trait Hc` and `trait App` with process implementations (`ProcHc`, `ProcApp`) selected by `HERDR_KIT_HC` / `HERDR_KIT_HERDR_APP` (default `hc` / `herdr-app` from `PATH`), plus in-memory fakes for unit tests. Every `hc` call removes `HC_TERMINAL_ID` from the child env. Git is a plain module of functions over real `git` (tests use temp repos); no trait. | A `Git` trait (nothing to fake: temp repos are cheap and real); `hc connect` JSON-RPC (more code, no gain for one-shot calls). |
| D7 | Calls are synchronous (`std::process` with a deadline, like `manager::bounded`) except the watcher's `hc events` stream (tokio child, line reader). Timeouts: `hc run` 30 s, `hc send` 90 s (draft hold ≤ 60 s), other `hc` 15 s, `herdr-app` 15 s, git 120 s (`worktree add` 600 s, as Herdr's worktree mod uses). | Async everywhere (no concurrency need in one-shot commands). |
| D8 | Watcher events: on any non-`title` event, re-read that terminal with `hc get --json`; `removed` drops it; a line with a `terminals` array replaces all facts (initial or `missed` snapshot). | Parsing per-type `data` payloads (unverified shapes). |
| D9 | Watcher rule evaluation is a pure function `rules::evaluate(&State, &Facts, &Config, now_ms) -> Vec<Action>`; idempotency comes from the journal (each action is checked against entries already written), so the watcher keeps no persistent state (spec §6). | Watcher-local timers/flags (lost on restart; spec forbids own state). |
| D10 | Release change: the watcher compares its start release with the manager's active release on every heartbeat; on change it appends `watcher.stopped{reason:"release_changed"}` and replaces itself in place with `exec` of `<data>/bin/herdr-kit --data-dir <root> coord watch` (`std::os::unix::process::CommandExt::exec`, safe). Same terminal, same tab, new binary. `coord up` also replaces a watcher whose heartbeat is stale or whose release differs. | Exit and wait for `coord up` (gap until the next up). |
| D11 | "Release" = `current` of the manager's `<data>/state.json`, read without the manager lock (written atomically); `"development"` when absent. The binary never needs to know its own revision. | Embedding the revision at build time (needs a build script; dev builds have none). |
| D12 | Child commands that the system starts (watcher, footer, nudges) use the stable path `<data>/bin/herdr-kit` when it exists, else `std::env::current_exe()` (development). `--data-dir <root>` is appended whenever `root != paths::data_dir()`. This string is `Ctx::cli()`. | `HERDR_KIT_DATA_DIR` in the child env (hc CLI has no `--env`) [cap: `hc run --help`]. |
| D13 | `herdr-app do` is always called with one JSON object argument, so ids stay strings and `index`/`pinned`/`confirmed` are JSON numbers/booleans. | `--key value` form (type coercion of `1`/`true` unverified). |
| D14 | Delegate tab release = `hc kill` + `hc rm` of the delegate terminal; if the tab still exists afterwards with no terminals left, `tab.close {id, confirmed:true}`. A tab the person split into keeps the person's pane. | `tab.close` first (would end a person's pane in the same tab). |
| D15 | Coordinator restart in the same tab: kill and `hc rm` the old terminal (frees the name `coordinator`), start the new one with the same `herdr.space` and `herdr.tab` labels, then `tab.move {id, index:1}` and `terminal.pin`. Herdr recreates a tab whose id it no longer knows [doc]. | Start the new one first (name collision: names are unique among live terminals [cap]). |
| D16 | Exit codes: `0` ok (JSON on stdout), `1` operation failed (effects may be partial; the journal holds the truth), `2` usage (clap), `3` refused (permission or precondition, nothing changed), `4` not ready (retry later, nothing changed). Error body unchanged: `{"error":{"code":"<code>","message":"…"}}` on stderr. | One exit code for all failures (agents could not tell "retry later" from "wrong"). |
| D17 | The coordinator mod is a second managed mod. `src/manager.rs` generalises to `MODS = ["herdr-kit-usage", "herdr-kit-coord"]`; new releases store `releases/<rev>/mods/<base>/`; legacy releases with `releases/<rev>/mod/` stay valid as `[("herdr-kit-usage", …)]`. No state migration. (User decision relayed by Main.) | Coordinator UI inside the usage mod (rejected by the user). |
| D18 | `update apply|maintain|rollback` and `install` report `"activated": true` when the active revision changed; `main.rs` then runs `<data>/bin/herdr-kit --data-dir <root> coord up --if-enabled` (the newly active binary) and adds its JSON as `"coord"` to the output. A failing or missing `coord` verb (rollback to an older release) yields `"coord":{"state":"unsupported"}` and never fails the update. | Calling `coord::up` in-process (would run the old binary's logic and start the watcher from the pre-activation code). |

### 3.2 Gaps in the spec, filled here

| # | Decision | Why |
|---|---|---|
| G1 | Extra journal kinds: `task.session` (session id found after `task.started`), `task.sent` (a follow-up message), `task.note` (delegate's `coord note`), `task.attention` (`blocked`/`error`/`clear` seen by the watcher), `coord.handoff_requested`. | Spec verbs `note`, `task send`, `handoff` and the rule "delegate blocked or error → append an event" need kinds; session ids can arrive after launch. |
| G2 | Extra verbs: `scout` (records `scout.result`), `correct` (records `correction`), `ask --answer <qid>` (records `question.answered`), `roles [--defaults] [<name>]` (spec §5 names `coord roles --defaults`). | The spec lists these kinds but no verb writes them. |
| G3 | `task start --reason <text>` is required (the chosen rung's reason; board `placement_reason`), `--reject N=<reason>` for every lower rung. Rung arguments: 1 `--session-of <task>`; 2 none; 3 `--space <id>`; 4 `--branch <b> [--base <ref>]`; 5 `--folder <abs path>`. Rungs 4 and 5 only for the first task of a workstream (spec §2: follow-ups cannot create spaces). Rung 2 needs the workstream to have a space. | Spec §5 fixes the ladder but not its arguments. |
| G4 | Task title = `--title`, else the first non-empty line of the brief, clipped to 80 chars; used for the tab name and the board (board-design `Task.title`). | Spec: "a tab is named after its task". |
| G5 | Role front matter key `trust:` (optional, split like `start`): arguments inserted after `argv[0]` of `start` and `resume` only when the workstream's repo is one the person works in = some space in `herdr-app ls` that the system does not own has a folder whose main checkout is that repo, or the journal has a `res.adopted` space in that repo. Used by `codex-default`. | Spec §5: codex trust override only for repos the person already uses; placeholders cannot drop arguments. |
| G6 | Model recorded in `task.placed` is the value after `--model` or `-m` in the expanded start argv (or `null`). Built-in omp roles use omp role references: `coord-main @plan`, `coord-light @smol`, `coord-default @task`, `coord-deep @slow` (project decision memory: omp model roles, `@role` references). | Spec: "the coordinator picks a role, never a model"; "models in built-ins are starting values". |
| G7 | Actor: `cli` when `HC_TERMINAL_ID` is unset (mod, scripts); `user` when set and the terminal has no `coord.role` label; `coordinator`, `watcher`, `task:<id>` from labels. Mechanical entries carry the actor of the command that caused them. | Spec §3 lists actors without mapping. |
| G8 | `ws close` refuses while a task is `placed` or `running`; `release <ws>` refuses an open workstream. Task "result recorded" for tab release = a `task.reported` after the task's last start/send. | Keeps the destructive gate mechanical (spec §8). |
| G9 | Waking kinds (for the inbox nudge): `task.reported`, `task.note`, `task.stalled`, `task.start_blocked`, `task.attention` (not `clear`), `task.dormant`, `reconcile.finding`, `ws.retained`, and `instruction.set`/`correction`/`thread.*` written by `user`/`cli`. Entries written by `coordinator` never wake it. `task.progress` never wakes (spec §6). | Spec: "waking events are undrained". |
| G10 | Retained re-checks during reconcile append `res.release_checked` and `ws.retained` only when a check result differs from the last recorded one for that resource. | Avoids journal growth every 120 s. |
| G11 | `restart [--force]`: without `--force` it records `coord.handoff_requested{by:"user"}` and the watcher restarts when the spec's conditions hold (immediately if they already hold); `--force` (person only) restarts now. | Spec allows restart only when idle; a stuck coordinator still needs a way out. |
| G12 | `coord up` after an activation that changed the release appends `decision{tier:"digest", what:"handoff after update", why:"release <old> → <new>"}` and `coord.handoff_requested{by:"cli"}`. | Spec §6 "the update appends a decision entry". |
| G13 | Briefs larger than 96 KiB are not passed as an argument; the first message becomes `Read your full brief in <path> before doing anything else.` (Linux `MAX_ARG_STRLEN` is 128 KiB per argument [INFERENCE: kernel constant, not probed]). The coordinator's startup read is capped at 32 KiB with a pointer to `coord board`. | argv limits. |

## 4. Module layout and Rust interfaces

All paths under `src/coord/`. `pub` means `pub(crate)` in practice; signatures
are binding for the slices. `Result` is `anyhow::Result`.

### 4.1 `mod.rs` (S0 declares modules; S9 adds `run`)

```rust
pub mod adapter; pub mod brief; pub mod caller; pub mod cli; pub mod config; pub mod error;
pub mod fold; pub mod git; pub mod ids; pub mod journal; pub mod kinds; pub mod launch;
pub mod lifecycle; pub mod paths; pub mod place; pub mod reconcile; pub mod release;
pub mod role; pub mod rules; pub mod state; pub mod views; pub mod watch;

pub struct Ctx {
    pub root: PathBuf,                 // absolute data dir
    pub paths: paths::CoordPaths,
    pub hc: Box<dyn adapter::Hc>,
    pub app: Box<dyn adapter::App>,
    pub clock: Box<dyn Fn() -> u64>,   // ms since epoch
}
impl Ctx {
    pub fn new(root: &Path) -> Result<Ctx>;               // Proc adapters from env, system clock
    pub fn now(&self) -> u64;
    pub fn cli(&self) -> String;                          // D12, e.g. "/home/u/.local/share/herdr-kit/bin/herdr-kit"
    pub fn cli_argv(&self) -> Vec<String>;                // same, as argv items
    pub fn config(&self) -> Result<config::Config>;       // stored or default
}
pub enum Output { Json(serde_json::Value), Text(String) }
pub async fn run(root: &Path, cmd: cli::CoordCommand) -> Result<Output>;   // S9: caller check + dispatch
```

### 4.2 `paths.rs` (S0)

```rust
pub struct CoordPaths {
    pub dir: PathBuf,        // <root>/coord           (mode 0700)
    pub journal: PathBuf,    // coord/journal/         YYYY-MM.jsonl (UTC month of ts_ms)
    pub state: PathBuf,      // coord/state.json
    pub lock: PathBuf,       // coord/lock             journal + state writes
    pub up_lock: PathBuf,    // coord/up.lock          serialises up/down/restart
    pub watcher_lock: PathBuf, // coord/watcher.lock   single watcher
    pub heartbeat: PathBuf,  // coord/heartbeat.json   outside journal/ (board-design Q7)
    pub config: PathBuf,     // coord/config.json
    pub briefs: PathBuf,     // coord/briefs/<task>.md, coordinator-<n>.md
    pub results: PathBuf,    // coord/results/<task>.md
    pub sessions: PathBuf,   // coord/sessions/<task>/, coordinator-<n>/
    pub roles: PathBuf,      // coord/roles/<name>.md (person overrides)
    pub home: PathBuf,       // coord/home             coordinator's cwd
}
impl CoordPaths {
    pub fn new(root: &Path) -> Self;
    pub fn ensure(&self) -> Result<()>;          // create dirs, refuse symlinks (same rule as manager::no_links)
    pub fn month_file(&self, ts_ms: u64) -> PathBuf;
    pub fn brief(&self, name: &str) -> PathBuf;
    pub fn result(&self, task: &TaskId) -> PathBuf;
    pub fn session_dir(&self, name: &str) -> PathBuf;
}
pub fn utc_month(ts_ms: u64) -> (i32, u32);      // civil-from-days, no chrono
```

### 4.3 `ids.rs` (S0)

```rust
pub struct WsId(String);    // ^ws-[a-z2-7]{6}$   (6 chars RFC 4648 base32, lowercase)
pub struct TaskId(String);  // ^ws-[a-z2-7]{6}-t[1-9][0-9]*$  (≤ 32 chars: valid hc --name)
impl WsId   { pub fn generate() -> Result<Self>; pub fn parse(s: &str) -> Result<Self>; pub fn as_str(&self) -> &str }
impl TaskId { pub fn new(ws: &WsId, n: u32) -> Self; pub fn parse(s: &str) -> Result<Self>; pub fn ws(&self) -> WsId; pub fn n(&self) -> u32; pub fn as_str(&self) -> &str }
pub fn space_id() -> Result<String>;    // "sp" + 32 lowercase hex (spec §5)
pub fn uuid_v4() -> Result<String>;     // claude/copilot preset session ids
pub fn random<const N: usize>() -> Result<[u8; N]>;   // /dev/urandom
```
All id types derive `Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize` (transparent strings).

### 4.4 `kinds.rs` (S0): envelope and every kind

```rust
pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {                      // one journal line; == board-design LogEntry
    pub v: u32,
    pub seq: u64,
    pub ts_ms: u64,
    pub actor: Actor,                   // serialised as "coordinator" | "watcher" | "user" | "cli" | "task:<id>"
    pub kind: String,
    pub ws: Option<WsId>,
    pub task: Option<TaskId>,
    pub data: serde_json::Map<String, serde_json::Value>,
}
impl Entry { pub fn typed(&self) -> Kind; }   // never fails: decode error or unknown kind -> Kind::Unknown

pub enum Actor { Coordinator, Watcher, User, Cli, Task(TaskId) }   // custom Serialize/Deserialize as strings

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum Kind {
  #[serde(rename = "ws.opened")]   WsOpened { title: String, repo: Option<PathBuf>, delivery_target: Option<String>, delivery_branch: Option<String>, purpose: Option<String> },
  #[serde(rename = "ws.closed")]   WsClosed { summary: String },
  #[serde(rename = "ws.cleaned")]  WsCleaned {},
  #[serde(rename = "ws.retained")] WsRetained { reason_code: String, reason: String, checks: Vec<Check> },
  #[serde(rename = "task.placed")] TaskPlaced { rung: u8, reason: String, rejected: Vec<Rejected>, role: String, harness: String, model: Option<String>, title: String, brief: PathBuf, session_of: Option<TaskId> },
  #[serde(rename = "task.started")] TaskStarted { terminal: String, tab: Option<String>, tab_name: String, space: String, cwd: PathBuf, pid: Option<u32>, created_ms: u64, session: Option<SessionRef>, resumed: bool },
  #[serde(rename = "task.session")] TaskSession { session: SessionRef },
  #[serde(rename = "task.sent")]   TaskSent { message: PathBuf, via: SendVia /* send|resume */, terminal: String, started: Option<bool> },
  #[serde(rename = "task.reported")] TaskReported { outcome: Outcome /* done|blocked|question */, summary: String, result: Option<PathBuf> },
  #[serde(rename = "task.progress")] TaskProgress { summary: String },
  #[serde(rename = "task.note")]   TaskNote { text: String },
  #[serde(rename = "task.start_blocked")] TaskStartBlocked { terminal: String, after_secs: u64 },
  #[serde(rename = "task.attention")] TaskAttention { state: AttentionState /* blocked|error|clear */, msg: Option<String> },
  #[serde(rename = "task.stalled")] TaskStalled { silent_secs: u64, nudge_seq: u64 },
  #[serde(rename = "task.dormant")] TaskDormant { reason: String /* terminal_finished|terminal_missing|replaced_after_restart */ },
  #[serde(rename = "task.closed")] TaskClosed { reason: String },
  #[serde(rename = "res.intent")]  ResIntent(ResourceRef),
  #[serde(rename = "res.created")] ResCreated(ResourceRef),
  #[serde(rename = "res.adopted")] ResAdopted(ResourceRef),
  #[serde(rename = "res.release_checked")] ResReleaseChecked { resource: String /* "<type>:<name>" */, r#type: ResType, name: String, check: String, ok: bool, detail: Option<String> },
  #[serde(rename = "res.released")] ResReleased { r#type: ResType, name: String, note: Option<String> },
  #[serde(rename = "nudge.sent")]  NudgeSent { target: NudgeTarget /* coordinator|task */, text: String, upto_seq: Option<u64> },
  #[serde(rename = "reconcile.finding")] ReconcileFinding { code: String, summary: String },
  #[serde(rename = "watcher.started")] WatcherStarted { release: String, terminal: Option<String>, pid: u32 },
  #[serde(rename = "watcher.stopped")] WatcherStopped { reason: String },
  #[serde(rename = "coord.session_started")] CoordSessionStarted { role: String, model: Option<String>, terminal: String, space: String, tab: Option<String>, session: Option<SessionRef>, release: String, reason: String, brief: PathBuf },
  #[serde(rename = "coord.session_ended")] CoordSessionEnded { reason: String, terminal: String },
  #[serde(rename = "coord.handoff_requested")] CoordHandoffRequested { by: String, note: Option<String> },
  #[serde(rename = "inbox.drained")] InboxDrained { cursor: u64, count: u64 },
  #[serde(rename = "decision")]    Decision { what: String, why: String, tier: Tier /* silent|digest|ask */, qid: Option<String> },
  #[serde(rename = "question.asked")] QuestionAsked { qid: String, text: String, options: Option<Vec<String>> },
  #[serde(rename = "question.answered")] QuestionAnswered { qid: String, text: String },
  #[serde(rename = "scout.result")] ScoutResult { question: String, summary: String },
  #[serde(rename = "instruction.set")] InstructionSet { id: String, text: Option<String> },   // None = removed
  #[serde(rename = "thread.opened")] ThreadOpened { id: String, title: String, note: Option<String> },
  #[serde(rename = "thread.closed")] ThreadClosed { id: String, title: String, note: Option<String> },
  #[serde(rename = "correction")]  Correction { what: CorrectionWhat /* placement|model|cleanup|other */, why: String, r#ref: String },
  #[serde(other)] Unknown,
}

pub struct ResourceRef {                  // serialised as `data` of res.*; field names per board-design §4.4.3
    pub r#type: ResType,                  // "space" | "worktree" | "branch" | "tab"
    pub name: String,                     // space id | worktree path | branch name | terminal address
    pub repo: Option<PathBuf>,            // worktree, branch
    pub folder: Option<PathBuf>,          // space
    pub label: Option<String>,            // space display name at creation/adoption
    pub branch: Option<String>,           // worktree intent
    pub base: Option<String>,             // worktree intent: ref; branch created: base sha
    pub create_branch: Option<bool>,      // worktree intent
    pub space: Option<String>,            // tab
    pub tab: Option<String>,              // tab
}
pub struct SessionRef { pub mode: SessionMode, pub id: Option<String>, pub dir: Option<PathBuf> }
pub struct Check { pub name: String, pub ok: bool, pub detail: Option<String> }
pub struct Rejected { pub rung: u8, pub reason: String }

impl Kind {
    pub fn name(&self) -> &'static str;
    pub fn is_waking(&self, actor: &Actor) -> bool;      // G9
}
```
All option fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`
except where the board-design table needs explicit `null` (`task.reported.result`).
`journal::Journal::append` fills `ws`/`task` in the envelope from its arguments;
`data` never repeats them.

### 4.5 `state.rs` (S0 types) and `fold.rs` (S1)

```rust
pub const FOLD_VERSION: u32 = 1;
pub struct State {
    pub fold_version: u32, pub last_seq: u64, pub last_ts_ms: u64,
    pub workstreams: BTreeMap<WsId, Ws>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub resources: Vec<ResourceRec>,
    pub intents: BTreeMap<u64, Intent>,               // seq -> unresolved res.intent
    pub projects: BTreeMap<PathBuf, Project>,          // repo main checkout -> purpose
    pub coordinator: CoordRec,                         // terminal, space, tab, session, role, release, started_ms, sessions_started, handoff: Option<(seq, by)>
    pub watcher: WatcherRec,                           // last watcher.started/stopped
    pub cursor: u64,                                   // last inbox.drained cursor
    pub waking: BTreeSet<u64>,                         // waking seqs > cursor
    pub last_coordinator_nudge_upto: u64,
    pub questions: BTreeMap<String, Question>,         // qid -> {text, options, ws, task, asked_seq, asked_ms, answered: Option<..>}
    pub instructions: BTreeMap<String, String>,
    pub threads: BTreeMap<String, Thread>,
    pub decisions: VecDeque<DecisionRec>,              // last 50
    pub counters: Counters,                            // next ids: instruction, thread, question
    pub anomalies: Vec<Anomaly>,                       // {seq, message}
}
pub struct Ws { pub id: WsId, pub title: String, pub repo: Option<PathBuf>, pub delivery_target: Option<String>,
    pub delivery_branch: Option<String>, pub state: WsState /* Open|Closed|Cleaned */, pub retained: Option<Retained>,
    pub summary: Option<String>, pub opened_ms: u64, pub closed_ms: Option<u64>, pub last_event_ms: u64,
    pub space: Option<SpaceRec /* id, name, owned, folder */>, pub cwd: Option<PathBuf>, pub tasks: Vec<TaskId> }
pub struct Retained { pub reason_code: String, pub reason: String, pub since_ms: u64, pub checked_ms: Option<u64>, pub checks: Vec<Check> }
pub struct Task { pub id: TaskId, pub ws: WsId, pub n: u32, pub title: String, pub state: TaskState /* Placed|Running|Reported|Dormant|Closed */,
    pub report: Option<Outcome>, pub start_blocked: Option<u64 /* seq */>, pub stalled: Option<u64>, pub attention: Option<AttentionState>,
    pub state_since_ms: u64, pub placed_ms: u64, pub started_ms: Option<u64>, pub last_event_ms: u64,
    pub rung: u8, pub reason: String, pub rejected: Vec<Rejected>, pub role: String, pub harness: String, pub model: Option<String>,
    pub summary: Option<String>, pub terminal: Option<String>, pub space: Option<String>, pub tab: Option<String>, pub cwd: Option<PathBuf>,
    pub pid: Option<u32>, pub created_ms: Option<u64>, pub session: Option<SessionRef>, pub brief: PathBuf, pub result: Option<PathBuf>,
    pub nudges: u32, pub last_nudge: Option<(u64 /*seq*/, u64 /*ms*/)>, pub last_activity_seq: u64 /* last started/sent */,
    pub reported_seq: Option<u64> }
pub struct ResourceRec { pub r: ResourceRef, pub ws: Option<WsId>, pub task: Option<TaskId>, pub owned: bool, pub released: bool,
    pub created_seq: u64, pub last_checks: BTreeMap<String, (bool, Option<String>)> }
pub struct Intent { pub seq: u64, pub ts_ms: u64, pub ws: Option<WsId>, pub task: Option<TaskId>, pub r: ResourceRef }

// fold.rs
pub fn apply(state: &mut State, e: &Entry);
pub fn fold<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> State;
```

### 4.6 `journal.rs` (S1)

```rust
pub struct Journal { paths: CoordPaths }
pub struct Locked<'a> { journal: &'a Journal, file: File, state: State }   // holds exclusive flock on coord/lock
pub struct LogQuery { pub limit: usize, pub before_seq: Option<u64>, pub after_seq: Option<u64>,
                      pub since_ms: Option<u64>, pub ws: Option<WsId>, pub task: Option<TaskId> }
pub struct LogPage { pub entries: Vec<serde_json::Value>, pub has_more: bool }

impl Journal {
    pub fn open(paths: &CoordPaths) -> Result<Self>;
    pub fn lock(&self) -> Result<Locked<'_>>;                // flock, 20 s deadline -> error code "lock_busy" (exit 4); catches up state
    pub fn read_state(&self) -> Result<State>;               // no lock, no write
    pub fn read_after(&self, seq: u64) -> Result<Vec<Entry>>; // typed, ascending
    pub fn page(&self, q: &LogQuery) -> Result<LogPage>;     // raw lines, newest first
    pub fn last_seq(&self) -> Result<u64>;
}
impl Locked<'_> {
    pub fn state(&self) -> &State;
    pub fn append(&mut self, actor: &Actor, now_ms: u64, items: Vec<(Option<WsId>, Option<TaskId>, Kind)>) -> Result<Vec<Entry>>;
        // assigns seq, writes lines (one write per line, O_APPEND, fsync file), applies to state, persists state.json atomically
}
pub fn transact<T>(j: &Journal, actor: &Actor, now_ms: u64,
    f: impl FnOnce(&State) -> Result<(Vec<(Option<WsId>, Option<TaskId>, Kind)>, T)>) -> Result<(Vec<Entry>, T)>;
```
Rule for all slices: never hold `Locked` across an `hc`, `herdr-app` or git
call. Pattern: `transact` (validate + intent) → external call → `transact`
(record outcome).

### 4.7 `config.rs` (S0)

```rust
#[derive(Serialize, Deserialize)] #[serde(deny_unknown_fields, default)]
pub struct Config { pub autostart: bool, pub coordinator_role: String, pub nudge_coordinator_secs: u64,
    pub nudge_delegate_secs: u64, pub stall_secs: u64, pub start_blocked_secs: u64, pub reconcile_secs: u64,
    pub idle_restart_hours: u64, pub worktree_root: String }
// Default: true, "coord-main", 10, 60, 300, 30, 120, 8, "~/.herdr/worktrees"  (spec §12)
impl Config {
    pub fn load(paths: &CoordPaths) -> Result<Option<Config>>;   // None = never enabled
    pub fn save(&self, paths: &CoordPaths) -> Result<()>;         // atomic
    pub fn worktree_root(&self) -> Result<PathBuf>;               // expands a leading "~/" with $HOME
}
```

### 4.8 `error.rs` (S0)

```rust
pub struct CoordError { pub code: &'static str, pub message: String, pub exit: i32 }   // impl Display + Error
pub fn refused(code: &'static str, message: impl Into<String>) -> anyhow::Error;   // exit 3
pub fn not_ready(code: &'static str, message: impl Into<String>) -> anyhow::Error; // exit 4
pub fn exit_of(e: &anyhow::Error) -> (&'static str, i32);   // downcast; default ("operation_failed", 1)
```
Codes (exit 3): `forbidden_caller`, `not_a_delegate`, `ws_not_found`, `task_not_found`, `ws_not_open`, `ws_open`, `tasks_running`, `task_closed`, `ladder_reason_missing`, `ladder_reason_extra`, `rung_arguments`, `follow_up_cannot_create_space`, `no_space`, `coordinator_space`, `no_session`, `role_mismatch`, `role_invalid`, `agent_blocked`, `invalid_argument`, `not_enabled`. Exit 4: `agent_not_ready`, `lock_busy`, `hc_unavailable`.

### 4.9 `adapter.rs` (S0)

```rust
pub struct RunSpec { pub name: String, pub cwd: PathBuf, pub labels: Vec<(String, String)>, pub argv: Vec<String> }
#[derive(Deserialize)] pub struct Term {          // hc Terminal JSON; unknown fields ignored
    pub address: String, pub state: String, pub status: Option<AgentStatus>, pub status_ms: Option<u64>,
    #[serde(default)] pub turn: u64, #[serde(default)] pub reported: bool,
    #[serde(default)] pub labels: BTreeMap<String, String>, pub pid: Option<u32>, pub created_ms: u64,
    pub cwd: String, pub foreground: Option<Foreground>, pub reason: Option<String>, pub name: Option<String> }
pub struct AgentStatus { pub state: String, pub app: Option<String>, pub kind: Option<String>, pub msg: Option<String> }
pub struct Foreground { pub pid: i32, pub name: String, pub cwd: String }
impl Term {
    pub fn live(&self) -> bool;                  // state == "live"
    pub fn word(&self) -> Option<&str>;          // status.state
    pub fn settled(&self) -> bool;               // word is idle or done
    pub fn sendable(&self) -> bool;              // live && settled && turn >= 1 && reported
    pub fn label(&self, k: &str) -> Option<&str>;
    pub fn managed(&self) -> bool;               // label coord.managed == "1"
}
pub struct SendOutcome { pub turn: u64, pub started: Option<bool>, pub status: Option<String> }
pub trait Hc {
    fn run(&self, spec: &RunSpec) -> Result<Term>;
    fn get(&self, terminal: &str) -> Result<Option<Term>>;           // None on not_found
    fn list(&self, all: bool, label: Option<(&str, &str)>) -> Result<Vec<String>>;   // addresses
    fn send(&self, terminal: &str, text: &str) -> Result<SendOutcome>;
    fn label(&self, terminal: &str, labels: &[(&str, &str)]) -> Result<()>;
    fn kill(&self, terminal: &str) -> Result<()>;
    fn rm(&self, terminal: &str) -> Result<()>;
    fn events_command(&self) -> std::process::Command;              // watcher spawns it via tokio
}
pub struct AppSnapshot { pub spaces: Vec<AppSpace> }
pub struct AppSpace { pub id: String, pub name: String, pub folder: String, pub machine: Option<String>, pub tabs: Vec<AppTab> }
pub struct AppTab { pub id: String, pub name: String, pub terminals: Vec<AppTerm> }
pub struct AppTerm { pub terminal: String, pub pane: String }
pub trait App {
    fn ls(&self) -> Result<AppSnapshot>;                                  // Err = window unavailable
    fn command(&self, name: &str, args: serde_json::Value) -> Result<serde_json::Value>;
}
impl AppSnapshot { pub fn tab_of(&self, terminal: &str) -> Option<(&AppSpace, &AppTab)>; pub fn space(&self, id: &str) -> Option<&AppSpace>; }
pub struct ProcHc { exe: PathBuf }   impl ProcHc  { pub fn from_env() -> Self }   // HERDR_KIT_HC
pub struct ProcApp { exe: PathBuf }  impl ProcApp { pub fn from_env() -> Self }   // HERDR_KIT_HERDR_APP
#[cfg(test)] pub mod fake { pub struct FakeHc { … } pub struct FakeApp { … } }   // record calls, scripted results
```
`ProcHc::get` maps a nonzero exit whose stdout or stderr contains `not_found` to `Ok(None)` [INFERENCE: CLI error text, §11].

### 4.10 `caller.rs` (S0)

```rust
pub enum Caller { Person { terminal: Option<String> }, Coordinator { terminal: String },
                  Delegate { terminal: String, task: TaskId, ws: WsId }, Watcher { terminal: String } }
impl Caller { pub fn actor(&self) -> Actor; }
pub fn resolve(hc: &dyn Hc) -> Result<Caller>;
```
`HC_TERMINAL_ID` unset or empty → `Person{None}` (actor `cli`). Set → `hc get`;
`not_found` or hc failure → error `hc_unavailable` (exit 4) for verbs that need
a caller. Labels `coord.managed=1` + `coord.role=coordinator` → Coordinator;
`coord.role=delegate` + `coord.task` → Delegate (task id must parse); `coord.role=watcher` → Watcher;
anything else → `Person{Some}` (actor `user`).

### 4.11 `role.rs` and `brief.rs` (S2)

```rust
pub enum SessionMode { Dir, Preset, CodexRollout, None }          // "dir" | "preset" | "codex-rollout" | "none"
pub enum RoleSource { Builtin, File(PathBuf) }
pub struct Role { pub name: String, pub harness: String, pub start: Vec<String>, pub resume: Option<Vec<String>>,
                  pub session: SessionMode, pub trust: Vec<String>, pub body: String, pub source: RoleSource }
pub struct Vars<'a> { pub brief: &'a str, pub session_dir: Option<&'a Path>, pub session_id: Option<&'a str>,
                      pub repo_root: Option<&'a Path>, pub cwd: &'a Path }
pub const BUILTIN: [(&str, &str); 7];   // (name, include_str!("roles/<name>.md"))
pub fn builtin(name: &str) -> Option<&'static str>;
pub fn load(paths: &CoordPaths, name: &str) -> Result<Role>;       // roles/<name>.md wins over built-in
pub fn list(paths: &CoordPaths) -> Result<Vec<(String, RoleSource, bool /*overrides a built-in*/)>>;
pub fn parse(name: &str, text: &str, source: RoleSource) -> Result<Role>;
pub fn split_words(s: &str) -> Result<Vec<String>>;
pub fn expand(template: &[String], vars: &Vars, trust: Option<&[String]>) -> Result<Vec<String>>;
pub fn model_of(argv: &[String]) -> Option<String>;
```
Parsing: front matter between the first two `---` lines; `key: value` per line;
a value is cut at the first ` #` (space + hash) for comments; keys `harness`,
`start` (required), `resume`, `session` (required), `trust`; any other key is an
error. `split_words`: whitespace separates; `'…'` literal; `"…"` with `\"` and
`\\`; backslash outside quotes escapes one char; unterminated quote is an
error. `expand`: placeholders `{brief}`, `{session_dir}`, `{session_id}`,
`{repo_root}`, `{cwd}` replaced inside each argument; an unknown `{name}` or a
needed placeholder with no value or an empty value is an error (protects
`codex resume ""`, spec §5); `trust` items are expanded and inserted after
`argv[0]`. Nothing is passed through a shell (spec §12).

```rust
// brief.rs
pub const FOOTER: &str;                  // §8.2, with {cli} and {result}
pub const DELEGATE_NUDGE: &str;          // §8.3
pub fn inbox_nudge(n: usize) -> String;  // "inbox: <n> new" (spec §6)
pub fn delegate_first(role: &Role, brief: &str, cli: &str, result: &Path) -> String;   // body + "\n\n" + brief + footer
pub fn follow_up(brief: &str, cli: &str, result: &Path) -> String;                   // brief + footer (live session already has the role)
pub fn coordinator_first(role: &Role, cli: &str, startup_read: &str) -> String;      // body + "## Commands" + "## Startup read"
pub fn for_argv(message: String, saved_at: &Path) -> String;                         // G13
pub fn title_of(brief: &str) -> String;                                               // G4
```

### 4.12 `views.rs` (S3): read models

```rust
pub fn attention(state: &State, coordinator_word: Option<&str>) -> Vec<Attention>;   // board-design §4.4.2
pub fn counts(state: &State, attention_len: usize) -> Counts;                          // running, needs_you, retained, open_workstreams
pub fn board(state: &State, coordinator_word: Option<&str>, now_ms: u64) -> serde_json::Value;   // Board, schema_version 1
pub fn board_text(state: &State, now_ms: u64) -> String;                               // human/agent Markdown
pub fn log(j: &Journal, q: &LogQuery, now_ms: u64) -> Result<serde_json::Value>;       // Log, schema_version 1
pub fn log_text(page: &LogPage) -> String;
pub fn explain(j: &Journal, state: &State, id: &str, now_ms: u64) -> Result<serde_json::Value>;
pub fn explain_text(v: &serde_json::Value) -> String;
pub fn inbox(j: &Journal, state: &State) -> Result<serde_json::Value>;
pub fn startup_read(state: &State, now_ms: u64) -> String;                             // ≤ 32 KiB (G13)
```

### 4.13 `git.rs` and `release.rs` (S4)

```rust
// git.rs  (all run `git -C <dir> …` with LC_ALL=C, GIT_TERMINAL_PROMPT=0, deadline)
pub fn main_checkout(dir: &Path) -> Result<PathBuf>;
pub fn current_branch(repo: &Path) -> Result<Option<String>>;
pub fn branch_exists(repo: &Path, branch: &str) -> Result<bool>;
pub fn rev(repo: &Path, rev: &str) -> Result<String>;
pub fn worktree_add(repo: &Path, path: &Path, branch: &str, create_from: Option<&str>) -> Result<()>;
pub struct Worktree { pub path: PathBuf, pub head: Option<String>, pub branch: Option<String> }
pub fn worktree_list(repo: &Path) -> Result<Vec<Worktree>>;
pub fn dirty(worktree: &Path) -> Result<Vec<String>>;                  // porcelain lines
pub fn unreachable_count(repo: &Path, head: &str, delivery_branch: &str) -> Result<u64>;
pub fn is_ancestor(repo: &Path, commit: &str, of: &str) -> Result<bool>;
pub fn worktree_remove(repo: &Path, path: &Path) -> Result<()>;
pub fn delete_branch(repo: &Path, branch: &str, expected_sha: &str) -> Result<()>;
pub fn slug(branch: &str) -> String;
pub fn worktree_path(root: &Path, main: &Path, branch: &str) -> PathBuf;

// release.rs
pub enum Mode { Command, Recheck }
pub struct Outcome { pub cleaned: bool, pub released: Vec<ResourceRef>, pub retained: Option<Retained> }
pub fn release_task_tab(ctx: &Ctx, j: &Journal, actor: &Actor, task: &TaskId) -> Result<Vec<Check>>;
pub fn release_ws(ctx: &Ctx, j: &Journal, actor: &Actor, ws: &WsId, mode: Mode) -> Result<Outcome>;
pub fn check_tab(ctx: &Ctx, state: &State, rec: &ResourceRec) -> Result<Vec<Check>>;      // pure-ish: reads hc
pub fn check_space(ctx: &Ctx, state: &State, rec: &ResourceRec, app: &AppSnapshot) -> Result<Vec<Check>>;
pub fn check_worktree(ctx: &Ctx, state: &State, rec: &ResourceRec) -> Result<Vec<Check>>;
pub fn check_branch(state: &State, rec: &ResourceRec) -> Result<Vec<Check>>;
```

### 4.14 `launch.rs` and `place.rs` (S5)

```rust
// launch.rs: shared by tasks, coordinator and watcher
pub struct LaunchPlan { pub name: String, pub role_label: &'static str /* delegate|coordinator|watcher */,
    pub ws: Option<WsId>, pub task: Option<TaskId>, pub space: String, pub tab: Option<String>,
    pub cwd: PathBuf, pub argv: Vec<String>, pub session: Option<SessionRef>, pub tab_name: String,
    pub space_name: Option<String> /* rename when the space is new */ }
pub struct Launched { pub term: Term, pub tab: Option<String>, pub session: Option<SessionRef> }
pub fn launch(ctx: &Ctx, plan: &LaunchPlan) -> Result<Launched>;     // hc run, tab lookup, renames; no journal writes
pub fn discover_session(ctx: &Ctx, s: &SessionRef, cwd: &Path, since_ms: u64, wait_ms: u64) -> Option<SessionRef>;

// place.rs: command handlers (each returns the JSON printed on stdout)
pub fn ws_open(ctx: &Ctx, c: &Caller, a: &cli::WsOpen) -> Result<Value>;
pub fn ws_close(ctx: &Ctx, c: &Caller, ws: &WsId, summary: &str) -> Result<Value>;
pub fn ws_list(ctx: &Ctx, all: bool) -> Result<Value>;
pub fn task_start(ctx: &Ctx, c: &Caller, a: &cli::TaskStart) -> Result<Value>;
pub fn task_send(ctx: &Ctx, c: &Caller, task: &TaskId, text: &str) -> Result<Value>;
pub fn task_close(ctx: &Ctx, c: &Caller, task: &TaskId, reason: &str) -> Result<Value>;
pub fn report(ctx: &Ctx, c: &Caller, outcome: cli::ReportKind, summary: &str, result: Option<&Path>) -> Result<Value>;
pub fn note(ctx: &Ctx, c: &Caller, text: &str) -> Result<Value>;
pub fn decide(ctx: &Ctx, c: &Caller, a: &cli::Decide) -> Result<Value>;
pub fn ask(ctx: &Ctx, c: &Caller, a: &cli::Ask) -> Result<Value>;
pub fn instruct(ctx: &Ctx, c: &Caller, a: &cli::Instruct) -> Result<Value>;
pub fn thread(ctx: &Ctx, c: &Caller, a: &cli::ThreadCmd) -> Result<Value>;
pub fn correct(ctx: &Ctx, c: &Caller, a: &cli::Correct) -> Result<Value>;
pub fn scout(ctx: &Ctx, c: &Caller, a: &cli::Scout) -> Result<Value>;
pub fn inbox_ack(ctx: &Ctx, c: &Caller, seq: u64) -> Result<Value>;
```

### 4.15 `lifecycle.rs` (S7)

```rust
pub struct Heartbeat { pub pid: u32, pub terminal: Option<String>, pub release: String,
                       pub started_ms: u64, pub beat_ms: u64, pub events_ok: bool }
pub const HEARTBEAT_EVERY_MS: u64 = 5_000; pub const HEARTBEAT_STALE_MS: u64 = 30_000;
pub fn read_heartbeat(paths: &CoordPaths) -> Result<Option<Heartbeat>>;
pub fn write_heartbeat(paths: &CoordPaths, hb: &Heartbeat) -> Result<()>;
pub fn active_release(root: &Path) -> String;                       // manager::active_release or "development"
pub fn up(ctx: &Ctx, if_enabled: bool) -> Result<Value>;
pub fn down(ctx: &Ctx) -> Result<Value>;
pub fn status(ctx: &Ctx) -> Result<Value>;                           // board-design §4.4.1
pub fn restart(ctx: &Ctx, c: &Caller, force: bool) -> Result<Value>;
pub fn handoff(ctx: &Ctx, c: &Caller, note: Option<&str>) -> Result<Value>;
pub fn start_coordinator(ctx: &Ctx, j: &Journal, reason: &str) -> Result<()>;   // also used by the watcher
pub fn restart_coordinator(ctx: &Ctx, j: &Journal, reason: &str) -> Result<()>; // same tab (D15)
pub fn after_activation(root: &Path) -> Value;                       // D18, called from main.rs
```

### 4.16 `rules.rs`, `watch.rs`, `reconcile.rs` (S6)

```rust
// rules.rs (pure)
pub struct Facts { pub terms: BTreeMap<String, Term>, pub events_ok: bool }
pub enum Action {
    Send { terminal: String, text: String, record: Kind, ws: Option<WsId>, task: Option<TaskId> },  // record = nudge.sent
    Append { ws: Option<WsId>, task: Option<TaskId>, kind: Kind },
    CloseLeftover { task: TaskId, terminal: String },        // kill + rm a replacement shell of an owned delegate tab
    StartCoordinator { reason: String },
    RestartCoordinator { reason: String },
}
pub fn evaluate(state: &State, facts: &Facts, cfg: &Config, now_ms: u64) -> Vec<Action>;

// watch.rs
pub enum Line { Snapshot(Vec<Term>), Event { terminal: String, kind: String }, Other }
pub fn parse_line(line: &str) -> Line;
pub struct Core { pub facts: Facts, pub release: String, pub started_ms: u64 }
impl Core {
    pub fn on_line(&mut self, line: Line, hc: &dyn Hc) -> Result<()>;     // D8
    pub fn tick(&mut self, ctx: &Ctx, j: &Journal, cfg: &Config) -> Result<Vec<Entry>>;   // evaluate + execute
}
pub async fn run(ctx: Ctx) -> Result<Value>;                          // `coord watch`

// reconcile.rs
pub fn reconcile(ctx: &Ctx, j: &Journal, facts: &mut Facts, cfg: &Config) -> Result<Vec<Entry>>;
```

## 5. Command surface

### 5.1 Clap tree (`cli.rs`, S0)

`main.rs` gains `Command::Coord { #[command(subcommand)] command: coord::cli::CoordCommand }`
(doc comment: "Coordinate parallel agent work"). Global `--data-dir` applies.

```
herdr-kit [--data-dir DIR] coord <verb>

  up [--if-enabled]
  down
  status [--json]                                   (always JSON; flag accepted for the mod)
  restart [--force]
  handoff [--note TEXT]
  board [--json] [--all]                            (--all includes cleaned workstreams)
  log [--json] [--limit N=200 (1..=500)] [--before-seq SEQ] [--after-seq SEQ]
      [--since SEQ|DURATION] [--ws WS] [--task TASK]      DURATION = <n>s|m|h|d
  explain <WS|TASK> [--json]
  inbox [--json]                                    (always JSON)
  inbox ack <SEQ>
  roles [--defaults] [NAME]                         (list; NAME prints the effective file; --defaults prints built-ins)
  ws open --title TEXT --target TEXT [--repo PATH] [--delivery-branch REF] [--purpose TEXT]
  ws close <WS> --summary TEXT
  ws list [--all]
  task start --ws WS --rung 1..5 --reason TEXT --brief FILE|- [--role NAME]
             [--reject N=REASON]... [--title TEXT]
             [--session-of TASK] [--space ID] [--branch NAME] [--base REF] [--folder PATH]
  task send <TASK> (--text TEXT | --file FILE|-)
  task close <TASK> --reason TEXT
  release <WS>
  decide --what TEXT --why TEXT --tier silent|digest|ask [--ws WS] [--task TASK] [--option TEXT]...
  ask [--ws WS] [--task TASK] [--option TEXT]... <TEXT>
  ask --answer QID <TEXT>
  instruct <TEXT> | instruct --replace ID <TEXT> | instruct --remove ID
  thread open <TITLE> [--note TEXT]
  thread close <ID> [--note TEXT]
  correct --what placement|model|cleanup|other --ref WS|TASK --why TEXT
  scout --question TEXT --summary TEXT [--ws WS]
  report done|blocked|question|progress --summary TEXT [--result FILE]
  note <TEXT>
  watch
```
Clap rules: `--role` required unless `--rung 1`; `--text`/`--file` exclusive
and one required; `instruct` arguments as a `#[command(group)]`; `ask --answer`
requires the text and conflicts with `--option`/`--ws`/`--task`. `--brief -`
and `--file -` read stdin (≤ 1 MiB).

### 5.2 Caller classes and permissions (enforced in `mod.rs::run`, S9)

| Verb | Person (`cli`, `user`) | Coordinator | Delegate | Watcher |
|---|---|---|---|---|
| `status`, `board`, `log`, `explain`, `roles`, `ws list`, `inbox` (read) | yes | yes | yes | yes |
| `up`, `down`, `restart` | yes | no (`handoff`) | no | no |
| `handoff` | no | yes | no | no |
| `ws open/close`, `task start/send/close`, `release`, `decide`, `ask`, `instruct`, `thread`, `correct`, `scout`, `inbox ack` | yes (drives by hand; spec §11) | yes | no | no |
| `report`, `note` | no (`not_a_delegate`) | no | own task only (from labels) | no |
| `watch` | yes (manual/testing) | no | no | yes |

Read verbs do not resolve the caller (no `hc` call). Refusals exit 3 with
`forbidden_caller` or `not_a_delegate`.

### 5.3 Outputs

`status`, `board --json`, `log --json`: exactly board-design §4.4.1–4.4.3
(including `schema_version: 1`, `generated_at_ms`, `*_ms` integers). Derivations:

- `status.state`: `running` = heartbeat fresh (`now - beat_ms ≤ 30 000`) and the recorded coordinator terminal is live with the recorded pid; `degraded` = config exists and exactly one of the two holds, or `hc` is unreachable; `not_running` = neither. `degraded[]` codes: `watcher_heartbeat_stale` (heartbeat exists but stale; `since_ms` = `beat_ms`; `detail` "watcher heartbeat stale for 3m 20s"), `watcher_missing` (no heartbeat or its terminal is gone), `coordinator_terminal_missing`, `hc_unreachable`.
- `status.release` = `active_release(root)`; `watcher.release` from the heartbeat; `coordinator.status` = the hc word of the coordinator terminal (one `hc get`).
- `counts.running` = tasks `running` without `stalled`, plus `placed`; `counts.needs_you` = `views::attention(...).len()` with the same coordinator word; `counts.retained` = workstreams with `retained`; `counts.open_workstreams` = state `open`.
- `journal_dir` = `coord/journal` absolute; `journal_seq` = `state.last_seq`.
- Board `Workstream.retained.auto_clears` = `true` (reconcile re-checks); `checked_ms` = last recheck ms; `repo.name` = basename of the main checkout; `space.name` = recorded label; `Task.placement_reason` = `task.placed.reason`; `Attention.id` = `q-<seq>` (question), `r-<seq>` (task_question/task_blocked, seq of the report), `sb-<seq>`, `st-<seq>`, `cb-<terminal>`.
- `log` entries are stored lines (D2); `has_more` true when older matching entries exist; `--before-seq`/`--after-seq` per board-design.

Shapes defined here (not in the design):

```json
// coord inbox
{"schema_version":1,"cursor":50,"last_seq":57,"pending":3,"entries":[<LogEntry>…]}   // seq > cursor, actor != coordinator, ascending
// coord inbox ack 57
{"cursor":57,"count":3}
// coord ws open
{"ws":"ws-k3m2ab"}
// coord task start / task send (resume) 
{"task":"ws-k3m2ab-t1","state":"running","terminal":"t31","space":"sp…","tab":"tb…","session":"01a1…"|null}
// coord task send (live)
{"task":"…","sent":true,"turn":3,"started":true}
// coord release
{"ws":"…","cleaned":false,"released":[{"type":"tab","name":"t31"}],"retained":{"reason_code":"uncommitted_files","reason":"…","checks":[…]}}
// coord explain --json
{"schema_version":1,"kind":"task"|"ws","id":"…","record":<board Task or Workstream>,"entries":[<LogEntry>… ascending],
 "checks":[{"resource":"worktree:/…","name":"clean","ok":false,"detail":"…","seq":81}],"anomalies":[…]}
// coord up
{"state":"running"|"disabled","actions":["created_config","started_watcher","started_coordinator"],"setup":[{"code":"claude_worktree_trust","message":"…"}],"status":<status>}
// coord down
{"state":"not_running","stopped":["coordinator","watcher"]}
// coord restart / handoff
{"state":"restarted"|"requested","reason":"user"|"handoff"}
// coord report / note / decide / ask / instruct / thread / correct / scout
{"seq":91}            (decide --tier ask and ask also return "qid")
// coord roles
{"roles":[{"name":"coord-main","source":"builtin"|"file","overrides_builtin":false,"harness":"omp","model":"@plan"}]}
```
Text forms (`board`, `log`, `explain` without `--json`; `roles NAME`,
`roles --defaults`) print Markdown for agents and people; `main.rs` prints
`Output::Text` as-is.

## 6. External command lines

All `hc` calls: `Command::new(hc_exe).env_remove("HC_TERMINAL_ID")`, stdin
null, stdout/stderr captured (≤ 8 MiB / 8 KiB). Address and labels are
verified by `[cap]` help/schema unless marked.

| Step | Command line | Fields read |
|---|---|---|
| Start a delegate in a new space (rung 4/5) | `hc run --json --bg --no-parent --keep --name <task> --cwd <dir> --label herdr.space=<new sp> --label coord.managed=1 --label coord.role=delegate --label coord.ws=<ws> --label coord.task=<task> -- <argv…>` | `address`, `pid`, `created_ms`, `state` |
| New tab in an existing space (rung 1 resume, 2, 3) | same, with the existing space id; no `herdr.tab` label | same |
| Watcher terminal | `hc run --json --bg --no-parent --keep --name coord-watcher --cwd <coord/home> --label herdr.space=<coord sp> [--label herdr.tab=<watcher tb>] --label coord.managed=1 --label coord.role=watcher -- <cli argv…> coord watch` | same |
| Coordinator terminal (start and restart in the same tab, D15) | `hc run --json --bg --no-parent --keep --name coordinator --cwd <coord/home> --label herdr.space=<coord sp> [--label herdr.tab=<coord tb>] --label coord.managed=1 --label coord.role=coordinator -- <coord-main argv…>` | same |
| Tab id lookup (poll every 250 ms, ≤ 5 s) | `herdr-app ls` | `spaces[].id`, `.name`, `.folder`, `.tabs[].id`, `.tabs[].name`, `.tabs[].terminals[].terminal` |
| Name a new space | `herdr-app do space.rename '{"id":"<sp>","name":"<repo> · <title>"}'` | exit code |
| Name a tab | `herdr-app do tab.rename '{"id":"<tb>","name":"<task title>"}'` (coordinator: `coordinator`, watcher: `watcher`) | exit code |
| Coordinator tab first, pinned | `herdr-app do tab.move '{"id":"<tb>","index":1}'`; `herdr-app do terminal.pin '{"terminal":"<t>","pinned":true}'` | exit code |
| Terminal facts | `hc get --json <t>` | `state`, `status.state`, `status_ms`, `turn`, `reported`, `labels`, `pid`, `created_ms`, `foreground.pid`, `foreground.cwd`, `reason` |
| Managed list | `hc ls --json --all --label coord.managed=1` | `terminals[].address`, `.state` |
| Relabel on rung-1 handover | `hc label --json <t> coord.task=<new task> coord.ws=<ws>` | exit code |
| Send / nudge (only when `Term::sendable()`) | `hc send --json <t> <text>` (never `--if`, never `--force`) | `turn`, `started`, `status` |
| End a terminal | `hc kill --json <t>`, then `hc rm --json <t>` (retry `rm` every 250 ms ≤ 5 s while it reports `live`) | exit code |
| Close an emptied tab | `herdr-app do tab.close '{"id":"<tb>","confirmed":true}'` | exit code |
| Close an owned space | `herdr-app do space.close '{"id":"<sp>","confirmed":true}'`; an error containing `no space` counts as success [spec skill] | exit code, `error` |
| Watcher stream | `hc events --json --label coord.managed=1` | snapshot `terminals[]`; event `params.type`, `params.terminal` |
| Main checkout | `git -C <dir> rev-parse --path-format=absolute --git-common-dir` (strip trailing `/.git`; bare repo → refuse) | stdout |
| Delivery branch default | `git -C <repo> symbolic-ref --quiet --short HEAD` | stdout |
| Branch exists | `git -C <repo> show-ref --verify --quiet refs/heads/<b>` | exit code |
| Worktree add (new branch) | `git -C <repo> worktree add -b <branch> <path> <base>` | exit code |
| Worktree add (existing branch, adopted) | `git -C <repo> worktree add <path> <branch>` | exit code |
| Worktree list | `git -C <repo> worktree list --porcelain` | `worktree`, `HEAD`, `branch` lines |
| Clean check | `git -C <wt> status --porcelain=v1 --untracked-files=all` (ignored files are not listed) | empty or not |
| Worktree head | `git -C <wt> rev-parse --verify HEAD` | sha |
| Reachability | `git -C <repo> rev-list --count <head> --not refs/heads/<delivery> --remotes` | `0` = reachable |
| Merged | `git -C <repo> rev-parse --verify refs/heads/<branch>`; `git -C <repo> merge-base --is-ancestor <sha> refs/heads/<delivery>` | sha; exit 0/1 |
| Remove worktree | `git -C <repo> worktree remove <path>` (no `--force`) | exit code |
| Delete branch (compare-and-swap) | `git -C <repo> update-ref -d refs/heads/<branch> <sha>` | exit code |

Session ids:

- `dir` (omp): poll `<sessions>/<task>/` for `*.jsonl`, id = file stem after the first `_` (`2026-10-29T21-01-29-913Z_01a18977-….jsonl` → `01a18977-…`) [cap: live session file name].
- `preset` (claude, copilot): `ids::uuid_v4()` before launch, passed as `{session_id}`.
- `codex-rollout`: newest `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` with mtime ≥ launch, whose first line has `type == "session_meta"`, `payload.cwd == cwd`; id = `payload.id` [spec, skill].
- `none`: no id; the tab stays until the workstream closes.

`launch` polls `dir`/`codex-rollout` for 10 s; when nothing appears,
`task.started.session` has `id: null` and reconcile/the watcher append
`task.session` later.

Never used: `herdr-app do space.new`/`tab.new` (move the view, spec §5),
`hc send --if`, `hc send --force`, `hc keys`, `git worktree remove --force`,
`git branch -D`.

## 7. State machines and crash safety

### 7.1 Workstream

| From | Entry | To | Notes |
|---|---|---|---|
| — | `ws.opened` | `open` | registers project purpose if given |
| `open` | `ws.closed` | `closed` | refused while a task is placed/running (G8) |
| `closed` | `ws.retained` | `closed` + `retained` | reasons replaced on each new `ws.retained` |
| `closed` (+retained) | `ws.cleaned` | `cleaned` | `retained` cleared; omitted from the board |

### 7.2 Task

| From | Entry | To | Flag effects |
|---|---|---|---|
| — | `task.placed` | `placed` | |
| `placed` | `task.started` | `running` | clears `start_blocked`, `stalled`, `attention`; `last_activity_seq` = this seq |
| `placed` | `task.closed` | `closed` | launch failed or interrupted |
| `running` | `task.reported{outcome}` | `reported` | `report` = outcome; clears `stalled`, `start_blocked`; `summary` |
| `running`, `reported` | `task.progress` | unchanged | `summary` |
| `reported`, `dormant` | `task.sent` / `task.started{resumed}` | `running` | `report` = null; clears `stalled`; `last_activity_seq` |
| `running`, `reported`, `placed` | `task.dormant` | `dormant` | `terminal` = null |
| `running` | `task.start_blocked` | unchanged | `start_blocked` = seq |
| any live | `task.attention{blocked|error}` / `{clear}` | unchanged | set / clear `attention`; `clear` also clears `start_blocked` |
| `running` | `nudge.sent{target:task}` | unchanged | `nudges += 1`, `last_nudge` |
| `running` | `task.stalled` | unchanged | `stalled` = seq |
| any but `closed` | `task.closed` | `closed` | |
| rung-1 predecessor | `task.closed{reason:"superseded by <task>"}` | `closed` | terminal now belongs to the new task |

### 7.3 Resource

| From | Entry | To |
|---|---|---|
| — | `res.intent` | `intended` (in `state.intents`) |
| `intended` | `res.created` (same type + name) | `owned`, intent resolved |
| `intended` | `res.released{note:"never created"}` | gone, intent resolved |
| — | `res.adopted` | `adopted` (never released; detaches on `ws.cleaned`) |
| `owned` | `res.release_checked` (per check) | `owned`, `last_checks` updated |
| `owned` | `res.released` | `released` |

Ownership (spec §8) = `owned` state **and**, for tabs/spaces, the
`coord.managed=1` label on the terminal at check time. Anything else is the
person's.

### 7.4 Crash safety

| Operation | Order | Crash between steps → reconcile resolution |
|---|---|---|
| Rung 4 | tx1: `task.placed`, `res.intent` worktree, `res.intent` space (id generated in tx1) → `git worktree add` → tx2: `res.created` worktree (+ `branch` with base sha when `-b`) → `hc run` → tx3: `res.created` space, `res.created` tab, `task.started` | intent older than 120 s: worktree path listed by `git worktree list` on the intent's branch → `res.created`; not listed → `res.released{note:"never created"}`. Space: a terminal labelled `herdr.space=<id>` + `coord.task` exists → `res.created`; else released. Terminal with `coord.task=<task>` exists → `res.created` tab + `task.started` (pid/created_ms from `hc get`, session discovered); else `task.closed{reason:"launch interrupted"}`. |
| Rung 1/2/3/5 | tx1: `task.placed`, `res.intent` tab (`name` = task id until known; `space`) [+ space intent for 5] → `hc run` → tx2 as above | same terminal-by-label rule |
| `hc run` fails | tx2: `task.closed{reason:"launch failed: <message>"}`, `res.released{note:"never created"}` for each intent | nothing left |
| Coordinator start | `res.intent` space (first `up` only) → `hc run` → `coord.session_started` | `up` finds a live `coord.role=coordinator` terminal via `hc ls --label` + `hc get` and records it instead of starting a second one |
| Release | per resource: checks → `res.release_checked` entries → action → `res.released` | action done but no `res.released`: next release/reconcile check finds the resource gone (terminal missing, `git worktree list` lacks path, branch missing, space absent from `herdr-app ls`) → `res.released{note:"gone"}` |
| Journal append | one `write` per line + fsync, under flock | a torn last line has no `\n`: readers skip it; the next append first truncates a trailing partial line (under the lock) |

## 8. Behaviour details per slice

### 8.1 Placement (`task start`)

Validation (tx1, under the lock): workstream exists and is `open`; rung 1–5;
`--reject` present and non-empty for exactly rungs `1..rung` (`ladder_reason_missing` / `ladder_reason_extra`); rung arguments per G3
(`rung_arguments`); rungs 4/5 only when the workstream has no tasks
(`follow_up_cannot_create_space`); rung 2 needs `ws.space` (`no_space`); rung 3
space must exist in `herdr-app ls` (read before tx1) and must not be the
coordinator space (`coordinator_space`); rung 1 needs a predecessor with a
session id (`no_session`) and `--role` equal to its role if given
(`role_mismatch`); role loads (`role_invalid`).

Then: task id = `<ws>-t<max n + 1>`; brief text read; `title_of`; first message
composed (`delegate_first`, or `follow_up` for a live rung-1 send) and saved as
`briefs/<task>.md`; `task.placed` and intents appended; rung 3 also appends
`res.adopted` space when the space is not owned by this workstream (`label` = its
`herdr-app ls` name).

cwd: rung 4 worktree path; rung 5 `--folder` (created with `create_dir_all`;
never deleted); rung 3 the space's `folder`; rung 2 `ws.cwd`; rung 1 resume:
predecessor's `cwd`.

Rung 1: predecessor terminal live, pid equal to its recorded pid and
`sendable()` → `hc label` (new task, ws) → `hc send` the follow-up →
tx: predecessor `task.closed{superseded by}`, `res.created` tab is not repeated
(fold moves the tab record to the new task on `task.started` with the same
terminal), `task.started{resumed:true}`, `task.sent{via:"send"}`; tab renamed.
Live but working → exit 4 `agent_not_ready`; blocked/error → exit 3
`agent_blocked`. Not live → resume: `role.resume` expanded with the
predecessor's session dir and id, new terminal in its space.

Trust (G5) is computed before expansion. Repo root for `{repo_root}` =
`ws.repo` (main checkout).

### 8.2 Delegate first message and footer (fixed text)

`delegate_first` = role body, blank line, the task brief, blank line, FOOTER:

```
---
Reporting (fixed by the coordinator system):

- When you are done, or cannot continue, report exactly once:
  {cli} coord report done --summary "<one line>"
  {cli} coord report blocked --summary "<what blocks you>"
  {cli} coord report question --summary "<your question>"
  Long results go into {result}; add --result {result} to the report.
- Use `{cli} coord report progress --summary "<milestone>"` only for milestones; it never interrupts anyone.
- If the person changes the scope with you directly, record it with `{cli} coord note "<what changed>"`.
- Do not start other agents, create spaces or worktrees, or close anything. Do not report with `hc send`.
- After reporting, stop and wait: a follow-up may arrive in this session.
```

`{cli}` = `Ctx::cli()`; `{result}` = `results/<task>.md` absolute. The
coordinator first message appends:

```
## Commands
`kit` is: {cli}
Read commands: kit coord board | inbox | log [--ws WS] [--task TASK] | explain <ws|task> | roles
(then the full `coord` verb list of §5.1 for the coordinator column)

## Startup read
{startup_read}
```
`startup_read` = open workstreams with summaries and tasks, undrained inbox
count and the last 20 waking entries, last 10 decisions, standing
instructions, open threads, open questions.

### 8.3 Nudge texts

- Coordinator: `inbox: <n> new` (spec §6).
- Delegate (`DELEGATE_NUDGE`): `Reminder from the coordinator system: your last turn ended without a report. Report now with {cli} coord report done|blocked|question --summary "<one line>" (see the end of your first message).`

### 8.4 Report and note

`report` requires a Delegate caller; task must not be `closed`. `--result`
must be an existing regular file (canonicalised path recorded). `progress`
appends `task.progress`; others `task.reported`. `note` appends `task.note`.

### 8.5 Release checks (S4)

Each check produces one `res.release_checked` entry (`resource`, `type`,
`name`, `check`, `ok`, `detail`).

| Resource | Checks (in order) | Action when all pass |
|---|---|---|
| Tab (delegate terminal) | `owned` (resource `owned` + label `coord.managed=1`); `idle` (terminal finished, or word `idle`/`done`); `result_recorded` (`reported_seq > last_activity_seq`); `session_recorded` (session id present; for session mode `none`: workstream closed); `nothing_running` (`foreground.pid == pid`, or finished) | `hc kill`, `hc rm`; if `herdr-app ls` still shows the tab with no terminals: `tab.close` |
| Space | `owned`; `no_person_terminal` (every terminal in the space per `herdr-app ls` has `coord.managed=1` per `hc get`); `tabs_released` (no unreleased owned tab in the space) | `space.close` (also succeeds when the space vanished after the last kill) |
| Worktree | `owned`; `ws_closed`; `clean` (porcelain empty); `reachable` (`rev-list --count … = 0`); `no_terminal_inside` (no live terminal in `hc ls --json` with `cwd` under the path, and no `foreground.cwd` under it for managed ones) | `git worktree remove` |
| Branch | `owned` (created with `-b`); `merged` (`is_ancestor(sha, delivery)`); `not_checked_out` (absent from `worktree list`) | `update-ref -d` with sha |
| Adopted | none | none; detaches |

`release_ws`: refuses `open` (`ws_open`); tabs first, then the space, the
worktree (only after its space is released or adopted), the branch (after the
worktree). All owned resources released → `ws.cleaned`. Otherwise
`ws.retained{reason_code: first failed check's name mapped: clean→uncommitted_files, reachable→unpushed_commits, merged→branch_not_merged, no_person_terminal→in_use_by_person, idle→agent_busy, result_recorded→no_result, session_recorded→no_session, nothing_running→program_running, tabs_released→tabs_open, no_terminal_inside→terminal_in_worktree, otherwise the check name; reason: one sentence; checks: the failed resource's checks}`.
`Mode::Recheck` follows G10.

`task close`: tx `task.closed`, then `release_task_tab`; a failing check leaves
the tab and records checks (no `ws.retained`; the workstream is still open).

### 8.6 Watcher (S6)

Process: `coord watch` runs inside its hc terminal (labels in §6).

1. `flock` `coord/watcher.lock` non-blocking; held → print `{"state":"already_running"}`, exit 0.
2. `release = active_release(root)`; append `watcher.started{release, terminal: $HC_TERMINAL_ID, pid}`; write the heartbeat.
3. Spawn `hc events --json --label coord.managed=1` (tokio `Command`, `kill_on_drop`, `process_group(0)`, `env_remove("HC_TERMINAL_ID")`), read lines with `BufReader::lines`.
4. Loop with `tokio::select!`:
   - line → `Core::on_line` (D8);
   - 1 s interval → `Core::tick`: `journal.read_state()`, `rules::evaluate`, execute actions (each `Send` re-checks `hc get` → `sendable()` immediately before sending, then appends its record; `Append` under the lock after re-checking the condition on the fresh state);
   - 5 s interval → heartbeat (`events_ok` = stream alive); compare `active_release` → on change D10;
   - `reconcile_secs` interval (and once at start) → `reconcile`;
   - child exit → append `reconcile.finding{code:"events_stream_ended"}` once per outage, respawn after 2 s; meanwhile ticks run on `hc get` refreshes from reconcile.
5. No signal handling (tokio `signal` feature absent): `hc kill` ends it; the lock is released by the OS; `coord down` writes `watcher.stopped{reason:"down"}` itself.

Rules (`evaluate`, pure; "settled" = `idle|done`; times from `status_ms`):

| # | Condition | Action |
|---|---|---|
| R1 | coordinator live, settled for ≥ `nudge_coordinator_secs`, `turn ≥ 1`, waking seqs > cursor exist and their max > `last_coordinator_nudge_upto` | `Send` `inbox: <n> new` + `nudge.sent{target:coordinator, upto_seq}` |
| R2 | task `running`, terminal live (pid matches), settled for ≥ `nudge_delegate_secs`, `turn ≥ 1`, no `task.reported` since `last_activity_seq`, no nudge since `last_activity_seq` | `Send` DELEGATE_NUDGE + `nudge.sent{target:task}` |
| R3 | task has a nudge since `last_activity_seq` at `t`, `now ≥ t + stall_secs`, no report since, not already `stalled` since the nudge | `Append task.stalled{silent_secs, nudge_seq}` |
| R4 | task `running`, `now ≥ started_ms + start_blocked_secs`, terminal `turn == 0` and word ≠ `working`, `start_blocked` unset | `Append task.start_blocked{after_secs}` |
| R5 | task terminal word `blocked`/`error` and `attention` differs; or word `working` and (`attention` or `start_blocked` set) | `Append task.attention{blocked|error|clear}` |
| R6 | task `placed|running|reported`, terminal missing, finished, or live with a different pid (replaced after core restart) | `Append task.dormant{reason}`; replaced and owned → `CloseLeftover`; no session id → also `reconcile.finding{code:"agent_gone_unresumable"}` |
| R7 | coordinator settled, cursor covers all waking seqs, no open question, `turn ≥ 1`, and (handoff requested since session start, or settled for ≥ `idle_restart_hours`) | `RestartCoordinator{reason: handoff|idle|user}` |
| R8 | config `autostart`, coordinator terminal missing/finished/replaced, no `coord.session_started` in the last 300 s | `Append coord.session_ended{reason:"terminal_gone"}` (if not yet), `StartCoordinator` |

Reconcile (every `reconcile_secs`): refresh facts for all managed terminals
(`hc ls --json --all --label coord.managed=1` + `hc get`); resolve intents
older than 120 s (§7.4); `herdr-app ls` (skip silently when unavailable):
terminals without `coord.managed` inside owned spaces of open workstreams →
`reconcile.finding{code:"person_terminal_in_owned_space"}` (once per space until it clears; spec §6 "mark the space retained: in use by person"), plus `ws.retained{reason_code:"in_use_by_person"}` when the workstream is already closed; the space's next release fails
`no_person_terminal`; rename owned spaces/tabs whose names differ from the
recorded labels; `git worktree list` per owned worktree: missing →
`res.released{note:"removed outside"}` + finding; retained workstreams →
`release_ws(Mode::Recheck)`; tasks with `session.id == null` →
`discover_session` → `task.session`.

Heartbeat file `coord/heartbeat.json` (atomic, every 5 s):
`{"pid":1234,"terminal":"t20","release":"<rev>|development","started_ms":…,"beat_ms":…,"events_ok":true}`.

### 8.7 Lifecycle (S7)

`up [--if-enabled]` (holds `up.lock`, 60 s deadline):
1. `--if-enabled` and (no config or `autostart == false`) → `{"state":"disabled"}`.
2. `paths.ensure()`; config created with defaults if absent, else `autostart = true` saved.
3. Coordinator space: from state, else `res.intent{type:"space", name: ids::space_id(), folder: coord/home, label:"coordinator"}`.
4. Watcher: heartbeat fresh and `release == active_release` and its terminal live → keep. Else kill + `hc rm` the recorded watcher terminal (if any, and if managed with `coord.role=watcher`), start it (§6), wait ≤ 10 s for a fresh heartbeat (else `degraded`). First start: tab lookup → `tab.rename` `watcher`; record `res.created` space (first time) and `res.created` tab for the watcher.
5. Coordinator: recorded terminal live with recorded pid → keep; recorded but gone → `coord.session_ended{reason:"terminal_gone"}`; a live terminal labelled `coord.role=coordinator` not recorded → adopt it into the record (`coord.session_started{reason:"recovered"}`); else `start_coordinator(reason:"up")`: session dir `sessions/coordinator-<n>`, brief `briefs/coordinator-<n>.md` from `coordinator_first`, `hc run`, tab rename `coordinator`, `tab.move` index 1, `terminal.pin`, discover session id, `coord.session_started`.
6. Release changed since the coordinator session's `release` → G12.
7. Setup items: when `~/.claude.json` exists and `.projects["<worktree_root>"].hasTrustDialogAccepted != true` → `{"code":"claude_worktree_trust","message":"Open claude once in <root> and accept the folder trust prompt, so delegates in owned worktrees do not stop at it."}` (spec §5; nothing written).
8. Return `{state, actions, setup, status}`.

`down`: `autostart = false`; coordinator: `coord.session_ended{reason:"down"}`, kill + rm; watcher: kill + rm, `watcher.stopped{reason:"down"}`. Delegates untouched (`--no-parent`).

`restart` (G11), `handoff` (records `coord.handoff_requested{by:"coordinator", note}`; the watcher acts via R7).

`restart_coordinator`: `coord.session_ended{reason}` → kill + rm → `start_coordinator` with the recorded `herdr.tab` → `tab.move`, `terminal.pin`.

`after_activation(root)`: if `<root>/coord/config.json` is absent → `{"state":"disabled"}` without spawning; else spawn `<root>/bin/herdr-kit --data-dir <root> coord up --if-enabled` (deadline 120 s), parse stdout JSON; nonzero exit or a clap error → `{"state":"unsupported"|"failed","message":…}`.

## 9. Manager, layout and mod integration (S8)

### 9.1 `src/manager.rs`

| Location | Change |
|---|---|
| `:19` `const MOD` | `const MODS: [&str; 2] = ["herdr-kit-usage", "herdr-kit-coord"]; const LEGACY_MOD: &str = "herdr-kit-usage";` |
| `:194` `name(rev)` | `fn name(base: &str, rev: &str) -> String { format!("{base}-{rev}") }` |
| `:200` `mod_path(home, rev)` | `fn mod_path(home: &Path, base: &str, rev: &str) -> PathBuf` |
| new | `fn release_mods(release_dir: &Path) -> Result<Vec<(String, PathBuf)>>`: `mods/` present → each subdir (must be in `MODS`, else error), sorted; else `mod/` present → `[(LEGACY_MOD, mod/)]`; else error `missing release mods`. |
| `:231-241` `verify_active` | loop `release_mods(&release(..))`: `tree(mod_path(home, base, rev)) == tree(dir)` |
| `:242-274` `recover` | same staging/rename block per mod |
| `:275-299` `activate` | existence/ownership checks per mod |
| `:322-352` `build` | for each `base` in `MODS` whose `src/mods/<base>` exists (usage required): `copy_tree` → `staged/mods/<base>`; `rewrite` every staged `.js`/`.json` replacing **every** present base with `name(base, rev)` (names are not substrings of each other); stamp `staged/mods/<base>/runtime.js` with its own `MOD_NAME`; layout must contain every present base and is rewritten for all; `herdr-app check` per mod. |
| `:368-389` `install`, `:409-442` `update` | when `activate` ran, set `"activated": true` in the returned JSON (rollback branch and apply/maintain work closure); otherwise `false`. |
| new | `pub fn active_release(data: &Path) -> Option<String>` (read `state.json` `current` without the lock; errors → `None`); `pub fn stable_binary(data: &Path) -> PathBuf` (= `bootstrap`). |
| tests (`:444-534`) | `fixture_release` gains a `mods/` variant; new tests: legacy `mod/` release verifies and activates; two-mod activation installs both generation dirs; rollback from a two-mod release to a legacy one leaves the coord generation dir on disk and the layout pointing at the legacy usage mod; `build` rewrite replaces both names in a fixture tree (unit-test `rewrite` directly). |

Migration: none of `State`, receipts or `managed_mods` change shape. Existing
installs keep their legacy release; the next `update apply` builds the
two-mod release and the atomic layout switch activates both generations.
Rollback to a legacy release restores the one-mod layout; the old coordinator
binary is gone, so the watcher's own release check (D10) execs a binary without
`coord`, which exits with a clap error and its terminal closes; `after_activation`
reports `unsupported`.

### 9.2 `src/main.rs` (S9)

- `mod coord;` (S0).
- Extract the data-root block (`:84-89`) into `fn data_root(arg: Option<PathBuf>) -> Result<PathBuf>`.
- In `main()`: `Command::Coord` is handled before the generic printer: `coord::run(&root, cmd).await` → `Output::Json(v)` printed like today, `Output::Text(s)` printed as-is; errors printed with `coord::error::exit_of` code and exit status.
- After `Install`/`Update` return a value with `activated == true`: `value["coord"] = coord::lifecycle::after_activation(&root)`.

### 9.3 `layouts/herdr-kit.json`

Replace the `status` block (`:170-182`) with the row of board-design §4.1.1:
`{"id":"status","type":"row","size":{"px":28},"resizable":false,"children":[{"id":"status-usage","type":"block","content":{"mod":"herdr-kit-usage","props":{"view":"statusbar"}}},{"id":"status-coord","type":"block","size":{"px":280},"min":96,"max":360,"content":{"mod":"herdr-kit-coord","props":{"view":"statusbar"}}}]}`.

### 9.4 `mods/herdr-kit-coord/` (S10)

Files and behaviour exactly as board-design §4.6 (`mod.json`, `runtime.js`,
`main.js`, `store.js`, `model.js`, `ui-common.js`, `ui-status.js`,
`ui-board.js`, `ui-timeline.js`, four icons) and `tests/coord.test.js`. The mod
calls only `coord status --json`, `coord up`, `coord board --json`,
`coord log --json …` and watches `status.journal_dir`. `runtime.js` in the
source tree: `BINARY='herdr-kit'`, `DATA_DIR=null`, `RELEASE='development'`,
`MOD_NAME='herdr-kit-coord'` (same shape as `mods/herdr-kit-usage/runtime.js`).

## 10. Slices

Dependency graph (arrows = needs the interface of):

```
S0 contract ─┬─> S1 journal+fold ──┬─> S3 views ──┐
             ├─> S2 roles+briefs ──┤              ├─> S7 lifecycle ─> S6 watcher ─> S9 wire-up+integration
             ├─> S4 git+release ───┼─> S5 placement┘
             ├─> S8 manager+layout (independent of coord code except D18 names)
             └─> S10 mod (needs only the JSON contract; can start immediately)
```
Parallel after S0: S1, S2, S4 (git part and checks against hand-built
`State`), S8, S10; S3 after S1; S5 after S1+S2 (+S4 `release_task_tab`
signature); S7 after S3+S5; S6 after S7 (rules can start after S0); S9 last.
Until S9 lands, `coord` code compiles via `mod coord;` with dead-code
warnings; no command is reachable, nothing is stubbed.

Each slice: run `cargo test --locked` for its module tests; S10 runs
`node --test tests/coord.test.js`.

### S0 Contract

- Files: `src/coord/{mod.rs (decls + Ctx + Output), paths.rs, ids.rs, kinds.rs, state.rs, config.rs, error.rs, adapter.rs, caller.rs, cli.rs}`, `src/main.rs` (`mod coord;` only).
- Steps: write the types and signatures of §4.1–4.10 and the clap tree of §5.1; implement `paths`, `ids`, `config`, `error`, `adapter` (Proc + fake), `caller`, `Entry::typed`, `Actor` serde, `Kind::is_waking`.
- Non-goals: fold logic, handlers, dispatch.
- Acceptance: unit tests — every `Kind` variant round-trips through `Entry` JSON matching `examples/journal-sample.jsonl`; an unknown kind and a `v:2` line decode to `Unknown`; id regexes; `utc_month`; the captured `hc get --json` agent and the events snapshot terminals decode into `Term`, the captured `hc ls --json` decodes into an address list (fixtures copied from `docs/coordinator/examples/captures/` into `src/coord/testdata/`); `caller::resolve` for each class with `FakeHc`; clap `debug_assert` (`CoordCommand::command().debug_assert()`).

### S1 Journal and fold

- Files: `src/coord/journal.rs`, `src/coord/fold.rs`.
- Steps: §4.5–4.6; torn-line handling; month files; state cache with `fold_version`; `transact`.
- Acceptance: tests — 8 threads × 50 appends through separate `Journal` handles produce seqs 1..=400 exactly once and parseable lines; incremental load after every prefix equals a full fold; month rollover (entries with ts in two months) reads in order; a torn trailing line is ignored and repaired on next append; fold table tests for every row of §7.1–7.3 (including rung-1 handover and anomalies); `waking`/cursor bookkeeping.

### S2 Roles and briefs

- Files: `src/coord/role.rs`, `src/coord/brief.rs`, `src/coord/roles/*.md` (copied verbatim from `docs/coordinator/roles/`).
- Acceptance: all seven built-ins parse; `split_words` cases (quotes, escapes, codex trust argument → exactly two items `-c`, `projects={ "{repo_root}" = { trust_level = "trusted" } }`); `expand` fills placeholders inside arguments, inserts trust after `argv[0]`, errors on unknown/empty placeholders; override file wins; `model_of`; footer contains `cli` and result path; `for_argv` cap; `title_of`.

### S3 Views

- Files: `src/coord/views.rs`.
- Acceptance: tests feed `examples/journal-sample.jsonl`-style entries and compare `board`/`status counts` JSON to board-design §4.4 fields (every key present with the right type); `counts.needs_you == attention.len()` across a table of states; `log` paging (`--before-seq`, `--after-seq`, `--limit`, `--ws`, `--task`, `--since`) and `has_more`; `inbox` excludes coordinator-authored entries; `startup_read` ≤ 32 KiB.

### S4 Git and release

- Files: `src/coord/git.rs`, `src/coord/release.rs`.
- Acceptance: temp-repo tests — `slug`/`worktree_path` match Herdr's convention (`feat/x y` → `feat-x-y`); clean vs modified vs untracked vs ignored-only; reachable via delivery branch, via a pushed remote branch (bare remote in temp dir), unreachable local commit; merged vs unmerged branch; `delete_branch` refuses when the branch moved; worktree removal; check tables for tab/space with `FakeHc`/`FakeApp` (person terminal in space, busy agent, missing result, session mode `none`); retained reason mapping; G10 change-only journaling.

### S5 Placement and handlers

- Files: `src/coord/launch.rs`, `src/coord/place.rs`.
- Acceptance: with `FakeHc`/`FakeApp` and temp repos — the exact `RunSpec` for rungs 1–5 (labels, name, cwd, argv) and `HC_TERMINAL_ID` absent (Proc adapter test with an env-dumping script); ladder refusals (exit 3 codes); rung-4 journal order intent → created → started; launch failure closes the task and releases intents; `task send` refuses `turn 0` (exit 4) and `blocked` (exit 3), resumes when dormant; rung-1 live handover relabels and closes the predecessor; trust args only for a repo with a person's space; `report` by non-delegate refused; `ws close` with running tasks refused.

### S6 Watcher

- Files: `src/coord/rules.rs`, `src/coord/watch.rs`, `src/coord/reconcile.rs`.
- Acceptance: `evaluate` table tests for R1–R8 including "already nudged", "progress does not count as report", `done` treated as settled, turn 0 never nudged; `parse_line` on the captured snapshot and title lines; `Core` driven with scripted lines + `FakeHc` + manual clock (no tokio time); reconcile resolves each §7.4 intent case; lock prevents a second `run` (second call returns `already_running`).

### S7 Lifecycle

- Files: `src/coord/lifecycle.rs`.
- Acceptance: with fakes — first `up` creates config, space intent, watcher then coordinator (`hc run` argv per §6), renames/pin/move calls; second `up` makes no `hc run`; stale heartbeat → watcher replaced in the same tab (`herdr.tab` label); release mismatch → replaced + G12 entries; `--if-enabled` no-op without config; `status` JSON for running/degraded/not_running with each `degraded[].code`; `down` stops both and sets `autostart:false`; restart refused-to-defer path records `coord.handoff_requested`; claude trust setup item from a temp `HOME/.claude.json`.

### S8 Manager and layout

- Files: `src/manager.rs`, `layouts/herdr-kit.json`.
- Steps: §9.1, §9.3.
- Acceptance: new manager tests listed in §9.1 pass; existing ones unchanged; `layouts/herdr-kit.json` parses and references both mods (test reads it); `herdr-app check` of both staged mods runs in `build` (skipped when `herdr-app` is absent, as today).

### S9 Wire-up, integration tests, docs

- Files: `src/coord/mod.rs` (`run`, permission table §5.2), `src/main.rs` (§9.2), `tests/coord_cli.rs`, `tests/fake/cli.sh` (from `examples/fake-cli.sh`), `README.md`, `RECIPE.md`, `CHANGELOG.md`.
- Acceptance: integration scenarios of §12.2 pass under `cargo test --locked`; README gains a "Coordinator" section (commands, data layout, autostart, safety rules); CHANGELOG "Unreleased" entry.

### S10 Coordinator mod

- Files (one owner): `mods/herdr-kit-coord/{mod.json, main.js, store.js, model.js, ui-common.js, ui-status.js, ui-board.js, ui-timeline.js, runtime.js, coord-*.svg}`, `tests/coord.test.js`.
- Steps and acceptance: board-design §4.6 required tests 1–13, §9 scenarios; `herdr-app check mods/herdr-kit-coord` in an isolated app home.

## 11. Risks and verification

| Risk | Mitigation | Verified by |
|---|---|---|
| Closing something the person owns | two-evidence ownership; per-check journal; `space.close` only after `no_person_terminal`; no `--force` anywhere; CAS branch delete | S4 tests; S9 release scenarios |
| Typing into a start-up dialog | `sendable()` (`turn ≥ 1 && reported && settled`) re-checked immediately before every send; first message only in argv | S5/S6 tests |
| Duplicate coordinators/watchers | `up.lock`, `watcher.lock`, adopt live labelled terminals; hc name uniqueness | S7 tests |
| Journal corruption / lost seq | flock, single-line writes, torn-line repair, contiguous-seq test | S1 tests |
| Stale artifacts across releases | per-generation mod dirs; watcher self-exec on release change; agents call the stable path | S8 tests; D10 manual check below |
| Journal growth from rechecks | G10 | S4 test |
| hc output shapes drift | `Term` ignores unknown fields; events re-read with `hc get` (D8) | S0 fixture tests from captures |

Not checked against a live tool (verify in a disposable Herdr fixture before
relying on it; the plan does not depend on them for correctness unless noted):

1. CLI error text/exit of `hc get` for an unknown terminal (adapter maps `not_found` in output to `None`).
2. `hc run --json --bg` printing the `RunResult` object (schema + `--json` help say yes; not run).
3. `data` payloads of event types other than `title` (design avoids them, D8).
4. `herdr-app ls` behaviour and error shape with no open window (treated as "unavailable": renames deferred to reconcile).
5. `tab.move`, `tab.rename`, `terminal.pin` not moving the person's view (spec verified renames and closes by id only).
6. Recreating a tab by reusing its `herdr.tab` id after its last terminal ended [doc] (D15).
7. Killing the last terminal of a tab closing that tab (spaces verified in the skill).
8. `omp --model @plan` (role reference on the CLI) [decision memory].
9. `codex -c … resume <id> "<msg>"` (global `-c` before the subcommand).
10. Reboot / core-restart path (spec §12 says the same).
11. `~/.claude.json` key layout for the trust check (from the skill, not read here).

Manual native check after S9 (disposable app home + core, never the person's
window, per `README.md` "Verification"): `coord up` creates the `coordinator`
space with two tabs without moving the view (`herdr-app status` before/after);
`task start --rung 4` with a cheap model (`gpt-6-luna`) creates the worktree
space named `<repo> · <title>`; the delegate reports; `task close`, `ws close`,
`release` remove tab, space and worktree; `update apply` of a new committed
revision replaces the watcher in the same tab (heartbeat `release` changes).

## 12. Test plan

### 12.1 Unit tests (`#[cfg(test)]` in each module)

Listed per slice in §10. Shared fixtures in `src/coord/testdata/`: the captured
`hc get` agent JSON, `hc ls --json`, the events snapshot line and one title
line, `herdr-app ls`, `git worktree list --porcelain`, and
`journal-sample.jsonl`. Git tests create temp repos with
`git init -b main`, set `user.name`/`user.email` via `-c`, and never touch the
real `$HOME` (set `HOME` to a temp dir for the child commands).

### 12.2 Integration tests (`tests/coord_cli.rs`, binary-level)

Harness: temp data dir (`--data-dir`), temp `HOME`, `FAKE_DIR`, copies of
`tests/fake/cli.sh` as `hc` and `herdr-app` with mode 0755,
`HERDR_KIT_HC`/`HERDR_KIT_HERDR_APP` pointing at them,
`env!("CARGO_BIN_EXE_herdr-kit")`. Helpers: `queue(name, key, stdout, code)`,
`calls(name) -> Vec<Vec<String>>`, `journal() -> Vec<Value>`.

| # | Scenario | Asserts |
|---|---|---|
| 1 | `ws open` (person, no `HC_TERMINAL_ID`) | stdout `{"ws":…}`; one `ws.opened` with actor `cli` |
| 2 | `task start --rung 3 --space <id>` with queued `herdr-app ls` (space present), `hc run` (RunResult JSON), `herdr-app ls` (terminal in a tab), `do.tab.rename`, `hc get` | recorded `hc` argv equals §6 row exactly; `HC_TERMINAL_ID=<unset>` in every `hc` call even when the test sets it for the CLI; journal order `task.placed, res.adopted, res.intent, res.created(tab), task.started` |
| 3 | `task start --rung 4` on a temp git repo with `worktree_root` in config | worktree exists at `<root>/<repo>/<slug>`; space id in `res.intent` equals the `herdr.space` label passed to `hc run`; `do.space.rename` name `<repo> · <title>` |
| 4 | ladder refusals | exit 3, `ladder_reason_missing`; no `hc` call; no journal entry |
| 5 | `report done` with `HC_TERMINAL_ID=t9` and queued `hc get` with delegate labels | `task.reported`, actor `task:<id>`; same with non-delegate labels → exit 3 `not_a_delegate` |
| 6 | `task send` with `hc get` turn 0 | exit 4 `agent_not_ready`; no `send` call |
| 7 | `task close` + `ws close` + `release` on the rung-4 workstream (clean worktree, branch merged) | `hc kill`, `hc rm`, `do.space.close`; worktree and branch gone; `ws.cleaned` |
| 8 | `release` with an untracked file in the worktree | exit 0, `retained.reason_code == "uncommitted_files"`; worktree still present |
| 9 | `up` twice with queued responses | first: two `hc run` (watcher, coordinator) with §6 labels; second: none |
| 10 | `status --json`, `board --json`, `log --json --limit 2 --before-seq 5` | keys/types of board-design §4.4; `needs_you == attention.len()` |
| 11 | `explain <task> --json`, `inbox`, `inbox ack` | shapes of §5.3; cursor advances |
| 12 | concurrent CLI appends (8 processes × 20 `decide`) | seqs contiguous and unique |

The watcher is tested in-process (S6), not through the binary.

### 12.3 JS

`node --test tests/*.test.js` runs `tests/usage.test.js` and
`tests/coord.test.js` (board-design §4.6 cases).

## 13. Open questions

| # | Question | Options | Recommendation (used in this plan) |
|---|---|---|---|
| O1 | Built-in models | omp role refs (`@plan/@smol/@task/@slow`) / fixed model ids / all `gpt-6-luna` | role refs (G6); the person overrides with `roles/<name>.md` |
| O2 | Does `coord down` stop the coordinator agent or only the watcher? | both / watcher only | both; delegates keep running |
| O3 | Tasks closed without a report keep their tab until the person acts | strict / `task close --abandon` that skips `result_recorded` | strict for v1 (spec: destructive steps gated mechanically); revisit with journal data |
| O4 | `restart --force` for a stuck coordinator | include / omit | include, person only (G11) |

## 14. Verified after planning (live probes, 2026-11-13)

These settle items of the "not checked" list in section 11 and are binding:

- Item 1: `hc get --json <unknown>` prints `{"error":{"hint":"see `hc ls --all`","kind":"not_found","message":"no terminal \"t9zz\""}}` and exits 3. Map `kind == "not_found"` to `None`.
- Item 2: `hc run --json --bg …` prints one JSON object with `address` (used throughout the probes).
- Item 5: `tab.move`, `tab.rename` and `terminal.pin` with a JSON object argument return `{"ok":true}` and do not move the person's view.
- Item 6: starting a terminal with the `herdr.tab` id of a tab that has closed recreates the tab with that id, but its name and position are lost (it comes back unnamed, at the end). After every restart in the same tab the tab must be renamed again, and the coordinator tab moved to index 1 again.
- Item 7: killing and removing the last terminal of a tab closes that tab (and the space when it was the last tab). A following `tab.close` / `space.close` may then fail with a "no tab"/"no space" error, which counts as success.
- Item 8: `omp --model @smol` and `omp --model @task` work on the command line.
- A terminal started with an explicit `herdr.tab` label carries that label in `hc get` (`labels["herdr.tab"]`); one started without it does not, so the tab id must still be looked up through `herdr-app ls`.

## 15. Insight records (slice S11; spec section 10 "Insight records")

Added after the plan was written; binding for S6, S9 and S11.

Envelope: `Entry` gains two optional fields, serialised only when present and
ignored by older readers: `op: Option<String>` (one id per CLI invocation,
`ids::random` 8 base32 chars, created in `Ctx` and stamped by the journal
append path on every entry of that process; the watcher uses one `op` per
evaluated action batch) and `cause: Option<u64>` (global flag `--cause <seq>`
on coordinator verbs that write entries).

Kinds and fields (all additive, old entries stay readable):

| Where | Addition |
|---|---|
| `ws.opened` | `request: Option<String>` (`ws open --request <text>`) |
| `ws.closed` | `outcome: Option<"delivered"\|"partial"\|"abandoned">` (`ws close --outcome`, default `delivered`) |
| `task.placed` | `request: Option<String>` (`task start --request`), `follows: Option<{task, relation}>` with relation `follow_up\|rework\|escalation` (`--follows <task> --relation <r>`; rung 1 defaults to `follow_up` of `--session-of`), `role_hash: String` (sha256 of the resolved role file text, first 12 hex), `brief_bytes: u64` |
| `task.closed` | `outcome: Option<"accepted"\|"rejected"\|"superseded"\|"abandoned">` (`task close --outcome`, required for the coordinator) |
| `coord.session_started` | `role_hash`, `release` |
| new `task.turn` | `{turn: u64, origin: "start"\|"send"\|"nudge"\|"person", working_ms: Option<u64>}`; written by the watcher when a delegate's `turn` counter advances; `working_ms` from the last `working` status time the watcher saw for that terminal (`None` after a watcher restart mid-turn) |
| new `coord.turn` | same shape, origin `person` or `inbox`, for the coordinator terminal |
| new `rating` | `{target: ws or task id, value: "good"\|"bad", note: Option<String>}`; verb `coord rate <id> good\|bad [--note]`, person only |
| new `config.changed` | `{key, old, new}`; written by `coord config set <key> <value>` (person only; the only supported way to change settings besides editing the file) and by `up`/`down` when they flip `autostart` |

Origin rule (pure, in `rules.rs`): for a terminal, the system messages
(`task.started`, `task.sent`, `nudge.sent`; for the coordinator
`coord.session_started` and inbox `nudge.sent`) with seq greater than the seq
of that terminal's last `*.turn` entry are "unused"; a new turn takes the
origin of the oldest unused one, else `person`. None of the turn kinds wake
the coordinator.

Fold: per task `turns`, `person_turns`, `working_ms` (sum), `outcome`,
`follows`, `rating`; per workstream `outcome`, `rating`; coordinator totals.
These are for `stats`; the board JSON is unchanged.

`coord stats --json [--since <30d|12h|…>]` (anyone may call): one object with
`schema_version`, `generated_at_ms`, `since_ms`, `by_role[]`, `by_rung[]`
(tasks, median and p90 `placed_to_first_report_ms`, mean turns, mean
person_turns, nudges, stalls, start_blocked, outcomes map, rework_rate,
escalation_rate), `by_role_hash[]` and `by_release[]` (same figures, coarser),
`workstreams[]` (id, title, lead_time_ms, tasks, retained_ms, outcome,
rating), and `coordinator` (turns by origin, working_ms, median
`wake_to_drain_ms`, median `question_to_answer_ms`, corrections by kind,
restarts by reason, sessions).

Ownership: S6 writes `task.turn`/`coord.turn` (rules + watch). S11 owns the
additive changes to `kinds.rs`, `state.rs`, `fold.rs`, `cli.rs`, `journal.rs`
(`op`/`cause` stamping), the flag handling in `place.rs` after S5 lands, the
`rate`, `config set` and `stats` handlers (new file `src/coord/stats.rs`), and
the role texts that tell the coordinator to pass `--request`, `--cause`,
`--follows` and `--outcome`.

## 16. Decisions after review (binding, override earlier sections)

- R8 (section 8.6) is changed: the watcher never starts a coordinator on its own. When the coordinator terminal is missing, finished or replaced it appends `coord.session_ended{reason:"terminal_gone"}` once and nothing else; `status` then reports `degraded` with `coordinator_terminal_missing`, and only `coord up` (status icon click, app start with autostart, or the person) starts a new one. A person who closes the coordinator tab must not see it come back by itself. The only watcher-initiated start is the restart half of R7 (handoff or idle restart), which replaces a session that is running.
- Locking rule: sends, handovers, releases and lifecycle stop/restart acquire a per-terminal operation lock first, then briefly acquire the journal lock to validate and record intent. Never hold the global journal lock while waiting for `hc send` (a person's draft may hold delivery for 60 seconds). Watcher sends use asynchronous subprocesses so heartbeats and delegate reports continue; only successful sends produce `nudge.sent` and advance the notification cursor. At most one delivery is pending per terminal, with bounded retry after failure.
- Start-up dialogs: nothing is ever sent to a terminal with `turn == 0` or `reported == false`, including the coordinator; `Term::sendable()` is the single guard.
- `PostLaunchError` (launch.rs): a failure after a successful `hc run` (tab lookup, rename) keeps the journal intents; reconcile resolves them by labels (section 7.4) and renames by id.
- `views::startup_read(state: &State, waking: &[Entry], now_ms: u64)`: the caller passes the pending waking entries read from the journal.
- `hc run --json --bg` returns a compact address record, not a full terminal. The adapter parses its address and inspects it with `hc get`; failed inspection after creation preserves the address in a typed error. Delegate identity is registered before naming and session discovery, so a very short task may report immediately.
- Reconcile preserves global coordinator space/tab records during lifecycle transitions; a transient absent app snapshot is not proof they were deleted.
- Replaced delegate terminals are retained rather than automatically killed: after core recovery the person may have started an editor in the restored shell.
