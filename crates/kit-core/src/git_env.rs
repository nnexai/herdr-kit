//! Keeps git on the repository it is pointed at.
use std::process::Command;

/// The variables `git rev-parse --local-env-vars` lists: those through which the
/// environment selects a repository. Git hooks and `git rebase --exec` export
/// them, and a `git -C <dir>` that inherits them works on the caller's
/// repository instead of `<dir>`.
const REPOSITORY_ENV: [&str; 15] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// A `git` command that ignores any repository named by the inherited environment.
pub fn git() -> Command {
    let mut cmd = Command::new("git");
    for name in REPOSITORY_ENV {
        cmd.env_remove(name);
    }
    cmd
}
