# P1: workspace and selected-artifact manager

Status: branch development authorized; no main merge or live deployment authorized.
P0 is accepted and remains on `coord/monorepo`. Freeze is a **P1 merge/deploy**
prerequisite, not a development prerequisite.

## Implementation decision

`crates/kit-manager` owns the existing `herdr-kit` binary and, temporarily,
its direct legacy coordinator/usage engines. The root is a Cargo workspace with
one lockfile and shared third-party versions, not kit-core. P2/P3 remove the
corresponding legacy commands when their complete vertical replacements land.
`kit-packages.json` is the versioned committed-source artifact catalog and bundle.

A package declares its executable (Cargo package + binary), self-contained mods,
and optionally the sole manager-owned herdr-kit layout. Component-only install
never claims or modifies a consumer layout. The installer returns exact resolved
mod generations, roots and selected package revisions. `--revision` accepts a
full lowercase commit ID; dirty source is never built.

Selected revisions and complete previous selections replace the old one-current
assumption. Existing `current_revision`/`previous_revision` output remains useful
for the suite; `components`, `previous_components` and `resolved_mods` make package
selection explicit. Immutable generation assets remain under `releases/<rev>`;
mods remain full-revision named. Generated owned layouts are immutable,
content-addressed deployment assets and are selected structurally. Runtime
injection replaces only runtime.js, never protocol literals in arbitrary JS.

Canonical schema-1 package history lives at `R/package-state.json`: `selection`,
`previous_selection`, `package_releases`, `files` and `mod_owners`. Root
`state.json` retains the legacy source/app-home/current/previous/release/
maintenance envelope, which retained old binaries may write without discarding
new ownership metadata. A deployment snapshot is restored only when its
current/previous revisions match that envelope; historical package ownership
survives even if an old legacy updater advances its revision. Activation
publishes canonical history before the legacy envelope under the existing
journal, with crash-prefix coverage for both metadata writes. Per-package
receipts bind executable hashes, complete mod trees, source layout hashes and
the committed package definition. An `activated` receipt bit distinguishes
retained published assets from incomplete, never-activated staging: missing
retained files are drift, never silently rebuilt. Legacy release directories
are never augmented; component migration uses a new committed revision, while
an unchanged legacy full-suite install remains a no-op. File receipts bind immutable source, hash,
executable mode and readiness ordering. Schema-2 activation journals list each
file's old/new hash and complete mod publications plus the next durable state.
Recovery accepts only old or new expected bytes and refuses all known drift
before publishing targets; readiness files are last. Old one-binary activation
journals are still parsed and recovered. Rollback restores the entire previous
selection, including reverse activation of retained legacy releases; it does not
prune immutable generations or rewrite another layout's pins.

Rejected root-state-only package metadata after an actual private old CLI
`source` write silently discarded its unknown fields, causing the new manager
to refuse a legitimate installed component with `missing legacy release receipt`.
The separate canonical history fixes the ownership loss without command aliases,
rewriting old binaries, or importing capability logic into a shared service.

P1 still has one direct legacy executable. Components sharing it cannot activate
mismatched generations: update that shared selected set together. This temporary
constraint is removed by the P2/P3 executable cutovers, not a compatibility alias.
Capability-only selection in P1 is a packaging gate, **not** a claim that final
standalone kit-coord or kit-usage already exists. No actors need starting for P1.

Rejected: retaining hard-coded two-mod full-suite builds (cannot honor selected
ownership); rewriting all JS strings (changes public shared-state keys);
introducing kit-core or another updater (unnecessary runtime coupling); assuming
an old builder can consume workspace source (cannot safely package it).

## Required merge/deployment authorization

Before any P0+P1 merge into the live trusted main checkout:

1. **The person freezes trusted source intake.** Choose the exact currently
   working installed `current_revision` from read-only
   `~/.local/share/herdr-kit/bin/herdr-kit status --json` (use the actual data root
   on another platform). Create a separate committed-source snapshot at it, not
   a worktree or a moving main branch, for example:

   ```sh
   git clone --no-hardlinks --no-checkout /path/to/herdr-kit \
     /home/user/.local/share/herdr-kit/source-freeze-p1
   git -C /home/user/.local/share/herdr-kit/source-freeze-p1 \
     checkout --detach <known-working-full-current-revision>
   /home/user/.local/share/herdr-kit/bin/herdr-kit source \
     --repo /home/user/.local/share/herdr-kit/source-freeze-p1
   ```

   Use a new snapshot directory; do not overwrite an existing one. The source
   command waits for the manager lock, resets scheduling and records only the
   trusted repository setting. Its frozen unchanged HEAD stays safe even when
   maintenance next becomes due. Do not hand-edit manager state, rely on a
   timer not being due, or repoint the old builder at incompatible main early.
   The coordinator can record the person's choice/confirmation; the freeze is
   person-owned and must not be performed by a delegate without authorization.

2. **Confirm manager work has finished.** The successfully returned `source`
   command is lock-serialized after prior manager work. Immediately before
   cutover, a nonblocking lock check can establish that no operation currently
   holds the lock:

   ```sh
   flock -n /home/user/.local/share/herdr-kit/manager.lock true
   ```

   A refusal means wait for the owning operation, not stop it. The freeze must
   stay in place throughout the merge/build/bootstrap window. A coordinator can
   record this evidence; it is not permission to stop/re-exec the live watcher.

3. **Choose a quiet merge owner/time.** Main is the live checkout used by the
   kit-coord-ui delegate. No merge there while it has uncommitted files, including
   CHANGELOG.md. Continue merging committed main into `coord/monorepo` during
   development, incorporate kit-coord-ui as its own package when available, and
   resolve/reverify the affected integration before handing over. The coordinator
   chooses a quiet moment or asks that checkout's owner to merge P0+P1. The main
   checkout owner checks its own changes are committed/preserved, then performs
   the agreed merge of `coord/monorepo`; no stash/reset, forced checkout or
   overwrite by this delegate. Do not modify the live checkout from this branch.

4. **Bootstrap only with the verified new manager**, after explicit deployment
   permission and normal read-only source/app-home/drift preflight. The old
   installed updater must not build the first workspace generation. From the
   merged committed source, the authorized deployment owner builds and runs:

   ```sh
   cargo build --locked --release -p kit-manager
   ./target/release/herdr-kit install --source /path/to/herdr-kit \
     --app-home /home/user/.config/herdr-app
   ```

   That installer atomically replaces only owned files and records matching
   selections. It deliberately re-registers trusted source from the frozen
   snapshot to the now-compatible main. Subsequent updates use the capable
   installed updater. Preserve the snapshot/retained release for rollback.
   P1 retains all live command, watcher exec, generated role and report paths;
   P2's person-drained namespace transition is a separate future gate. Never
   manually restart, stop or re-exec the live coordinator or watcher.

## Gate evidence

P1 acceptance requires committed-source private install/check/apply/rollback,
old receipt/journal migration, selected-only ownership, preservation/refusal of
edited or unowned bytes, and multi-artifact crash-prefix recovery. Evidence and
observed results are recorded in the phase report under ignored
`evidence/monorepo-p1/`; no live deployment is implied by branch verification.
