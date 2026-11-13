# Modular monorepo P0: contract lock and native baseline

Status: P0 accepted as a phase result and retained on `coord/monorepo`, without
merge/deployment. P1 branch development is authorized; freeze is a P1 merge gate.
The person approved execution of [the extraction plan](standalone-mod-plan.md).
This record does not authorize a live cutover or later-phase rollout.
Baseline code: `dba532eee69613c79576211b043806b9a7181e9d`.
Installed native hosts exercised: herdr-app 0.1.15 / hc 0.1.21, Linux.

## Contracts retained from the approved plan

- Packages: independent `kit-coord`, independent `kit-usage` (UI label Limits),
  optional `kit-manager` producing the existing installer binary `herdr-kit`,
  and the `herdr-kit` layout/bar assets. No `kit-core` initially. The concurrent
  `kit-coord-ui` chat mod becomes its own package depending only on kit-coord
  when its implementation reaches main; no usage dependency.
- Final capability entry points are `kit-coord <operation>` and
  `kit-usage snapshot ...`. No aliases, forwarding legacy namespaces, new keys,
  or fallback through the suite executable. P0 changes none of these paths.
- `--data-dir R` keeps the existing parent-root meaning and platform defaults.
  Coordinator journal/state/config/locks/roles/briefs/results/home/sessions
  remain under `R/coord`, usage cache/lease under `R/usage`. Journal format 1,
  fold version 5, usage persisted schema 2/public schema 1 and existing read
  protocols remain unchanged. Never rewrite historical absolute paths or IDs.
- New explicit environment contracts are `KIT_COORD_DATA_DIR`, `KIT_COORD_HC`,
  `KIT_COORD_HERDR_APP`, `KIT_USAGE_DATA_DIR`, `KIT_USAGE_OMP_EXECUTABLE`.
  Manager keeps `HERDR_KIT_DATA_DIR` and passes explicit roots. Existing callers
  and overrides migrate at their vertical cutover, not via indefinite aliases.
  Caller identity and person/delegate authorization do not change.
- Immutable component generations use full source revisions. Runtime binds an
  absolute generation-specific executable, explicit data root, release and
  matching mod name. Presentation/shared protocol keys are stable literals;
  no blanket JS replacement. External layouts own their references and pins;
  installing a component does not edit them, switch layouts or start actors.
- Manager receipts bind each selected package's binaries, mods and owned layout
  assets with hashes and current/previous selections. Activation recovery tracks
  old/new hashes for all changed owned artifacts; readiness is published last.
  Legacy one-binary receipts/journals remain readable migration/rollback inputs.
  Drift and unowned collisions refuse activation. No ownership of settings,
  keys, user layouts, click state, coordinator journals or usage private cache.
- Coordinator readiness is a versioned public descriptor at
  `R/kit-coord/active.json`, independent of manager-private state. Its required
  information is schema version, selected coordinator revision and absolute
  immutable executable; receipts additionally bind the matching mod generation.
  The concrete serialized contract lands with its implementation and fixtures.
  The running executable carries its actual build identity. Missing/invalid
  desired targets degrade health without killing the current watcher or looping
  failed execs. Usage-only updates cannot change the coord selection.
- Without any deployment descriptor, a manually installed component uses its
  embedded revision; it neither reads suite manager state nor requires an
  updater. Separate installers owning one root must not compete.
- Each P1–P4 boundary includes committed-source install/update/rollback proof,
  its own authorization request, main synchronization and full deployability.
  P5 cross-version rollout remains separately authorized.

## Observed native baseline

Evidence is ignored at `evidence/monorepo-p0/` in this worktree. No product code
or live installation changed. The frozen checkout CLI read a copy of the existing
12-entry sample journal. A bounded fake OMP source reported anonymous Claude 32%
usage; no credentials or real model/agent were used. The fixture core had zero
terminals before and after. The coordinator stayed off throughout.

| Probe | Observed result | Evidence |
|---|---|---|
| Build / native load | `cargo build --locked --release` succeeded; native checks passed for coord, usage and diagnostic carrier | `build.log`, `check-{coord,usage,carrier}.json` |
| Board | One workstream, task and delegate question rendered from the real CLI | `board.json`, `baseline-board.png`, `baseline-status.json` |
| Timeline | Journal events rendered in the native Timeline | `log.json`, `baseline-timeline.png`, `timeline-status.json` |
| Limits | Native readout included `Limits Claude 32%`; details showed Claude 32%, reset/pace and explicit unsigned-in providers | `usage.json`, `baseline-usage.png`, `usage-status.json` |
| Hot coord removal | Host reported the missing coord directory; usage details still opened and displayed 32% | `coord-removed-status.json`, `coord-removed-usage.png` |
| Hot usage removal | Host reported the missing usage directory; Board still opened with its task/question | `usage-removed-status.json`, `usage-removed-board.png` |
| Fresh layout mount, usage absent | Missing usage mod error stayed local; new coord copy opened the populated Board | `cold-absence-status.json`, `cold-absence-board-status.json`, `cold-absence-board.png` |
| Fresh layout mount, coord absent | Missing coord mod error stayed local; new usage copy opened populated details | `cold-coord-absence-status.json`, `cold-coord-absence-usage.png` |
| Shown zero-space carrier | A block with px/min/max 0 initialized; root measured 0×0; ticks advanced 7→19; `ping` reached it | `carrier-layout.json`, `carrier-probe.js`, `carrier-initial.json`, `carrier-after-command.json`, `carrier-zero.png` |
| Hidden lifecycle/commands | Initially hidden copy had no published key. After hiding the initialized zero copy, ticks continued (19→36→57), but a command with no shown copies was refused. Showing the initially hidden copy initialized it; only that copy received `second-ping` | `carrier-hidden-command.json`, `carrier-after-show.json`, `carrier-final.json` |
| Durable read safety | Journal SHA-256 unchanged; inbox cursor 0, last_seq 12 and pending event preserved; zero terminals, same private core PID/identity | `journal-{before,after}.sha256`, `inbox{,-after}.json`, `terminals-{before,after}.json`, `core-{start,after}.json` |

The carrier's `unknown` key is a preliminary check/discovery copy, not a hidden
layout producer: its tick count stayed at 15. The layout probes use only the
separate `zero` and `hidden` keys.

These are observations for this host version, not proof of extracted packages.
Hot removal can retain an old mod instance; the fresh-layout tests additionally
prove that unrelated surfaces work without loading the absent mod. Neither
behavior promises an absent component will still work or that its slot vanishes.
Selected required artifacts must still cause manager refusal; intentionally
omitted components require structural layout generation in P4.

## Decision and rejected alternatives

Proceed with the approved phased vertical extraction, retaining the current
working visible composition until P4's complete gate. Missing-mod isolation is
not a host blocker in this baseline. A shown zero-space source carrier is a
viable implementation candidate, but real coordinator/usage lifecycle, commands,
width, generation validation and the both/each/neither matrix remain P4 gates.

Rejected: JS-only renaming while keeping the suite backend/updater dependency
(fails the primary other-layout requirement); a mandatory kit-core (no proven
nontrivial shared policy); hidden-never-shown producer nodes (observably do not
initialize); moving lifecycle into the bar (violates package independence).
No approved-plan deviation was necessary for P0.

## Release controls and rollback

P0 is documentation/evidence only and keeps the existing suite deployable.
Before P1 or any incompatible source-format commit is merged into the trusted
source, the person must freeze source intake using the existing `source --repo`
against a known-working committed snapshot and wait for manager work to finish.
No live source freeze was performed by this delegate; coordinator/person
confirmation is a prerequisite to the P1 merge/deploy request. Do not assume the
maintenance timer is not due. Bootstrap the first new format with the verified
new manager, not the old installed builder.

Before P2 live activation, drain delegates/inbox, persist open threads and
instructions, and have the person run old `herdr-kit coord down`. Only then
activate the matching new package set and have the person run `kit-coord up`.
Rollback is the reverse drained transition to the complete retained prior set.
The delegate never stops/restarts/re-execs the live coordinator or watcher.

The concurrent chat mod must be incorporated from main before a relevant phase
merge. Source/spec conflicts or changes to these decisions require a question
before deviation. P0 itself has no command-name, watcher, report-footer or role
path cutover.
