use super::*;
use std::process::Stdio;
use tokio::{
    io::AsyncRead,
    process::{Child, Command},
};

fn executable(override_path: Option<&Path>, binary: &str, home_subdirs: &[&str]) -> PathBuf {
    if let Some(path) = override_path {
        return std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    }
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        directories.extend(home_subdirs.iter().map(|sub| home.join(sub)));
    }
    directories.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/home/linuxbrew/.linuxbrew/bin",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    use std::os::unix::fs::PermissionsExt;
    directories
        .into_iter()
        .map(|p| p.join(binary))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_else(|| PathBuf::from(binary))
}

struct OwnedChild {
    child: Child,
    pid: Option<rustix::process::Pid>,
    finished: bool,
}
impl OwnedChild {
    fn kill_group(&mut self) {
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

async fn bounded<R: AsyncRead + Unpin>(input: R, max: u64) -> Result<Vec<u8>, QuotaErrorCode> {
    let mut data = Vec::new();
    input
        .take(max + 1)
        .read_to_end(&mut data)
        .await
        .map_err(|_| QuotaErrorCode::Malformed)?;
    if data.len() as u64 > max {
        return Err(QuotaErrorCode::Malformed);
    }
    Ok(data)
}

async fn run_command(
    mut command: Command,
    stderr_limit: u64,
) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), QuotaErrorCode> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn().map_err(|_| QuotaErrorCode::SourceMissing)?;
    let stdout = child.stdout.take().ok_or(QuotaErrorCode::Failed)?;
    let stderr = child.stderr.take().ok_or(QuotaErrorCode::Failed)?;
    let pid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(rustix::process::Pid::from_raw);
    let mut owned = OwnedChild {
        child,
        pid,
        finished: false,
    };
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let (out, err, status) = tokio::try_join!(
            bounded(stdout, 2 * 1024 * 1024),
            bounded(stderr, stderr_limit),
            async { owned.child.wait().await.map_err(|_| QuotaErrorCode::Failed) }
        )?;
        Ok((out, err, status))
    })
    .await
    .unwrap_or(Err(QuotaErrorCode::Timeout));
    match result {
        Ok((out, err, status)) => {
            owned.finished = true;
            Ok((out, err, status))
        }
        Err(error) => {
            owned.kill_group();
            let _ = owned.child.wait().await;
            owned.finished = true;
            Err(error)
        }
    }
}

pub(super) async fn collect_omp(
    root: &Path,
    override_path: Option<&Path>,
) -> Result<Vec<ProviderRecord>, QuotaErrorCode> {
    let mut command = Command::new(executable(
        override_path,
        "omp",
        &[".local/bin", ".bun/bin"],
    ));
    command
        .args(["usage", "--json", "--redact", "--no-extensions"])
        .current_dir(root)
        .env("NO_COLOR", "1");
    let (out, _err, status) = run_command(command, 8192).await?;
    if status.success() {
        parse::omp(&out, now())
    } else {
        Err(QuotaErrorCode::Failed)
    }
}

pub(super) async fn collect_antigravity(
    root: &Path,
    override_path: Option<&Path>,
) -> Result<Vec<ProviderRecord>, QuotaErrorCode> {
    let mut command = Command::new(executable(
        override_path,
        "agy",
        &[
            ".gemini/antigravity-cli/bin",
            ".local/bin",
            ".omp/antigravity-cli/bin",
            ".bun/bin",
        ],
    ));
    command
        .args([
            "-p",
            "/usage",
            "--output-format",
            "json",
            "--print-timeout",
            "25s",
        ])
        .current_dir(root)
        .env("NO_COLOR", "1");
    let (out, err, status) = run_command(command, 65536).await?;
    if status.success() {
        parse::antigravity(&out, now())
    } else if parse::looks_not_signed_in(&out) || parse::looks_not_signed_in(&err) {
        Ok(vec![empty(
            QuotaProvider::Antigravity,
            QuotaProviderState::NotSignedIn,
            Some(QuotaErrorCode::NotSignedIn),
        )])
    } else {
        Err(QuotaErrorCode::Failed)
    }
}
