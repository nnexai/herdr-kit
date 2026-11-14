//! Trusted local-source component builds and owned-artifact activation.
//! Component installs do not own consumer layouts.

use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use kit_core::paths::{SymlinkMessage, no_links};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod activation;
mod deployment;
mod packages;
pub use activation::after_activation;
pub use deployment::{UpdateAction, install, source, status, update};
use packages::*;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    source: Option<PathBuf>,
    app_home: Option<PathBuf>,
    current: Option<String>,
    next_check: u64,
    selection: BTreeMap<String, String>,
    package_releases: BTreeMap<String, BTreeMap<String, PackageReceipt>>,
    files: BTreeMap<PathBuf, FileReceipt>,
    mod_owners: BTreeMap<PathBuf, BTreeMap<String, String>>,
}
impl State {
    fn receipt(&self, id: &str, rev: &str) -> Option<&PackageReceipt> {
        self.package_releases.get(rev)?.get(id)
    }
}

/// The layout a release carries before its mod pins are projected.
const LAYOUT_SOURCE: &str = "layout-source.json";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn directory(path: &Path) -> Result<()> {
    no_links(path, SymlinkMessage::Generic)?;
    fs::create_dir_all(path)?;
    no_links(path, SymlinkMessage::Generic)?;
    ensure!(fs::metadata(path)?.is_dir(), "expected directory");
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    no_links(&path, SymlinkMessage::Generic)?;
    Ok(path)
}

fn atomic(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    directory(parent)?;
    no_links(path, SymlinkMessage::Generic)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    if executable {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tmp.as_file()
                .set_permissions(fs::Permissions::from_mode(0o755))?;
        }
    }
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    atomic(path, &serde_json::to_vec(value)?, false)
}
fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<Option<T>> {
    no_links(path, SymlinkMessage::Generic)?;
    match fs::read(path) {
        Ok(b) => Ok(Some(
            serde_json::from_slice(&b).context("invalid manager metadata")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_hash(path: &Path) -> Result<Option<String>> {
    no_links(&path, SymlinkMessage::Generic)?;
    match fs::read(path) {
        Ok(b) => Ok(Some(hash(&b))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn tree(root: &Path) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        no_links(dir, SymlinkMessage::Generic)?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            ensure!(!ty.is_symlink(), "symlink in managed tree");
            if ty.is_dir() {
                visit(root, &path, out)?;
            } else {
                ensure!(ty.is_file(), "unsupported managed file");
                let rel = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF8 managed path")?;
                // Herdr creates these editor support files itself.
                if rel == "jsconfig.json" || rel.ends_with(".d.ts") {
                    continue;
                }
                out.insert(rel.to_owned(), file_hash(&path)?.context("missing file")?);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out)?;
    Ok(out)
}
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    no_links(from, SymlinkMessage::Generic)?;
    directory(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let ty = e.file_type()?;
        ensure!(!ty.is_symlink(), "symlink in source assets");
        let dest = to.join(e.file_name());
        if ty.is_dir() {
            copy_tree(&e.path(), &dest)?;
        } else {
            ensure!(ty.is_file(), "unsupported asset");
            no_links(&dest, SymlinkMessage::Generic)?;
            fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}
fn bounded(cmd: &mut Command, seconds: u64) -> std::io::Result<std::process::ExitStatus> {
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "command timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn run(cmd: &mut Command, error: &str) -> Result<()> {
    let status =
        bounded(cmd.stdout(Stdio::null()).stderr(Stdio::null()), 540).context(error.to_owned())?;
    ensure!(status.success(), "{error}");
    Ok(())
}
fn revision(repo: &Path) -> Result<String> {
    let mut output = tempfile::tempfile()?;
    let status = bounded(
        kit_core::git_env::git()
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", "HEAD^{commit}"])
            .stdout(output.try_clone()?)
            .stderr(Stdio::null()),
        30,
    )
    .context("cannot read source revision")?;
    ensure!(status.success(), "cannot read source revision");
    use std::io::{Read, Seek, SeekFrom};
    output.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    output.take(256).read_to_string(&mut text)?;
    let rev = text.trim().to_owned();
    ensure!(full_commit_id(&rev), "invalid source revision");
    Ok(rev)
}
fn full_commit_id(rev: &str) -> bool {
    (rev.len() == 40 || rev.len() == 64)
        && rev
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn source_path(path: &Path) -> Result<PathBuf> {
    let p = absolute(path)?;
    let p = fs::canonicalize(p).context("source directory unavailable")?;
    ensure!(p.is_dir(), "source must be a directory");
    revision(&p)?;
    Ok(p)
}
fn name(base: &str, rev: &str) -> String {
    format!("{base}-{rev}")
}
fn release(data: &Path, rev: &str) -> PathBuf {
    data.join("releases").join(rev)
}
fn state_path(data: &Path) -> PathBuf {
    data.join("state.json")
}
fn journal_path(data: &Path) -> PathBuf {
    data.join("activation.json")
}
fn layout(home: &Path) -> PathBuf {
    home.join("layouts/herdr-kit.json")
}
fn bootstrap(data: &Path) -> PathBuf {
    data.join("bin/herdr-kit")
}
fn mod_path(home: &Path, base: &str, rev: &str) -> PathBuf {
    home.join("mods").join(name(base, rev))
}
