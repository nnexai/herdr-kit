//! Bounded child processes for the hc, herdr-app and git adapters.
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

const MAX_OUTPUT: u64 = 8 * 1024 * 1024;

pub struct Captured {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run `cmd` to completion, or kill its process group after `seconds`.
/// `what` names the program in errors. Files instead of pipes keep a verbose
/// child from deadlocking while we wait; each stream is read back with a limit.
pub fn run(mut cmd: Command, what: &str, seconds: u64) -> Result<Captured> {
    let mut out = tempfile::tempfile()?;
    let mut err = tempfile::tempfile()?;
    cmd.stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(err.try_clone()?)
        .process_group(0);
    let mut child = cmd.spawn().with_context(|| format!("starting {what}"))?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            result => {
                if let Some(pid) = i32::try_from(child.id())
                    .ok()
                    .and_then(rustix::process::Pid::from_raw)
                {
                    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                match result {
                    Err(e) => return Err(e.into()),
                    _ => bail!("{what} timed out after {seconds}s"),
                }
            }
        }
    };
    let read = |file: &mut std::fs::File| -> Result<Vec<u8>> {
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.take(MAX_OUTPUT + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_OUTPUT,
            "{what} output exceeds limit"
        );
        Ok(bytes)
    };
    Ok(Captured {
        status,
        stdout: read(&mut out)?,
        stderr: read(&mut err)?,
    })
}
