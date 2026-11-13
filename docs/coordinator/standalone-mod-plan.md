# Standalone coordinator mod and `kit-coord`: feasibility and cutover plan

Status: decision proposal only; no extraction or deployment authorized.
Evidence baseline: main at `8afa371dc185deeb27027c00b270b5a39971cf17` (2026-11-13).
Source references below refer to that baseline, not to future file locations.
“Observed” means source/history or the explicitly listed local commands; “Proposed”
means a design decision, not an implemented or verified capability.

## 1. Current shape and couplings

### Observed component map

| Piece | Current owner and evidence |
|---|---|
| CLI | One package/binary, no workspace or shared library crate (`Cargo.toml:1-22`). `src/main.rs:1-4,20-24,73-117` embeds coord, uses its `Output` type even for suite commands, and dispatches `herdr-kit coord …`. `src/coord/cli.rs:18-178` defines up/down/status/restart/handoff, board/log/explain/inbox/roles, workstreams/tasks/release, decisions/questions/instructions/threads/corrections/scouting, reports/notes/stats/ratings/config and watch. |
| Coordinator engine | `src/coord/mod.rs:3-25` owns adapters, briefs, caller resolution, CLI, config, journal/fold/state/kinds, Git, placement/launch/release/reconciliation, roles/rules/stats/views, lifecycle and watcher. The engine talks to `hc` and `herdr-app`, not usage (`src/coord/mod.rs:31-63`). |
| Visible watcher | `src/coord/lifecycle.rs:246-262` launches the CLI's `coord watch` in a named watcher tab in the coordinator space. `src/coord/watch.rs:420-513` takes an exclusive watcher lock, consumes managed-terminal events, ticks each second, heartbeats every five seconds, and reconciles on the configured interval. It is not a hidden daemon. |
| Persistence | `src/coord/paths.rs:25-42` builds every path under `<suite-root>/coord`: `journal/`, `state.json`, `lock`, `up.lock`, `watcher.lock`, `heartbeat.json`, `config.json`, `briefs/`, `results/`, `sessions/`, `roles/`, `home/`; terminal-specific locks additionally live in `coord/locks/` (`src/coord/paths.rs:96-119`). Journal files are monthly JSONL; cached state is replayable (`src/coord/journal.rs:57-74,149-170`). Journal format is 1 and fold version is 5 (`src/coord/kinds.rs:8-23`; `src/coord/state.rs:12-35`). |
| Board, Timeline and “inbox” UI | Already a separate native GPUI JS mod, `mods/herdr-kit-coord`, not Rust GPUI code. Its entry imports only its own modules plus host APIs (`mods/herdr-kit-coord/main.js:1-9`). Board and Timeline share the Coordinator overlay (`main.js:14-18,45-50`; `ui-board.js:25-45`). The CLI inbox is the agent's pending-event/ack protocol; the dialog does **not** execute `coord inbox`: store reads status/board/log (`mods/herdr-kit-coord/store.js:45-76`). Do not add another inbox surface. |
| Mod lifecycle/status | The first status-strip copy owns polling, journal watching and `nextCommand()` (show, timeline, restart, focus and navigation); overlay copies refresh their visible data (`mods/herdr-kit-coord/main.js:24-42`). Store publishes `herdr-kit.coord.status.v1`, conditionally autostarts, and invokes up/restart (`store.js:5-6,95-120,131-169`). Status rendering uses mod-local icons (`ui-status.js:3-5,33-39`). |
| Layout | `layouts/herdr-kit.json:170-189` already adds the coord mod using `content.mod`, just as tabs/files/changes are referenced (`:89-98,150-162`). The current 28px status row has a flexible usage block followed by a separate 80px coord block. The proposal is **not** inventing mod support; it completes backend/distribution independence. |
| Usage bar | No cross-mod JS imports or direct calls into the coordinator. Usage owns the left indicators and its Limits readout, with provider omitted below its measured width of 500px (`mods/herdr-kit-usage/main.js:59-99`). It starts suite update maintenance at startup/every six hours (`:27,55-58`), so usage indirectly updates/restarts the coordinator through the suite release. |
| Post-activation | `src/main.rs:97-115` invokes `coord::lifecycle::after_activation` only after an activating install/update, including rollback. The hook checks for coord config, then runs `<root>/bin/herdr-kit --data-dir <root> coord up --if-enabled`, with a 120-second bound (`src/coord/lifecycle.rs:738-783`). Delegate refusal is skipped/forbidden_caller, not failed; other failures remain failures. |

### Complete extraction-boundary inventory

* **Rust/package boundary:** production coord code reaches outside its subtree only
  for `crate::manager::stable_binary` and `crate::paths::data_dir` in
  `src/coord/mod.rs:82-96`, and `crate::manager::active_release` in
  `src/coord/lifecycle.rs:56-58`. The reverse dependencies are the CLI dispatch,
  shared output/error handling and activation hook in `src/main.rs:73-117`.
  There are no shared workspace crates today. Normal dependencies are the root's
  anyhow/clap/serde/serde_json/fs2/sha2/tempfile/rustix/tokio set
  (`Cargo.toml:8-17`); keep only those actually needed by each resulting package.
  Test-only `crate::coord` imports and relative capture `include_str!` paths must
  move with the engine (`src/coord/brief.rs:87-90`;
  `src/coord/adapter.rs:481-484`; `src/coord/reconcile.rs:413-417`).
* **Root/config/environment:** root CLI supplies `--data-dir` / `HERDR_KIT_DATA_DIR`
  (`src/main.rs:13-15,89-92`). Defaults are XDG/Linux or macOS suite paths
  (`src/paths.rs:4-27`). Coordinator config is separate from updater state and
  contains autostart, coordinator role, timers and worktree root
  (`src/coord/config.rs:9-35`). External executable overrides are
  `HERDR_KIT_HC` and `HERDR_KIT_HERDR_APP`
  (`src/coord/adapter.rs:211-214,323-326`). Caller identity comes from
  `HC_TERMINAL_ID` and live `coord.*` terminal labels, not executable naming
  (`src/coord/caller.rs:36-84`). Preserve the permission matrix: up/down/restart,
  config and rate are person-only; report/note are delegate-only
  (`src/coord/mod.rs:162-184`). Never clear caller identity to make the hook work.
* **Updater ownership/pinning:** `src/manager.rs:19-20,195-201` hard-codes both mod
  bases, suite layout, release directory and the sole stable binary. Manager
  state holds current/previous revisions, file-hash receipts, managed generations,
  source/app home and maintenance time; its activation journal has only one old
  binary hash (`:22-41`). It archives committed local source and builds only
  `--bin herdr-kit` (`:351-368`). It copies known mods, text-rewrites both names,
  and injects identical suite BINARY/DATA_DIR/RELEASE/MOD_NAME bindings into each
  (`:369-384`). Release and installed assets are hash-verified, with collision/
  drift refusal (`:263-310`). Rollback activates retained assets (`:461-472`).
* **Release identity is operational, not cosmetic:** lifecycle reads the suite's
  root `state.json.current` through manager (`src/manager.rs:409-413`;
  `src/coord/lifecycle.rs:56-58`). Watcher compares that identity at heartbeat,
  writes `watcher.stopped/release_changed`, then execs **the stable herdr-kit with
  `coord watch`**, keeping its terminal (`src/coord/watch.rs:414-417,515-551`).
  `up` also requests a coordinator handoff when its session's release differs
  (`src/coord/lifecycle.rs:560-590`). Merely moving files breaks both edges.
* **Prompts/session paths:** `Ctx::cli_argv` prefers the installed stable binary,
  falls back to current_exe, and includes the explicit nondefault data root;
  `Ctx::cli` shell-quotes it (`src/coord/mod.rs:74-96`). The hard-coded **suffix**
  `coord` comes from `src/coord/brief.rs:5-18,38-60`; `kit coord …` wording is also
  embedded in `src/coord/roles/coord-main.md:12-18,75-109`. The absolute binary is
  generated, not fixed in the role file. Coordinator launch combines that role,
  generated CLI and startup read (`src/coord/lifecycle.rs:304-339`). Delegate
  first/follow-up messages use the same CLI/result renderer
  (`src/coord/place.rs:474-480,799-814`). There is also a watcher rule's literal
  “using coord report” (`src/coord/rules.rs:41,254-258`). Built-in roles are
  compiled from `src/coord/roles/`; override files win by name
  (`src/coord/role.rs:37-44,61-70`). Documentation duplicates the main role
  (`docs/coordinator/roles/coord-main.md:1-18`).
* **JS/protocol/assets:** development coord runtime still says BINARY=herdr-kit,
  DATA_DIR=null and MOD_NAME=herdr-kit-coord
  (`mods/herdr-kit-coord/runtime.js:1-4`); generated installed runtime instead
  uses an absolute immutable suite binary. All store argv includes `coord`, and
  errors name herdr-kit (`store.js:45-76,136-153`). JSON schema decoders and
  shared-state key are public integration contracts (`model.js:14-20`;
  `store.js:5,95-98`). Updater's global replacement also rewrites names inside
  JS/JSON, so changing the mod base without reviewing cache keys is unsafe
  (`src/manager.rs:226-234`). Keep self-overlay/icon paths generation-correct.
* **Tests/docs/tooling:** integration harness pins `CARGO_BIN_EXE_herdr-kit`,
  legacy env names and the `coord` argv component (`tests/coord_cli.rs:13-17`).
  Native model/store tests live in `tests/coord.test.js` and
  `tests/coord-store.test.js`; manager layout/runtime tests live in
  `src/manager.rs:546-664`. README's single-tool policy explicitly rejects helper
  executables (`README.md:3-6,167-177`): this decision must intentionally replace
  that policy and update CLI examples, recipe, spec, implementation plan and role
  documentation, not leave contradictory guidance behind.

### History and observed feasibility probes

`e67f5c0` introduced the whole coordinator and separate mod; `46e5684` fixed
forbidden-caller activation reporting; `7dc3c3f` corrected the 80px layout contract;
`8afa371` fixed a new live coordinator falsely being ended by stale watcher facts.
The last fix explicitly rechecks hc before recording loss
(`src/coord/watch.rs:270-289,1183-1284`); preserve this behavior during moves.

Read-only CLI help on the existing checkout release binary reproduced the full
`herdr-kit coord` surface. A disposable HOME/data root, seeded from the repository's
12-line journal fixture, returned board schema 1, seq 12, workstream ws-k3m2ab and
task ws-k3m2ab-t1; inbox returned cursor 0, last_seq 12, pending 1. The journal
remained byte-identical. No live coord reads/writes or actors were needed.
Evidence is ignored under `evidence/coord-standalone/` (help, read-model/inbox JSON
and scenario note). `herdr-app check` on a nonexistent path in a temporary app
home exited 1 (“does not exist”); this proves an installer can detect absence,
**not** that a running layout handles a removed mod gracefully. Rust LSP navigation
was attempted but its server exited; boundary claims above use direct source reads.
No new binary, updater execution or native UI migration was tested in this plan.

## 2. Target shape and alternatives

### Recommendation: one repository, separate binary package, reusable native mod

**Proposed:** keep herdr-kit as the layout/updater/usage suite. Add a Cargo workspace
member `crates/kit-coord`, producing `kit-coord`; move the engine subtree and its
roles/testdata into that crate, with its own CLI main and integration tests. Keeping
an internal `coord` module initially avoids gratuitous import renames; do not add a
public library/shared-framework crate without a consumer. Remove coord code and
its output/error types from the suite binary. Both executables may use ordinary
Cargo dependencies; neither depends on the other's implementation.

Keep the existing mod identifier/directory `mods/herdr-kit-coord`: it is already
separate, and “standalone” concerns its executable/runtime dependencies, not a
cosmetic rename. It becomes a reusable mod executing **only kit-coord**. Other
layouts may reference it by folder name using the same `content.mod` shape as the
herdr-kit layout. No imports from usage or suite internals. A source/package user
can build `cargo build --locked --release -p kit-coord`, install that binary and
copy the mod with its documented runtime binding without installing the herdr-kit
layout or usage collector. Do not add a second self-updater or require Herdr to
resolve Cargo packages. Keep OMP roles/model selection and resume behavior intact;
no new harness support belongs in this extraction.

| Approach | Benefits | Costs / decision |
|---|---|---|
| Same-repo independent crate + mod, suite-managed distribution | Atomic source change, one lockfile/trust root, existing receipt/rollback mechanism; independently runnable backend and layout-reusable UI | Two binaries to stage/verify; release adapter needed. **Choose this.** |
| Separate coordinator repository and independent updater | Independent releases and governance | New source trust/pinning, cross-repo compatibility matrix, two activation/rollback owners and potentially competing watchers. No demonstrated need yet; reject for this cutover. |
| Keep the single executable, rename/repackage only the JS | Smallest cosmetic diff | UI is already a mod; leaves the actual engine, prompt, hook and release couplings untouched. Reject: it does not deliver kit-coord. |

### Install, pin and update contract

**Proposed:** preserve one trusted source updater and one suite revision pin for
now. Standalone execution does not imply independent suite update cadence. Build
both workspace binaries from the same committed archive/lockfile; release contents
include herdr-kit, kit-coord, both mods and the compiled layout. Generate per-mod
runtime bindings: usage gets the immutable herdr-kit, coord the immutable kit-coord.
The stable `<root>/bin/kit-coord` is for people, prompts and watcher replacement;
there is no PATH/shell configuration edit.

Do not make kit-coord parse the manager's private State struct. Introduce a small,
versioned **deployment descriptor**, `<root>/kit-coord/active.json`, outside the
unchanged journal tree. It contains coordinator build revision and absolute
immutable executable path. Manager owns its publication/hash receipt; a documented
standalone package installation uses the same descriptor shape, never simultaneously
with suite ownership. This is a deployment contract, not another coordinator journal.
Embed the build revision in kit-coord when built from the archived source; its
running identity must not be inferred solely from a mutable current pointer.
An unmanaged checkout can use development identity without a descriptor.

Extend activation/recovery's owned-artifact set to both stable binaries and the
descriptor. Stage and verify the complete release and mod generations first; install
both stable binaries and the layout before publishing the descriptor as the
coordinator-ready signal. Record enough old/new hashes in the activation journal to
recover every interruption. The watcher compares its running build to the desired
descriptor and execs that verified immutable kit-coord with `--data-dir R watch`,
not `herdr-kit coord watch`. Preserve the singleton lock/visible terminal semantics.
Rollback publishes the retained coordinator descriptor/assets together; no state
format change is required. A missing/invalid target must produce degraded health
and retain the running watcher rather than exec a nonexistent file in a loop.

Move the updater hook to a narrow suite-owned subprocess adapter: invoke the
activated kit-coord's `up --if-enabled`, retaining timeout and caller attribution,
with disabled/skipped/failed distinctions. No Rust dependency on the coord crate.
The suite still owns *when* activation invokes it; kit-coord owns authorization,
autostart and lifecycle. Standalone use needs no suite hook to function.

### Graceful absence and Herdr constraints

* **Binary absent/bad protocol:** existing exec error handling and unavailable/
  stale UI provide the starting point (`store.js:56-64,114-120`). Change diagnostics
  to kit-coord, retain the last good board as stale, disable destructive/lifecycle
  actions until a healthy compatible response, and offer retry. Never fall back to
  `herdr-kit coord`, download code, create actors or spin on failures. Usage and
  terminal navigation continue. The suite activation hook reports missing component
  as skipped/unavailable, not “started”. Managed artifact loss still triggers
  updater drift refusal; graceful UI does not mean silently repairing drift.
* **Mod intentionally not installed:** compile the layout without its coordinator
  node; leave the usage/terminal surfaces intact. Parse/transform the layout JSON,
  not a blind mod-name string substitution. A full suite installation treats its
  selected coordinator artifacts as required and refuses partial activation.
* **Mod unexpectedly removed after activation:** do not claim a native fallback
  based on the nonexistent-path check. A disposable native fixture must establish
  whether only the coord block fails while the rest remains usable. If the host
  invalidates the whole layout, require an install-time fallback integration before
  release (or decline the graceful-absence claim). No fabricated `optional` or
  `fallback` field: none is documented for general `content.mod`.
* **Status composition:** extraction must retain the 28px bar, 80px coordinator
  budget, flexible ellipsized left content and correct normal/narrow Limits text.
  Current code has separate blocks, not the desired eventual single responsive
  composition. If composed together, the usage view may read only the cached
  coordinator status and dispatch existing registered coord commands; lifecycle
  stays in the coord mod/CLI, with no cross-mod imports or layout-state writes.
  Do not smuggle a statusbar redesign into this extraction. The mod works without
  the usage bar; usage works without it. Keep the shared-status schema/key stable
  during the boundary move, and avoid generation-rewriting protocol key literals.

**Observed API basis:** layout already references external mod folders; the local
installed guide documents `content.mod`/props and `place:none`
(`~/.config/herdr-app/AGENTS.md:222-243,800-826`). Host exec, overlays, shared state,
commands and folder watches are in the generated contract
(`mods/herdr-kit-coord/gpui-kit.d.ts:22215-22219,22544-22576,22601-22626`; ignored,
regenerated by Herdr, not a repository dependency to commit). No known API blocker
to a standalone mod/binary. The layout format is **composition**, not a dependency
installer/version resolver. Native discovery and unexpected-mod-loss behavior are
acceptance gates, not proven guarantees. Do not add command-palette integration
or keybindings; reuse the mod's existing commands.

## 3. Migration and clean cutover

### Persistence and command contract

**Proposed:** keep `--data-dir R` meaning the existing parent root, with actual
coordinator files at `R/coord`; preserve the current Linux/macOS default parent
paths exactly. The old brand in a storage pathname is compatibility, not a runtime
dependency. Do not rename/copy/rebase journals, role overrides, briefs, results,
home, sessions or lock files. All journal-embedded absolute paths and IDs remain
valid. Keep journal format 1, fold version 5, config fields and JSON read models
unchanged for this extraction. Replay a fresh cache from the same journal and
compare meaningful state/seq/cursor; do not delete a person's cache to “migrate”.

Use only `kit-coord <operation>` after cutover, with no extra `coord` namespace.
`kit-coord --data-dir R report … --result R/coord/results/TASK.md` is the delegate
contract. Rename executable override envs to `KIT_COORD_HC` and
`KIT_COORD_HERDR_APP`, and the explicit root env to `KIT_COORD_DATA_DIR`; update
all launch/test/docs consumers atomically, without accepting the old env names as
aliases. Default path compatibility does not depend on the old suite env. Suite
calls pass R explicitly. Preserve `HC_TERMINAL_ID` and `coord.*` ownership labels.

Regenerate the command from one kit-coord argv builder, shell-quoted only for
human/prompt text. Remove the hard-coded suffix from all footer, nudge, coordinator
command/truncation strings, role bodies and documentation copies listed in section
1; update lifecycle watch argv, store argv, error text, tests and recipes. Retain
the useful `kit` explanatory shorthand only if it expands to the complete
kit-coord command, never “kit coord”. New and resumed/follow-up delegates get the
new footer (`src/coord/place.rs:474-480,809-814` already centralizes these cases).
User role overrides remain untouched: preflight identifies outdated command text
and asks the person to update their own override before it is used. Do not rewrite
custom instructions automatically or mutate old saved briefs/transcripts.

### Why a drained transition, not a compatibility shim

An already-running old watcher has a compiled-in `bin/herdr-kit coord watch`
replacement path, and already-running agents have the old command in context.
Changing a prompt file or adding kit-coord to disk cannot rewrite those processes.
An immediate hot replacement would let the old watcher exec a command removed from
the new herdr-kit, while an old delegate could lose its reporting path. This is a
concrete migration hazard, not an API limitation.

**Choose a short, person-owned quiescent cutover:** finish/record current work,
settle all active delegates, persist coordinator instructions/open threads, drain
the inbox, then use the old person's `coord down` (which disables autostart and
stops coordinator/watcher). Leave workstreams, resources and saved sessions intact;
no user spaces or core are closed. Activate the new release and explicitly run
kit-coord up as the person. A new coordinator session reads the same journal; old
sessions remain historical and delegate sessions resume with a new footer. This
preserves durable continuity, **not uninterrupted process uptime**. If zero
interruption is required, this plan is no-go until the person approves a separate
handoff mechanism: old processes cannot be made compatible by a clean rename.

Do not keep an old command shim/alias or indefinitely dual-run coordinators. An
old immutable release remains solely a rollback artifact. Reverse cutover must
also quiesce new sessions before rollback, then start the old command; resume
follow-ups regenerate the old footer. Never replay stale commands into terminals.
The person may authorize this transition later; a delegate cannot execute it or
bypass the person-only start/restart guard.

### Updater bootstrap and drift

The **installed old updater** builds/copies only herdr-kit and binds the coord mod
to it (`src/manager.rs:365-377`). Its `update apply` cannot safely perform the first
extraction even if the new source builds. Use the newly built, verified manager's
`install --source … --app-home …`, still through the trusted source channel, not
an in-place copy into the live bin directory.

Before extraction commits can become auto-maintenance's source HEAD, the person
must freeze the configured source on a trusted local snapshot repository at the
current committed revision via the existing `source --repo` operation. This is a
temporary release-control snapshot, **not** the proposed separate product repo.
`source` resets the due timer but unchanged frozen HEAD is safe
(`src/manager.rs:450-456,474-483`). Wait out any in-flight manager operation; verify
the configured source and active hashes. Build/test the extraction elsewhere in
the source checkout; at the authorized window, the new manager's install registers
the intended main checkout and activates its committed HEAD. This avoids automatic
old-manager staging before delegates are drained. Do not change updater state JSON
by hand, add an authorization bypass, or rely on “maintenance probably is not due”.

Drift remains a stop condition: changed layout, mod, binary, descriptor or release
must be explained/preserved, not overwritten. Recovery must accept old single-bin
receipts/activation journals while adding the new owned files, and rollback must
know which artifacts existed in each generation. Retained old mod generations are
not stale code aliases and are not deleted as part of extraction.

## 4. Dependency-ordered implementation and acceptance plan

Sizes are engineering-day ranges for one implementer familiar with this code,
including focused verification; mostly moves, but activation is the high-risk work.
They are estimates, not delivery promises. The baseline move surface is about
16,000 lines including engine, coord JS and tests (local `wc -l`), not 16,000 lines
of new behavior. No phase below was implemented by this assessment.

| Phase / depends on | Work | Acceptance check | Rough size |
|---|---|---|---|
| P0: approve contracts and freeze release intake | Accept same-repo structure, old paths, one pin, brief pause, no aliases. Arrange person-owned frozen source before extraction lands. Establish disposable native mod-present/mod-absent baseline and documented host limits. | No live changes by a delegate. Native fixture proves missing coord cannot take down usage/terminals, or an explicit blocker is resolved before proceeding. Record expected board/status/inbox and protocol contracts from fixtures. | 0.5–1 day |
| P1: crate + commands; after P0 | Move engine/roles/testdata and CLI integration tests into kit-coord; remove coord from herdr-kit and detach its Output/errors. Centralize kit-coord argv/default paths/envs; migrate every renderer and caller. Preserve permissions and wire/state formats. | Build both packages with the locked workspace. Run moved coordinator behavior/integration tests, especially forbidden callers, report attribution, replay, placement/resume and graceful restart race. A throwaway CLI flow creates work, renders first/follow-up briefs, reports via the new executable, reads/acks inbox; herdr-kit help has no coord. Custom data root and spaced paths stay isolated. | 1–1.5 days |
| P2: two-binary activation; after P1 | Add descriptor and embedded build revision; extend receipts, staging, bootstrap paths, recovery and rollback. Generate per-component runtimes. Replace direct activation dependency with the bounded kit-coord subprocess hook; watcher reads deployment contract, not manager internals. | Private committed-source install stages both bins and matching mods. Crash-point fixtures cover every owned-artifact publication; recovery is idempotent. Release changes replace exactly one visible watcher without terminal duplication. Missing target stays degraded without exec loop; skipped/forbidden_caller stays skipped; real failures remain failed. Old receipts/rollback remain readable and modified owned bytes are preserved on refusal. | 1–2 days |
| P3: reusable mod/layout integration; after P2 | Remove suite-command knowledge from coord JS/default runtime; keep self-overlay/assets/protocol key generation-safe. Compile optional coordinator node structurally, no dangling selected dependency. Keep existing interaction semantics and bar constraints; document standalone mod/binary installation. | JS model/store tests use kit-coord and exercise missing binary/stale data. `herdr-app check` plus a real disposable native window shows Board + Timeline, start/off/failed states, clear-context pending/confirmation and focus/navigation. Reuse mod from a second fixture layout without usage. Verify normal/narrow bar with seeded real quota-shaped data; native mod removal cannot disable unrelated surfaces. | 0.5–1 day |
| P4: full migration rehearsal; after P3 | Exercise old install → quiesce → new-manager install → kit-coord up, then reverse rollback; use representative journal/overrides/briefs/results/session refs, never live state. Update README policy, recipe, spec/implementation plan and role copies; remove obsolete command references in active guidance. | Existing journal bytes preserved as an append-only prefix; IDs, seq/cursor, questions, instructions, threads, task/result/session paths survive. One healthy new coordinator and watcher; no spurious session_ended for the live replacement. A resumed delegate receives/uses a new report footer. Suite maintain/apply after migration and rollback both select complete matching artifacts; no shims required. | 1 day |
| P5: separately authorized person rollout; after P4 | Confirm frozen intake and no pending launches/reports, drain the real coordinator, person runs down, invoke the verified new source updater, person starts kit-coord, restore normal source intake. | Post-install status verifies both binary/descriptor receipts and source, no drift, one watcher and healthy coordinator, same durable workstreams/inbox state; Board/Timeline/usage visible. No core restart, terminal typing, user-space closure, keys/settings edits or direct binary overwrite. Record a rollback command/runbook before proceeding. | 0.5 day |

Total indicative size: **4.5–7 days**, dominated by activation and transition proof,
not by rewriting the coordinator. Stop at a failing phase gate rather than deploy
a partial extraction. Run affected checks once after each settled implementation
slice; no need for a broad baseline suite for this documentation-only decision.

### Highest-risk failures and controls

| Risk | Required control |
|---|---|
| Old watcher execs a removed coord command | Drain/stop before first activation; do not rely on its release-change restart. Thereafter descriptor + kit-coord-only replacement, singleton lock and native fixture proof. |
| Fresh coordinator incorrectly marked ended | Carry the hc recheck and regression from 8afa371 unchanged; migration fixture must observe healthy replacement after queued old terminal events. |
| Hook starts from a forbidden caller or bypasses guard | Preserve caller environment, person-only matrix, skipped refusal and real failure distinction; never retry as an impersonated person. Explicit person up is required after the drained transition. |
| Update source advances before new installer is ready | Frozen trusted source before landing extraction; first activation by the new manager, never old update apply. No opportunistic live deployment by an implementation delegate. |
| Partial activation / rollback loses new binary ownership | Extend journal/receipts for both binaries + descriptor; immutable artifacts first, descriptor readiness last; crash/recovery and old-release rollback fixture. |
| Runtime absent, mod missing, stale cache says running | Bounded exec failures, explicit unavailable/stale state, no auto-start on stale/unvalidated state; optional-node compilation and native absent-mod gate. Never mask drift or claim repair succeeded without status proof. |
| Persisted old prompts or custom roles keep using old CLI | Quiescent boundary, fresh coordinator, regenerated delegate follow-ups, explicit custom-role preflight. Historical briefs/transcripts remain evidence, not editable migration targets. |
| Updater text replacement mutates shared keys / future pin divergence | Structural component bindings and a stable literal protocol key. One source revision until independent versioning has a real consumer; no floating latest dependency. |

## 5. Verdict

**Go for the same-repo extraction if the person wants independent reuse and accepts
one planned coordinator pause; no-go for an unannounced hot rename.** The mod
boundary already exists and the engine has only three production outward symbol
uses. The work buys a genuine reusable coordinator command/UI, a smaller suite CLI,
and explicit ownership of lifecycle versus layout/updater/usage. It also makes a
later repository split possible without paying that distribution cost now.

It does **not** buy a different orchestration model, independent suite auto-update
cadence, smaller total system complexity, or a new inbox. Cost is a second binary,
a small deployment contract, more activation/recovery coverage, full prompt/caller
migration and a deliberately managed first cutover. Keeping everything in one
executable remains cheaper if no other layout or caller will use the coordinator;
renaming only the mod would have almost no benefit because it is already separate.

There is no demonstrated Herdr API blocker to composition or binary execution.
There is no documented general optional-mod fallback/dependency resolver, and
native missing-mod isolation is unproven here; it is a P0/P3 release gate, not a
claim of support. The actual non-negotiable transition constraint is the old
watcher's and agents' compiled/persisted command contract. The proposed pause
solves that cause without permanent aliases or weakened authorization.
