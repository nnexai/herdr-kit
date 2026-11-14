use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use super::paths::{CoordPaths, no_links};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub autostart: bool,
    pub coordinator_role: String,
    pub nudge_coordinator_secs: u64,
    pub nudge_delegate_secs: u64,
    pub stall_secs: u64,
    pub start_blocked_secs: u64,
    pub blocked_reminder_secs: u64,
    pub blocked_reminder_cap_secs: u64,
    pub reconcile_secs: u64,
    pub idle_restart_hours: u64,
    pub worktree_root: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            autostart: true,
            coordinator_role: "coord-main".into(),
            nudge_coordinator_secs: 10,
            nudge_delegate_secs: 60,
            stall_secs: 300,
            start_blocked_secs: 30,
            blocked_reminder_secs: 600,
            blocked_reminder_cap_secs: 14_400,
            reconcile_secs: 120,
            idle_restart_hours: 8,
            worktree_root: "~/.herdr/worktrees".into(),
        }
    }
}

impl Config {
    pub fn load(paths: &CoordPaths) -> Result<Option<Config>> {
        no_links(&paths.config)?;
        match fs::read(&paths.config) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).context("invalid coordinator config")?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, paths: &CoordPaths) -> Result<()> {
        paths.ensure()?;
        no_links(&paths.config)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&paths.dir)?;
        tmp.write_all(&serde_json::to_vec(self)?)?;
        tmp.as_file().sync_all()?;
        tmp.persist(&paths.config).map_err(|e| e.error)?;
        File::open(&paths.dir)?.sync_all()?;
        Ok(())
    }
    pub fn worktree_root(&self) -> Result<PathBuf> {
        if let Some(rest) = self.worktree_root.strip_prefix("~/") {
            let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
            ensure!(home.is_absolute(), "HOME must be absolute");
            Ok(home.join(rest))
        } else {
            Ok(PathBuf::from(&self.worktree_root))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_roundtrip_and_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(temp.path());
        assert!(Config::load(&paths).unwrap().is_none());
        Config::default().save(&paths).unwrap();
        assert_eq!(Config::load(&paths).unwrap().unwrap().stall_secs, 300);
        assert!(serde_json::from_str::<Config>(r#"{"unknown":true}"#).is_err());
        assert_eq!(
            serde_json::from_str::<Config>("{}")
                .unwrap()
                .nudge_delegate_secs,
            60
        );
    }
}
