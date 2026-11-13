# Changelog

## Unreleased

- Add the standalone `kit-coord-ui` Rail / Mission Log: a native coordinator
  conversation with truthful turn state, elapsed time, question draft pills,
  existing lifecycle/focus actions and reduced motion. Keep Board/Timeline
  and the raw terminal available; fix the narrow working placeholder.
- Add person-only `coord say` and branch-aware `coord conversation` using
  the recorded OMP session, with exact-text delivery and explicit source states.

- Add `coord-design`, selecting the person's OMP `@design` model role for
  UI/UX and visual design. Route design work there instead of `coord-deep`,
  and document custom role/model selection, resume, escalation and statistics.
- Add coordinator workstreams, task delegation, a visible watcher and native
  status, board and timeline views, with journal-based insights and statistics.
- Integrate usage into the existing 28px statusbar, replacing the bottom-right
  command-palette hint with a compact readout; remove the separate usage row.
- Place compact Limits and coordinator controls together on the right, with
  machine and agent indicators on the left.
- Refine coordinator board metadata, action visibility and timeline readability.
- Redesign the Coordinator Board around grouped workstreams, title-led task rows
  and a stable selected-item action dock. Move technical metadata into Details,
  quiet healthy diagnostics, improve narrow layouts and show Start only once
  when the coordinator is off. Keep closed task titles readable and centre
  empty-state explanations with their title and action.
- Report an unauthorized coordinator post-activation hook as skipped rather
  than as a failed activation hook, without changing coordinator permissions.
- Add Clear context to the Coordinator dialog, with an idle handoff, confirmed
  immediate restart, dialog shortcut and bindable mod command.
- Confirm coordinator terminal loss against Herdr before ending a session,
  so buffered events cannot mark a live replacement missing after a restart.
- Give Coordinator Board and Timeline one shared popup frame at every width,
  and replace the Board's bottom action dock with selected-row actions. Keep
  actions keyboard-reachable without new shortcuts; quiet healthy diagnostics
  on both tabs and keep coordinator controls in the same header.

## 0.1.0

- One extensible Rust CLI and native GPUI Herdr layout/mod suite.
- Anonymous OMP Codex, Claude and Copilot usage strip and account/window details,
  with unscaled AIC, reset countdowns, pace estimates and explicit stale states.
- Independent bounded usage cache, cross-process collection lock and backoff.
- Trusted local Git source registration, committed-revision builds, installation,
  automatic self/suite maintenance and rollback, with pinned generations,
  managed-file drift checks and recoverable activation.
- Preserve Herdr's built-in layout/mods, settings, shortcuts and live terminals.
