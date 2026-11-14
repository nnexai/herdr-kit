//! Post-activation, receipt-scoped collection using readable process paths.
use super::*;
use std::collections::BTreeSet;

pub(super) fn process_paths() -> Result<Vec<PathBuf>> {
    #[cfg(target_os = "linux")]
    {
        process_paths_at(Path::new("/proc"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // lsof exposes executable text and cwd paths on macOS.
        let stdout = tempfile::tempfile()?;
        let stderr = tempfile::tempfile()?;
        let status = bounded(
            Command::new("lsof")
                .args(["-n", "-P", "-F0n", "-d", "cwd,txt"])
                .stdout(stdout.try_clone()?)
                .stderr(stderr.try_clone()?),
            30,
        )?;
        ensure!(
            status.success() && stderr.metadata()?.len() == 0,
            "process census failed"
        );
        use std::io::{Read, Seek, SeekFrom};
        let mut stdout = stdout;
        stdout.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes)?;
        Ok(bytes
            .split(|byte| *byte == 0)
            .filter_map(|field| {
                let name = field.strip_prefix(b"n/")?;
                let name = std::str::from_utf8(name).ok()?;
                Some(Path::new("/").join(name))
            })
            .collect())
    }
}

#[cfg(target_os = "linux")]
fn process_paths_at(proc_root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(proc_root)? {
        let entry = entry?;
        if !entry
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        for field in ["exe", "cwd"] {
            if let Ok(path) = fs::read_link(entry.path().join(field)) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

fn layout_strings(home: &Path) -> Result<Vec<String>> {
    fn visit(path: &Path, strings: &mut Vec<String>) -> Result<()> {
        no_links(path)?;
        if !path.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            no_links(&path)?;
            if entry.file_type()?.is_dir() {
                visit(&path, strings)?;
            } else if path.extension().is_some_and(|ext| ext == "json") {
                fn collect(value: Value, strings: &mut Vec<String>) {
                    match value {
                        Value::String(value) => strings.push(value),
                        Value::Array(values) => {
                            for value in values {
                                collect(value, strings);
                            }
                        }
                        Value::Object(values) => {
                            for (key, value) in values {
                                strings.push(key);
                                collect(value, strings);
                            }
                        }
                        _ => (),
                    }
                }
                collect(serde_json::from_slice(&fs::read(path)?)?, strings);
            }
        }
        Ok(())
    }
    let mut strings = Vec::new();
    visit(&home.join("layouts"), &mut strings)?;
    Ok(strings)
}

fn snapshot(root: &Path) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
        no_links(dir)?;
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            no_links(&path)?;
            if entry.file_type()?.is_dir() {
                visit(root, &path, files)?;
            } else {
                ensure!(entry.file_type()?.is_file(), "unsupported cleanup entry");
                files.insert(
                    path.strip_prefix(root)?
                        .to_str()
                        .context("non-UTF8 cleanup path")?
                        .to_owned(),
                    file_hash(&path)?.context("missing cleanup file")?,
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

impl Locked {
    /// Superseded generations that are provably unused and unmodified. One
    /// that fails any check is retained for a later sweep, not reported.
    pub(super) fn generation_plan(
        &self,
        state: &State,
        processes: &[PathBuf],
    ) -> Vec<GenerationCleanup> {
        let Some(home) = &state.app_home else {
            return Vec::new();
        };
        let Ok(layouts) = layout_strings(home) else {
            return Vec::new();
        };
        state
            .package_releases
            .iter()
            .filter(|(rev, _)| !state.selection.values().any(|selected| selected == *rev))
            .filter(|(_, packages)| packages.values().any(|receipt| receipt.activated))
            .filter_map(|(rev, packages)| {
                self.unused_generation(state, home, rev, packages, &layouts, processes)
                    .ok()
            })
            .collect()
    }

    /// Receipts, not directory naming, grant deletion authority.
    fn unused_generation(
        &self,
        state: &State,
        home: &Path,
        rev: &str,
        packages: &BTreeMap<String, PackageReceipt>,
        layouts: &[String],
        processes: &[PathBuf],
    ) -> Result<GenerationCleanup> {
        let release_root = release(&self.data, rev);
        let mut expected = BTreeMap::new();
        let mut roots = BTreeMap::new();
        let mut mods = Vec::new();
        let mut names = BTreeSet::new();
        for receipt in packages.values() {
            if let Some(binary) = &receipt.definition.binary {
                expected.insert(
                    binary.name.clone(),
                    receipt.binary_hash.clone().context("missing binary hash")?,
                );
            }
            if let Some(hash) = &receipt.layout_hash {
                expected.insert(LAYOUT_SOURCE.to_owned(), hash.clone());
            }
            for (base, hashes) in &receipt.mods {
                for (path, hash) in hashes {
                    expected.insert(format!("mods/{base}/{path}"), hash.clone());
                }
                let target = mod_path(home, base, rev);
                if let Some(owned) = state.mod_owners.get(&target) {
                    ensure!(owned == hashes, "mod ownership mismatch");
                    if target.exists() {
                        ensure!(&tree(&target)? == hashes, "superseded mod drift");
                    }
                    roots.insert(target.clone(), snapshot(&target)?);
                    let cache = home.join("data").join(name(base, rev));
                    roots.insert(cache.clone(), snapshot(&cache)?);
                    names.insert(name(base, rev));
                    mods.push(target);
                }
            }
        }
        if release_root.exists() {
            ensure!(
                tree(&release_root)? == expected,
                "superseded release drift or unowned files"
            );
        }
        roots.insert(release_root.clone(), snapshot(&release_root)?);
        ensure!(
            !layouts
                .iter()
                .any(|text| names.iter().any(|name| text.contains(name))
                    || text.contains(release_root.to_string_lossy().as_ref())),
            "generation referenced by layout"
        );
        ensure!(
            !processes
                .iter()
                .any(|path| roots.keys().any(|root| path.starts_with(root))),
            "generation in use by process"
        );
        Ok(GenerationCleanup {
            revision: rev.to_owned(),
            roots,
            mods,
        })
    }

    pub(super) fn finish_generations(
        &self,
        transaction: &Transaction,
        processes: &[PathBuf],
    ) -> Result<State> {
        let mut state = transaction.state.clone();
        let home = state.app_home.as_ref().context("missing app home")?;
        let plan = transaction
            .cleanup
            .as_ref()
            .context("missing cleanup phase")?;
        if !plan.is_empty() {
            let layouts = layout_strings(home)?;
            // Preflight the entire deletion set before removing its first file.
            for generation in plan {
                ensure!(
                    !processes
                        .iter()
                        .any(|path| generation.roots.keys().any(|root| path.starts_with(root))),
                    "generation became busy; cleanup journal retained"
                );
                let release_root = release(&self.data, &generation.revision);
                ensure!(
                    !layouts
                        .iter()
                        .any(|text| generation.mods.iter().any(|path| path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| text.contains(name)))
                            || text.contains(release_root.to_string_lossy().as_ref())),
                    "generation acquired a layout reference; cleanup journal retained"
                );
                for (root, expected) in &generation.roots {
                    let current = snapshot(root)?;
                    ensure!(
                        current
                            .iter()
                            .all(|(path, hash)| expected.get(path) == Some(hash)),
                        "cleanup target drift; journal retained: {}",
                        root.display()
                    );
                }
            }
        }
        for generation in plan {
            for root in generation.roots.keys() {
                no_links(root)?;
                if root.exists() {
                    fs::remove_dir_all(root)?;
                    File::open(root.parent().context("missing cleanup parent")?)?.sync_all()?;
                }
            }
            for target in &generation.mods {
                state.mod_owners.remove(target);
            }
            state.package_releases.remove(&generation.revision);
        }
        Ok(state)
    }

    /// Activate nothing; the replay still collects superseded generations.
    pub(super) fn sweep_generations(&self, state: State) -> Result<State> {
        self.activate(Transaction::maintenance(state))
    }
}

#[cfg(test)]
mod tests;
