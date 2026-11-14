//! Public coordinator readiness contract. No manager state is read here.
use std::{fs, io::Read, os::unix::fs::{MetadataExt, PermissionsExt}, path::{Path, PathBuf}, process::{Command, Stdio}, time::{Duration, Instant}};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;

pub const BUILD_REVISION: &str = env!("KIT_COORD_BUILD_REVISION");

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Active {
    pub schema_version: u32,
    pub revision: String,
    pub executable: PathBuf,
}

pub fn read(root: &Path) -> Result<Option<Active>> {
    let path = root.join("kit-coord/active.json");
    crate::coord::paths::no_links(&path)?;
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 16 * 1024, "coordinator deployment descriptor is too large");
    let active: Active = serde_json::from_slice(&bytes).context("invalid coordinator deployment descriptor")?;
    ensure!(active.schema_version == 1, "unsupported coordinator deployment descriptor");
    ensure!((active.revision.len() == 40 || active.revision.len() == 64) && active.revision.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)), "invalid coordinator deployment revision");
    ensure!(active.executable.is_absolute(), "coordinator deployment executable must be absolute");
    target_metadata(&active)?;
    Ok(Some(active))
}

fn target_metadata(active: &Active) -> Result<fs::Metadata> {
    crate::coord::paths::no_links(&active.executable)?;
    let metadata = fs::metadata(&active.executable).context("coordinator deployment executable is missing")?;
    ensure!(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0, "coordinator deployment target is not executable");
    Ok(metadata)
}

pub fn ensure_current(root: &Path) -> Result<()> {
    if let Some(active) = read(root)? {
        ensure!(active.revision == BUILD_REVISION, "selected coordinator revision differs from this executable; use its matching installed CLI/mod");
    }
    Ok(())
}

fn verify_version(active: &Active) -> Result<()> {
    let mut output = tempfile::tempfile()?;
    let mut child = Command::new(&active.executable).arg("--version")
        .stdin(Stdio::null()).stdout(output.try_clone()?).stderr(Stdio::null()).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "coordinator deployment version probe failed");
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("coordinator deployment version probe timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    use std::io::{Seek, SeekFrom};
    output.seek(SeekFrom::Start(0))?;
    let mut text = String::new();
    output.take(1025).read_to_string(&mut text)?;
    ensure!(text.len() <= 1024 && text.starts_with("kit-coord ") && text.trim_end().ends_with(&format!(" ({})", active.revision)), "coordinator deployment executable revision does not match descriptor");
    Ok(())
}

#[derive(Default)]
pub struct Replacement {
    checked: Option<(Active, u64, u64, u64, i64, i64)>,
    failure: Option<String>,
}
impl Replacement {
    pub fn reject(&mut self, error: String) { self.failure = Some(error); }
    /// A bad unchanged candidate is probed once, not exec'ed on each heartbeat.
    pub fn selected(&mut self, root: &Path, running: &str) -> Result<Option<Active>> {
        let Some(active) = read(root)? else { self.checked = None; self.failure = None; return Ok(None); };
        if active.revision == running { self.checked = None; self.failure = None; return Ok(None); }
        let metadata = target_metadata(&active)?;
        let unchanged = self.checked.as_ref().is_some_and(|(previous, dev, ino, len, mtime, nsec)| {
            previous == &active && *dev == metadata.dev() && *ino == metadata.ino()
                && *len == metadata.len() && *mtime == metadata.mtime() && *nsec == metadata.mtime_nsec()
        });
        if !unchanged {
            self.failure = verify_version(&active).err().map(|error| error.to_string());
            self.checked = Some((active.clone(), metadata.dev(), metadata.ino(), metadata.len(), metadata.mtime(), metadata.mtime_nsec()));
        }
        if let Some(error) = &self.failure { anyhow::bail!("{error}"); }
        Ok(Some(active))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_candidate_is_retained_and_repaired_target_is_rechecked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("kit-coord")).unwrap();
        let executable = root.join("candidate");
        let counter = root.join("probes");
        let revision = "b".repeat(40);
        fs::write(&executable, format!("#!/bin/sh\necho probe >> '{}'\necho wrong-version\n", counter.display())).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(root.join("kit-coord/active.json"), serde_json::json!({"schema_version":1,"revision":revision,"executable":executable}).to_string()).unwrap();
        let mut replacement = Replacement::default();
        assert!(replacement.selected(root, BUILD_REVISION).is_err());
        assert!(replacement.selected(root, BUILD_REVISION).is_err());
        assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 1);
        fs::write(&executable, format!("#!/bin/sh\necho 'kit-coord 0.1.0 ({revision})'\n")).unwrap();
        assert_eq!(replacement.selected(root, BUILD_REVISION).unwrap().unwrap().revision, revision);
        fs::remove_file(executable).unwrap();
        assert!(replacement.selected(root, BUILD_REVISION).is_err());
    }
}
