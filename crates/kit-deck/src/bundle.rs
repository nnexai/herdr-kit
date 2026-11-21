use anyhow::{Context, Result, bail, ensure};
use kit_core::paths::{SymlinkMessage, no_links};
use serde::Serialize;
use serde_json::json;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const PLUGIN_DIR_NAME: &str = "dev.herdr.kit.sdPlugin";

const MANIFEST_JSON: &str = include_str!("../assets/manifest.json");
const ICONS: &[(&str, &str)] = &[
    (
        "icons/plugin.svg",
        include_str!("../assets/icons/plugin.svg"),
    ),
    (
        "icons/category.svg",
        include_str!("../assets/icons/category.svg"),
    ),
    (
        "icons/overview.svg",
        include_str!("../assets/icons/overview.svg"),
    ),
    ("icons/inbox.svg", include_str!("../assets/icons/inbox.svg")),
    (
        "icons/inbox-action.svg",
        include_str!("../assets/icons/inbox-action.svg"),
    ),
    ("icons/agent.svg", include_str!("../assets/icons/agent.svg")),
    (
        "icons/coordinator.svg",
        include_str!("../assets/icons/coordinator.svg"),
    ),
    (
        "icons/limits.svg",
        include_str!("../assets/icons/limits.svg"),
    ),
    (
        "icons/limit-bucket.svg",
        include_str!("../assets/icons/limit-bucket.svg"),
    ),
];
const PI_FILES: &[(&str, &str)] = &[
    ("pi/common.js", include_str!("../assets/pi/common.js")),
    ("pi/agent.html", include_str!("../assets/pi/agent.html")),
    ("pi/limits.html", include_str!("../assets/pi/limits.html")),
    (
        "pi/limit-bucket.html",
        include_str!("../assets/pi/limit-bucket.html"),
    ),
    (
        "pi/inbox-action.html",
        include_str!("../assets/pi/inbox-action.html"),
    ),
];

#[derive(Serialize)]
pub struct InstallReport {
    pub installed: bool,
    pub plugin_dir: PathBuf,
    pub binary: PathBuf,
    pub mode: &'static str,
    pub actions: usize,
}

#[derive(Serialize)]
pub struct UninstallReport {
    pub removed: bool,
    pub plugin_dir: PathBuf,
}

pub fn default_plugins_dir() -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    ensure!(home.is_absolute(), "HOME must be absolute");
    #[cfg(target_os = "macos")]
    {
        return Ok(home.join("Library/Application Support/opendeck/plugins"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let native = home.join(".config/opendeck");
        if native.exists() {
            return Ok(native.join("plugins"));
        }
        let flatpak = home.join(".var/app/me.amankhanna.opendeck/config/opendeck");
        if flatpak.exists() {
            return Ok(flatpak.join("plugins"));
        }
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
            let xdg = PathBuf::from(xdg);
            if !xdg.is_absolute() {
                bail!("XDG_CONFIG_HOME must be absolute");
            }
            return Ok(xdg.join("opendeck/plugins"));
        }
        Ok(native.join("plugins"))
    }
}

fn resolve_abs(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub fn install(
    plugins_dir: &Path,
    binary_path: &Path,
    explicit_data_dir: Option<&Path>,
    wrapper_mode: bool,
) -> Result<InstallReport> {
    let plugins_dir = resolve_abs(plugins_dir)?;
    no_links(&plugins_dir, SymlinkMessage::WithPath)?;
    fs::create_dir_all(&plugins_dir)?;
    let target = plugins_dir.join(PLUGIN_DIR_NAME);
    no_links(&target, SymlinkMessage::WithPath)?;

    let staging = tempfile::Builder::new()
        .prefix(".dev.herdr.kit.sdPlugin-")
        .tempdir_in(&plugins_dir)?;
    let root = staging.path();

    write_file(&root.join("manifest.json"), MANIFEST_JSON.as_bytes(), false)?;
    for (rel, content) in ICONS {
        write_file(&root.join(rel), content.as_bytes(), false)?;
    }
    for (rel, content) in PI_FILES {
        write_file(&root.join(rel), content.as_bytes(), false)?;
    }
    if let Some(data_dir) = explicit_data_dir {
        let cfg = serde_json::to_vec_pretty(&json!({ "data_dir": data_dir }))?;
        write_file(&root.join("config.json"), &cfg, false)?;
    }

    let bin_dest = root.join("bin/kit-deck");
    let source_exe = std::fs::canonicalize(binary_path).unwrap_or_else(|_| binary_path.to_owned());
    if wrapper_mode {
        let escaped = source_exe.display().to_string().replace('\'', "'\\''");
        let script = format!("#!/bin/sh\nexec '{escaped}' \"$@\"\n");
        write_file(&bin_dest, script.as_bytes(), true)?;
    } else {
        let bytes = fs::read(&source_exe)
            .with_context(|| format!("failed to read binary {}", source_exe.display()))?;
        write_file(&bin_dest, &bytes, true)?;
    }

    let backup = plugins_dir.join(".dev.herdr.kit.sdPlugin.old");
    if backup.exists() {
        no_links(&backup, SymlinkMessage::WithPath)?;
        fs::remove_dir_all(&backup)?;
    }
    if target.exists() {
        fs::rename(&target, &backup)?;
    }
    let staged_dir = staging.keep();
    if let Err(err) = fs::rename(&staged_dir, &target) {
        if backup.exists() && !target.exists() {
            let _ = fs::rename(&backup, &target);
        }
        let _ = fs::remove_dir_all(&staged_dir);
        return Err(err.into());
    }
    if backup.exists() {
        let _ = fs::remove_dir_all(&backup);
    }
    Ok(InstallReport {
        installed: true,
        binary: target.join("bin/kit-deck"),
        plugin_dir: target,
        mode: if wrapper_mode { "wrapper" } else { "copy" },
        actions: 7,
    })
}

pub fn uninstall(plugins_dir: &Path) -> Result<UninstallReport> {
    let plugins_dir = resolve_abs(plugins_dir)?;
    let target = plugins_dir.join(PLUGIN_DIR_NAME);
    no_links(&target, SymlinkMessage::WithPath)?;
    let removed = if target.exists() {
        fs::remove_dir_all(&target)?;
        true
    } else {
        false
    };
    Ok(UninstallReport {
        removed,
        plugin_dir: target,
    })
}

fn write_file(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    file.sync_all()?;
    Ok(())
}
