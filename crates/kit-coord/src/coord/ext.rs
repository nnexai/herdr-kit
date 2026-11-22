//! Local extensions: helper commands the person configures for one machine in
//! `<data-dir>/coord/extensions.json`. Nothing ships with the product. The
//! coordinator's startup read lists every extension with the current output of
//! `startup: true` ones, `kit board` lists them, and `kit ext <name>` runs one
//! on demand.
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use kit_core::paths::{SymlinkMessage, no_links};
use kit_core::process::{ProcessError, run_bounded};
use serde::Deserialize;
use serde_json::{Value, json};

use super::error;
use super::paths::CoordPaths;

/// Helpers run concurrently at coordinator start, so this bounds the delay.
const MAX_TIMEOUT_SECS: u64 = 10;
/// Output kept for `kit ext <name>`; more fails the helper while it runs.
const MAX_OUTPUT: u64 = 64 * 1024;
const MAX_STDERR: u64 = 16 * 1024;
/// Output shown per extension in the startup read.
const BOARD_OUTPUT: usize = 2000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    /// One line telling the coordinator what the output means.
    pub description: String,
    /// Program and arguments, run without a shell.
    pub command: Vec<String>,
    /// Run it for the coordinator's startup read.
    #[serde(default)]
    pub startup: bool,
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

pub fn run(ext: &Extension) -> Outcome {
    let mut cmd = tokio::process::Command::new(&ext.command[0]);
    // The helper never acts with the caller's identity.
    cmd.args(&ext.command[1..]).env_remove("HC_TERMINAL_ID");
    let timeout = Duration::from_secs(ext.timeout_secs);
    // Its own thread and runtime: callers may already run inside the CLI's runtime.
    let result = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(ProcessError::Spawn)?
                    .block_on(run_bounded(cmd, timeout, MAX_OUTPUT, MAX_STDERR))
            })
            .join()
            .unwrap_or_else(|_| Err(ProcessError::Wait(std::io::Error::other("runner panicked"))))
    });
    match result {
        Ok(captured) => {
            let output = String::from_utf8_lossy(&captured.stdout).trim().to_owned();
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
            error: Some(e.to_string()),
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
            "startup": ext.startup,
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
    let outcome = run(&ext);
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

/// The "Local extensions" section; empty when the machine has none. With
/// `run_startup`, `startup: true` helpers run (concurrently) and their output
/// is shown; the startup read does this, `kit board` only lists them.
pub fn section(paths: &CoordPaths, run_startup: bool) -> String {
    let map = match load(paths) {
        Ok(map) if map.is_empty() => return String::new(),
        Ok(map) => map,
        Err(e) => {
            return format!(
                "\n## Local extensions\n\nThe extensions file is invalid ({e:#}); tell the person.\n"
            );
        }
    };
    let outcomes: BTreeMap<&str, Outcome> = std::thread::scope(|scope| {
        let running = map
            .iter()
            .filter(|(_, ext)| run_startup && ext.startup)
            .map(|(name, ext)| (name.as_str(), scope.spawn(|| run(ext))))
            .collect::<Vec<_>>();
        running
            .into_iter()
            .filter_map(|(name, handle)| Some((name, handle.join().ok()?)))
            .collect()
    });
    let mut out = String::from("\n## Local extensions\n\n");
    for (name, ext) in &map {
        out.push_str(&format!(
            "- `{name}` (`kit ext {name}`): {}\n",
            ext.description.trim()
        ));
        let Some(outcome) = outcomes.get(name.as_str()) else {
            continue;
        };
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
        assert_eq!(section(&paths, true), "");
        let helper = tmp.path().join("helper");
        fs::write(&helper, "#!/bin/sh\nprintf 'first\\nsecond\\n'\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        let config = json!({
            "ok": {"description": "Prints two lines.", "command": [helper], "startup": true},
            "quiet": {"description": "Only on demand.", "command": ["/nonexistent/x"]},
            "broken": {"description": "Fails.", "command": ["/nonexistent/y"], "startup": true},
            "noisy": {"description": "Floods.", "command": ["/bin/sh", "-c", "yes x"], "startup": true},
            "slow": {"description": "Hangs.", "command": ["/bin/sleep", "30"], "startup": true, "timeout_secs": 1},
        });
        fs::write(&paths.extensions, config.to_string()).unwrap();
        let started = std::time::Instant::now();
        let text = section(&paths, true);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let has = |part: &str| assert!(text.contains(part), "{part}\n{text}");
        has("- `ok` (`kit ext ok`): Prints two lines.\n  Now: first\n    second\n");
        has("- `quiet` (`kit ext quiet`): Only on demand.\n- `slow`");
        has("- `broken` (`kit ext broken`): Fails.\n  Now: unavailable (");
        has("Floods.\n  Now: unavailable (command stdout exceeded 65536 byte limit)\n");
        has("Hangs.\n  Now: unavailable (command timed out after 1000ms)\n");
        let listed = section(&paths, false);
        assert!(listed.contains("- `ok` (`kit ext ok`): Prints two lines.\n- "));
        assert!(!listed.contains("Now:"));
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
        assert!(section(&paths, true).contains("invalid"));
        assert_eq!(
            error::exit_of(&list(&paths).unwrap_err()).0,
            "extensions_invalid"
        );
    }
}
