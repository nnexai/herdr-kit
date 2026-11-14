# P2: independent kit-coord

P2 changes only the coordinator vertical slice. Usage remains on
`herdr-kit usage snapshot` until P3. The manager remains `herdr-kit`; it has no
`coord` subcommand or coordinator Rust dependency. No kit-core, alias, shortcut
or hidden updater is introduced. Merge, deployment and live lifecycle permission
are separate from development/private verification permission.

## Runtime and installation contracts

- Build only the capability: `cargo build --locked --release -p kit-coord`.
  Its source is `crates/kit-coord/`, native assets `mods/kit-coord/`, and CLI
  integration fixtures `crates/kit-coord/tests/`. Dependencies are third-party
  workspace dependencies, not kit-manager or kit-usage.
- Commands are `kit-coord --data-dir R <verb>`. The root remains the existing
  parent; journal, state, config, locks, overrides, briefs, results and sessions
  stay at `R/coord/`. Journal format 1, fold version 5 and read-model schema 1
  are unchanged. Linux/macOS default parent paths are unchanged.
- Explicit overrides are `KIT_COORD_DATA_DIR`, `KIT_COORD_HC` and
  `KIT_COORD_HERDR_APP`. `HC_TERMINAL_ID` and `coord.*` ownership labels retain
  their existing meaning. There are no legacy environment aliases.
- `kit-coord-ui` depends on kit-coord only. Its generated binary and coordinator
  mod command binding match that package's revision; it never routes through
  the manager or usage.
- Package-only managed install, from a built source manager:
  `target/release/herdr-kit --data-dir R install --source REPO --revision FULL_COMMIT --component kit-coord --app-home APP_HOME`.
  This installs the matching coordinator executable/mod and returns its exact
  `kit-coord-FULL_COMMIT` name. It does not install usage, the herdr-kit layout,
  a manager executable, keys or settings, switch layouts or start actors on a
  fresh root. Existing enabled installations retain the bounded authorized hook.
- Another layout references only
  `{"content":{"mod":"kit-coord-FULL_COMMIT","props":{"view":"statusbar"}}}`
  in its own visible 80px block. This shown copy owns polling and command
  receipt. `<resolved-mod>.show`, `.timeline` and `.restart` use that capability.
  Layout ownership/pinning stays with the consumer; it is not rewritten by a
  component install. External layouts have no maintenance timer.

## Source export without an installed manager

An owner may export a complete committed checkout with `git archive FULL_COMMIT`,
retain Cargo.lock and build only `-p kit-coord`. For archives without Git metadata,
set `KIT_COORD_BUILD_REVISION=FULL_COMMIT` during the build. A Git checkout's build
script reads its actual commit and tracks HEAD/ref changes. `kit-coord --version`
includes the embedded revision. Installing binaries from dirty source while
claiming a release pin is not a supported distribution procedure.

The owner's installer copies the frozen executable to the immutable absolute
`R/releases/FULL_COMMIT/kit-coord` and all files from `mods/kit-coord/` to
`APP_HOME/mods/kit-coord-FULL_COMMIT/`. Its generated `runtime.js` contains exactly
these JSON-encoded exports, with absolute paths (no shell or PATH changes):

```js
export const BINARY = "/absolute/R/releases/FULL_COMMIT/kit-coord";
export const DATA_DIR = "/absolute/R";
export const RELEASE = "FULL_COMMIT";
export const MOD_NAME = "kit-coord-FULL_COMMIT";
```

The owner may atomically install a matching `R/bin/kit-coord` for people and
newly generated agent commands. Without that stable path, generated commands use
the actual current executable. No `herdr-kit` binary is needed. The owner verifies
pins/hashes, refuses collisions/drift, owns atomic publication and rollback, and
must pause for incompatible upgrades; no self-updater is implied by this recipe.

A fixed manual installation may omit `R/kit-coord/active.json`: its watcher stays
on its own embedded revision and never discovers manager state. An owner that
implements managed activation publishes this public descriptor **last**, after
matching immutable executable, stable binary and mod/layout references exist:

```json
{"schema_version":1,"revision":"FULL_COMMIT","executable":"/absolute/R/releases/FULL_COMMIT/kit-coord"}
```

The descriptor requires a full lowercase commit id and absolute executable.
Watcher replacement is that immutable executable with `--data-dir R watch`.
Missing, invalid, non-executable or wrong-version targets retain the current
watcher/heartbeat with degraded health. A bounded version probe is cached for an
unchanged candidate; failed exec races are retained/degraded rather than killing
or repeatedly execing the singleton. A usage/manager-only selection change does
not advance this descriptor. The optional manager owns its descriptor hash in
the same readiness-last recoverable transaction as its other artifacts.

## Person-owned initial drained cutover

This is durable continuity, not uninterrupted process/session uptime. Do not run
these steps merely because P2 development passed. The person separately authorizes
the trusted-source freeze, quiet main merge, install and lifecycle boundary.

### What drained means

1. Stop accepting/placing new work temporarily. Every unfinished old-namespace
   task must reach a durable report; judge it and close its task record with an
   explicit outcome. This includes the monorepo delegate's current task. Do not
   close unfinished work just to make a count zero: record continuation briefs,
   result paths and session/task IDs first. Open workstreams, pending decisions,
   instructions and discussion threads may remain; persist their intended next
   action. Do not release/delete resources or session history as part of drain.
2. Inspect `herdr-kit coord board --json --all` and `herdr-kit coord inbox --json`.
   There must be no running/placed/dormant/unclosed tasks, in-flight starts or sends,
   unhandled delegate reports or unanswered blocking questions. Handle each inbox
   item, then `herdr-kit coord inbox ack SEQ` at the last actually handled seq.
   The coordinator may perform task/inbox bookkeeping; it cannot approve its own
   lifecycle or the person's trusted-source change.
3. Wait until the coordinator has finished its turn and is idle/done. Idle alone
   is not the cutover: the person then runs old `coord down`, which disables
   autostart and stops coordinator **and watcher**, but deliberately leaves
   delegates/resources/sessions alone. Confirm stopped status and no managed
   writer in flight before activating the new namespace.
4. Idle retained delegate terminal processes may remain, but none may still be
   doing task work or be expected to issue the old footer. Close old-generation
   coordinator/Mission Log/Limits overlays through the person's normal UI before
   activation, so their old commands cannot be invoked afterwards. Do not close
   Spaces, tabs, user terminals or the Herdr core.

`down` intentionally does not terminate delegates. Cutting over with an active
old-footer delegate is unsafe: it can lose `herdr-kit coord report` mid-task.
Do not hot-switch it, retain a shim, or rely on the coordinator delivering a new
footer to a busy process. Finish/report/close it first, or postpone cutover.
Afterward the fresh coordinator can use `task start --session-of OLD_TASK` with
an explicit continuation brief where the existing resume/role checks permit it;
that supplies the new footer and keeps the saved session/model. If those checks
refuse, create an explicitly approved new task/session in the retained placement,
passing the previous result/context. Never strand an unreported running task.

### Ordered actions and ownership

Set these in the person's ordinary unmanaged terminal (do not clear a managed
caller's identity to impersonate the person):

```sh
DATA=/home/user/.local/share/herdr-kit
MAIN=/path/to/herdr-kit
APP_HOME=/home/user/.config/herdr-app
```

1. **Person trust decision; authorized delegate may execute only the exact grant:**
   read installed manager status, clone main with `git clone --no-hardlinks
   --no-checkout "$MAIN" "$FREEZE"`, detach at the installed known-working P1
   `current_revision`, run `"$DATA/bin/herdr-kit" --data-dir "$DATA" source --repo
   "$FREEZE"`, then `flock -n "$DATA/manager.lock" true`. If busy, wait; never
   stop manager work. Record revision/path/stdout. This must happen before P2
   enters the auto-updated main checkout. Keep snapshot and prior releases.
2. **Coordinator bookkeeping / person drain:** perform the readiness procedure
   above. Review effective `coord roles` and explicit environment overrides.
   The person backs up and manually reviews any custom `R/coord/roles/*.md`
   mentioning old `kit coord`/`herdr-kit coord` commands; new code flags these
   before use and never rewrites them. Migrate personal executable/data overrides
   to `KIT_COORD_*` in their actual launch settings; do not edit OMP config as a
   shortcut. Built-in roles require no installed role-file rewrite.
3. **Person only, after idle:**
   `"$DATA/bin/herdr-kit" --data-dir "$DATA" coord down`.
   Verify old coordinator/watcher stopped, autostart false, inbox handled and
   tasks closed. Journal/state/brief/result/session paths and IDs stay in place.
4. **Quiet main-checkout owner, only after merge go:** check main is clean and
   its HEAD is included in coord/monorepo; merge main into the branch first,
   then `git merge coord/monorepo` in main. No stash/reset/forced checkout under
   another delegate. Run the affected merged-main checks and build
   `cargo build --locked --release -p kit-manager -p kit-coord`.
5. **Authorized deploy owner:**
   `"$MAIN/target/release/herdr-kit" --data-dir "$DATA" install --source "$MAIN" --app-home "$APP_HOME"`.
   The complete bundle installs `bin/kit-coord` and `mods/kit-coord-REV`, retains
   manager/legacy usage `bin/herdr-kit`, binds kit-coord-ui to kit-coord, changes
   the owned layout's mod reference, and publishes `kit-coord/active.json` last.
   Root manager receipts/source re-registration are separate from coord data.
   Autostart is still false, so the bounded hook must not start an actor.
6. **Person only:** `"$DATA/bin/kit-coord" --data-dir "$DATA" up`.
   This deliberately starts a **fresh coordinator session**, not the old OMP
   context containing old command vocabulary. The new visible watcher runs
   `kit-coord --data-dir R watch`. Startup reads the same durable journal.
   Compiled roles move from `crates/kit-manager/src/coord/roles/` to
   `crates/kit-coord/src/coord/roles/`; override paths remain `R/coord/roles/`.
   New briefs remain `R/coord/briefs/`; new session directories advance under
   `R/coord/sessions/`. First/follow-up/resume prompts use one argv builder and
   `kit-coord ... report`, not an old `coord` suffix.
7. **Authorized delegate read-only verification:** installed status, one healthy
   visible watcher/coordinator, matching embedded/descriptor/UI revisions, no
   false terminal-loss session ending, board/inbox seq continuity, Mission Log /
   Board / Timeline / Limits screenshots. Coordinator assigns continuation work
   with new footers only after health is established. Ask before any repair that
   would introduce another lifecycle action.

Prepare source/build verification before drain where possible. The actual pause
is from old `down` through install and new `up`/healthy status; source builds and
native checks happen inside installation, so it is not a zero-downtime rename.
Allow a maintenance window for build plus OMP startup/any person approvals, not
a guaranteed fixed duration. Private rehearsal timings belong in the phase
result; they do not bound model startup or human response time.

### Exact reverse rollback

If P2 health fails, do not restart things opportunistically. The person drains
new tasks/inbox and closes the P2 overlays, then:

```sh
# Person, using the still-selected new namespace:
"$DATA/bin/kit-coord" --data-dir "$DATA" down
# Authorized deploy owner; P2_REV is the recorded deployed full commit:
"$DATA/releases/$P2_REV/herdr-kit" --data-dir "$DATA" update rollback
# Restore known-working trusted source through the public old manager:
"$DATA/bin/herdr-kit" --data-dir "$DATA" source --repo "$FREEZE"
# Person restores any separately changed custom-role text/launch env from its
# pre-cutover backup; do not rewrite journal, saved briefs or transcripts.
# Person then starts a fresh old-namespace coordinator from durable history:
"$DATA/bin/herdr-kit" --data-dir "$DATA" coord up
```

Rollback must report the complete retained P1 selection/current revision, remove
new stable kit-coord/readiness files from active ownership and restore matching
old layout/mod/binary bindings. Immutable P2 assets remain available for evidence
and retry. Verify exactly one old watcher/coordinator and unchanged journal prefix,
paths and IDs. The person chooses whether to resume further work. If any drain,
receipt/drift, role or startup check fails, keep the source frozen and ask; never
edit state or call the other namespace as an alias.
