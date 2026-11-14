//! Bounded coordinator activation adapter. Caller identity is preserved.
use super::*;
use std::thread;

fn activation_failure(code: Option<i32>, stderr: &[u8]) -> Value {
    let message = String::from_utf8_lossy(stderr).into_owned();
    let forbidden = code == Some(3)
        && serde_json::from_slice::<Value>(stderr)
            .ok()
            .and_then(|value| value["error"]["code"].as_str().map(str::to_owned))
            .as_deref()
            == Some("forbidden_caller");
    if forbidden {
        json!({"state":"skipped","reason":"forbidden_caller","message":message})
    } else {
        json!({"state":"failed","message":message})
    }
}

pub fn after_activation(root: &Path) -> Value {
    if !root.join("coord/config.json").exists() {
        return json!({"state":"disabled"});
    }
    let result = (|| -> Result<Value> {
        let state: State = read_json(&state_path(root))?.context("suite is not installed")?;
        if !state.selection.contains_key("kit-coord") {
            return Ok(json!({"state":"skipped","reason":"coordinator_not_selected"}));
        }
        let mut command = Command::new(root.join("bin/kit-coord"));
        command.arg("--data-dir").arg(root);
        let mut child = command
            .args(["up", "--if-enabled"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if child.try_wait()?.is_some() {
                let output = child.wait_with_output()?;
                if !output.status.success() {
                    return Ok(activation_failure(output.status.code(), &output.stderr));
                }
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
        assert_eq!(
            activation_failure(
                Some(3),
                br#"{"error":{"code":"operation_failed","message":"failed"}}"#
            )["state"],
            "failed"
        );
        assert_eq!(
            activation_failure(Some(2), b"unrecognized subcommand").get("state"),
            Some(&json!("failed"))
        );
    }
}
