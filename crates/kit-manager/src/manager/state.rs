//! The legacy envelope remains writable by retained pre-workspace binaries.
//! Package history is canonical here, never in unknown fields an old writer drops.
use super::*;

#[derive(Deserialize)]
struct PackageState {
    schema_version: u32,
    current: Option<String>,
    previous: Option<String>,
    selection: BTreeMap<String, String>,
    previous_selection: Option<BTreeMap<String, String>>,
    package_releases: BTreeMap<String, BTreeMap<String, PackageReceipt>>,
    files: BTreeMap<PathBuf, FileReceipt>,
    mod_owners: BTreeMap<PathBuf, BTreeMap<String, String>>,
}

#[derive(Serialize)]
struct PackageStateRef<'a> {
    schema_version: u32,
    current: &'a Option<String>,
    previous: &'a Option<String>,
    selection: &'a BTreeMap<String, String>,
    previous_selection: &'a Option<BTreeMap<String, String>>,
    package_releases: &'a BTreeMap<String, BTreeMap<String, PackageReceipt>>,
    files: &'a BTreeMap<PathBuf, FileReceipt>,
    mod_owners: &'a BTreeMap<PathBuf, BTreeMap<String, String>>,
}

#[derive(Serialize)]
struct LegacyStateRef<'a> {
    source: &'a Option<PathBuf>,
    app_home: &'a Option<PathBuf>,
    current: &'a Option<String>,
    previous: &'a Option<String>,
    releases: &'a BTreeMap<String, BTreeMap<String, String>>,
    managed_mods: &'a Vec<String>,
    next_check: u64,
}

fn path(data: &Path) -> PathBuf {
    data.join("package-state.json")
}

pub(super) fn merge_history(data: &Path, mut state: State) -> Result<State> {
    if let Some(history) = read_json::<PackageState>(&path(data))? {
        ensure!(
            history.schema_version == 1,
            "unsupported package state schema"
        );
        // Old maintenance can advance the legacy revision. Never mistake a
        // historical deployment snapshot for that new active selection.
        if state.current == history.current && state.previous == history.previous {
            state.selection = history.selection;
            state.previous_selection = history.previous_selection;
            state.files = history.files;
        }
        state.package_releases = history.package_releases;
        state.mod_owners = history.mod_owners;
    }
    Ok(state)
}

pub(super) fn load(data: &Path) -> Result<State> {
    merge_history(data, read_json(&state_path(data))?.unwrap_or_default())
}

pub(super) fn store_history(data: &Path, state: &State) -> Result<()> {
    super::save(
        &path(data),
        &PackageStateRef {
            schema_version: 1,
            current: &state.current,
            previous: &state.previous,
            selection: &state.selection,
            previous_selection: &state.previous_selection,
            package_releases: &state.package_releases,
            files: &state.files,
            mod_owners: &state.mod_owners,
        },
    )
}

pub(super) fn store_legacy(data: &Path, state: &State) -> Result<()> {
    super::save(
        &state_path(data),
        &LegacyStateRef {
            source: &state.source,
            app_home: &state.app_home,
            current: &state.current,
            previous: &state.previous,
            releases: &state.releases,
            managed_mods: &state.managed_mods,
            next_check: state.next_check,
        },
    )
}

pub(super) fn store(data: &Path, state: &State) -> Result<()> {
    // The activation journal stays authoritative across a crash between these
    // two atomic metadata publications. History first preserves staging receipts.
    store_history(data, state)?;
    store_legacy(data, state)
}

