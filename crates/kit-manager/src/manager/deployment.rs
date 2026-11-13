use super::*;

struct Locked {
    data: PathBuf,
    _lock: File,
}
impl Locked {
    fn open(path: &Path) -> Result<Self> {
        let data = absolute(path)?;
        directory(&data)?;
        let path = data.join("manager.lock");
        no_links(&path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => bail!("manager lock busy"),
                Err(e) => return Err(e.into()),
            }
        }
        let this = Self { data, _lock: lock };
        this.recover()?;
        Ok(this)
    }
    fn state(&self) -> Result<State> {
        state::load(&self.data)
    }
    fn save(&self, state: &State) -> Result<()> {
        state::store(&self.data, state)
    }
    fn verify_legacy(&self, state: &State, rev: &str) -> Result<()> {
        let expected = state
            .releases
            .get(rev)
            .context("missing legacy release receipt")?;
        ensure!(
            &tree(&release(&self.data, rev))? == expected,
            "release drift detected"
        );
        Ok(())
    }
    fn verify_package(&self, rev: &str, receipt: &PackageReceipt) -> Result<()> {
        let root = release(&self.data, rev);
        if let Some(binary) = &receipt.definition.binary {
            ensure!(
                file_hash(&root.join(&binary.name))? == receipt.binary_hash,
                "release binary drift detected"
            );
        }
        for (base, hashes) in &receipt.mods {
            ensure!(
                &tree(&root.join("mods").join(base))? == hashes,
                "release mod drift detected"
            );
        }
        if receipt.definition.layout.is_some() {
            ensure!(
                file_hash(&root.join("layout-source.json"))? == receipt.layout_hash,
                "release layout drift detected"
            );
        }
        Ok(())
    }
    fn selected_receipts<'a>(
        &self,
        state: &'a State,
    ) -> Result<Vec<(&'a str, &'a str, &'a PackageReceipt)>> {
        state
            .selection
            .iter()
            .map(|(id, rev)| {
                let receipt = state
                    .package_releases
                    .get(rev)
                    .and_then(|r| r.get(id))
                    .context("missing package receipt")?;
                for required in &receipt.definition.requires {
                    ensure!(state.selection.get(required) == Some(rev), "dependent component pins must be updated together: {id} requires {required}");
                }
                self.verify_package(rev, receipt)?;
                Ok((id.as_str(), rev.as_str(), receipt))
            })
            .collect()
    }
    fn legacy_files(&self, state: &State, rev: &str) -> Result<BTreeMap<PathBuf, FileReceipt>> {
        self.verify_legacy(state, rev)?;
        let home = state.app_home.as_ref().context("missing install target")?;
        let root = release(&self.data, rev);
        [
            (layout(home), root.join("layout.json"), false),
            (bootstrap(&self.data), root.join("herdr-kit"), true),
        ]
        .into_iter()
        .map(|(target, source, executable)| {
            Ok((
                target,
                FileReceipt {
                    hash: file_hash(&source)?.context("missing release file")?,
                    source,
                    executable,
                    ready: false,
                },
            ))
        })
        .collect()
    }
    fn active_files(&self, state: &State) -> Result<BTreeMap<PathBuf, FileReceipt>> {
        if state.selection.is_empty() && state.files.is_empty() {
            if let Some(rev) = &state.current {
                return self.legacy_files(state, rev);
            }
        }
        Ok(state.files.clone())
    }
    fn verify_active(&self, state: &State) -> Result<()> {
        for (target, receipt) in self.active_files(state)? {
            ensure!(
                file_hash(&target)?.as_deref() == Some(&receipt.hash),
                "managed file drift detected: {}",
                target.display()
            );
            ensure!(
                file_hash(&receipt.source)?.as_deref() == Some(&receipt.hash),
                "release file drift detected"
            );
        }
        if state.selection.is_empty() {
            if let Some(rev) = &state.current {
                self.verify_legacy(state, rev)?;
                for (base, source) in release_mods(&release(&self.data, rev))? {
                    ensure!(
                        tree(&mod_path(
                            state.app_home.as_ref().context("missing target")?,
                            &base,
                            rev
                        ))? == tree(&source)?,
                        "mod drift detected"
                    );
                }
            }
        } else {
            for (_, rev, receipt) in self.selected_receipts(state)? {
                for (base, hashes) in &receipt.mods {
                    ensure!(
                        &tree(&mod_path(
                            state.app_home.as_ref().context("missing target")?,
                            base,
                            rev
                        ))? == hashes,
                        "mod drift detected"
                    );
                }
            }
        }
        Ok(())
    }
    fn owns_mod(&self, state: &State, target: &Path) -> Result<bool> {
        if state.mod_owners.contains_key(target) {
            return Ok(true);
        }
        if let Some(home) = &state.app_home {
            for rev in &state.managed_mods {
                self.verify_legacy(state, rev)?;
                if release_mods(&release(&self.data, rev))?
                    .iter()
                    .any(|(base, _)| mod_path(home, base, rev) == target)
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn apply_mod(&self, change: &ModChange) -> Result<()> {
        ensure!(
            tree(&change.source)? == change.hashes,
            "release mod drift detected"
        );
        no_links(&change.target)?;
        if change.target.exists() {
            ensure!(
                tree(&change.target)? == change.hashes,
                "mod collision or drift detected"
            );
        } else {
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
        }
        Ok(())
    }
    fn apply_file(&self, change: &FileChange) -> Result<()> {
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
    fn recover(&self) -> Result<()> {
        let Some(journal): Option<Journal> = read_json(&journal_path(&self.data))? else {
            return Ok(());
        };
        match journal {
            Journal::Components(transaction) => {
                ensure!(
                    transaction.schema_version == 2,
                    "unsupported activation journal"
                );
                if !transaction.state.selection.is_empty() {
                    self.selected_receipts(&transaction.state)?;
                }
                // Refuse drift anywhere before publishing the first target.
                for change in &transaction.mods {
                    ensure!(
                        tree(&change.source)? == change.hashes,
                        "release mod drift detected"
                    );
                    no_links(&change.target)?;
                    if change.target.exists() {
                        ensure!(
                            tree(&change.target)? == change.hashes,
                            "mod collision or drift detected"
                        );
                    }
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
                for change in &transaction.mods {
                    self.apply_mod(change)?;
                }
                for ready in [false, true] {
                    for change in &transaction.files {
                        if change.next.as_ref().is_some_and(|n| n.ready) == ready {
                            self.apply_file(change)?;
                        }
                    }
                }
                self.save(&transaction.state)?;
            }
            Journal::Legacy(journal) => {
                let state = state::merge_history(&self.data, journal.state)?;
                let state = &state;
                let rev = state
                    .current
                    .as_ref()
                    .context("invalid legacy activation journal")?;
                self.verify_legacy(state, rev)?;
                let home = state.app_home.as_ref().context("missing legacy target")?;
                let files = self.legacy_files(state, rev)?;
                // Validate all legacy targets before resuming its old transaction.
                for (target, next) in &files {
                    let old = if *target == layout(home) {
                        &journal.old_layout
                    } else {
                        &journal.old_binary
                    };
                    let actual = file_hash(target)?;
                    ensure!(
                        &actual == old || actual.as_ref() == Some(&next.hash),
                        "legacy target drift detected"
                    );
                }
                let mods = release_mods(&release(&self.data, rev))?;
                for (base, source) in &mods {
                    let target = mod_path(home, base, rev);
                    no_links(&target)?;
                    if target.exists() {
                        ensure!(tree(&target)? == tree(source)?, "legacy mod drift detected");
                    }
                }
                for (base, source) in mods {
                    self.apply_mod(&ModChange {
                        target: mod_path(home, &base, rev),
                        hashes: tree(&source)?,
                        source,
                    })?;
                }
                for (target, next) in files {
                    let old_hash = if target == layout(home) {
                        journal.old_layout.clone()
                    } else {
                        journal.old_binary.clone()
                    };
                    self.apply_file(&FileChange {
                        target,
                        old_hash,
                        next: Some(next),
                    })?;
                }
                self.save(state)?;
            }
        }
        fs::remove_file(journal_path(&self.data))?;
        File::open(&self.data)?.sync_all()?;
        Ok(())
    }
    fn prepare(
        &self,
        old: &State,
        mut next: State,
        selection: BTreeMap<String, String>,
        legacy: Option<&str>,
    ) -> Result<Transaction> {
        self.verify_active(old)?;
        let home = next.app_home.clone().context("missing target")?;
        let old_files = self.active_files(old)?;
        next.selection = selection;
        next.files.clear();
        let mut mods = Vec::new();
        if let Some(rev) = legacy {
            next.files = self.legacy_files(&next, rev)?;
            for (base, source) in release_mods(&release(&self.data, rev))? {
                mods.push(ModChange {
                    target: mod_path(&home, &base, rev),
                    hashes: tree(&source)?,
                    source,
                });
            }
        } else {
            let mut bindings = BTreeMap::new();
            let mut layout_source = None;
            let mut files = BTreeMap::<PathBuf, FileReceipt>::new();
            for (_, rev, receipt) in self.selected_receipts(&next)? {
                if let Some(binary) = &receipt.definition.binary {
                    let source = release(&self.data, rev).join(&binary.name);
                    let target = self.data.join("bin").join(&binary.name);
                    let file = FileReceipt {
                        hash: receipt.binary_hash.clone().context("missing binary hash")?,
                        source,
                        executable: true,
                        ready: false,
                    };
                    if let Some(other) = files.insert(target, file.clone()) {
                        ensure!(
                            other.source == file.source,
                            "selected generations share a legacy binary; update them together until their vertical extraction"
                        );
                    }
                }
                for (base, hashes) in &receipt.mods {
                    bindings.insert(base.clone(), name(base, rev));
                    mods.push(ModChange {
                        target: mod_path(&home, base, rev),
                        source: release(&self.data, rev).join("mods").join(base),
                        hashes: hashes.clone(),
                    });
                }
                if receipt.definition.layout.is_some() {
                    layout_source = Some(release(&self.data, rev).join("layout-source.json"));
                }
            }
            if let Some(source) = layout_source {
                let mut value: Value = serde_json::from_slice(&fs::read(source)?)?;
                let mut known = std::collections::BTreeSet::from_iter(
                    LEGACY_MODS.iter().map(|s| s.to_string()),
                );
                for packages in next.package_releases.values() {
                    for package in packages.values() {
                        known.extend(package.mods.keys().cloned());
                    }
                }
                project_layout(&mut value, &known, &bindings);
                let bytes = serde_json::to_vec_pretty(&value)?;
                let digest = hash(&bytes);
                let source = self
                    .data
                    .join("deployments")
                    .join(&digest)
                    .join("herdr-kit.json");
                if let Some(existing) = file_hash(&source)? {
                    ensure!(existing == digest, "deployment layout drift detected");
                } else {
                    atomic(&source, &bytes, false)?;
                }
                files.insert(
                    layout(&home),
                    FileReceipt {
                        source,
                        hash: digest,
                        executable: false,
                        ready: false,
                    },
                );
            }
            next.files = files;
        }
        for change in &mods {
            no_links(&change.target)?;
            if change.target.exists() {
                ensure!(
                    self.owns_mod(old, &change.target)?,
                    "mod target already exists without ownership"
                );
                ensure!(tree(&change.target)? == change.hashes, "mod drift detected");
            }
            next.mod_owners
                .insert(change.target.clone(), change.hashes.clone());
        }
        let mut files = Vec::new();
        for (target, receipt) in &next.files {
            if !old_files.contains_key(target) {
                ensure!(
                    file_hash(target)?.is_none(),
                    "file target already exists without ownership: {}",
                    target.display()
                );
            }
            files.push(FileChange {
                target: target.clone(),
                old_hash: old_files.get(target).map(|r| r.hash.clone()),
                next: Some(receipt.clone()),
            });
        }
        for (target, receipt) in old_files {
            if !next.files.contains_key(&target) {
                files.push(FileChange {
                    target,
                    old_hash: Some(receipt.hash),
                    next: None,
                });
            }
        }
        for (id, rev) in &next.selection {
            next.package_releases
                .get_mut(rev)
                .and_then(|r| r.get_mut(id))
                .context("missing selected package receipt")?
                .activated = true;
        }
        next.previous = old.current.clone();
        next.previous_selection = (!old.selection.is_empty()).then(|| old.selection.clone());
        next.current = legacy.map(str::to_owned).or_else(|| {
            next.selection
                .get("kit-manager")
                .or_else(|| next.selection.get("herdr-kit"))
                .or_else(|| next.selection.values().next())
                .cloned()
        });
        Ok(Transaction {
            schema_version: 2,
            state: next,
            files,
            mods,
        })
    }
    fn activate(&self, transaction: Transaction) -> Result<State> {
        save(&journal_path(&self.data), &Journal::Components(transaction))?;
        self.recover()?;
        self.state()
    }
    fn owns_binary(&self, state: &State, rev: &str, binary: &str, digest: &str) -> bool {
        state.package_releases.get(rev).is_some_and(|packages| {
            packages.values().any(|r| {
                r.definition
                    .binary
                    .as_ref()
                    .is_some_and(|b| b.name == binary)
                    && r.binary_hash.as_deref() == Some(digest)
            })
        }) || state
            .releases
            .get(rev)
            .and_then(|r| r.get(binary))
            .is_some_and(|h| h == digest)
    }
    fn build(&self, state: &mut State, rev: &str, requested: &[String]) -> Result<Vec<String>> {
        ensure!(
            (rev.len() == 40 || rev.len() == 64)
                && rev
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "revision must be a full lowercase commit id"
        );
        if state.releases.contains_key(rev) {
            self.verify_legacy(state, rev)?;
            bail!(
                "legacy release is immutable; component migration requires a new committed revision"
            );
        }
        if let Some(packages) = state.package_releases.get(rev) {
            for receipt in packages.values().filter(|r| r.activated) {
                self.verify_package(rev, receipt)?;
            }
        }
        let repo = state
            .source
            .clone()
            .context("no trusted local source configured")?;
        let home = state.app_home.clone().context("no install target")?;
        let releases = self.data.join("releases");
        directory(&releases)?;
        let tmp = tempfile::Builder::new()
            .prefix(".build-")
            .tempdir_in(&releases)?;
        let archive = tmp.path().join("source.tar");
        run(
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["archive", "--format=tar", "--output"])
                .arg(&archive)
                .arg(rev),
            "source archive failed",
        )?;
        let source = tmp.path().join("source");
        directory(&source)?;
        run(
            Command::new("tar")
                .arg("-xf")
                .arg(archive)
                .arg("-C")
                .arg(&source),
            "source extraction failed",
        )?;
        ensure!(
            source.join("Cargo.lock").is_file(),
            "committed Cargo.lock required"
        );
        let catalog = Catalog::load(&source)?;
        let selected = catalog.selected(requested)?;
        let final_dir = release(&self.data, rev);
        no_links(&final_dir)?;
        if final_dir.exists() {
            ensure!(
                state.releases.contains_key(rev) || state.package_releases.contains_key(rev),
                "release target exists without receipt"
            );
        }
        for id in &selected {
            if let Some(receipt) = state.package_releases.get(rev).and_then(|r| r.get(id)) {
                if receipt.activated || self.verify_package(rev, receipt).is_ok() {
                    continue;
                }
                ensure!(
                    state.selection.get(id).map(String::as_str) != Some(rev),
                    "selected release drift detected"
                );
                // Receipt publication precedes immutable publication. Only
                // missing files are recoverable; changed existing bytes refuse.
                if let Some(binary) = &receipt.definition.binary {
                    if let Some(actual) = file_hash(&final_dir.join(&binary.name))? {
                        ensure!(
                            Some(&actual) == receipt.binary_hash.as_ref(),
                            "release drift detected"
                        );
                    }
                }
                for (base, expected) in &receipt.mods {
                    let path = final_dir.join("mods").join(base);
                    if path.exists() {
                        ensure!(&tree(&path)? == expected, "release drift detected");
                    }
                }
                if receipt.definition.layout.is_some() {
                    if let Some(actual) = file_hash(&final_dir.join("layout-source.json"))? {
                        ensure!(
                            Some(&actual) == receipt.layout_hash.as_ref(),
                            "release drift detected"
                        );
                    }
                }
            }
            let definition = catalog.packages[id].clone();
            let staged = tmp.path().join(id);
            directory(&staged)?;
            let mut receipt = PackageReceipt {
                definition: definition.clone(),
                binary_hash: None,
                mods: BTreeMap::new(),
                layout_hash: None,
                activated: false,
            };
            if let Some(binary) = &definition.binary {
                let existing = final_dir.join(&binary.name);
                if let Some(digest) = file_hash(&existing)? {
                    ensure!(
                        self.owns_binary(state, rev, &binary.name, &digest),
                        "release binary collision"
                    );
                    receipt.binary_hash = Some(digest);
                } else {
                    run(
                        Command::new("cargo")
                            .current_dir(&source)
                            .args([
                                "build",
                                "--locked",
                                "--release",
                                "-p",
                                &binary.cargo_package,
                                "--bin",
                                &binary.name,
                            ])
                            .env("CARGO_TARGET_DIR", tmp.path().join("target")),
                        "release build failed",
                    )?;
                    fs::copy(
                        tmp.path().join("target/release").join(&binary.name),
                        staged.join(&binary.name),
                    )?;
                    receipt.binary_hash = file_hash(&staged.join(&binary.name))?;
                }
            }
            for base in &definition.mods {
                let dir = staged.join("mods").join(base);
                copy_tree(&source.join("mods").join(base), &dir)?;
                let binary = definition
                    .binary
                    .as_ref()
                    .or_else(|| {
                        definition
                            .runtime_package
                            .as_ref()
                            .and_then(|id| catalog.packages.get(id))
                            .and_then(|package| package.binary.as_ref())
                    })
                    .context("missing mod executable")?;
                let mut runtime = format!(
                    "export const BINARY = {};\nexport const DATA_DIR = {};\nexport const RELEASE = {};\nexport const MOD_NAME = {};\n",
                    serde_json::to_string(&final_dir.join(&binary.name))?,
                    serde_json::to_string(&self.data)?,
                    serde_json::to_string(rev)?,
                    serde_json::to_string(&name(base, rev))?
                );
                if let Some(base) = &definition.runtime_mod {
                    runtime.push_str(&format!(
                        "export const DEPENDENCY_MOD = {};\n",
                        serde_json::to_string(&name(base, rev))?
                    ));
                }
                fs::write(dir.join("runtime.js"), runtime)?;
                let mut diagnostic = tempfile::tempfile()?;
                let check = bounded(
                    Command::new("herdr-app")
                        .arg("check")
                        .arg(&dir)
                        .env("HERDR_APP_HOME", &home)
                        .stdout(diagnostic.try_clone()?)
                        .stderr(diagnostic.try_clone()?),
                    30,
                );
                match check {
                    Ok(status) if !status.success() => {
                        use std::io::{Read, Seek, SeekFrom};
                        diagnostic.seek(SeekFrom::Start(0))?;
                        let mut message = String::new();
                        diagnostic.take(2048).read_to_string(&mut message)?;
                        bail!("native mod check failed ({base}): {}", message.trim());
                    }
                    Ok(_) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => bail!("native mod check failed ({base}): {error}"),
                }
                receipt.mods.insert(base.clone(), tree(&dir)?);
            }
            if let Some(path) = &definition.layout {
                fs::copy(source.join(path), staged.join("layout-source.json"))?;
                receipt.layout_hash = file_hash(&staged.join("layout-source.json"))?;
            }
            for (base, expected) in &receipt.mods {
                let path = final_dir.join("mods").join(base);
                if path.exists() {
                    let owned = state
                        .package_releases
                        .get(rev)
                        .and_then(|r| r.get(id))
                        .and_then(|r| r.mods.get(base));
                    ensure!(
                        owned == Some(expected) && &tree(&path)? == expected,
                        "release mod collision"
                    );
                }
            }
            if definition.layout.is_some() {
                if let Some(actual) = file_hash(&final_dir.join("layout-source.json"))? {
                    ensure!(
                        state
                            .package_releases
                            .get(rev)
                            .and_then(|r| r.get(id))
                            .and_then(|r| r.layout_hash.as_ref())
                            == Some(&actual)
                            && receipt.layout_hash.as_ref() == Some(&actual),
                        "release layout collision"
                    );
                }
            }
            state
                .package_releases
                .entry(rev.to_owned())
                .or_default()
                .insert(id.clone(), receipt.clone());
            self.save(state)?;
            directory(&final_dir)?;
            if let Some(binary) = &definition.binary {
                let target = final_dir.join(&binary.name);
                if !target.exists() {
                    atomic(&target, &fs::read(staged.join(&binary.name))?, true)?;
                }
            }
            for (base, hashes) in &receipt.mods {
                self.apply_mod(&ModChange {
                    target: final_dir.join("mods").join(base),
                    source: staged.join("mods").join(base),
                    hashes: hashes.clone(),
                })?;
            }
            if definition.layout.is_some() && !final_dir.join("layout-source.json").exists() {
                atomic(
                    &final_dir.join("layout-source.json"),
                    &fs::read(staged.join("layout-source.json"))?,
                    false,
                )?;
            }
        }
        Ok(selected)
    }
}

fn result(state: &State, available: Option<&str>, status: &str) -> Value {
    let mut mods = BTreeMap::new();
    for (id, rev) in &state.selection {
        if let Some(receipt) = state.package_releases.get(rev).and_then(|r| r.get(id)) {
            mods.insert(
                id,
                receipt
                    .mods
                    .keys()
                    .map(|base| name(base, rev))
                    .collect::<Vec<_>>(),
            );
        }
    }
    json!({"state":status,"current_revision":state.current,"available_revision":available,"previous_revision":state.previous,"source":state.source,"targets":{"app_home":state.app_home},"next_check":state.next_check,"components":state.selection,"previous_components":state.previous_selection,"resolved_mods":mods})
}
pub fn active_release(data: &Path) -> Option<String> {
    state::coordinator_revision(data)
}
pub fn stable_binary(data: &Path) -> PathBuf {
    bootstrap(data)
}
fn activated(mut value: Value, old: &State, next: &State, changed: bool) -> Value {
    let coord = |state: &State| {
        if state.selection.is_empty() {
            state.current.clone()
        } else {
            state.selection.get("kit-coord").cloned()
        }
    };
    value["activated"] = json!(changed);
    value["coord_changed"] = json!(changed && coord(next).is_some() && coord(old) != coord(next));
    value
}
pub fn install(
    data: &Path,
    home: &Path,
    source: &Path,
    components: &[String],
    pinned: Option<&str>,
) -> Result<Value> {
    let lock = Locked::open(data)?;
    let old = lock.state()?;
    lock.verify_active(&old)?;
    let home = absolute(home)?;
    ensure!(home.is_dir(), "app home must exist");
    if old.current.is_some() {
        ensure!(
            old.app_home.as_ref() == Some(&home),
            "different install target already registered"
        );
    }
    let mut next = old.clone();
    next.source = Some(source_path(source)?);
    next.app_home = Some(home);
    let rev = if let Some(rev) = pinned {
        rev.to_owned()
    } else {
        revision(next.source.as_ref().unwrap())?
    };
    if old.selection.is_empty() && old.current.as_deref() == Some(&rev) && components.is_empty() {
        lock.save(&next)?;
        return Ok(activated(
            result(&next, Some(&rev), "current"),
            &old,
            &next,
            false,
        ));
    }
    let ids = lock.build(&mut next, &rev, components)?;
    let selection = ids.into_iter().map(|id| (id, rev.clone())).collect();
    if old.selection == selection && old.current.is_some() {
        lock.save(&next)?;
        return Ok(activated(
            result(&next, Some(&rev), "current"),
            &old,
            &next,
            false,
        ));
    }
    let transaction = lock.prepare(&old, next, selection, None)?;
    let next = lock.activate(transaction)?;
    Ok(activated(
        result(&next, Some(&rev), "installed"),
        &old,
        &next,
        true,
    ))
}
pub fn status(data: &Path) -> Result<Value> {
    let lock = Locked::open(data)?;
    let state = lock.state()?;
    lock.verify_active(&state)?;
    Ok(result(
        &state,
        None,
        if state.current.is_some() {
            "installed"
        } else {
            "not_installed"
        },
    ))
}
pub fn source(data: &Path, repo: Option<&Path>) -> Result<Value> {
    let lock = Locked::open(data)?;
    let mut state = lock.state()?;
    if let Some(repo) = repo {
        state.source = Some(source_path(repo)?);
        state.next_check = 0;
        lock.save(&state)?;
    }
    Ok(result(&state, None, "source"))
}
pub fn update(data: &Path, action: &str, components: &[String]) -> Result<Value> {
    ensure!(
        matches!(action, "check" | "apply" | "maintain" | "rollback"),
        "unknown update action"
    );
    let lock = Locked::open(data)?;
    let mut state = lock.state()?;
    ensure!(state.current.is_some(), "suite is not installed");
    lock.verify_active(&state)?;
    if action == "rollback" {
        ensure!(
            components.is_empty(),
            "rollback restores the complete prior selection"
        );
        let rev = state.previous.clone().context("no previous release")?;
        let transaction = if let Some(selection) = state.previous_selection.clone() {
            lock.prepare(&state, state.clone(), selection, None)?
        } else {
            lock.prepare(&state, state.clone(), BTreeMap::new(), Some(&rev))?
        };
        let old = state;
        let state = lock.activate(transaction)?;
        return Ok(activated(
            result(&state, Some(&rev), "rolled_back"),
            &old,
            &state,
            true,
        ));
    }
    if action == "maintain" && now() < state.next_check {
        return Ok(activated(
            result(&state, None, "not_due"),
            &state,
            &state,
            false,
        ));
    }
    for id in components {
        ensure!(
            state.selection.contains_key(id),
            "component is not selected: {id}"
        );
    }
    let work = (|| -> Result<Value> {
        let rev = revision(
            state
                .source
                .as_ref()
                .context("no trusted local source configured")?,
        )?;
        let ids = if components.is_empty() {
            state.selection.keys().cloned().collect::<Vec<_>>()
        } else {
            components.to_vec()
        };
        let changed = if state.selection.is_empty() {
            state.current.as_deref() != Some(&rev)
        } else {
            ids.iter().any(|id| state.selection.get(id) != Some(&rev))
        };
        if action == "check" {
            return Ok(activated(
                result(
                    &state,
                    Some(&rev),
                    if changed { "available" } else { "current" },
                ),
                &state,
                &state,
                false,
            ));
        }
        let old = state.clone();
        if changed {
            let built = lock.build(&mut state, &rev, &ids)?;
            let mut selection = state.selection.clone();
            for id in built {
                selection.insert(id, rev.clone());
            }
            let transaction = lock.prepare(&old, state.clone(), selection, None)?;
            state = lock.activate(transaction)?;
        }
        if action == "maintain" {
            state.next_check = now().saturating_add(DAY);
            lock.save(&state)?;
        }
        Ok(activated(
            result(&state, Some(&rev), "current"),
            &old,
            &state,
            changed,
        ))
    })();
    if action == "maintain" && work.is_err() && !journal_path(&lock.data).exists() {
        state.next_check = now().saturating_add(BACKOFF);
        lock.save(&state)?;
    }
    work
}

#[cfg(test)]
mod tests;
