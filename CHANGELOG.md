# Changelog

## Unreleased

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

## 0.1.0

- One extensible Rust CLI and native GPUI Herdr layout/mod suite.
- Anonymous OMP Codex, Claude and Copilot usage strip and account/window details,
  with unscaled AIC, reset countdowns, pace estimates and explicit stale states.
- Independent bounded usage cache, cross-process collection lock and backoff.
- Trusted local Git source registration, committed-revision builds, installation,
  automatic self/suite maintenance and rollback, with pinned generations,
  managed-file drift checks and recoverable activation.
- Preserve Herdr's built-in layout/mods, settings, shortcuts and live terminals.
