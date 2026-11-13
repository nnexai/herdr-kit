# herdr-kit

One local Rust tool with native JavaScript mods and a personal layout for
[Herdr](https://herdr.dev). Usage/limits and agent coordination are capabilities;
installation and repository-based self/suite updates are included. This is not a Herdr fork,
a browser frontend, or a collection of backend executables.

![Herdr kit usage](preview.png)

## Build and install

Requirements: Linux or macOS, Rust/Cargo, Git, tar, `hc`, `herdr-app`, and OMP
for usage reports. The JavaScript runs inside Herdr's GPUI host, not Node.
Node is only needed for the pure model tests.

```sh
cargo test --locked
node --test tests/usage.test.js
cargo build --locked --release
# Installation uses committed source, including Cargo.lock.
./target/release/herdr-kit install --source "$PWD" \
  --app-home "$HOME/.config/herdr-app"
herdr-app do layout.switch --name herdr-kit
```

On macOS, pass your actual Herdr app home. `HERDR_APP_HOME` selects the window
for `herdr-app`; the installer takes its target explicitly. Installation does
not switch layouts, restart Herdr/Core, change shortcuts, or touch terminals.
The first install requires an existing app home and refuses naming collisions.

The stable installed command is `<data-dir>/bin/herdr-kit`. Default data is
`$XDG_DATA_HOME/herdr-kit` (normally `~/.local/share/herdr-kit`) on Linux and
`~/Library/Application Support/herdr-kit` on macOS. `--data-dir` or
`HERDR_KIT_DATA_DIR` provides explicit isolation. No shell configuration or
system-wide PATH is changed.

## Initial update source

The installed tool trusts the exact local Git repository supplied to `install`.
A revision is built from **committed HEAD**, via `git archive`, not from dirty
working files. Nothing is fetched, pulled, reset, stashed, or pushed.

```sh
<data-dir>/bin/herdr-kit status --json
<data-dir>/bin/herdr-kit source
<data-dir>/bin/herdr-kit update check
<data-dir>/bin/herdr-kit update apply
<data-dir>/bin/herdr-kit update rollback
```

`source --repo /absolute/repository` explicitly changes that trust decision.
The source must remain available, with its committed lockfile and build tools.
Cargo can access its configured dependency registry when a build needs it.
This is a trusted **local source-build channel**, not an unsigned remote binary
updater. Internet release downloads and release signing are not implemented.

The usage strip requests maintenance at startup and every six hours. Rust
allows a check once daily after success (one-hour failure backoff), under a
cross-process manager lock. A new committed revision is automatically built
and applied; unchanged or not-due checks do not rebuild. Maintenance checks
stop when no mod is running. Usage reads never trigger update work.

Each immutable release contains the executable and matching native assets.
Mods have revision-specific names, and their generated runtime calls the exact
matching executable. A single atomic layout replacement activates a staged
mod generation; native file discovery settles asynchronously. Activation has
an on-disk recovery journal. Old generations remain available for rollback and
for personal layouts still referencing them. No automatic release deletion.

The tool records content hashes. Modified managed layouts, mods, executables
or releases block replacement rather than discarding edits. User clones,
shipped Herdr files, `keys.json`, settings, programs, shared/view state and
layout click state are not owned by the updater. One app home is registered
per data directory; use an explicit separate data directory for an independent
installation.

## Usage statusbar

The named layout preserves the existing 28px Herdr statusbar. Machine, branch
and agent indicators stay on the left. On the right, the compact Limits
readout sits beside the coordinator status. Click Limits to view all providers,
windows, credits and estimates in the native details dialog. Escape/Close
dismisses it. Narrow bars omit the provider name instead of shrinking the text;
the command-palette shortcut still works.

Rust runs `omp usage --json --redact --no-extensions`, then keeps only anonymous
allowlisted quotas for Codex, Claude and Copilot. OMP owns authentication and
may refresh its existing credentials; herdr-kit does not read credentials or
make separate provider quota calls. Override source discovery with
`HERDR_KIT_OMP_EXECUTABLE` or `usage snapshot --omp-executable PATH`.

```sh
<data-dir>/bin/herdr-kit usage snapshot --json --agents-working
```

Source checks are spaced five minutes while idle, one minute after success
while agents work, and back off after failures. A private cache/lease and lock
prevent duplicate collectors across mod copies. UI reads normally run once a
minute. Unknown, zero, unlimited, unavailable and stale remain distinct.
Copilot AIC values are not rescaled. Details retain all accounts, reset times
and conditional pace estimates. Quotas are local to this machine, even when
viewing a remote Space. No Cockpit process, state, cache or installed files are
used.

## Coordinator

The coordinator helps a person organize agent work in workstreams and tasks.
It keeps the coordinator and its watcher visible in Herdr terminals, with a
status icon that can start the watcher when it is not running.

```sh
<data-dir>/bin/herdr-kit coord up
<data-dir>/bin/herdr-kit coord status --json
<data-dir>/bin/herdr-kit coord down
```

To try an uninstalled checkout without using an older installed executable,
build once and use a separate data directory:

```sh
cargo build --locked --release
./target/release/herdr-kit --data-dir "$HOME/.local/share/herdr-kit-preview" coord up
```

Keep that executable in place while the preview runs; rebuilding a running
development binary can invalidate its saved launch path. The app icon, board
and timeline ship as the separate `herdr-kit-coord` mod with normal suite
installation. Installation still reads committed source as described above.

The person can open and close workstreams, start, send to and close tasks,
release resources, record decisions and instructions, and rate outcomes.
The coordinator can do the same work-management operations, but only the person
can change configuration or ratings and start or stop the coordinator. Delegates
report and add notes to their own tasks. Anyone can read the board, log,
explanations, inbox, roles and statistics. `coord --help` lists commands.

State lives under `<data-dir>/coord/`, including the append-only journal,
configuration, roles and task files. Built-in roles can be overridden with
files in `coord/roles/`; `coord roles` lists the effective roles. Autostart is
controlled by coordinator configuration. The watcher is a `coord watch`
subcommand running in an hc terminal, not a hidden service.

The coordinator never closes spaces created by the person, never types into
startup dialogs, and does not reopen a coordinator tab the person closed.
The journal includes insight records such as operation and cause identifiers,
ratings and outcomes; `coord stats --json` summarizes them. See
[the coordinator specification](docs/coordinator-spec.md) for the full model
and safety rules.

## Structure and extension boundary

- `src/main.rs`: one CLI, shared by people, agents and mods.
- `src/usage*`: source adapter, anonymous protocol, scheduling and cache.
- `src/manager.rs`: source registration, release builds, deployment and updates.
- `mods/herdr-kit-usage/`: native GPUI presentation and pure view model.
- `layouts/herdr-kit.json`: managed personal layout, built-in mods reused.

Future Git, issue and local task-tracker capabilities belong behind additional
subcommands/modules in the same tool, not separate helpers or placeholder
commands. Future secure-store adapters must keep credentials in Rust and the
OS keychain, use them internally or through a child tool's supported input,
and never return them to JS/JSON or put them in argv/logs. This is an exposure
policy, not a same-user process sandbox. No secret-store commands ship today.

## Verification and provenance

Unit checks are the two commands above; native acceptance additionally needs
`herdr-app check` and a real window/screenshot in an isolated app home/core.
Never test update/recovery experiments against a personal running installation.
Initial Linux delivery passed 12 Rust behavior tests and 6 JS model tests, plus
native `herdr-app check` and real OMP usage in an isolated Herdr App 0.1.15 /
Core 0.1.20 window. The source-update smoke built a committed 0.1.1 executable
from an installed 0.1.0 source, verified rollback to 0.1.0, automatic maintenance
back to the new revision, and preservation of modified managed files and
unrelated configuration. Native macOS verification remains unperformed.

The usage parser/scheduling and view calculations adapt MIT-licensed Cockpit
by Konni Hartmann. Repository packaging follows Herdr's documented layout/mod
convention, illustrated by [herdr-max](https://github.com/maxjkfc/herdr-max);
its installer, keybindings and unrelated automation are not copied.
