# Modular Herdr kit packages

`kit-coord` and `kit-usage` are independent capabilities composed by the
herdr-kit layout. P5 release work remains separately authorized.

## Independent packages

| Package | Executable / native assets |
|---|---|
| kit-coord | `kit-coord`; `mods/kit-coord/`, engine, built-in roles and tests |
| kit-coord-ui | Mission Log; depends only on kit-coord |
| kit-usage | `kit-usage`; `mods/kit-usage/`, anonymous collector/cache and Limits |
| kit-manager, optional | `herdr-kit`; trusted committed-source installation/update |
| herdr-kit | Layout plus `mods/herdr-kit-bar/` composition/maintenance glue |

There is no kit-core, cross-mod JS import, shared capability service or hidden
updater. Coordinator and usage do not depend on each other or the manager.
Manager's CLI has no `coord` or `usage` namespaces.

```sh
cargo test --locked
node --test tests/*.test.js
cargo build --locked --release
```

Coordinator commands are `kit-coord --data-dir R <verb>`, usage is
`kit-usage --data-dir R snapshot --json`. Explicit environment settings are
`KIT_COORD_DATA_DIR`, `KIT_COORD_HC`, `KIT_COORD_HERDR_APP`,
`KIT_USAGE_DATA_DIR` and `KIT_USAGE_OMP_EXECUTABLE`. Defaults use the existing
Linux/macOS parent directory; each capability owns its own subtree.
Generated coordinator prompts, reports and watcher argv use kit-coord directly.
Built-in roles live in `crates/kit-coord/src/coord/roles/`; custom roles remain
under `R/coord/roles/`. No custom role or OMP configuration is rewritten.

## Include only kit-coord in another layout

Build the optional manager once from source, then select only the capability:

```sh
REV=$(git rev-parse HEAD)
./target/release/herdr-kit --data-dir /absolute/R install \
  --source "$PWD" --revision "$REV" --component kit-coord \
  --app-home /absolute/APP_HOME
```

This installs `R/bin/kit-coord`, immutable `R/releases/REV/kit-coord` and
`APP_HOME/mods/kit-coord-REV/`. It installs no usage, suite layout/bar, manager
executable, settings or shortcuts, and does not switch the consumer's layout.
Mount the returned exact module pin in that layout's visible status block:

```json
{"type":"block","size":{"px":80},"content":{"mod":"kit-coord-FULL_COMMIT","props":{"view":"statusbar"}}}
```

That copy owns polling, command receipt and the Board/Timeline overlay.
`<resolved-mod>.show` opens Board and `.timeline` opens Timeline. Lifecycle is
still person-owned. The external layout is never rewritten by the manager,
and has no maintenance invocation or installed updater.

An owner can instead export committed source with `git archive`, retain
Cargo.lock, and build only `cargo build --locked --release -p kit-coord`.
For an archive without Git metadata set `KIT_COORD_BUILD_REVISION=FULL_COMMIT`.
Copy the executable and complete mod directory to the immutable paths above,
and generate the mod-local runtime using JSON-encoded absolute paths:

```js
export const BINARY = "/absolute/R/releases/FULL_COMMIT/kit-coord";
export const DATA_DIR = "/absolute/R";
export const RELEASE = "FULL_COMMIT";
export const MOD_NAME = "kit-coord-FULL_COMMIT";
```

Without a stable `R/bin/kit-coord`, generated commands use the actual executable.
Without `R/kit-coord/active.json`, a fixed manual installation stays on its
embedded build revision. No manager state is discovered or read.
A managed installer publishes this public descriptor last:

```json
{"schema_version":1,"revision":"FULL_COMMIT","executable":"/absolute/R/releases/FULL_COMMIT/kit-coord"}
```

Watcher replacement execs that immutable target with `--data-dir R watch`.
Descriptor/target failures retain degraded health rather than repeatedly
execing a bad target. Updating usage alone does not change coordinator identity.
The optional manager owns only its selected artifacts, publishes readiness last
and refuses collisions/drift. Its one current receipt format is `R/state.json`.

## Layout composition

The suite's single 28px bar keeps flexible machine/branch/agent chrome left,
coordinator's 80px readout next, and Limits flush right. Narrow bars omit the
provider label. Neither capability renders the other's control.

Selected capabilities are mounted as zero-space `view: "source"` copies.
They retain their own polling, autostart and registered-command lifecycle and
publish version-1 presentation readouts through Herdr shared state:
`herdr-kit.coord.readout.v1` and `herdr-kit.usage.readout.v1`.
Each readout identifies its exact module generation. The layout bar consumes
only the pins supplied in its props and forwards clicks to those producers;
it does not collect data, read coordinator files, start a watcher or import
another mod's JS. Missing optional selections are omitted structurally.
With no capability selected the bar still provides host chrome.

Only the layout bar invokes optional manager maintenance, at startup/every six
hours when the manager is selected. Source scheduling/backoff remains in the
manager. A capability-only install never requests updates.

`task send` steers an already-ready working OMP delegate using the native
paste-plus-Enter send, guarded by `--if working`. Its JSON includes
`delivery: "steering", steered: true`; idle/resumed delivery says
`delivery: "new_turn", steered: false`. Never-ready agents, startup dialogs,
blocked states and permission prompts still refuse; no force flag is used.

