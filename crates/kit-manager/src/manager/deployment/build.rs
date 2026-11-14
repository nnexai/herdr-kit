//! Builds selected packages from committed source into the immutable
//! `releases/<revision>` tree. A package's receipt is saved before its files
//! are published, so an interrupted publication can be completed later.
use super::*;

/// One committed revision extracted into a temporary directory inside `releases/`.
struct Checkout<'a> {
    lock: &'a Locked,
    rev: &'a str,
    tmp: tempfile::TempDir,
    source: PathBuf,
    release: PathBuf,
    app_home: PathBuf,
    catalog: Catalog,
}

impl Locked {
    /// Returns the built component ids: `requested` plus their dependencies,
    /// or the default bundle when `requested` is empty.
    pub(super) fn build(
        &self,
        state: &mut State,
        rev: &str,
        requested: &[String],
    ) -> Result<Vec<String>> {
        ensure!(
            full_commit_id(rev),
            "revision must be a full lowercase commit id"
        );
        if let Some(packages) = state.package_releases.get(rev) {
            for receipt in packages.values().filter(|r| r.activated) {
                self.verify_package(rev, receipt)?;
            }
        }
        let checkout = Checkout::extract(self, state, rev)?;
        let selected = checkout.catalog.selected(requested)?;
        no_links(&checkout.release, SymlinkMessage::Generic)?;
        if checkout.release.exists() {
            ensure!(
                state.package_releases.contains_key(rev),
                "release target exists without receipt"
            );
        }
        for id in &selected {
            if let Some(receipt) = state.receipt(id, rev) {
                if receipt.activated || self.verify_package(rev, receipt).is_ok() {
                    continue;
                }
                ensure!(
                    state.selection.get(id).map(String::as_str) != Some(rev),
                    "selected release drift detected"
                );
                checkout.verify_published_part(receipt)?;
            }
            let receipt = checkout.stage(id, state)?;
            checkout.refuse_collisions(id, &receipt, state)?;
            state
                .package_releases
                .entry(rev.to_owned())
                .or_default()
                .insert(id.clone(), receipt.clone());
            self.save(state)?;
            checkout.publish(id, &receipt)?;
        }
        Ok(selected)
    }
}

impl<'a> Checkout<'a> {
    fn extract(lock: &'a Locked, state: &State, rev: &'a str) -> Result<Self> {
        let repo = state
            .source
            .clone()
            .context("no trusted local source configured")?;
        let app_home = state.app_home.clone().context("no install target")?;
        let releases = lock.data.join("releases");
        directory(&releases)?;
        let tmp = tempfile::Builder::new()
            .prefix(".build-")
            .tempdir_in(&releases)?;
        let archive = tmp.path().join("source.tar");
        run(
            kit_core::git_env::git()
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
        Ok(Self {
            lock,
            rev,
            tmp,
            source,
            release: release(&lock.data, rev),
            app_home,
            catalog,
        })
    }

    fn staged(&self, id: &str) -> PathBuf {
        self.tmp.path().join(id)
    }

    /// A receipt without activation may describe an interrupted publication.
    /// Only missing files are recoverable; changed existing bytes refuse.
    fn verify_published_part(&self, receipt: &PackageReceipt) -> Result<()> {
        if let Some(binary) = &receipt.definition.binary {
            if let Some(actual) = file_hash(&self.release.join(&binary.name))? {
                ensure!(
                    Some(&actual) == receipt.binary_hash.as_ref(),
                    "release drift detected"
                );
            }
        }
        for (base, expected) in &receipt.mods {
            let path = self.release.join("mods").join(base);
            if path.exists() {
                ensure!(&tree(&path)? == expected, "release drift detected");
            }
        }
        if receipt.definition.layout.is_some() {
            if let Some(actual) = file_hash(&self.release.join(LAYOUT_SOURCE))? {
                ensure!(
                    Some(&actual) == receipt.layout_hash.as_ref(),
                    "release drift detected"
                );
            }
        }
        Ok(())
    }

    /// Produce the package's artifacts under the temporary directory and
    /// describe them. An executable already published for this revision is
    /// adopted instead of rebuilt.
    fn stage(&self, id: &str, state: &State) -> Result<PackageReceipt> {
        let definition = self.catalog.packages[id].clone();
        let staged = self.staged(id);
        directory(&staged)?;
        let mut receipt = PackageReceipt {
            definition: definition.clone(),
            binary_hash: None,
            mods: BTreeMap::new(),
            layout_hash: None,
            activated: false,
        };
        if let Some(binary) = &definition.binary {
            if let Some(digest) = file_hash(&self.release.join(&binary.name))? {
                ensure!(
                    owns_binary(state, self.rev, &binary.name, &digest),
                    "release binary collision"
                );
                receipt.binary_hash = Some(digest);
            } else {
                self.compile(binary, &staged)?;
                receipt.binary_hash = file_hash(&staged.join(&binary.name))?;
            }
        }
        for base in &definition.mods {
            let dir = staged.join("mods").join(base);
            copy_tree(&self.source.join("mods").join(base), &dir)?;
            fs::write(dir.join("runtime.js"), self.runtime_js(&definition, base)?)?;
            self.check_mod(base, &dir)?;
            receipt.mods.insert(base.clone(), tree(&dir)?);
        }
        if let Some(path) = &definition.layout {
            fs::copy(self.source.join(path), staged.join(LAYOUT_SOURCE))?;
            receipt.layout_hash = file_hash(&staged.join(LAYOUT_SOURCE))?;
        }
        Ok(receipt)
    }

    fn compile(&self, binary: &Binary, staged: &Path) -> Result<()> {
        let target = self.tmp.path().join("target");
        run(
            Command::new("cargo")
                .current_dir(&self.source)
                .args([
                    "build",
                    "--locked",
                    "--release",
                    "-p",
                    &binary.cargo_package,
                    "--bin",
                    &binary.name,
                ])
                .env("CARGO_TARGET_DIR", &target)
                .env("KIT_COORD_BUILD_REVISION", self.rev),
            "release build failed",
        )?;
        fs::copy(
            target.join("release").join(&binary.name),
            staged.join(&binary.name),
        )?;
        Ok(())
    }

    /// The generated module binding a mod generation to its exact executable.
    fn runtime_js(&self, definition: &Package, base: &str) -> Result<String> {
        let binary = definition.binary.as_ref().or_else(|| {
            definition
                .runtime_package
                .as_ref()
                .and_then(|id| self.catalog.packages.get(id))
                .and_then(|package| package.binary.as_ref())
        });
        let mut runtime = format!(
            "export const BINARY = {};\nexport const DATA_DIR = {};\nexport const RELEASE = {};\nexport const MOD_NAME = {};\n",
            serde_json::to_string(&binary.map(|binary| self.release.join(&binary.name)))?,
            serde_json::to_string(&self.lock.data)?,
            serde_json::to_string(self.rev)?,
            serde_json::to_string(&name(base, self.rev))?
        );
        if let Some(base) = &definition.runtime_mod {
            runtime.push_str(&format!(
                "export const DEPENDENCY_MOD = {};\n",
                serde_json::to_string(&name(base, self.rev))?
            ));
        }
        Ok(runtime)
    }

    /// Run `herdr-app check` on a staged mod. A missing `herdr-app` skips the check.
    fn check_mod(&self, base: &str, dir: &Path) -> Result<()> {
        let mut diagnostic = tempfile::tempfile()?;
        let check = bounded(
            Command::new("herdr-app")
                .arg("check")
                .arg(dir)
                .env("HERDR_APP_HOME", &self.app_home)
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
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => bail!("native mod check failed ({base}): {error}"),
        }
    }

    /// Files already in the release tree must be the ones this package's
    /// earlier receipt recorded and the ones just staged.
    fn refuse_collisions(&self, id: &str, receipt: &PackageReceipt, state: &State) -> Result<()> {
        let recorded = state.receipt(id, self.rev);
        for (base, expected) in &receipt.mods {
            let path = self.release.join("mods").join(base);
            if path.exists() {
                let owned = recorded.and_then(|r| r.mods.get(base));
                ensure!(
                    owned == Some(expected) && &tree(&path)? == expected,
                    "release mod collision"
                );
            }
        }
        if receipt.definition.layout.is_some() {
            if let Some(actual) = file_hash(&self.release.join(LAYOUT_SOURCE))? {
                ensure!(
                    recorded.and_then(|r| r.layout_hash.as_ref()) == Some(&actual)
                        && receipt.layout_hash.as_ref() == Some(&actual),
                    "release layout collision"
                );
            }
        }
        Ok(())
    }

    /// Copy staged artifacts that the release tree does not hold yet.
    fn publish(&self, id: &str, receipt: &PackageReceipt) -> Result<()> {
        let staged = self.staged(id);
        directory(&self.release)?;
        if let Some(binary) = &receipt.definition.binary {
            let target = self.release.join(&binary.name);
            if !target.exists() {
                atomic(&target, &fs::read(staged.join(&binary.name))?, true)?;
            }
        }
        for (base, hashes) in &receipt.mods {
            apply_mod(&ModChange {
                target: self.release.join("mods").join(base),
                source: staged.join("mods").join(base),
                hashes: hashes.clone(),
            })?;
        }
        let layout = self.release.join(LAYOUT_SOURCE);
        if receipt.definition.layout.is_some() && !layout.exists() {
            atomic(&layout, &fs::read(staged.join(LAYOUT_SOURCE))?, false)?;
        }
        Ok(())
    }
}

/// Whether some receipt of this revision recorded exactly this executable.
fn owns_binary(state: &State, rev: &str, binary: &str, digest: &str) -> bool {
    state.package_releases.get(rev).is_some_and(|packages| {
        packages.values().any(|r| {
            r.definition
                .binary
                .as_ref()
                .is_some_and(|b| b.name == binary)
                && r.binary_hash.as_deref() == Some(digest)
        })
    })
}
