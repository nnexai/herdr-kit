use anyhow::{Context, Result, bail};
use std::path::PathBuf;

pub fn data_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return Ok(home()?.join("Library/Application Support/herdr-kit"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(value) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                bail!("XDG_DATA_HOME must be absolute");
            }
            return Ok(path.join("herdr-kit"));
        }
        Ok(home()?.join(".local/share/herdr-kit"))
    }
}

fn home() -> Result<PathBuf> {
    let path = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    if !path.is_absolute() {
        bail!("HOME must be absolute");
    }
    Ok(path)
}
