//! The manager reads the source it is given, not a repository named by its environment.
use kit_core::git_env;

use std::{fs, path::Path, process::Command};

fn init_repo(home: &Path, repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ][..],
    ] {
        assert!(
            git_env::git()
                .env("HOME", home)
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
}

/// Runs `herdr-kit source --repo <source>` the way a git hook of `inherited` would.
fn set_source(root: &Path, source: &Path, inherited: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_herdr-kit"))
        .env("HOME", root)
        .env("GIT_DIR", inherited.join(".git"))
        .env("GIT_WORK_TREE", inherited)
        .arg("--data-dir")
        .arg(root.join("data"))
        .arg("source")
        .arg("--repo")
        .arg(source)
        .output()
        .unwrap()
}

#[test]
fn inherited_git_repository_does_not_stand_in_for_the_source() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let decoy = root.join("decoy");
    init_repo(root, &decoy);

    let plain = root.join("plain");
    fs::create_dir(&plain).unwrap();
    let refused = set_source(root, &plain, &decoy);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("cannot read source revision"));

    let source = root.join("source");
    init_repo(root, &source);
    let accepted = set_source(root, &source, &decoy);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
}
