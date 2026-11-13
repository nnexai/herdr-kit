# Coordinator: spec

Status: final for the first version. Facts marked *(verified)* were observed on
a live hc / herdr-app install on 2026-11-13; everything else is a decision.

## 1. Purpose

One agent, the **coordinator**, is the person's single point of contact for
parallel work. It decides where work goes (an existing agent, a new tab, a new
worktree, a new project), hands it to **delegates**, tracks it, and cleans up.
It does no project work itself.

Goals:

- The coordinator's session is disposable; nothing important lives only in it.
- Work-agnostic: the system knows tasks, placements and resources, not kinds of work.
- Nothing the person created is ever closed or removed.
- Everything that happens is recorded, readable by the person and by agents.
- Harness-agnostic; omp is the first supported harness.

Non-goals for the first version: remote machines, several coordinators,
quota-aware scheduling.

## 2. Concepts

| Term | Meaning |
|---|---|
| Workstream | A line of work with one delivery target. Owns resources. |
| Task | One brief given to one agent session inside a workstream. Owns nothing except possibly a tab. |
| Resource | A space, worktree, branch or tab. Either `owned` (created by this system) or `adopted` (the person's). |
| Journal | Append-only record of everything. The source of truth. |
| Watcher | A small process that turns hc status changes and timeouts into journal events and nudges. |
| Scout | A read-only subagent the coordinator uses to gather context for a placement decision. |

A follow-up is a new task in an existing workstream, so it cannot create a
space. Only opening a workstream can create a space or worktree.

### States

- Workstream: `open → closed → cleaned`, or `closed (retained: <reason>)`.
- Task: `placed → running → reported(done | blocked | question) → closed`.
  A task whose agent is gone but whose session is recorded is `dormant`, not an error.

## 3. Storage

Location: `<data-dir>/coord/` (same data dir as the rest of herdr-kit).

```
coord/
  journal/2026-11.jsonl   append-only, one JSON object per line, one file per month
  state.json              derived by folding the journal; a cache, safe to delete
  lock                    flock for appends and state rebuilds
  briefs/<task>.md        the exact brief each delegate received
  results/<task>.md       long results written by delegates
  sessions/<task>/        harness session storage, where the harness allows it
```

The journal is the only thing written by hand-off points; `state.json` is
rebuilt from it. Journal files are never deleted automatically.

Every entry has `seq`, `ts`, `actor` (`coordinator`, `task:<id>`, `watcher`,
`user`, `cli`) and `kind`. Kinds:

| Group | Kinds |
|---|---|
| Workstream | `ws.opened` (title, repo, delivery target), `ws.closed`, `ws.cleaned`, `ws.retained` |
| Task | `task.placed` (rung, rejected rungs with reasons, role, model, brief path), `task.started` (terminal, tab, session ref), `task.reported`, `task.progress`, `task.closed`, `task.dormant` |
| Resource | `res.intent`, `res.created`, `res.adopted`, `res.release_checked` (each check and its result), `res.released` |
| Watcher | `nudge.sent`, `task.stalled`, `reconcile.finding`, `watcher.started`, `watcher.stopped` |
| Coordinator | `coord.session_started`, `coord.session_ended` (reason), `inbox.drained` (cursor), `decision` (what, why, tier), `question.asked`, `question.answered`, `scout.result` |
| Person | `instruction.set` (standing instruction), `thread.opened`, `thread.closed`, `correction` (what the person overrode and why) |

`res.intent` is written before a resource is created and `res.created` after,
so a crash in between leaves a recorded intent that reconcile can resolve.

`correction` is the main signal for tuning: it marks each time the person
overrode a placement, a model choice or a cleanup.

## 4. Command surface

All under the one existing tool: `herdr-kit coord <verb>`. The agents never
compose `hc`, `herdr-app` or `git worktree` commands for structure; they
decide, and the CLI executes and records in one step.

| Caller | Verbs |
|---|---|
| Person / mod | `up`, `down`, `status`, `restart`, `board --json`, `log [--since] [--ws] [--task]`, `explain <task\|ws>` |
| Coordinator | `inbox` (events after the cursor), `inbox ack <seq>`, `ws open\|close\|list`, `task start`, `task send`, `task close`, `release <ws>`, `decide`, `ask`, `instruct`, `thread`, `handoff` |
| Delegate | `report done\|blocked\|question\|progress [--result <file>]`, `note` |
| Watcher | `watch` |

Caller identity comes from `$HC_TERMINAL_ID` and that terminal's labels
(`coord.role`, `coord.task`). A delegate can therefore write only to its own
task, and needs no ids in its brief.

## 5. Placement

`task start` takes the decision as arguments and refuses a higher rung without
a recorded reason for each lower one.

1. Same session, live or resumed.
2. Same space, new tab and agent.
3. An existing space of another workstream or the main checkout, for work that needs no isolation.
4. New worktree, only for parallel conflicting edits in one repo.
5. New project.

Before deciding, the coordinator may run scouts. A scout is read-only, answers
"which candidate fits, with what evidence", and its answer is journaled as
`scout.result`. If a scout cannot decide, the coordinator asks the person.

`state.json` carries a project registry (repo path, one-line purpose) and a
current summary per workstream, so most follow-ups resolve without a scout.

### Mechanics *(verified unless noted)*

- Terminals are started with `hc run --bg --no-parent --keep --label herdr.space=<id> --label coord.managed=1 --label coord.task=<id>`.
  This does not move the person's view, also with a real omp agent. `herdr-app do space.new` and `tab.new` do, and are not used for background work.
- The CLI calls `hc` with `HC_TERMINAL_ID` unset. Otherwise hc wraps messages in `<hc from="…" reach="hc send … when done or stuck">`, which tells the delegate to report by typing into the sender instead of through `coord report`.
- The delegate's own shell has `HC_TERMINAL_ID` set to its address, which is what `coord report` uses to identify the caller.
- Reusing the id of a space that was closed recreates that space.
- A new space's id is generated before creation (`sp` + 32 hex) and written in `res.intent` first.
- Spaces and tabs are renamed by id afterwards. An owned worktree space is named `<repo> · <workstream title>`; a tab is named after its task.
- The tab id is looked up through `herdr-app ls` by terminal address.
- Worktrees are created with `git worktree add` under `~/.herdr/worktrees/<repo>/<branch-slug>`, the path convention of Herdr's shipped worktree dialog, so that dialog lists them too.
- `--no-parent` means a coordinator restart does not end delegates.

### Roles and models

The coordinator picks a **role**, never a model. A role is a file in
`<data-dir>/coord/roles/<name>.md`: front matter with the harness, the start
command and the resume command, each with placeholders (`{session_dir}`,
`{session_id}`, `{brief}`), and a body with standing instructions.

hc's own templates were tested and are not used: their command is fixed text,
so a per-task session folder cannot be passed, and they always add the
`<hc from … reach …>` wrapper described above.

Defaults are built into herdr-kit. A file of the same name in `roles/`
overrides the built-in one and is never written by updates; `coord roles
--defaults` prints the current built-ins for comparison.

| Role | Used for |
|---|---|
| `coord-main` | The coordinator |
| `coord-light` | Mechanical, fully specified tasks |
| `coord-default` | Clear goal, known approach |
| `coord-deep` | Open design, unclear cause, high cost of error |

Choice is by how open the problem is. A resumed session keeps its model, so
escalation after `blocked` or failed acceptance is a new session on a stronger
role in the same space, given the previous result path.

### Harness contract

| Capability | Required | If missing |
|---|---|---|
| hc can start it and send it text | Yes | Not usable |
| Reports working / idle / blocked to hc | Yes | Not usable |
| Can run a shell command | Yes | Not usable |
| Session can be located and resumed | No | Its tab stays open until the workstream closes |
| Model selectable at start | No | Runs on its default |

The first message is delivered either as the harness's start argument, when
the role's start command contains `{brief}`, or by waiting for the agent to
be ready and sending it.

omp *(verified with `gpt-6-luna`)*:

- Start: `omp --model <m> --session-dir <coord>/sessions/<task> "<first message>"`. The message is delivered, and hc reports `working` then `done`.
- The session folder then holds one `<timestamp>_<id>.jsonl`; the id is read from that file name and journaled in `task.started`.
- Resume: `omp --session-dir <same> --resume <id> "<message>"`. The earlier conversation is available and the model is unchanged without passing `--model`.

Other harnesses *(verified 2026-11-13; codex 0.166/0.168, Claude Code 2.1.322, Copilot CLI 1.0.103)*:

| | omp | codex | claude | copilot |
|---|---|---|---|---|
| Start with first message | positional | positional | positional | `-i "<msg>"` |
| Model | `--model` | `-m` | `--model` | `--model` |
| Status reported to hc | yes | yes | yes | yes in 2 of 3 runs; one run reported nothing at all |
| Session id | read from the file in `--session-dir` | read from `session_meta` in the newest `~/.codex/sessions/**/rollout-*.jsonl` whose `cwd` matches | chosen up front with `--session-id <uuid>` | chosen up front with `--session-id <uuid>` |
| Resume | `--resume <id> "<msg>"` | `codex resume <id> "<msg>"` | `--resume <id> "<msg>"` | `--resume <id> -i "<msg>"` |
| History and model kept on resume | yes | yes | yes | yes |
| `HC_TERMINAL_ID` in the agent's shell | yes | yes | yes | not shown: the shell call was denied by a local pre-tool hook |
| Start-up dialogs seen | none | update offer | folder trust | folder trust, on every start |

### Start-up dialogs

A harness can stop at a dialog before its first turn: folder trust (claude,
copilot) or an update offer (codex). Owned worktrees are new folders, so this
is the normal case there, not an exception. During such a dialog hc showed no
status at all (claude, codex) or `idle` (copilot), never `blocked`, and
`hc send` typed into it: in the probe a sent line accepted codex's update
offer and codex updated itself.

Rules that follow:

- The first message always goes in the start command, never as a send after launch.
- Nothing is ever sent to a terminal until it has completed its first turn (`turn` ≥ 1). This applies to nudges and to `task send`.
- A task whose terminal has not reached `working` within 30 s of launch gets `task.start_blocked`. It shows on the board as needing the person. The system does not answer dialogs.
- Role files may carry harness flags that avoid dialogs where the harness offers them.

### Folder trust for owned worktrees *(verified)*

Tested with a main repo in one place and its worktree in another, as owned
worktrees are laid out.

| Harness | New worktree asks? | What removes the question |
|---|---|---|
| omp | No | Nothing needed |
| codex | Yes, in any git repo not yet trusted. Trust of a parent folder is not inherited. | Trust belongs to the repository root, so a worktree of a trusted repo does not ask. Per launch, without saving anything: `-c 'projects={ "<repo root>" = { trust_level = "trusted" } }'` |
| claude | Yes. Being a worktree of a trusted repo does not help. | Trust of a parent folder is inherited: trusting the worktree root once covers every worktree below it |
| copilot | Yes | A remembered folder covers its subfolders, and also a worktree elsewhere whose main repo is inside a remembered folder. `COPILOT_ALLOW_ALL=true` also skips the question, but approves all tools as well |

Consequences:

- omp needs nothing.
- codex: the role's start command passes the trust override for the repo root, but only when the workstream's repo is one the person already uses with the system; this saves nothing to codex's config.
- claude: `coord up` checks whether the worktree root (`~/.herdr/worktrees`) is trusted and, if not, reports it as a one-time setup step for the person. The system does not write claude's config.
- copilot: no step is needed when the main repo is already remembered; otherwise the task surfaces as `task.start_blocked`.
- Trust is a decision about running a repo's own hooks and settings, so the system never grants it for a repo the person has not worked in.

## 6. Events and the watcher

Data is pulled; wake-ups carry no payload.

- Delegates append `task.reported` through `coord report`.
- The coordinator keeps a cursor. `coord inbox` returns everything after it; `inbox ack` advances it.
- The watcher follows `hc events --label coord.managed=1`. Each line is `{params: {type, terminal, seq, data}}`; it uses `status`, `turn`, `created`, `finished`, `removed` and `labels`, and ignores `title`, which was about 90% of lines in the probe. A `turn` event marks the end of a turn.
- hc reports four resting or active states for an agent: `idle`, `working`, `blocked`, `done`. `done` means the turn ended while nobody was looking at the tab, which is the normal case for a delegate. "Settled" below means `idle` or `done`.
- Rules:

| Condition | Action |
|---|---|
| Coordinator settled for 10 s and waking events are undrained | Send `inbox: <n> new` to `coordinator` |
| Delegate task `running`, turn ended 60 s ago, no report | Nudge the delegate once to report |
| Still no report 5 min after the nudge | Append `task.stalled` |
| Delegate `blocked` or `error` | Append an event; never type into it |
| Terminal without `coord.managed` appears in an owned space | Mark the space `retained: in use by person` |
| Slow tick | Reconcile: journal against `hc ls`, `herdr-app ls`, `git worktree list` |

`progress` never causes a nudge. The three durations are starting values and
settings; the journal's nudge and stall counts are the basis for changing them.

Before sending, the watcher checks the state itself and sends without `--if`:
`hc send --if idle` refuses an agent in `done` *(verified)*. hc holds sends
while the person has an unfinished draft and refuses to type into a blocked
program *(documented by hc)*, so nudges do not collide with the person typing.

The watcher holds no state of its own; it can be killed and restarted freely.

### Running the watcher

herdr-kit has had no daemon so far. The watcher keeps to that spirit: it is a
subcommand of the same tool, run as an hc terminal, so hc keeps it alive, its
output is readable with `hc read`, and it is visible like any other terminal.

- `coord up` starts it if absent; a lock makes it single-instance.
- It writes a heartbeat. `coord status` reports a stale heartbeat, and the status bar shows it.
- After a reboot, hc may restore kept terminals as shells instead of re-running programs, so `coord up` repairs the coordinator and watcher. The tested private core restart preserved live programs and `coord up` correctly made no duplicates. See "Autostart" below.

### Autostart and the status icon

- Autostart is a setting, on by default once the coordinator has been enabled with a first `coord up`. `coord down` turns it off.
- With autostart on, the mod runs `coord up` at app start.
- The status bar carries a small coordinator icon with three states: running, not running, degraded (watcher heartbeat stale, or coordinator terminal missing). Clicking it when not running or degraded runs `coord up`; clicking it when running opens the board.
- The mod calls `coord up` for autostart and on a click, and otherwise only reads (`coord status`, `coord board`, `coord log`). It never restarts a coordinator session that is running.
- A coordinator tab the person closes stays closed. The watcher records that the session ended and the icon turns to degraded; only a click, the next app start, or `coord up` brings it back.

### Updates

herdr-kit already builds and activates new releases on its own. The
coordinator parts follow that without a separate mechanism:

- **Watcher:** its heartbeat records the release it runs from. `coord up` compares that with the active release and, if they differ, ends the old watcher and starts the new one in the same tab. `update apply` and `update rollback` run `coord up` after activation, so the watcher is replaced immediately. This is safe at any moment because the watcher holds no state.
- **CLI used by agents:** agents call the stable `<data-dir>/bin/herdr-kit` path, so they pick up the new release on their next call.
- **Roles and the coordinator brief:** built-in defaults change with the release; a running session keeps what it started with. The coordinator picks them up at its next restart; the update appends a `decision` entry so the reason for a requested handoff is visible.
- **Mod:** replaced by the existing generation mechanism.
- **Journal:** every entry carries a format version. A new release must read all older entries; it never rewrites them.

## 7. Coordinator session

### Start

`coord up` is idempotent and ensures, in order: the journal folder, the
coordinator space, the watcher, the coordinator agent.

- The coordinator lives in its own owned space named `coordinator`, in `<data-dir>/coord/home`.
- Its terminal is named `coordinator`, so `hc send coordinator "..."` always reaches it, and it is pinned to the top of the agents list.
- Tabs in that space: `coordinator` (the agent) and `watcher` (the watcher's output).
- Its first message is the `coord-main` role's instructions plus a startup read produced by `coord board`: open workstreams with summaries, undrained events, recent decisions, standing instructions, open threads.

### Restart

A fresh session replaces compaction; a compaction summary would be a second
memory that can disagree with the journal.

Restart happens only when the coordinator is idle, the inbox is drained and no
question to the person is pending, and one of:

- the coordinator asked for it with `coord handoff` (it notices its own context growing),
- it has been idle for a configured gap (default: overnight),
- the person ran `coord restart`.

The new session starts in the same tab. Anything the coordinator learns from
the person that must survive is written when it is learned: preferences as
`instruction.set`, unfinished discussions as `thread.opened`.

### Decision tiers

Each decision is journaled with a tier: `silent` (act and log), `digest`
(mention in the next summary), `ask` (queue a question and wait).

## 8. Release and cleanup

"Done" is the coordinator's judgment that the delivery target was reached; it
only moves the workstream to `closed`. Every destructive step is gated by a
mechanical check run by `coord release`, never by the model.

| Resource | Releasable when |
|---|---|
| Tab / agent | Owned, idle, result recorded, session ref recorded, nothing running |
| Space | Owned, all its tabs releasable, no terminal of the person in it |
| Worktree | Owned, workstream closed, no uncommitted or untracked non-ignored files, commits reachable from the delivery branch or a remote |
| Branch | Owned, merged into the delivery target |
| Anything adopted | Never; the workstream detaches |
| Non-git folder | Never deleted |

Ownership needs two pieces of evidence: a `res.created` entry and the
`coord.managed` label on the terminal. If either is missing the resource is
treated as the person's.

Agents are closed eagerly once their result is recorded; spaces and worktrees
live as long as their workstream. Removing a worktree is the point of no
return for resuming its sessions, hence the extra condition.

A failed check produces `ws.retained` with the reason and shows on the board.
Reconcile re-checks retained workstreams, so they clear on their own once the
blocking condition is gone.

Closing by id does not move the person's view *(verified)*.

## 9. What the person sees

| Surface | Content |
|---|---|
| `coordinator` space | The coordinator agent tab and the watcher tab |
| Ordinary spaces and tabs | Delegates appear as normal tabs; owned worktree spaces are recognisable by name |
| Status bar | A coordinator icon (running / not running / degraded; click to start or open the board) and a compact readout next to the usage readout: running, needing the person, retained |
| Board (click the readout) | Workstreams → tasks with state, placement reason, role, age; pending questions; retained reasons |
| Timeline (second view of the board) | The journal, newest first, filterable by workstream or task |

The board and timeline are one new native mod reading `coord board --json` and
`coord log --json`, refreshed through `watchFolder` on the journal folder.
Herdr's own `done` and `blocked` agent states keep working as attention signals.

App commands that move the view (`agent.focus`, `space.select`) are used only
when the person asks to be shown something.

## 10. Records for analysis

- The journal already holds every decision with its reasons, every nudge, stall, release check and correction.
- `briefs/` and `results/` hold what was asked and what came back; `task.started` holds the session reference for the full transcript.
- `coord log --json` and `coord explain` are the stable read interface; agents analysing the system use those rather than parsing files.
- Questions the records should answer: how often rung 4 or 5 was chosen and why; which placements were corrected; nudge and stall rates per role; time from report to drain; how often release was blocked and by which check; coordinator restarts and their reasons.

### Insight records

Session transcripts answer deep questions later, but only if the journal says
which transcript belongs to which decision and what the outcome was. These
records are cheap to write now and are what later analysis joins on.

| Record | Written by | What it enables |
|---|---|---|
| `op` on every entry: one short id per CLI invocation | CLI | Groups the entries one command produced; the unit of "one action" |
| `cause` on coordinator entries: the seq of the inbox entry or question that led to it (`--cause <seq>`, optional) | coordinator | Reaction chains: report → decision → follow-up; time from event to action |
| `request` in `ws.opened` and `task.placed`: the person's ask, verbatim (`--request`, optional) | coordinator | Comparing ask, brief and result: was the delegation faithful |
| `task.turn`: turn number, working time, and origin (`start`, `send`, `nudge`, `person`) | watcher | Rounds per task; steering by the person (turns nobody in the system sent); idle versus working time |
| `coord.turn`: same for the coordinator terminal (origin `person` or `inbox`) | watcher | Coordinator load and latency; how often the person had to talk to it |
| `follows` in `task.placed`: `{task, relation}` with relation `follow_up`, `rework`, `escalation` (`--follows <task> --relation …`) | coordinator | Rework and escalation rates per role and rung |
| `outcome` in `task.closed`: `accepted`, `rejected`, `superseded`, `abandoned`; in `ws.closed`: `delivered`, `partial`, `abandoned` | coordinator | A success label per task and workstream |
| `rating`: `good` or `bad` with an optional note, on a workstream or task (`coord rate`) | person | The person's own verdict, the strongest signal |
| `role_hash`, `brief_bytes` in `task.placed`; `role_hash` and `release` in `coord.session_started`; `config.changed` with old and new values | CLI | Before/after comparison when roles, briefs, settings or the release change |

A turn's origin is decided by the watcher from the journal alone: every
`task.started`, `task.sent` and `nudge.sent` for a terminal is used up by the
next turn of that terminal, which gets that origin; a turn with no unused
system message before it is `person`.

`coord stats --json [--since <duration>]` computes the standard figures from
these records so that tuning does not start with ad-hoc scripts:

- per role and per rung: tasks, time from placed to first report, turns, person-origin turns, nudges, stalls, start-blocked count, outcomes, rework and escalation rate;
- per workstream: lead time, tasks, retained time, outcome, rating;
- coordinator: turns by origin, time from waking event to drain, time from question asked to answered, corrections by kind, restarts by reason;
- breakdowns by `role_hash` and `release`, so a changed role file or release shows up as a separate row.

Not recorded in the journal: token counts and cost. They live in the harness
session files, which the session reference already points to.

## 11. Build order

1. Journal, state fold, `log`, `explain`, `board --json`.
2. `task start`, `task send`, `report`, `release`: placement mechanics and ownership.
3. Watcher.
4. Roles, coordinator brief, `up`, `restart`, `handoff`.
5. Status readout, board and timeline mod.

Each step is usable alone: after step 2 the person can drive the CLI by hand
and watch the journal, which is the cheapest way to find design errors.

## 12. Details fixed for the first version

### Identifiers

- Workstream: `ws-<6 base32>`; task: `<workstream>-t<n>`; both are stable and appear in labels, file names and the journal.
- Terminal labels: `coord.managed=1` on everything the system starts; `coord.role=coordinator|watcher|delegate`; `coord.ws=<id>` and `coord.task=<id>` on delegates.
- Terminal names: `coordinator`, `coord-watcher`, and the task id for delegates.

### Settings

`<data-dir>/coord/config.json`, created with defaults by the first `coord up`:

| Key | Default | Meaning |
|---|---|---|
| `autostart` | `true` after first `up` | The mod runs `coord up` at app start |
| `coordinator_role` | `coord-main` | Role used for the coordinator |
| `nudge_coordinator_secs` | 10 | Settled time before an inbox nudge |
| `nudge_delegate_secs` | 60 | Time after a turn end without a report |
| `stall_secs` | 300 | Time after the delegate nudge before `task.stalled` |
| `start_blocked_secs` | 30 | Time after launch without `working` |
| `reconcile_secs` | 120 | Slow tick |
| `idle_restart_hours` | 8 | Idle gap after which the coordinator session is replaced |
| `worktree_root` | `~/.herdr/worktrees` | Where owned worktrees go |

### Role file

```
---
harness: omp
start: omp --model gpt-6-luna --session-dir {session_dir} {brief}
resume: omp --session-dir {session_dir} --resume {session_id} {brief}
session: dir            # dir | preset | codex-rollout | none
---
Standing instructions for this role.
```

- `start` and `resume` are split into arguments like a shell would, then placeholders are replaced inside single arguments; nothing is passed through a shell.
- Placeholders: `{brief}` (the whole first message as one argument), `{session_dir}`, `{session_id}`, `{repo_root}`, `{cwd}`.
- `session` says how the session id is obtained: read from the session folder (omp), generated up front and passed as `{session_id}` (claude, copilot), found in codex's rollout files by folder, or not resumable.
- The four `coord-*` built-ins use omp. Three more built-ins, `codex-default`, `claude-default` and `copilot-default`, carry the verified start and resume commands for the other harnesses, including codex's per-launch trust override for `{repo_root}`. The models named in built-ins are starting values the person is expected to override.

### What a delegate is told

Every first message is: the role's standing instructions, the task brief, and a
fixed footer with the reporting contract:

- Report with `<stable path>/herdr-kit coord report done|blocked|question --summary "<one line>" [--result <file>]`; long results go into the given results file.
- Use `coord report progress` only for milestones; it never interrupts anyone.
- If the person changes the scope directly, record it with `coord note`.
- Do not start other agents, create spaces or worktrees, or close anything.

### What the coordinator is told

The `coord-main` instructions cover: it routes and decides and does no project
work; the placement ladder and that every rung above the lowest needs reasons;
scouts are read-only subagents of its own harness; every decision goes through
`coord decide` with a tier; preferences and unfinished discussions are written
with `coord instruct` and `coord thread` when they come up; on `inbox: <n> new`
it runs `coord inbox`, acts, and acknowledges; it asks for `coord handoff` when
its context grows; it never runs `hc`, `herdr-app` or `git worktree` for
structure.

### Coordinator restart mechanics

The old terminal is ended and a new one is started with the same
`herdr.space` and `herdr.tab` labels, so the tab stays in place. The session
is always fresh; the previous session reference stays in the journal.

### After a reboot or core restart

In the tested hc version, restarting the private core preserved live
terminals; `coord up` remained idempotent afterwards. A real machine reboot
can instead restore kept terminals as shells at the same addresses. The
watcher treats a delegate terminal whose recorded process identity changed
as ended: its task becomes `dormant`, with its session still resumable.
It retains the replacement terminal rather than killing it: the person may
already have started an editor there. `coord up` explicitly repairs the
watcher and coordinator. Shell restoration is covered by fake fixtures, not
by a real machine reboot.

### Testing hooks

- `HERDR_KIT_HC` and `HERDR_KIT_HERDR_APP` override the two executables, so tests run against recorded fakes.
- Everything is rooted in the data dir, so `--data-dir` gives full isolation.
- Automated tests never touch the person's running window or core.

## 13. Known limits

- Copilot's status reporting was missing in one of several runs; the person regards this as a known Copilot bug. A copilot task can then look permanently unstarted.
- Private core restart was verified; real machine reboot remains unobserved.
- The codex, claude and copilot roles are built from the probes in section 5 and are not exercised by automated tests.
