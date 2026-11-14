use super::super::tests::{BUNDLE, component_release, empty_state, selected};
use super::*;

fn activate(lock: &Locked, transaction: Transaction, paths: &[PathBuf]) -> State {
    save(&journal_path(&lock.data), &transaction).unwrap();
    lock.recover_with_processes(|| Ok(paths.to_vec())).unwrap();
    lock.state().unwrap()
}

fn first(lock: &Locked, home: &Path, rev: &str) -> State {
    let mut state = empty_state(home);
    component_release(lock, &mut state, rev);
    activate(
        lock,
        lock.prepare(&State::default(), state, selected(BUNDLE, rev))
            .unwrap(),
        &[],
    )
}

fn second(lock: &Locked, old: &State, rev: &str, paths: &[PathBuf]) -> State {
    let mut next = old.clone();
    component_release(lock, &mut next, rev);
    activate(
        lock,
        lock.prepare(old, next, selected(BUNDLE, rev)).unwrap(),
        paths,
    )
}

#[cfg(target_os = "linux")]
#[test]
fn unreadable_process_links_do_not_hide_readable_pins() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let proc_root = temp.path().join("proc");
    directory(&proc_root.join("1")).unwrap();
    directory(&proc_root.join("2")).unwrap();
    // A directory cannot be read as a link, and a missing link also fails.
    directory(&proc_root.join("1/exe")).unwrap();
    symlink("/generation/bin", proc_root.join("2/exe")).unwrap();
    symlink("/generation/work", proc_root.join("2/cwd")).unwrap();
    let paths = process_paths_at(&proc_root).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&PathBuf::from("/generation/bin")));
    assert!(paths.contains(&PathBuf::from("/generation/work")));
}

#[test]
fn unused_generations_remove_mods_caches_releases_and_ownership_not_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let old = first(&lock, &home, &a);
    let cache = home.join("data").join(name("kit-coord", &a));
    directory(&cache.join("nested")).unwrap();
    fs::write(cache.join("nested/state.json"), "mutable cache").unwrap();
    // Matching-looking siblings without receipts are not collection candidates.
    let unowned = mod_path(&home, "personal", &a);
    directory(&unowned).unwrap();
    fs::write(unowned.join("main.js"), "personal").unwrap();
    let unowned_release = release(&lock.data, &"f".repeat(40));
    directory(&unowned_release).unwrap();
    fs::write(unowned_release.join("keep"), "personal").unwrap();
    directory(&home.join("data/personal")).unwrap();
    fs::write(home.join("data/personal/keep"), "personal").unwrap();
    let next = second(&lock, &old, &b, &[]);
    assert!(!release(&lock.data, &a).exists());
    assert!(!mod_path(&home, "kit-coord", &a).exists());
    assert!(!cache.exists());
    assert!(!next.package_releases.contains_key(&a));
    assert!(
        !next
            .mod_owners
            .contains_key(&mod_path(&home, "kit-coord", &a))
    );
    lock.verify_active(&next).unwrap();
    assert!(release(&lock.data, &b).exists());
    assert!(mod_path(&home, "kit-coord", &b).exists());
    assert_eq!(
        fs::read_to_string(unowned.join("main.js")).unwrap(),
        "personal"
    );
    assert_eq!(
        fs::read_to_string(unowned_release.join("keep")).unwrap(),
        "personal"
    );
    assert_eq!(
        fs::read_to_string(home.join("data/personal/keep")).unwrap(),
        "personal"
    );
}

#[test]
fn external_nested_layout_pins_generation_until_later_maintenance() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let a = "a".repeat(40);
    let old = first(&lock, &home, &a);
    let external = home.join("layouts/custom/nested.json");
    save(&external, &json!({"content":{"mod":name("kit-coord", &a)}})).unwrap();
    let pinned = second(&lock, &old, &"b".repeat(40), &[]);
    assert!(release(&lock.data, &a).exists());
    assert!(mod_path(&home, "kit-coord", &a).exists());
    assert!(pinned.package_releases.contains_key(&a));
    fs::remove_file(&external).unwrap();
    let next = activate(&lock, Transaction::maintenance(pinned), &[]);
    assert!(!release(&lock.data, &a).exists());
    assert!(!next.package_releases.contains_key(&a));
    lock.verify_active(&next).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn real_process_executable_and_cwd_each_pin_a_generation() {
    struct Running(std::process::Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for executable_pin in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let lock = Locked::open(&temp.path().join("data")).unwrap();
        let home = temp.path().join("app");
        let a = "a".repeat(40);
        let mut state = empty_state(&home);
        component_release(&lock, &mut state, &a);
        let executable = release(&lock.data, &a).join("kit-coord");
        fs::copy("/bin/sleep", &executable).unwrap();
        state
            .package_releases
            .get_mut(&a)
            .unwrap()
            .get_mut("kit-coord")
            .unwrap()
            .binary_hash = file_hash(&executable).unwrap();
        let old = activate(
            &lock,
            lock.prepare(&State::default(), state, selected(BUNDLE, &a))
                .unwrap(),
            &[],
        );
        let cwd = if executable_pin {
            temp.path().to_path_buf()
        } else {
            mod_path(&home, "kit-coord", &a)
        };
        let mut child = Running(
            Command::new(if executable_pin {
                executable.as_path()
            } else {
                Path::new("/bin/sleep")
            })
            .arg("60")
            .current_dir(&cwd)
            .spawn()
            .unwrap(),
        );
        let proc = PathBuf::from(format!("/proc/{}", child.0.id()));
        // Use the fixture actor's real kernel links.
        let paths = [
            fs::read_link(proc.join("exe")).unwrap(),
            fs::read_link(proc.join("cwd")).unwrap(),
        ];
        assert_eq!(paths[1], cwd);
        if executable_pin {
            assert_eq!(paths[0], executable);
        }
        let pinned = second(&lock, &old, &"b".repeat(40), &paths);
        assert!(release(&lock.data, &a).exists());
        assert!(mod_path(&home, "kit-coord", &a).exists());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let next = activate(&lock, Transaction::maintenance(pinned), &[]);
        assert!(!release(&lock.data, &a).exists());
        lock.verify_active(&next).unwrap();
    }
}

#[test]
fn cleanup_journal_recovers_partial_trees_and_post_save_crash() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let a = "a".repeat(40);
    let old = first(&lock, &home, &a);
    // Keep the old revision pinned until the recovery fixture is prepared.
    let mut next = old.clone();
    component_release(&lock, &mut next, &"b".repeat(40));
    let tx = lock
        .prepare(&old, next, selected(BUNDLE, &"b".repeat(40)))
        .unwrap();
    save(&journal_path(&lock.data), &tx).unwrap();
    lock.recover_with_processes(|| Ok(vec![release(&lock.data, &a)]))
        .unwrap();
    let mut cleanup = Transaction::maintenance(lock.state().unwrap());
    cleanup.cleanup = Some(lock.generation_plan(&cleanup.state, &[]));
    assert_eq!(cleanup.cleanup.as_ref().unwrap().len(), 1);
    save(&journal_path(&lock.data), &cleanup).unwrap();
    fs::remove_file(release(&lock.data, &a).join("kit-coord")).unwrap();
    fs::remove_dir_all(mod_path(&home, "kit-usage", &a)).unwrap();
    lock.recover_with_processes(|| Ok(Vec::new())).unwrap();
    assert!(!release(&lock.data, &a).exists());
    assert!(!journal_path(&lock.data).exists());
    // State was saved, but journal unlink did not reach disk.
    save(&journal_path(&lock.data), &cleanup).unwrap();
    lock.recover_with_processes(|| Ok(Vec::new())).unwrap();
    let state = lock.state().unwrap();
    assert!(!state.package_releases.contains_key(&a));
    lock.verify_active(&state).unwrap();
}

#[test]
fn drift_and_new_pins_never_authorize_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let a = "a".repeat(40);
    let old = first(&lock, &home, &a);
    let pinned_path = release(&lock.data, &a).join("kit-coord");
    let state = second(&lock, &old, &"b".repeat(40), &[pinned_path.clone()]);
    let extra = release(&lock.data, &a).join("unowned");
    fs::write(&extra, "keep").unwrap();
    assert!(lock.generation_plan(&state, &[]).is_empty());
    fs::remove_file(extra).unwrap();
    let old_mod = mod_path(&home, "kit-coord", &a).join("main.js");
    let original = fs::read(&old_mod).unwrap();
    fs::write(&old_mod, "edit").unwrap();
    assert!(lock.generation_plan(&state, &[]).is_empty());
    fs::write(&old_mod, original).unwrap();
    let malformed = home.join("layouts/personal.json");
    fs::write(&malformed, "malformed").unwrap();
    assert!(lock.generation_plan(&state, &[]).is_empty());
    fs::remove_file(&malformed).unwrap();
    let mut cleanup = Transaction::maintenance(state);
    cleanup.cleanup = Some(lock.generation_plan(&cleanup.state, &[]));
    save(&journal_path(&lock.data), &cleanup).unwrap();
    assert!(
        lock.recover_with_processes(|| Ok(vec![pinned_path.clone()]))
            .is_err()
    );
    assert!(pinned_path.exists());
    assert!(journal_path(&lock.data).exists());
    lock.recover_with_processes(|| Ok(Vec::new())).unwrap();
    assert!(!pinned_path.exists());
}
