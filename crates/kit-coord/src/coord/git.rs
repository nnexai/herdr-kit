use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

use super::exec;

/// Exit code, stdout and stderr of `git -C dir args`.
fn run(dir: &Path, args: &[OsString], seconds: u64) -> Result<(i32, String, String)> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0");
    let done = exec::run(cmd, "git", seconds)?;
    Ok((
        done.status.code().unwrap_or(-1),
        String::from_utf8(done.stdout)?,
        String::from_utf8(done.stderr)?,
    ))
}
fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}
fn checked(dir: &Path, a: &[OsString], seconds: u64) -> Result<String> {
    let (code, out, err) = run(dir, a, seconds)?;
    ensure!(code == 0, "git failed ({code}): {err}");
    Ok(out)
}
fn boolean(dir: &Path, a: &[OsString]) -> Result<bool> {
    let (code, _, err) = run(dir, a, 120)?;
    match code {
        0 => Ok(true),
        1 => Ok(false),
        _ => bail!("git failed ({code}): {err}"),
    }
}
pub fn main_checkout(dir: &Path) -> Result<PathBuf> {
    let p = checked(
        dir,
        &args(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        120,
    )?;
    let p = Path::new(p.trim_end_matches('\n'));
    ensure!(
        p.file_name() == Some(OsStr::new(".git")),
        "bare or non-standard git common directory"
    );
    Ok(p.parent().context("missing checkout parent")?.to_path_buf())
}
pub fn current_branch(repo: &Path) -> Result<Option<String>> {
    let (code, out, err) = run(
        repo,
        &args(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
        120,
    )?;
    match code {
        0 => Ok(Some(out.trim().to_owned())),
        1 => Ok(None),
        _ => bail!("git failed ({code}): {err}"),
    }
}
pub fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    boolean(
        repo,
        &args(&[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ]),
    )
}
pub fn rev(repo: &Path, rev: &str) -> Result<String> {
    Ok(checked(
        repo,
        &args(&["rev-parse", "--verify", "--end-of-options", rev]),
        120,
    )?
    .trim()
    .to_owned())
}
pub fn worktree_add(
    repo: &Path,
    path: &Path,
    branch: &str,
    create_from: Option<&str>,
) -> Result<()> {
    let mut a = args(&["worktree", "add"]);
    if create_from.is_some() {
        a.extend(args(&["-b", branch]));
    }
    a.push(path.as_os_str().to_owned());
    a.push(OsString::from(create_from.unwrap_or(branch)));
    checked(repo, &a, 600)?;
    Ok(())
}
#[derive(Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
}
pub fn worktree_list(repo: &Path) -> Result<Vec<Worktree>> {
    let output = checked(repo, &args(&["worktree", "list", "--porcelain", "-z"]), 120)?;
    let mut result = Vec::new();
    let mut current: Option<Worktree> = None;
    for line in output.split('\0') {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(w) = current.take() {
                result.push(w);
            }
            current = Some(Worktree {
                path: p.into(),
                head: None,
                branch: None,
            });
        } else if let Some(w) = current.as_mut() {
            if let Some(h) = line.strip_prefix("HEAD ") {
                w.head = Some(h.into());
            }
            if let Some(b) = line.strip_prefix("branch refs/heads/") {
                w.branch = Some(b.into());
            }
        }
    }
    if let Some(w) = current {
        result.push(w);
    }
    Ok(result)
}
pub fn dirty(worktree: &Path) -> Result<Vec<String>> {
    Ok(checked(
        worktree,
        &args(&["status", "--porcelain=v1", "--untracked-files=all"]),
        120,
    )?
    .lines()
    .map(str::to_owned)
    .collect())
}
pub fn unreachable_count(repo: &Path, head: &str, delivery_branch: &str) -> Result<u64> {
    Ok(checked(
        repo,
        &args(&[
            "rev-list",
            "--count",
            head,
            "--not",
            &format!("refs/heads/{delivery_branch}"),
            "--remotes",
        ]),
        120,
    )?
    .trim()
    .parse()?)
}
pub fn is_ancestor(repo: &Path, commit: &str, of: &str) -> Result<bool> {
    boolean(repo, &args(&["merge-base", "--is-ancestor", commit, of]))
}
pub fn worktree_remove(repo: &Path, path: &Path) -> Result<()> {
    let mut a = args(&["worktree", "remove"]);
    a.push(path.as_os_str().to_owned());
    checked(repo, &a, 120)?;
    Ok(())
}
pub fn delete_branch(repo: &Path, branch: &str, expected_sha: &str) -> Result<()> {
    checked(
        repo,
        &args(&[
            "update-ref",
            "-d",
            &format!("refs/heads/{branch}"),
            expected_sha,
        ]),
        120,
    )?;
    Ok(())
}
pub fn slug(branch: &str) -> String {
    let mut s = String::new();
    let mut invalid = false;
    for c in branch.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            s.push(c);
            invalid = false;
        } else if !invalid {
            s.push('-');
            invalid = true;
        }
    }
    s.trim_matches('-').to_owned()
}
pub fn worktree_path(root: &Path, main: &Path, branch: &str) -> PathBuf {
    root.join(main.file_name().unwrap_or_default())
        .join(slug(branch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn git(home: &Path, dir: &Path, a: &[&str]) {
        let status = Command::new("git")
            .env("HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(dir)
            .args(a)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
    }
    #[test]
    fn repository_operations() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let repo = home.join("repo");
        fs::create_dir(&repo).unwrap();
        git(home, &repo, &["init", "-b", "main"]);
        git(
            home,
            &repo,
            &["config", "user.email", "test@example.invalid"],
        );
        git(home, &repo, &["config", "user.name", "Test"]);
        fs::write(repo.join("tracked"), "one").unwrap();
        fs::write(repo.join(".gitignore"), "ignored\n").unwrap();
        git(home, &repo, &["add", "."]);
        git(home, &repo, &["commit", "-m", "initial"]);
        assert_eq!(slug("feat/x y"), "feat-x-y");
        assert_eq!(
            worktree_path(home, &repo, "feat/x y"),
            home.join("repo/feat-x-y")
        );
        assert_eq!(current_branch(&repo).unwrap().as_deref(), Some("main"));
        let wt = home.join("wt");
        worktree_add(&repo, &wt, "topic", Some("main")).unwrap();
        assert_eq!(main_checkout(&wt).unwrap(), repo);
        assert!(branch_exists(&repo, "topic").unwrap());
        assert!(dirty(&wt).unwrap().is_empty());
        fs::write(wt.join("ignored"), "ignored").unwrap();
        assert!(dirty(&wt).unwrap().is_empty());
        fs::write(wt.join("untracked"), "x").unwrap();
        assert!(!dirty(&wt).unwrap().is_empty());
        fs::remove_file(wt.join("untracked")).unwrap();
        fs::write(wt.join("tracked"), "two").unwrap();
        assert!(!dirty(&wt).unwrap().is_empty());
        git(home, &wt, &["add", "tracked"]);
        git(home, &wt, &["commit", "-m", "topic"]);
        let first = rev(&wt, "HEAD").unwrap();
        assert_eq!(unreachable_count(&repo, &first, "main").unwrap(), 1);
        assert!(!is_ancestor(&repo, &first, "refs/heads/main").unwrap());
        let bare = home.join("remote.git");
        fs::create_dir(&bare).unwrap();
        git(home, &bare, &["init", "--bare"]);
        git(
            home,
            &repo,
            &["remote", "add", "origin", bare.to_str().unwrap()],
        );
        git(home, &wt, &["push", "origin", "topic"]);
        assert_eq!(unreachable_count(&repo, &first, "main").unwrap(), 0);
        fs::write(wt.join("tracked"), "three").unwrap();
        git(home, &wt, &["add", "tracked"]);
        git(home, &wt, &["commit", "-m", "more"]);
        let second = rev(&wt, "HEAD").unwrap();
        assert!(delete_branch(&repo, "topic", &first).is_err());
        assert_eq!(unreachable_count(&repo, &second, "main").unwrap(), 1);
        worktree_remove(&repo, &wt).unwrap();
        git(home, &repo, &["merge", "--ff-only", "topic"]);
        assert!(is_ancestor(&repo, &second, "refs/heads/main").unwrap());
        delete_branch(&repo, "topic", &second).unwrap();
        assert!(!branch_exists(&repo, "topic").unwrap());
    }
}
