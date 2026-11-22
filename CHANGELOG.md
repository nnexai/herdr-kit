# Changelog

## Unreleased

- Make the coordinator machine-neutral. kit-coord now ships only the
  coordinator instructions and two portable built-in roles (`coord-main`,
  `coord-default`, both `omp` on the harness default model); a machine's role
  set, models, subscriptions and fallbacks are local files in
  `<data-dir>/coord/roles/`. Roles gain `description` and `fallback` fields,
  and the coordinator's first message lists the roles that can start on the
  machine, derived from them (`kit-coord roles --guidance`). The coordinator
  role's body is added to the built-in instructions as local notes instead of
  replacing them.
- Detect missing harnesses: `kit-coord roles` reports `available` and
  `unavailable_reason`, and task start, cold-resume sends and coordinator
  launch, restart and handoff refuse a role whose program is not on `PATH`
  (`harness_unavailable`) before changing anything. The Roles view marks such
  roles and shows descriptions and fallbacks.
- Add local extensions: helper commands configured in
  `<data-dir>/coord/extensions.json` are listed in the coordinator's startup
  read (with the bounded, concurrent output of `startup: true` helpers) and in
  `kit-coord board`, and run on demand with `kit-coord ext <name>`.
- Document machine setup with a Copilot-only example role set
  (`docs/coordinator/machine-setup.md`).

- Add `kit-deck` (`crates/kit-deck`), a standalone OpenDeck / OpenAction Rust
  Stream Deck extension (`dev.herdr.kit.sdPlugin`) surfacing live `hc events`
  agents, Agent Inbox / Coordinator needs-you items with hold-to-reply actions,
  and 240° radial usage gauges for Codex, Claude, Copilot and Antigravity.
- Add Antigravity subscription limits to `kit-usage` via `agy -p /usage
  --output-format json` alongside OMP providers, with independent source
  scheduling/backoff and anonymous tier/window projection in the Limits UI.
- Replace the separate coordinator views with one dialog for needs-you items,
  messages, workstream receipts and the journal, opened from the bottom-bar
  coordinator item (the sidebar row is gone). The journal groups plain-language
  events by day and workstream, and the compact bar item keeps the needs-you count.
- Align the Coordinator composer and make it wrap and grow up to six lines;
  Enter inserts a newline and the send icon delivers the draft.
- Project all open questions from the coordinator fold for the dialog, including
  questions sharing a blocked episode, and close their indicators on any resolution.
- Collect unused generations containing Herdr-generated `jsconfig.json` inside
  nested mod roots, using the same exact-name exception as per-mod receipts.
  Other unreceipted files still prevent collection.
- Avoid repeated retained-workstream inbox wakeups on unchanged reconcile
  passes, and stop recording new terminal turns after a task is closed.

- Introduce the shared `kit-core` Rust crate for data-root/path policy, JSON
  error formatting and repository-isolated Git commands. Standalone coordinator,
  usage/limits and optional manager builds include it as a Cargo dependency;
  CLI bytes, runtime package selection and stored formats are unchanged.
- Keep blocked/question delegates in a durable Needs you item across inbox
  acknowledgements and coordinator handoffs, with linked-question resolution,
  restart-safe 10-minute-to-4-hour coordinator reminder backoff and a persistent
  compact-bar attention icon. Include the unconsumed handoff note on startup
  and remind coordinators to arrange person-only lifecycle steps before briefing.
- Remove the rollback command, prior-selection receipt fields, obsolete
  activation fallback and dead coordinator helpers; stop hiding dead-code warnings.
- Split manager deployment, coordinator placement, reconcile, the watcher
  tick, rule evaluation, workstream release and the journal fold into smaller
  units, share one bounded process runner and format the Rust workspace with
  rustfmt. Commands, output and stored formats are unchanged; an oversized Git
  reply is now reported as `git output exceeds limit`. A scripted watcher run
  with a recorded transcript guards the watcher, rules, release and fold.
- Ignore `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and the other
  repository-selecting variables inherited from a Git hook or `git rebase
  --exec` in every kit-coord and manager Git call, the kit-coord build script
  and the tests. They previously redirected work to the caller's repository.
- Remove superseded extraction/migration plans and duplicated role/fixture docs.
  Canonical roles and fixtures live with kit-coord.
- Manager receipt format changes are clean cuts, not automatic conversions.
- Delete superseded owned deployment snapshots after activation, with
  drift refusal and idempotent crash recovery; preserve unowned siblings.
- Sweep superseded receipt-owned mod folders, matching mod-data directories and
  release generations after activation and on maintenance (including not-due
  checks). Keep selected revisions, layout/readable-process pins and uncertain
  ownership; journal exact cleanup trees for partial-deletion crash recovery.
- Ignore unreadable process links during generation cleanup, and make coordinator
  replies more concise.
- Document the clean manager ownership cutover required by installed rollback-era
  receipts, preserving the coordinator's journal, configuration, roles and sessions.
- Complete the mechanical P2–P4 split into independent kit-coord and kit-usage
  CLI/mod packages, optional kit-manager, kit-coord-ui (coord-only dependency),
  and the herdr-kit layout, with shared Rust functionality in kit-core and no
  legacy CLI aliases.
- Move host chrome, readout composition and optional maintenance into the
  layout's small bar. Capability producers keep their lifecycle and publish
  generation-bound presentation snapshots; Limits remains flush right.
- Remove legacy receipt envelopes, old-writer sidecars, catalog fallbacks,
  migration journals and legacy rollback code. The initial modular deployment
  is a person-arranged clean replacement with fresh coordinator/watcher.
- Keep coordinator identity on its own build/ready descriptor, not manager state;
  migrate prompts, reports, watcher argv and environment overrides together.
- Let `task send` steer an already-ready working OMP delegate via paste+Enter,
  report steering/new-turn delivery in JSON, and retain startup/permission
  refusals. Update the coordinator prompt and add the CLI regression.

- Add the standalone `kit-coord-ui` conversation view with truthful turn state,
  elapsed time, question responses, lifecycle/focus actions and reduced motion.
- Add person-only `coord say` and branch-aware `coord conversation` using
  the recorded OMP session, with exact-text delivery and explicit source states.

- Add `coord-design`, selecting the person's OMP `@design` model role for
  UI/UX and visual design. Route design work there instead of `coord-deep`,
  and document custom role/model selection, resume, escalation and statistics.
- Add coordinator workstreams, task delegation, a visible watcher and native
  status, with journal-based insights and statistics.
- Integrate usage into the existing 28px statusbar, replacing the bottom-right
  command-palette hint with a compact readout; remove the separate usage row.
- Place compact Limits and coordinator controls together on the right, with
  machine and agent indicators on the left.
- Report an unauthorized coordinator post-activation hook as skipped rather
  than as a failed activation hook, without changing coordinator permissions.
- Add Clear context to the Coordinator dialog, with an idle handoff and
  confirmed immediate restart.
- Confirm coordinator terminal loss against Herdr before ending a session,
  so buffered events cannot mark a live replacement missing after a restart.

## 0.1.0

- One extensible Rust CLI and native GPUI Herdr layout/mod suite.
- Anonymous OMP Codex, Claude and Copilot usage strip and account/window details,
  with unscaled AIC, reset countdowns, pace estimates and explicit stale states.
- Independent bounded usage cache, cross-process collection lock and backoff.
- Trusted local Git source registration, committed-revision builds, installation,
  automatic self/suite maintenance and rollback, with pinned generations,
  managed-file drift checks and recoverable activation.
- Preserve Herdr's built-in layout/mods, settings, shortcuts and live terminals.
