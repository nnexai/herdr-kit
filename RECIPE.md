# Herdr Kit

A native Herdr workbench with a compact bottom-right subscription readout and
agent coordinator. Independent kit-coord and kit-usage packages work without
each other or the optional manager. The layout composes their readouts; see the
[package guide](docs/coordinator/monorepo-p2.md).

![Herdr Kit](preview.png)

## What you see

Keep the stock Herdr Spaces/Agents sidebar, tab bar, terminal panes, Files and
Changes panels. In the existing 28px statusbar, preserve machine, branch and
agent indicators on the left. Place the compact Limits readout beside the
coordinator status on the right, with no extra row. Use the current theme and
normal Herdr UI font. Narrow bars omit the provider name. Click for a native
subscription-details dialog with all anonymous accounts,
source timestamps, windows, meters, AIC, resets and conditional exhaustion
estimates. Unknown/unlimited/zero/unavailable/stale remain distinct.

## How it behaves

The strip normally reads once per minute and reacts to working-agent demand.
Rust coordinates OMP collection (five-minute idle, one-minute successful-working
cadence, failure backoff) across copies. The details dialog uses the shared
anonymous snapshot instead of creating another collection loop. Escape or
Close dismisses it without terminal input. Changing Spaces does not retarget
local usage to a remote machine.

Only the layout integration requests optional manager maintenance. It checks
the trusted local Git source and builds selected committed packages. Neither
capability requires or invokes an updater. User layouts/settings stay untouched.

The coordinator keeps workstreams, tasks, decisions and reports in an
append-only local journal. The person controls it with `kit-coord up`, `kit-coord down`
and `kit-coord status`; the status icon can start its visible watcher terminal.
People and the coordinator manage work, delegates report on their own tasks,
and the board, log, inbox, roles and statistics are readable by all callers.
Only the person changes configuration or rates outcomes. The data directory's
`coord/` holds state and role overrides; autostart is configurable.

The coordinator does not close spaces created by the person, type into startup
dialogs or reopen a coordinator tab the person closed. Operation, cause,
outcome and rating records support `kit-coord stats --json`. See
`docs/coordinator-spec.md` for the full behavior and safety rules.

## Built from

- `layouts/herdr-kit.json`: custom layout retaining built-in mod references.
- `mods/kit-usage`: native GPUI View, pure JS model, deployment runtime.
- `mods/kit-coord`: native coordinator status, board and timeline.
- `crates/kit-coord/`: independent kit-coord CLI, roles and fixtures.
- `crates/kit-usage/`: independent OMP usage adapter/cache and CLI.
- `crates/kit-manager/src/manager/`: selected committed-source builds, receipts,
  recoverable owned-artifact activation and drift refusal.
- `kit-packages.json`: package ownership, mod/runtime bindings and default bundle.
- `mods/herdr-kit-bar/`: host chrome, published-readout composition and optional maintenance.
- `herdr.exec`: explicit argv to the pinned release executable.
- `herdr.openOverlay`, `props`, `theme`, `shared`, `nextCommand`: native UI.

## Watch out

No HTML, DOM, browser React, Node runtime or `Intl` in this mod. Never keep a
callback context for later use. Generation-specific mod names are stamped into
both the managed layout and runtime; overlays must use that generated name.
Never read credentials into JS or parse raw OMP output in the frontend. Source
build updates require Git, tar, Cargo and a committed Cargo.lock. This channel
trusts the local source repository; it is not a remote release downloader.

## Done when

Rust and pure JS behavior checks pass; `herdr-app check` passes; an isolated
native window shows real usage and details with no errors. A new committed
local-source revision updates executable and managed assets together, and
edited/unowned files are preserved. There is no rollback command. The
selected live instance then shows the verified named layout without restarting
Core or ending existing terminals.
