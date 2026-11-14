//! Bounded subprocess adapter, including retained pre-extraction rollback inputs.
use super::*;
use std::thread;

fn activation_failure(code: Option<i32>, stderr: &[u8]) -> Value {
    let message = String::from_utf8_lossy(stderr).into_owned();
    let forbidden = code == Some(3)
        && serde_json::from_slice::<Value>(stderr).ok()
            .and_then(|value| value["error"]["code"].as_str().map(str::to_owned)).as_deref() == Some("forbidden_caller");
    if forbidden {
        json!({"state":"skipped","reason":"forbidden_caller","message":message})
    } else {
        json!({"state":if message.contains("unrecognized subcommand") || message.contains("unexpected argument") {"unsupported"} else {"failed"},"message":message})
    }
}

pub fn after_activation(root: &Path) -> Value {
    if !root.join("coord/config.json").exists() { return json!({"state":"disabled"}); }
    let result = (|| -> Result<Value> {
        let state = state::load(root)?;
        let binary = if state.selection.is_empty() {
            "herdr-kit"
        } else if let Some(revision) = state.selection.get("kit-coord") {
            &state.package_releases.get(revision).and_then(|packages| packages.get("kit-coord"))
                .and_then(|receipt| receipt.definition.binary.as_ref()).context("missing selected coordinator executable")?.name
        } else { return Ok(json!({"state":"skipped","reason":"coordinator_not_selected"})); };
        let mut command = Command::new(root.join("bin").join(binary));
        command.arg("--data-dir").arg(root);
        // A retained legacy selection is a rollback artifact, never a new CLI alias.
        if binary == "herdr-kit" { command.arg("coord"); }
        let mut child = command.args(["up", "--if-enabled"])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if child.try_wait()?.is_some() {
                let output = child.wait_with_output()?;
                if !output.status.success() { return Ok(activation_failure(output.status.code(), &output.stderr)); }
                return Ok(serde_json::from_slice(&output.stdout)?);
            }
            if Instant::now() >= deadline {
                child.kill()?;
                let _ = child.wait();
                bail!("coordinator activation timed out");
            }
            thread::sleep(Duration::from_millis(50));
        }
    })();
    result.unwrap_or_else(|error| json!({"state":"failed","message":error.to_string()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_hook_distinguishes_refusal_from_failure() {
        let refusal = br#"{"error":{"code":"forbidden_caller","message":"caller is not permitted to run this command"}}"#;
        let skipped = activation_failure(Some(3), refusal);
        assert_eq!(skipped["state"], "skipped");
        assert_eq!(skipped["reason"], "forbidden_caller");
        assert_eq!(activation_failure(Some(1), refusal)["state"], "failed");
        assert_eq!(activation_failure(Some(3), br#"{"error":{"code":"operation_failed","message":"failed"}}"#)["state"], "failed");
    }
}
