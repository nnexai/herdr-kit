//! Local extensions: helper commands the person configures for one machine in
//! `<data-dir>/coord/extensions.json`. Nothing ships with the product. The
//! coordinator sees every extension in its startup read and in `kit board`
//! (with current output for `board: true` ones) and runs one on demand with
//! `kit ext <name>`.
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::process::Command;

use anyhow::{Context, Result, ensure};
use kit_core::paths::{SymlinkMessage, no_links};
use serde::Deserialize;
use serde_json::{Value, json};

use super::error;
use super::exec;
use super::paths::CoordPaths;

const MAX_TIMEOUT_SECS: u64 = 30;
/// Output kept for `kit ext <name>`.
const MAX_OUTPUT: usize = 64 * 1024;
/// Output shown per extension in the board and startup read.
const BOARD_OUTPUT: usize = 2000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    /// One line telling the coordinator what the output means.
    pub description: String,
    /// Program and arguments, run without a shell.
    pub command: Vec<String>,
    /// Run it for every startup read and `kit board`.
    #[serde(default)]
    pub board: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    5
}

pub struct Outcome {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub output: String,
    pub error: Option<String>,
}

pub fn load(paths: &CoordPaths) -> Result<BTreeMap<String, Extension>> {
    no_links(&paths.extensions, SymlinkMessage::WithPath)?;
    let bytes = match fs::read(&paths.extensions) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading {}", paths.extensions.display()));
        }
    };
    let map: BTreeMap<String, Extension> =
        serde_json::from_slice(&bytes).context("invalid extensions.json")?;
    for (name, ext) in &map {
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid extension name: {name}"
        );
        ensure!(
            ext.command.first().is_some_and(|p| !p.is_empty()),
            "extension {name} has an empty command"
        );
        ensure!(
            !ext.description.trim().is_empty() && !ext.description.contains('\n'),
            "extension {name} needs a one-line description"
        );
        ensure!(
            (1..=MAX_TIMEOUT_SECS).contains(&ext.timeout_secs),
            "extension {name}: timeout_secs must be between 1 and {MAX_TIMEOUT_SECS}"
        );
    }
    Ok(map)
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

pub fn run(name: &str, ext: &Extension) -> Outcome {
    let mut cmd = Command::new(&ext.command[0]);
    // The helper never acts with the coordinator's caller identity.
    cmd.args(&ext.command[1..]).env_remove("HC_TERMINAL_ID");
    match exec::run(cmd, &format!("extension {name}"), ext.timeout_secs) {
        Ok(captured) => {
            let output = truncate(String::from_utf8_lossy(&captured.stdout).trim(), MAX_OUTPUT);
            let error = (!captured.status.success()).then(|| {
                let stderr = String::from_utf8_lossy(&captured.stderr);
                let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                truncate(
                    &format!("exited with {}: {}", captured.status, line.trim()),
                    500,
                )
            });
            Outcome {
                ok: error.is_none(),
                exit_code: captured.status.code(),
                output,
                error,
            }
        }
        Err(e) => Outcome {
            ok: false,
            exit_code: None,
            output: String::new(),
            error: Some(format!("{e:#}")),
        },
    }
}

/// `kit ext`: the configured extensions.
pub fn list(paths: &CoordPaths) -> Result<Value> {
    let map = load(paths).map_err(|e| error::refused("extensions_invalid", format!("{e:#}")))?;
    Ok(json!({
        "extensions": map.iter().map(|(name, ext)| json!({
            "name": name,
            "description": ext.description,
            "command": ext.command,
            "board": ext.board,
            "timeout_secs": ext.timeout_secs,
        })).collect::<Vec<_>>(),
    }))
}

/// `kit ext <name>`: run one extension now.
pub fn query(paths: &CoordPaths, name: &str) -> Result<Value> {
    let mut map =
        load(paths).map_err(|e| error::refused("extensions_invalid", format!("{e:#}")))?;
    let ext = map.remove(name).ok_or_else(|| {
        error::refused("extension_not_found", format!("unknown extension: {name}"))
    })?;
    let outcome = run(name, &ext);
    let parsed = serde_json::from_str::<Value>(&outcome.output).ok();
    Ok(json!({
        "name": name,
        "description": ext.description,
        "ok": outcome.ok,
        "exit_code": outcome.exit_code,
        "output": outcome.output,
        "json": parsed,
        "error": outcome.error,
    }))
}

/// The "Local extensions" section for the startup read and `kit board`;
/// empty when the machine has none.
pub fn section(paths: &CoordPaths) -> String {
    let map = match load(paths) {
        Ok(map) if map.is_empty() => return String::new(),
        Ok(map) => map,
        Err(e) => {
            return format!(
                "\n## Local extensions\n\nThe extensions file is invalid ({e:#}); tell the person.\n"
            );
        }
    };
    let mut out = String::from("\n## Local extensions\n\n");
    for (name, ext) in &map {
        out.push_str(&format!(
            "- `{name}` (`kit ext {name}`): {}\n",
            ext.description.trim()
        ));
        if !ext.board {
            continue;
        }
        let outcome = run(name, ext);
        if let Some(error) = &outcome.error {
            out.push_str(&format!("  Now: unavailable ({error})\n"));
        } else if outcome.output.is_empty() {
            out.push_str("  Now: (no output)\n");
        } else {
            let text = truncate(&outcome.output, BOARD_OUTPUT);
            let mut lines = text.lines();
            out.push_str(&format!("  Now: {}\n", lines.next().unwrap_or("")));
            for line in lines {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn section_lists_runs_board_extensions_and_reports_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(tmp.path());
        paths.ensure().unwrap();
        assert_eq!(section(&paths), "");
        let helper = tmp.path().join("helper");
        fs::write(&helper, "#!/bin/sh\nprintf 'first\\nsecond\\n'\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        let config = json!({
            "ok": {"description": "Prints two lines.", "command": [helper], "board": true},
            "quiet": {"description": "Only on demand.", "command": ["/nonexistent/x"]},
            "broken": {"description": "Fails.", "command": ["/nonexistent/y"], "board": true},
        });
        fs::write(&paths.extensions, config.to_string()).unwrap();
        let text = section(&paths);
        assert!(
            text.contains("- `ok` (`kit ext ok`): Prints two lines.\n  Now: first\n    second\n")
        );
        assert!(text.ends_with("- `quiet` (`kit ext quiet`): Only on demand.\n"));
        assert!(text.contains("- `broken` (`kit ext broken`): Fails.\n  Now: unavailable ("));
        let queried = query(&paths, "ok").unwrap();
        assert_eq!(queried["ok"], true);
        assert_eq!(queried["output"], "first\nsecond");
        assert_eq!(
            error::exit_of(&query(&paths, "missing").unwrap_err()).0,
            "extension_not_found"
        );
        fs::write(
            &paths.extensions,
            r#"{"bad name": {"description": "x", "command": ["x"]}}"#,
        )
        .unwrap();
        assert!(section(&paths).contains("invalid"));
        assert_eq!(
            error::exit_of(&list(&paths).unwrap_err()).0,
            "extensions_invalid"
        );
    }
}
