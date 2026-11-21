//! Bounded async subprocess execution with fail-fast output caps and process-group cleanup.

use std::fmt;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
}

#[derive(Debug)]
pub enum ProcessError {
    Spawn(std::io::Error),
    Read(std::io::Error),
    Wait(std::io::Error),
    OutputLimitExceeded { stream: &'static str, limit: u64 },
    Timeout(Duration),
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(err) => write!(f, "spawn failed: {err}"),
            Self::Read(err) => write!(f, "read failed: {err}"),
            Self::Wait(err) => write!(f, "wait failed: {err}"),
            Self::OutputLimitExceeded { stream, limit } => {
                write!(f, "command {stream} exceeded {limit} byte limit")
            }
            Self::Timeout(duration) => {
                write!(f, "command timed out after {}ms", duration.as_millis())
            }
        }
    }
}

impl std::error::Error for ProcessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(err) | Self::Read(err) | Self::Wait(err) => Some(err),
            Self::OutputLimitExceeded { .. } | Self::Timeout(_) => None,
        }
    }
}

struct OwnedChild {
    child: Child,
    #[cfg(unix)]
    pid: Option<rustix::process::Pid>,
    finished: bool,
}

impl OwnedChild {
    fn kill_group(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.child.start_kill();
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.finished {
            self.kill_group();
        }
    }
}

/// Read up to `max_bytes` from `input`, failing immediately once `max_bytes + 1` bytes arrive.
pub async fn read_bounded<R: AsyncRead + Unpin>(
    input: R,
    max_bytes: u64,
    stream: &'static str,
) -> Result<Vec<u8>, ProcessError> {
    let mut data = Vec::new();
    input
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut data)
        .await
        .map_err(ProcessError::Read)?;
    if data.len() as u64 > max_bytes {
        return Err(ProcessError::OutputLimitExceeded {
            stream,
            limit: max_bytes,
        });
    }
    Ok(data)
}

/// Run `command` with null stdin, fail-fast bounded stdout/stderr caps, and a timeout.
///
/// The child runs in its own process group on Unix so timeout, cancellation, or
/// output-limit failure terminates the child and its descendants cleanly.
/// Inherited environment variables are preserved unless explicitly modified on `command`.
pub async fn run_bounded(
    mut command: Command,
    timeout: Duration,
    max_stdout: u64,
    max_stderr: u64,
) -> Result<CommandOutput, ProcessError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(ProcessError::Spawn)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessError::Wait(std::io::Error::other("missing stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessError::Wait(std::io::Error::other("missing stderr")))?;
    #[cfg(unix)]
    let pid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(rustix::process::Pid::from_raw);
    let mut owned = OwnedChild {
        child,
        #[cfg(unix)]
        pid,
        finished: false,
    };

    let result = tokio::time::timeout(timeout, async {
        let (stdout, stderr, status) = tokio::try_join!(
            read_bounded(stdout, max_stdout, "stdout"),
            read_bounded(stderr, max_stderr, "stderr"),
            async { owned.child.wait().await.map_err(ProcessError::Wait) },
        )?;
        Ok(CommandOutput {
            stdout,
            stderr,
            status,
        })
    })
    .await
    .unwrap_or(Err(ProcessError::Timeout(timeout)));

    match result {
        Ok(output) => {
            owned.finished = true;
            Ok(output)
        }
        Err(error) => {
            owned.kill_group();
            let _ = owned.child.wait().await;
            owned.finished = true;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn oversized_stdout_fails_fast_without_waiting_for_timeout() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes x"]);
        let started = Instant::now();
        let err = run_bounded(cmd, Duration::from_secs(10), 1024, 1024)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ProcessError::OutputLimitExceeded {
                    stream: "stdout",
                    limit: 1024
                }
            ),
            "expected stdout limit error, got: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "oversized stdout took too long: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn oversized_stderr_fails_fast_without_waiting_for_timeout() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "yes x >&2"]);
        let started = Instant::now();
        let err = run_bounded(cmd, Duration::from_secs(10), 1024, 512)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                ProcessError::OutputLimitExceeded {
                    stream: "stderr",
                    limit: 512
                }
            ),
            "expected stderr limit error, got: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "oversized stderr took too long: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn timeout_kills_command_and_preserves_env_on_success() {
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 5"]);
        let err = run_bounded(slow, Duration::from_millis(120), 1024, 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, ProcessError::Timeout(_)));

        let mut env_cmd = Command::new("sh");
        env_cmd
            .args(["-c", "printf '%s' \"$HC_TERMINAL_ID\""])
            .env("HC_TERMINAL_ID", "t-delegate-1");
        let output = run_bounded(env_cmd, Duration::from_secs(5), 1024, 1024)
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"t-delegate-1");
    }
}
