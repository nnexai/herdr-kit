//! Trusted local-Git suite installation. Each layout points at a revision-named
//! mod directory, so a watcher can never load files from two releases together.
//! Native watcher discovery of new mod directories must be verified by the caller.

use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DAY: u64 = 86_400;
const BACKOFF: u64 = 3_600;
const MOD: &str = "herdr-kit-usage";

#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    source: Option<PathBuf>,
    app_home: Option<PathBuf>,
    current: Option<String>,
    previous: Option<String>,
    #[serde(default)]
    releases: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    managed_mods: Vec<String>,
    #[serde(default)]
    next_check: u64,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    state: State,
    old_layout: Option<String>,
    old_binary: Option<String>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

// Refuse all existing symlinks in a path, including ancestors. Never follow a
// substituted managed target. Missing components are permitted for creation.
fn no_links(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "path must be absolute");
    let mut part = PathBuf::new();
    for c in path.components() {
        match c {
            Component::RootDir | Component::Normal(_) => part.push(c.as_os_str()),
            _ => bail!("unsafe path"),
        }
        match fs::symlink_metadata(&part) {
            Ok(m) => ensure!(!m.file_type().is_symlink(), "symlink in managed path"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn directory(path: &Path) -> Result<()> {
    no_links(path)?;
    fs::create_dir_all(path)?;
    no_links(path)?;
    ensure!(fs::metadata(path)?.is_dir(), "expected directory");
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    no_links(&path)?;
    Ok(path)
}

fn atomic(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    directory(parent)?;
    no_links(path)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    if executable {
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            tmp.as_file().set_permissions(fs::Permissions::from_mode(0o755))?;
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
    no_links(path)?;
    match fs::read(path) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b).context("invalid manager metadata")?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn file_hash(path: &Path) -> Result<Option<String>> {
    no_links(path)?;
    match fs::read(path) {
        Ok(b) => Ok(Some(hash(&b))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn tree(root: &Path) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        no_links(dir)?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            ensure!(!ty.is_symlink(), "symlink in managed tree");
            if ty.is_dir() { visit(root, &path, out)?; }
            else {
                ensure!(ty.is_file(), "unsupported managed file");
                let rel = path.strip_prefix(root)?.to_str().context("non-UTF8 managed path")?;
                // Herdr creates these editor support files itself.
                if rel == "jsconfig.json" || rel.ends_with(".d.ts") { continue; }
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
    no_links(from)?;
    directory(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let ty = e.file_type()?;
        ensure!(!ty.is_symlink(), "symlink in source assets");
        let dest = to.join(e.file_name());
        if ty.is_dir() { copy_tree(&e.path(), &dest)?; }
        else {
            ensure!(ty.is_file(), "unsupported asset");
            no_links(&dest)?;
            fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}
fn bounded(cmd: &mut Command, seconds: u64) -> std::io::Result<std::process::ExitStatus> {
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(status) = child.try_wait()? { return Ok(status); }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "command timed out"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn run(cmd: &mut Command, error: &str) -> Result<()> {
    let status = bounded(cmd.stdout(Stdio::null()).stderr(Stdio::null()), 540).context(error.to_owned())?;
    ensure!(status.success(), "{error}");
    Ok(())
}
fn revision(repo: &Path) -> Result<String> {
    let mut output = tempfile::tempfile()?;
    let status = bounded(Command::new("git").arg("-C").arg(repo).args(["rev-parse", "--verify", "HEAD^{commit}"]).stdout(output.try_clone()?).stderr(Stdio::null()), 30).context("cannot read source revision")?;
    ensure!(status.success(), "cannot read source revision");
    use std::io::{Read, Seek, SeekFrom};
    output.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    output.take(256).read_to_string(&mut text)?;
    let rev = text.trim().to_owned();
    ensure!((rev.len() == 40 || rev.len() == 64) && rev.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "invalid source revision");
    Ok(rev)
}
fn source_path(path: &Path) -> Result<PathBuf> {
    let p = absolute(path)?;
    let p = fs::canonicalize(p).context("source directory unavailable")?;
    ensure!(p.is_dir(), "source must be a directory");
    revision(&p)?;
    Ok(p)
}
fn name(rev: &str) -> String { format!("{MOD}-{rev}") }
fn release(data: &Path, rev: &str) -> PathBuf { data.join("releases").join(rev) }
fn state_path(data: &Path) -> PathBuf { data.join("state.json") }
fn journal_path(data: &Path) -> PathBuf { data.join("activation.json") }
fn layout(home: &Path) -> PathBuf { home.join("layouts/herdr-kit.json") }
fn bootstrap(data: &Path) -> PathBuf { data.join("bin/herdr-kit") }
fn mod_path(home: &Path, rev: &str) -> PathBuf { home.join("mods").join(name(rev)) }

struct Locked { data: PathBuf, _lock: File }
impl Locked {
    fn open(path: &Path) -> Result<Self> {
        let data = absolute(path)?;
        directory(&data)?;
        let p = data.join("manager.lock");
        no_links(&p)?;
        let lock = OpenOptions::new().create(true).read(true).write(true).open(p)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                    std::thread::sleep(Duration::from_millis(100)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => bail!("manager lock busy"),
                Err(e) => return Err(e.into()),
            }
        }
        let this = Self { data, _lock: lock };
        this.recover()?;
        Ok(this)
    }
    fn state(&self) -> Result<State> { Ok(read_json(&state_path(&self.data))?.unwrap_or_default()) }
    fn save(&self, s: &State) -> Result<()> { save(&state_path(&self.data), s) }
    fn verify_release(&self, state: &State, rev: &str) -> Result<()> {
        let expected = state.releases.get(rev).context("missing release receipt")?;
        ensure!(&tree(&release(&self.data, rev))? == expected, "release drift detected");
        Ok(())
    }
    fn verify_active(&self, state: &State) -> Result<()> {
        if let Some(rev) = &state.current {
            self.verify_release(state, rev)?;
            let home = state.app_home.as_ref().context("missing target")?;
            let r = release(&self.data, rev);
            ensure!(tree(&mod_path(home, rev))? == tree(&r.join("mod"))?, "mod drift detected");
            ensure!(file_hash(&layout(home))? == file_hash(&r.join("layout.json"))?, "layout drift detected");
            ensure!(file_hash(&bootstrap(&self.data))? == file_hash(&r.join("herdr-kit"))?, "bootstrap drift detected");
        }
        Ok(())
    }
    fn recover(&self) -> Result<()> {
        let Some(j): Option<Journal> = read_json(&journal_path(&self.data))? else { return Ok(()); };
        let s = &j.state;
        let rev = s.current.as_ref().context("invalid activation journal")?;
        let home = s.app_home.as_ref().context("invalid activation journal")?;
        self.verify_release(s, rev)?;
        let r = release(&self.data, rev);
        let target = mod_path(home, rev);
        if target.exists() {
            ensure!(tree(&target)? == tree(&r.join("mod"))?, "mod collision or drift detected");
        } else {
            no_links(&target)?;
            let mods = home.join("mods");
            directory(&mods)?;
            let staging = tempfile::Builder::new().prefix(".herdr-kit-").tempdir_in(&mods)?;
            copy_tree(&r.join("mod"), staging.path())?;
            ensure!(tree(staging.path())? == tree(&r.join("mod"))?, "mod staging mismatch");
            fs::rename(staging.path(), &target)?;
            File::open(&mods)?.sync_all()?;
        }
        let new_layout = file_hash(&r.join("layout.json"))?.context("missing release layout")?;
        let old = file_hash(&layout(home))?;
        ensure!(old == j.old_layout || old.as_ref() == Some(&new_layout), "layout collision or drift detected");
        if old.as_ref() != Some(&new_layout) { atomic(&layout(home), &fs::read(r.join("layout.json"))?, false)?; }
        let new_bin = file_hash(&r.join("herdr-kit"))?.context("missing release binary")?;
        let old = file_hash(&bootstrap(&self.data))?;
        ensure!(old == j.old_binary || old.as_ref() == Some(&new_bin), "bootstrap collision or drift detected");
        if old.as_ref() != Some(&new_bin) { atomic(&bootstrap(&self.data), &fs::read(r.join("herdr-kit"))?, true)?; }
        self.save(s)?;
        fs::remove_file(journal_path(&self.data))?;
        File::open(&self.data)?.sync_all()?;
        Ok(())
    }
    fn activate(&self, old: &State, mut next: State, rev: &str) -> Result<State> {
        self.verify_active(old)?;
        self.verify_release(&next, rev)?;
        let home = next.app_home.as_ref().context("missing install target")?;
        let target = mod_path(home, rev);
        no_links(&target)?;
        if target.exists() {
            ensure!(old.managed_mods.iter().any(|item| item == rev), "mod target already exists");
            ensure!(tree(&target)? == tree(&release(&self.data, rev).join("mod"))?, "mod collision or drift detected");
        }
        if old.current.is_none() {
            ensure!(file_hash(&layout(home))?.is_none(), "layout target already exists");
            ensure!(file_hash(&bootstrap(&self.data))?.is_none(), "bootstrap target already exists");
        }
        let old_layout = file_hash(&layout(home))?;
        let old_binary = file_hash(&bootstrap(&self.data))?;
        next.previous = old.current.clone();
        next.current = Some(rev.to_owned());
        if !next.managed_mods.iter().any(|item| item == rev) {
            next.managed_mods.push(rev.to_owned());
        }
        save(&journal_path(&self.data), &Journal { state: next.clone(), old_layout, old_binary })?;
        self.recover()?;
        Ok(next)
    }
    fn build(&self, state: &mut State, rev: &str) -> Result<()> {
        if state.releases.contains_key(rev) {
            if release(&self.data, rev).exists() { return self.verify_release(state, rev); }
            ensure!(!state.managed_mods.iter().any(|item| item == rev), "managed release is missing");
            // Receipt publication can precede the final rename if interrupted.
            state.releases.remove(rev);
            self.save(state)?;
        }
        let repo = state.source.as_ref().context("no trusted local source configured")?;
        let home = state.app_home.as_ref().context("no install target configured")?;
        let releases = self.data.join("releases");
        directory(&releases)?;
        let final_dir = release(&self.data, rev);
        no_links(&final_dir)?;
        ensure!(!final_dir.exists(), "release target already exists without receipt");
        let tmp = tempfile::Builder::new().prefix(".build-").tempdir_in(&releases)?;
        let archive = tmp.path().join("source.tar");
        run(Command::new("git").arg("-C").arg(repo).args(["archive", "--format=tar", "--output"]).arg(&archive).arg(rev), "source archive failed")?;
        let src = tmp.path().join("source");
        directory(&src)?;
        run(Command::new("tar").arg("-xf").arg(&archive).arg("-C").arg(&src), "source extraction failed")?;
        ensure!(src.join("Cargo.lock").is_file(), "committed Cargo.lock required");
        run(Command::new("cargo").current_dir(&src).args(["build", "--locked", "--release", "--bin", "herdr-kit"]).env("CARGO_TARGET_DIR", tmp.path().join("target")), "release build failed")?;
        let staged = tmp.path().join("release");
        directory(&staged)?;
        fs::copy(tmp.path().join("target/release/herdr-kit"), staged.join("herdr-kit"))?;
        copy_tree(&src.join("mods").join(MOD), &staged.join("mod"))?;
        // The app resolves a mod by folder name. Rewrite only the suite's own
        // literal name in its assets/layout, yielding an isolated generation.
        let new_name = name(rev);
        fn rewrite(dir: &Path, old: &str, new: &str) -> Result<()> {
            for e in fs::read_dir(dir)? {
                let e = e?;
                if e.file_type()?.is_dir() { rewrite(&e.path(), old, new)?; }
                else if e.path().extension().and_then(|s| s.to_str()).is_some_and(|x| x == "js" || x == "json") {
                    let text = fs::read_to_string(e.path())?;
                    fs::write(e.path(), text.replace(old, new))?;
                }
            }
            Ok(())
        }
        rewrite(&staged.join("mod"), MOD, &new_name)?;
        let runtime = format!("export const BINARY = {};\nexport const DATA_DIR = {};\nexport const RELEASE = {};\nexport const MOD_NAME = {};\n", serde_json::to_string(&final_dir.join("herdr-kit").to_string_lossy())?, serde_json::to_string(&self.data.to_string_lossy())?, serde_json::to_string(rev)?, serde_json::to_string(&new_name)?);
        fs::write(staged.join("mod/runtime.js"), runtime)?;
        let layout_text = fs::read_to_string(src.join("layouts/herdr-kit.json"))?;
        ensure!(layout_text.contains(MOD), "layout does not reference suite mod");
        fs::write(staged.join("layout.json"), layout_text.replace(MOD, &new_name))?;
        let check = bounded(Command::new("herdr-app").arg("check").arg(staged.join("mod")).env("HERDR_APP_HOME", home).stdout(Stdio::null()).stderr(Stdio::null()), 30);
        match check {
            Ok(status) => ensure!(status.success(), "native mod check failed"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => bail!("native mod check failed"),
        }
        let receipt = tree(&staged)?;
        // Publish the receipt first: after interruption, a missing, never
        // activated directory can be rebuilt, and a renamed one is verifiable.
        state.releases.insert(rev.to_owned(), receipt);
        self.save(state)?;
        fs::rename(&staged, &final_dir)?;
        File::open(&releases)?.sync_all()?;
        Ok(())
    }
}

fn result(s: &State, available: Option<&str>, status: &str) -> Value {
    json!({"state": status, "current_revision": s.current, "available_revision": available, "previous_revision": s.previous, "source": s.source, "targets": {"app_home": s.app_home}, "next_check": s.next_check})
}

pub fn install(data_dir: &Path, app_home: &Path, source: &Path) -> Result<Value> {
    let lock = Locked::open(data_dir)?;
    let old = lock.state()?;
    let home = absolute(app_home)?;
    ensure!(home.is_dir(), "app home must exist");
    let repo = source_path(source)?;
    if old.current.is_some() {
        ensure!(old.app_home.as_ref() == Some(&home), "different install target already registered");
        lock.verify_active(&old)?;
    }
    let mut next = old.clone();
    next.source = Some(repo);
    next.app_home = Some(home);
    let rev = revision(next.source.as_ref().unwrap())?;
    lock.build(&mut next, &rev)?;
    if old.current.as_deref() == Some(&rev) {
        lock.save(&next)?;
        return Ok(result(&next, Some(&rev), "current"));
    }
    let next = lock.activate(&old, next, &rev)?;
    Ok(result(&next, Some(&rev), "installed"))
}

pub fn status(data_dir: &Path) -> Result<Value> {
    let lock = Locked::open(data_dir)?;
    let state = lock.state()?;
    lock.verify_active(&state)?;
    Ok(result(&state, None, if state.current.is_some() { "installed" } else { "not_installed" }))
}

pub fn source(data_dir: &Path, repo: Option<&Path>) -> Result<Value> {
    let lock = Locked::open(data_dir)?;
    let mut state = lock.state()?;
    if let Some(repo) = repo {
        state.source = Some(source_path(repo)?);
        state.next_check = 0;
        lock.save(&state)?;
    }
    Ok(result(&state, None, "source"))
}

pub fn update(data_dir: &Path, action: &str) -> Result<Value> {
    ensure!(matches!(action, "check" | "apply" | "maintain" | "rollback"), "unknown update action");
    let lock = Locked::open(data_dir)?;
    let mut state = lock.state()?;
    ensure!(state.current.is_some(), "suite is not installed");
    lock.verify_active(&state)?;
    if action == "rollback" {
        let rev = state.previous.clone().context("no previous release")?;
        let old = state.clone();
        let state = lock.activate(&old, state, &rev)?;
        return Ok(result(&state, Some(&rev), "rolled_back"));
    }
    if action == "maintain" && now() < state.next_check { return Ok(result(&state, None, "not_due")); }
    let work = (|| -> Result<Value> {
        let repo = state.source.as_ref().context("no trusted local source configured")?;
        let rev = revision(repo)?;
        if action == "check" { return Ok(result(&state, Some(&rev), if state.current.as_deref() == Some(&rev) { "current" } else { "available" })); }
        if state.current.as_deref() != Some(&rev) {
            let old = state.clone();
            lock.build(&mut state, &rev)?;
            state = lock.activate(&old, state.clone(), &rev)?;
        }
        if action == "maintain" { state.next_check = now().saturating_add(DAY); lock.save(&state)?; }
        Ok(result(&state, Some(&rev), "current"))
    })();
    if action == "maintain" && work.is_err() {
        // If activation was interrupted, leave its journal authoritative.
        if !journal_path(&lock.data).exists() {
            state.next_check = now().saturating_add(BACKOFF);
            lock.save(&state)?;
        }
    }
    work
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_symlink_and_parent_traversal() {
        let t = tempfile::tempdir().unwrap();
        #[cfg(unix)] {
            std::os::unix::fs::symlink(t.path(), t.path().join("link")).unwrap();
            assert!(no_links(&t.path().join("link/file")).is_err());
        }
        assert!(no_links(&t.path().join("../outside")).is_err());
    }
    #[test]
    fn detects_tree_drift() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("main.js"), "a").unwrap();
        let before = tree(t.path()).unwrap();
        fs::write(t.path().join("main.js"), "b").unwrap();
        assert_ne!(before, tree(t.path()).unwrap());
    }
    #[test]
    fn refuses_unowned_release_collision() {
        let t = tempfile::tempdir().unwrap();
        let lock = Locked::open(t.path()).unwrap();
        let rev = "a".repeat(40);
        fs::create_dir_all(release(t.path(), &rev)).unwrap();
        let mut state = State { source: Some(t.path().to_path_buf()), app_home: Some(t.path().to_path_buf()), ..State::default() };
        assert!(lock.build(&mut state, &rev).is_err());
    }
    fn fixture_release(data: &Path, state: &mut State, rev: &str) {
        let r = release(data, rev);
        fs::create_dir_all(r.join("mod")).unwrap();
        fs::write(r.join("mod/main.js"), rev).unwrap();
        fs::write(r.join("layout.json"), rev).unwrap();
        fs::write(r.join("herdr-kit"), rev).unwrap();
        state.releases.insert(rev.to_owned(), tree(&r).unwrap());
    }
    #[test]
    fn activation_recovers_and_rolls_back() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let home = t.path().join("home");
        fs::create_dir(&home).unwrap();
        let lock = Locked::open(&data).unwrap();
        let mut first = State { app_home: Some(home.clone()), ..State::default() };
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        fixture_release(&data, &mut first, &a);
        let empty = State::default();
        let first = lock.activate(&empty, first, &a).unwrap();
        let mut second = first.clone();
        fixture_release(&data, &mut second, &b);
        second.previous = Some(a.clone());
        second.current = Some(b.clone());
        second.managed_mods.push(b.clone());
        let old_layout = file_hash(&layout(&home)).unwrap();
        let old_binary = file_hash(&bootstrap(&data)).unwrap();
        save(&journal_path(&data), &Journal { state: second, old_layout, old_binary }).unwrap();
        // Simulate a crash after the layout switch and before the bootstrap switch.
        atomic(&layout(&home), b.as_bytes(), false).unwrap();
        lock.recover().unwrap();
        let recovered = lock.state().unwrap();
        lock.verify_active(&recovered).unwrap();
        assert_eq!(recovered.current.as_deref(), Some(b.as_str()));
        let rolled = lock.activate(&recovered, recovered.clone(), &a).unwrap();
        lock.verify_active(&rolled).unwrap();
        assert_eq!(rolled.current.as_deref(), Some(a.as_str()));
    }
    #[test]
    fn global_lock_excludes_another_writer() {
        let t = tempfile::tempdir().unwrap();
        let lock = Locked::open(t.path()).unwrap();
        let other = OpenOptions::new().write(true).open(t.path().join("manager.lock")).unwrap();
        assert!(other.try_lock_exclusive().is_err());
        drop(lock);
        other.try_lock_exclusive().unwrap();
    }
    #[test]
    fn refuses_existing_unowned_mod_even_if_identical() {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        let home = t.path().join("home");
        fs::create_dir(&home).unwrap();
        let lock = Locked::open(&data).unwrap();
        let rev = "a".repeat(40);
        let mut next = State { app_home: Some(home.clone()), ..State::default() };
        fixture_release(&data, &mut next, &rev);
        copy_tree(&release(&data, &rev).join("mod"), &mod_path(&home, &rev)).unwrap();
        assert!(lock.activate(&State::default(), next, &rev).is_err());
    }
}
