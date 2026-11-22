use anyhow::{Result, ensure};
use kit_core::paths::{SymlinkMessage, no_links};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::ids::TaskId;

#[derive(Clone, Debug)]
pub struct CoordPaths {
    pub dir: PathBuf,
    pub journal: PathBuf,
    pub state: PathBuf,
    pub lock: PathBuf,
    pub up_lock: PathBuf,
    pub watcher_lock: PathBuf,
    pub heartbeat: PathBuf,
    pub config: PathBuf,
    pub briefs: PathBuf,
    pub results: PathBuf,
    pub sessions: PathBuf,
    pub roles: PathBuf,
    pub extensions: PathBuf,
    pub computer_config: PathBuf,
    pub home: PathBuf,
}

impl CoordPaths {
    pub fn new(root: &Path) -> Self {
        let dir = root.join("coord");
        Self {
            journal: dir.join("journal"),
            state: dir.join("state.json"),
            lock: dir.join("lock"),
            up_lock: dir.join("up.lock"),
            watcher_lock: dir.join("watcher.lock"),
            heartbeat: dir.join("heartbeat.json"),
            config: dir.join("config.json"),
            briefs: dir.join("briefs"),
            results: dir.join("results"),
            sessions: dir.join("sessions"),
            roles: dir.join("roles"),
            extensions: dir.join("extensions.json"),
            computer_config: dir.join("omp-computer.yml"),
            home: dir.join("home"),
            dir,
        }
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.dir,
            &self.journal,
            &self.briefs,
            &self.results,
            &self.sessions,
            &self.roles,
            &self.home,
        ] {
            no_links(dir, SymlinkMessage::WithPath)?;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            no_links(dir, SymlinkMessage::WithPath)?;
            ensure!(
                fs::metadata(dir)?.is_dir(),
                "expected directory: {}",
                dir.display()
            );
        }
        fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))?;
        for file in [
            &self.state,
            &self.lock,
            &self.up_lock,
            &self.watcher_lock,
            &self.heartbeat,
            &self.config,
            &self.computer_config,
        ] {
            no_links(file, SymlinkMessage::WithPath)?;
        }
        Ok(())
    }

    /// Ensure the per-session OMP computer-use config overlay exists with `computer.enabled: true`.
    pub fn ensure_computer_config(&self) -> Result<&Path> {
        const OVERLAY: &[u8] = b"computer:\n  enabled: true\nbrowser:\n  enabled: true\n";
        self.ensure()?;
        let needs_write = match fs::read(&self.computer_config) {
            Ok(existing) => existing != OVERLAY,
            Err(_) => true,
        };
        if needs_write {
            kit_core::paths::write_atomic(
                &self.computer_config,
                OVERLAY,
                SymlinkMessage::WithPath,
            )?;
            fs::set_permissions(&self.computer_config, fs::Permissions::from_mode(0o600))?;
        }
        Ok(&self.computer_config)
    }

    pub fn month_file(&self, ts_ms: u64) -> PathBuf {
        let (year, month) = utc_month(ts_ms);
        self.journal.join(format!("{year:04}-{month:02}.jsonl"))
    }
    pub fn brief(&self, name: &str) -> PathBuf {
        self.briefs.join(format!("{name}.md"))
    }
    pub fn result(&self, task: &TaskId) -> PathBuf {
        self.results.join(format!("{}.md", task.as_str()))
    }
    pub fn session_dir(&self, name: &str) -> PathBuf {
        self.sessions.join(name)
    }
}

impl super::Ctx {
    /// Acquire before the journal lock; never wait for a terminal while holding it.
    pub fn terminal_lock(&self, terminal: &str) -> Result<std::fs::File> {
        use fs2::FileExt;
        use std::fmt::Write;
        self.paths.ensure()?;
        let dir = self.paths.dir.join("locks");
        no_links(&dir, SymlinkMessage::WithPath)?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        no_links(&dir, SymlinkMessage::WithPath)?;
        let mut name = String::with_capacity(terminal.len() * 2);
        for byte in terminal.as_bytes() {
            write!(&mut name, "{byte:02x}")?;
        }
        let path = dir.join(name);
        no_links(&path, SymlinkMessage::WithPath)?;
        let file = fs::OpenOptions::new().create(true).write(true).open(path)?;
        file.try_lock_exclusive().map_err(|e| {
            super::error::not_ready("terminal_busy", format!("terminal {terminal} is busy: {e}"))
        })?;
        Ok(file)
    }
}

pub fn utc_month(ts_ms: u64) -> (i32, u32) {
    // Gregorian civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = (ts_ms / 86_400_000) as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = mp + if mp < 10 { 3 } else { -9 };
    ((year + i64::from(month <= 2)) as i32, month as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn months_cross_year_and_leap_day() {
        assert_eq!(utc_month(0), (1970, 1));
        assert_eq!(utc_month(1_704_067_199_999), (2023, 12));
        assert_eq!(utc_month(1_704_067_200_000), (2024, 1));
        assert_eq!(utc_month(1_709_251_200_000), (2024, 3));
    }
    #[test]
    fn refuses_symlinked_paths() {
        let temp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(temp.path(), temp.path().join("coord")).unwrap();
        assert!(CoordPaths::new(temp.path()).ensure().is_err());
    }
}
