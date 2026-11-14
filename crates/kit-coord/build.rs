use std::{env, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=KIT_COORD_BUILD_REVISION");
    println!("cargo:rerun-if-changed=build.rs");
    let revision = env::var("KIT_COORD_BUILD_REVISION").ok().or_else(|| {
        for name in ["HEAD".to_owned(), "packed-refs".to_owned(), git(&["symbolic-ref", "-q", "HEAD"]).unwrap_or_default()] {
            if !name.is_empty() {
                if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
        }
        git(&["rev-parse", "HEAD"])
    }).expect("build a committed Git checkout, or supply KIT_COORD_BUILD_REVISION for an archived source export");
    assert!(
        (revision.len() == 40 || revision.len() == 64)
            && revision
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "KIT_COORD_BUILD_REVISION must be a full lowercase commit id"
    );
    println!("cargo:rustc-env=KIT_COORD_BUILD_REVISION={revision}");
    println!(
        "cargo:rustc-env=KIT_COORD_VERSION={} ({revision})",
        env::var("CARGO_PKG_VERSION").unwrap()
    );
}
