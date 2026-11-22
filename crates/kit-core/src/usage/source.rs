use super::*;
use crate::process::{ProcessError, run_bounded};
use tokio::process::Command;

fn executable(override_path: Option<&Path>, binary: &str, home_subdirs: &[&str]) -> PathBuf {
    if let Some(path) = override_path {
        return std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    }
    let mut directories = crate::paths::path_dirs();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        directories.extend(home_subdirs.iter().map(|sub| home.join(sub)));
    }
    directories.extend(
        [
            #[cfg(target_os = "macos")]
            "/opt/homebrew/bin",
            "/usr/local/bin",
            #[cfg(target_os = "linux")]
            "/home/linuxbrew/.linuxbrew/bin",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    crate::paths::find_executable(binary, &directories)
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .unwrap_or_else(|| PathBuf::from(binary))
}

async fn run_command(
    command: Command,
    stderr_limit: u64,
) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), QuotaErrorCode> {
    let output = run_bounded(
        command,
        Duration::from_secs(30),
        2 * 1024 * 1024,
        stderr_limit,
    )
    .await
    .map_err(|error| match error {
        ProcessError::Spawn(_) => QuotaErrorCode::SourceMissing,
        ProcessError::Timeout(_) => QuotaErrorCode::Timeout,
        ProcessError::OutputLimitExceeded { .. } | ProcessError::Read(_) => {
            QuotaErrorCode::Malformed
        }
        ProcessError::Wait(_) => QuotaErrorCode::Failed,
    })?;
    Ok((output.stdout, output.stderr, output.status))
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
