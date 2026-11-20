# Integrating the coordinator

The coordinator can run in another Herdr layout without the herdr-kit layout,
bar, or usage mod. Integration is **moderate**: the packages are independent,
but the consumer must install and pin their runtime artifacts, wire the layout,
and provide a working Herdr and OMP environment.

1. Provide Herdr core, `herdr-app`, `hc`, and `omp` on the runtime PATH. The
   built-in OMP roles use the model roles `@plan`, `@task`, `@smol`, `@slow`,
   and `@design`; configure the roles you intend to use. Built-in coordinator
   roles are embedded in the binary; optional overrides live in
   `<data-dir>/coord/roles/`.
2. From a committed checkout, build the optional manager and select the desired
   component for an existing consumer app home and a dedicated absolute data
   directory:

   ```sh
   cargo build --locked --release -p kit-manager
   REV=$(git rev-parse HEAD)
   ./target/release/herdr-kit --data-dir /absolute/data-dir install \
     --source "$PWD" --revision "$REV" --component kit-coord \
     --app-home /absolute/app-home
   ```

   For the optional Coordinator dialog launcher, select `kit-coord-ui` instead; its
   `kit-coord` dependency is included at the same revision. The response's
   `resolved_mods` gives the exact installed mod names. The manager does not
   change the consumer layout or install itself as an updater.
3. Add a visible coordinator status block to the consumer layout, substituting
   the returned coordinator mod name:

   ```json
   {"type":"block","size":{"px":80},"content":{"mod":"kit-coord-FULL_COMMIT","props":{"view":"statusbar"}}}
   ```

   It owns polling and registered commands; clicking the running standalone
   strip focuses the coordinator terminal. For the Coordinator dialog, also
   mount a visible block with `content.mod` set to the returned `kit-coord-ui`
   name and `props.view` set to `launcher`. Keep the coordinator block mounted
   for its actions. The layout owner activates their own layout.
4. As the person, start the coordinator and its watcher and inspect status:

   ```sh
   /absolute/data-dir/bin/kit-coord up
   /absolute/data-dir/bin/kit-coord status --json
   ```

   Coordinator journal, configuration, sessions, and role overrides are under
   `<data-dir>/coord/`. `KIT_COORD_DATA_DIR`, `KIT_COORD_HC`, and
   `KIT_COORD_HERDR_APP` override the default data root and executable discovery;
   use the same data root for CLI and installed mods.

## Caveats

- The native mods require Herdr's `herdr`, `gpui-kit`, and `gpui-base` APIs;
  they are not generic web components. The coordinator uses the Herdr core,
  `hc`, and `herdr-app` APIs and a visible watcher terminal.
- `kit-core` is a build-time Rust dependency, not an installable runtime
  package. Source builds need the workspace manifest, `Cargo.lock`, and
  `crates/kit-core`; `kit-packages.json` declares the runtime artifacts.
- Pins are revision-specific. The consumer owns layout changes on upgrades;
  the dialog and coordinator must remain on the same selected revision.
  Capability-only installs do not schedule manager maintenance.
- No hard dependency on the suite layout, bar, or usage mod was found in this
  quick source review. The shared-state keys retain `herdr-kit` names; custom
  bar composition must consume the coordinator readout or use the status block
  directly.

For manual source exports and generated runtime bindings, see the
[standalone package guide](../../docs/coordinator/monorepo-p2.md).
