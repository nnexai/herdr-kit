//! Journaled activation of a component selection. `prepare` plans every owned
//! file and mod change, `activate` records the plan in `activation.json`, and
//! recovery replays it. Each step tolerates having already run, so a crash at
//! any point resumes from the journal under the next manager lock.
use super::*;
use std::collections::BTreeSet;

impl Locked {
    /// Plan the changes that make `bin/` and the app home match `selection`.
    /// Owned targets are only inspected; generated layout and descriptor
    /// content is stored content-addressed under `deployments/`.
    pub(super) fn prepare(
        &self,
        old: &State,
        mut next: State,
        selection: BTreeMap<String, String>,
    ) -> Result<Transaction> {
        self.verify_active(old)?;
        let home = next.app_home.clone().context("missing target")?;
        next.selection = selection;
        next.files.clear();
        let mut files = BTreeMap::<PathBuf, FileReceipt>::new();
        let mut mods = Vec::new();
        let mut bindings = BTreeMap::new();
        let mut layout_source = None;
        for (_, rev, receipt) in self.selected_receipts(&next)? {
            let root = release(&self.data, rev);
            if let Some(binary) = &receipt.definition.binary {
                let source = root.join(&binary.name);
                let file = FileReceipt {
                    hash: receipt.binary_hash.clone().context("missing binary hash")?,
                    source: source.clone(),
                    executable: true,
                    ready: false,
                };
                let target = self.data.join("bin").join(&binary.name);
                if let Some(other) = files.insert(target, file) {
                    ensure!(
                        other.source == source,
                        "selected binaries have multiple owners"
                    );
                }
            }
            for (base, hashes) in &receipt.mods {
                bindings.insert(base.clone(), name(base, rev));
                mods.push(ModChange {
                    target: mod_path(&home, base, rev),
                    source: root.join("mods").join(base),
                    hashes: hashes.clone(),
                });
            }
            if receipt.definition.layout.is_some() {
                layout_source = Some(root.join(LAYOUT_SOURCE));
            }
        }
        if let Some(source) = layout_source {
            files.insert(
                layout(&home),
                self.projected_layout(&next, &source, &bindings)?,
            );
        }
        if let Some(descriptor) = self.coordinator_descriptor(&next)? {
            files.insert(self.data.join("kit-coord/active.json"), descriptor);
        }
        next.files = files;
        for change in &mods {
            no_links(&change.target)?;
            if change.target.exists() {
                ensure!(
                    old.mod_owners.contains_key(&change.target),
                    "mod target already exists without ownership"
                );
                ensure!(tree(&change.target)? == change.hashes, "mod drift detected");
            }
            next.mod_owners
                .insert(change.target.clone(), change.hashes.clone());
        }
        let files = file_changes(&old.files, &next.files)?;
        for (id, rev) in &next.selection {
            next.package_releases
                .get_mut(rev)
                .and_then(|r| r.get_mut(id))
                .context("missing selected package receipt")?
                .activated = true;
        }
        next.current = next
            .selection
            .get("kit-manager")
            .or_else(|| next.selection.get("herdr-kit"))
            .or_else(|| next.selection.values().next())
            .cloned();
        let deployments = self.data.join("deployments");
        let obsolete_deployments = old
            .files
            .values()
            .filter(|file| {
                file.source.starts_with(&deployments)
                    && !next.files.values().any(|next| next.source == file.source)
            })
            .map(|file| (file.source.clone(), file.hash.clone()))
            .collect();
        Ok(Transaction {
            schema_version: 1,
            state: next,
            files,
            mods,
            obsolete_deployments,
            cleanup: None,
        })
    }

    /// The release layout with its mod references pinned to the selected generations.
    fn projected_layout(
        &self,
        next: &State,
        source: &Path,
        bindings: &BTreeMap<String, String>,
    ) -> Result<FileReceipt> {
        let mut value: Value = serde_json::from_slice(&fs::read(source)?)?;
        let mut known = BTreeSet::from_iter(
            ["kit-coord", "kit-coord-ui", "kit-usage", "herdr-kit-bar"].map(str::to_owned),
        );
        for packages in next.package_releases.values() {
            for package in packages.values() {
                known.extend(package.mods.keys().cloned());
            }
        }
        let manager = next
            .selection
            .contains_key("kit-manager")
            .then(|| bootstrap(&self.data));
        project_layout(&mut value, &known, bindings, manager.as_deref(), &self.data);
        self.publish_deployment(
            "herdr-kit.json",
            &serde_json::to_vec_pretty(&value)?,
            false,
            "deployment layout drift detected",
        )
    }

    /// The public readiness descriptor naming the selected coordinator executable.
    fn coordinator_descriptor(&self, next: &State) -> Result<Option<FileReceipt>> {
        let Some(rev) = next.selection.get("kit-coord") else {
            return Ok(None);
        };
        let receipt = next
            .receipt("kit-coord", rev)
            .context("missing selected coordinator receipt")?;
        let has_executable = receipt
            .definition
            .binary
            .as_ref()
            .is_some_and(|binary| binary.name == "kit-coord");
        if !has_executable {
            return Ok(None);
        }
        let bytes = serde_json::to_vec(&json!({
            "schema_version": 1,
            "revision": rev,
            "executable": release(&self.data, rev).join("kit-coord"),
        }))?;
        self.publish_deployment(
            "kit-coord-active.json",
            &bytes,
            true,
            "coordinator descriptor drift detected",
        )
        .map(Some)
    }

    /// Store generated content as `deployments/<hash>/<file_name>`; activation
    /// copies it to its target like any other release file.
    fn publish_deployment(
        &self,
        file_name: &str,
        bytes: &[u8],
        ready: bool,
        drift: &'static str,
    ) -> Result<FileReceipt> {
        let digest = hash(bytes);
        let source = self.data.join("deployments").join(&digest).join(file_name);
        if let Some(existing) = file_hash(&source)? {
            ensure!(existing == digest, drift);
        } else {
            atomic(&source, bytes, false)?;
        }
        Ok(FileReceipt {
            source,
            hash: digest,
            executable: false,
            ready,
        })
    }

    pub(super) fn activate(&self, transaction: Transaction) -> Result<State> {
        save(&journal_path(&self.data), &transaction)?;
        self.recover()?;
        self.state()
    }

    pub(super) fn recover(&self) -> Result<()> {
        if !journal_path(&self.data).exists() {
            return Ok(());
        }
        self.recover_with_processes(generations::process_paths)
    }

    pub(super) fn recover_with_processes(
        &self,
        processes: impl Fn() -> Result<Vec<PathBuf>>,
    ) -> Result<()> {
        let Some(mut transaction): Option<Transaction> = read_json(&journal_path(&self.data))?
        else {
            return Ok(());
        };
        ensure!(
            transaction.schema_version == 1,
            "unsupported activation journal"
        );
        if transaction.cleanup.is_some() {
            // The targets were applied before the cleanup plan was journaled.
            self.verify_active(&transaction.state)?;
        } else {
            self.preflight(&transaction)?;
            self.apply(&transaction)?;
            remove_obsolete_deployments(&transaction)?;
            transaction.cleanup = Some(self.generation_plan(&transaction.state, &processes()?));
            save(&journal_path(&self.data), &transaction)?;
        }
        // Deletion uses a census taken after the plan became durable.
        let state = self.finish_generations(&transaction, &processes()?)?;
        self.save(&state)?;
        fs::remove_file(journal_path(&self.data))?;
        File::open(&self.data)?.sync_all()?;
        Ok(())
    }

    /// Refuse before the first write when a source or target is not what the plan recorded.
    fn preflight(&self, transaction: &Transaction) -> Result<()> {
        self.selected_receipts(&transaction.state)?;
        for change in &transaction.mods {
            mod_installed(change)?;
        }
        for change in &transaction.files {
            let current = file_hash(&change.target)?;
            ensure!(
                current == change.old_hash
                    || current == change.next.as_ref().map(|n| n.hash.clone()),
                "managed file collision or drift detected"
            );
            if let Some(next) = &change.next {
                ensure!(
                    file_hash(&next.source)?.as_deref() == Some(&next.hash),
                    "release file drift detected"
                );
            }
        }
        for (path, expected) in &transaction.obsolete_deployments {
            ensure!(
                file_hash(path)?
                    .as_ref()
                    .is_none_or(|actual| actual == expected),
                "superseded deployment drift detected"
            );
        }
        Ok(())
    }

    /// Mods first, then files, with readiness descriptors last; then the receipt.
    fn apply(&self, transaction: &Transaction) -> Result<()> {
        for change in &transaction.mods {
            apply_mod(change)?;
        }
        for ready in [false, true] {
            for change in &transaction.files {
                if change.next.as_ref().is_some_and(|n| n.ready) == ready {
                    apply_file(change)?;
                }
            }
        }
        self.save(&transaction.state)
    }
}

/// Replacements and additions for `next`, then removals of what only `old` owned.
fn file_changes(
    old: &BTreeMap<PathBuf, FileReceipt>,
    next: &BTreeMap<PathBuf, FileReceipt>,
) -> Result<Vec<FileChange>> {
    let mut changes = Vec::new();
    for (target, receipt) in next {
        let old_hash = old.get(target).map(|r| r.hash.clone());
        if old_hash.is_none() {
            ensure!(
                file_hash(target)?.is_none(),
                "file target already exists without ownership: {}",
                target.display()
            );
        }
        changes.push(FileChange {
            target: target.clone(),
            old_hash,
            next: Some(receipt.clone()),
        });
    }
    for (target, receipt) in old {
        if !next.contains_key(target) {
            changes.push(FileChange {
                target: target.clone(),
                old_hash: Some(receipt.hash.clone()),
                next: None,
            });
        }
    }
    Ok(changes)
}

/// Whether the target already holds exactly this mod. Any other content, or
/// a changed release source, refuses.
fn mod_installed(change: &ModChange) -> Result<bool> {
    ensure!(
        tree(&change.source)? == change.hashes,
        "release mod drift detected"
    );
    no_links(&change.target)?;
    if !change.target.exists() {
        return Ok(false);
    }
    ensure!(
        tree(&change.target)? == change.hashes,
        "mod collision or drift detected"
    );
    Ok(true)
}

/// Copy a mod into place through a sibling staging directory and one rename.
pub(super) fn apply_mod(change: &ModChange) -> Result<()> {
    if mod_installed(change)? {
        return Ok(());
    }
    let parent = change.target.parent().context("invalid mod target")?;
    directory(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".herdr-kit-")
        .tempdir_in(parent)?;
    copy_tree(&change.source, staging.path())?;
    ensure!(
        tree(staging.path())? == change.hashes,
        "mod staging mismatch"
    );
    fs::rename(staging.path(), &change.target)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn apply_file(change: &FileChange) -> Result<()> {
    let current = file_hash(&change.target)?;
    let next_hash = change.next.as_ref().map(|n| n.hash.clone());
    ensure!(
        current == change.old_hash || current == next_hash,
        "managed file collision or drift detected: {}",
        change.target.display()
    );
    if let Some(next) = &change.next {
        ensure!(
            file_hash(&next.source)?.as_deref() == Some(&next.hash),
            "release file drift detected"
        );
        if current != next_hash {
            atomic(&change.target, &fs::read(&next.source)?, next.executable)?;
        }
    } else if current.is_some() {
        fs::remove_file(&change.target)?;
        File::open(change.target.parent().context("missing parent")?)?.sync_all()?;
    }
    Ok(())
}

/// Delete superseded generated files and their then-empty hash directories.
fn remove_obsolete_deployments(transaction: &Transaction) -> Result<()> {
    for path in transaction.obsolete_deployments.keys() {
        if path.exists() {
            fs::remove_file(path)?;
        }
        let parent = path.parent().context("missing deployment parent")?;
        if parent.exists() {
            File::open(parent)?.sync_all()?;
            if fs::read_dir(parent)?.next().is_none() {
                fs::remove_dir(parent)?;
                File::open(parent.parent().context("missing deployments directory")?)?
                    .sync_all()?;
            }
        }
    }
    Ok(())
}
