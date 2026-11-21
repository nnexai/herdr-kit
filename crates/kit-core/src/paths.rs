use anyhow::{Context, Result, bail, ensure};
use std::fs;
use std::path::{Component, Path, PathBuf};

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

/// Resolve an explicit or default data root without canonicalizing it.
pub fn data_root(arg: Option<PathBuf>) -> Result<PathBuf> {
    let root = arg.map(Ok).unwrap_or_else(data_dir)?;
    if root.is_absolute() {
        Ok(root)
    } else {
        Ok(std::env::current_dir()
            .context("current directory unavailable")?
            .join(root))
    }
}

/// Preserve each CLI's existing symlink diagnostic.
#[derive(Clone, Copy)]
pub enum SymlinkMessage {
    Generic,
    WithPath,
}

/// Refuse existing symlinks, including ancestors, and unsafe components.
/// Missing components are permitted for creation.
pub fn no_links(path: &Path, message: SymlinkMessage) -> Result<()> {
    ensure!(path.is_absolute(), "path must be absolute");
    let mut part = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => part.push(component.as_os_str()),
            _ => bail!("unsafe path"),
        }
        match fs::symlink_metadata(&part) {
            Ok(metadata) if metadata.file_type().is_symlink() => match message {
                SymlinkMessage::Generic => bail!("symlink in managed path"),
                SymlinkMessage::WithPath => bail!("symlink in managed path: {}", part.display()),
            },
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Return the validated absolute user home directory from `HOME`.
pub fn home() -> Result<PathBuf> {
    let path = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    if !path.is_absolute() {
        bail!("HOME must be absolute");
    }
    Ok(path)
}

/// Return the default Antigravity CLI data directory (`$HOME/.gemini/antigravity-cli`).
pub fn agy_data_dir() -> Result<PathBuf> {
    Ok(home()?.join(".gemini/antigravity-cli"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agy_data_dir_joins_validated_home() {
        let dir = agy_data_dir().unwrap();
        assert!(dir.is_absolute());
        assert!(dir.ends_with(".gemini/antigravity-cli"));
    }

    #[test]
    fn symlink_ancestors_keep_each_cli_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(temp.path(), &link).unwrap();
        let path = link.join("missing/child");
        assert_eq!(
            no_links(&path, SymlinkMessage::Generic)
                .unwrap_err()
                .to_string(),
            "symlink in managed path"
        );
        assert_eq!(
            no_links(&path, SymlinkMessage::WithPath)
                .unwrap_err()
                .to_string(),
            format!("symlink in managed path: {}", link.display())
        );
        for message in [SymlinkMessage::Generic, SymlinkMessage::WithPath] {
            assert_eq!(
                no_links(Path::new("relative"), message)
                    .unwrap_err()
                    .to_string(),
                "path must be absolute"
            );
            assert_eq!(
                no_links(&temp.path().join("../outside"), message)
                    .unwrap_err()
                    .to_string(),
                "unsafe path"
            );
            no_links(&temp.path().join("missing/child"), message).unwrap();
        }
    }
}
