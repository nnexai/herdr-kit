use super::*;

fn legacy_release(lock: &Locked, state: &mut State, rev: &str) {
    let root = release(&lock.data, rev);
    fs::create_dir_all(root.join("mod")).unwrap();
    fs::write(root.join("mod/main.js"), rev).unwrap();
    fs::write(root.join("layout.json"), name(LEGACY_MOD, rev)).unwrap();
    fs::write(root.join("herdr-kit"), rev).unwrap();
    state.releases.insert(rev.to_owned(), tree(&root).unwrap());
}
fn component_release(lock: &Locked, state: &mut State, rev: &str) {
    let catalog: Catalog = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../kit-packages.json"
    )))
    .unwrap();
    let root = release(&lock.data, rev);
    fs::create_dir_all(&root).unwrap();
    for (id, definition) in catalog.packages {
        let mut receipt = PackageReceipt {
            definition: definition.clone(),
            binary_hash: None,
            mods: BTreeMap::new(),
            layout_hash: None,
        };
        if let Some(binary) = &definition.binary {
            fs::write(root.join(&binary.name), rev).unwrap();
            receipt.binary_hash = file_hash(&root.join(&binary.name)).unwrap();
        }
        for base in &definition.mods {
            let dir = root.join("mods").join(base);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("main.js"), format!("{base} {rev}")).unwrap();
            receipt.mods.insert(base.clone(), tree(&dir).unwrap());
        }
        if definition.layout.is_some() {
            fs::write(
                root.join("layout-source.json"),
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../layouts/herdr-kit.json"
                )),
            )
            .unwrap();
            receipt.layout_hash = file_hash(&root.join("layout-source.json")).unwrap();
        }
        state
            .package_releases
            .entry(rev.to_owned())
            .or_default()
            .insert(id, receipt);
    }
}
fn selected(ids: &[&str], rev: &str) -> BTreeMap<String, String> {
    ids.iter()
        .map(|id| (id.to_string(), rev.to_owned()))
        .collect()
}
const BUNDLE: &[&str] = &["kit-manager", "kit-coord", "kit-usage", "herdr-kit"];
fn empty_state(home: &Path) -> State {
    fs::create_dir_all(home).unwrap();
    State {
        app_home: Some(home.to_path_buf()),
        ..State::default()
    }
}

#[test]
fn legacy_journal_recovery_then_component_upgrade_and_reverse_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let mut legacy = empty_state(&home);
    legacy_release(&lock, &mut legacy, &a);
    legacy.current = Some(a.clone());
    legacy.managed_mods.push(a.clone());
    // This serialized shape is the deployed old journal: no new owned fields.
    let mut value = serde_json::to_value(&legacy).unwrap();
    for key in [
        "selection",
        "previous_selection",
        "package_releases",
        "files",
        "mod_owners",
    ] {
        value.as_object_mut().unwrap().remove(key);
    }
    save(
        &journal_path(&lock.data),
        &json!({"state":value,"old_layout":null,"old_binary":null}),
    )
    .unwrap();
    atomic(&layout(&home), name(LEGACY_MOD, &a).as_bytes(), false).unwrap();
    lock.recover().unwrap();
    let old = lock.state().unwrap();
    lock.verify_active(&old).unwrap();
    let mut next = old.clone();
    component_release(&lock, &mut next, &b);
    let next = lock
        .activate(
            lock.prepare(&old, next, selected(BUNDLE, &b), None)
                .unwrap(),
        )
        .unwrap();
    lock.verify_active(&next).unwrap();
    assert_eq!(active_release(&lock.data).as_deref(), Some(b.as_str()));
    let back = lock
        .activate(
            lock.prepare(&next, next.clone(), BTreeMap::new(), Some(&a))
                .unwrap(),
        )
        .unwrap();
    lock.verify_active(&back).unwrap();
    assert_eq!(fs::read_to_string(bootstrap(&lock.data)).unwrap(), a);
    assert_eq!(
        fs::read_to_string(layout(&home)).unwrap(),
        name(LEGACY_MOD, &a)
    );
    assert!(mod_path(&home, "herdr-kit-coord", &b).exists());
    assert_eq!(back.previous_selection.unwrap(), selected(BUNDLE, &b));
}

#[test]
fn every_multi_artifact_crash_prefix_recovers_complete_selection() {
    for prefix in 0..=6 {
        let temp = tempfile::tempdir().unwrap();
        let lock = Locked::open(&temp.path().join("data")).unwrap();
        let home = temp.path().join("app");
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let mut first = empty_state(&home);
        component_release(&lock, &mut first, &a);
        let first = lock
            .activate(
                lock.prepare(&State::default(), first, selected(BUNDLE, &a), None)
                    .unwrap(),
            )
            .unwrap();
        let mut second = first.clone();
        component_release(&lock, &mut second, &b);
        let mut transaction = lock
            .prepare(&first, second, selected(BUNDLE, &b), None)
            .unwrap();
        // Include a second executable and a readiness descriptor in the same
        // persisted contract, as later vertical packages require.
        for (name, ready) in [("kit-second", false), ("active.json", true)] {
            let source = release(&lock.data, &b).join(name);
            fs::write(&source, &b).unwrap();
            let target = lock
                .data
                .join(if ready { "kit-coord" } else { "bin" })
                .join(name);
            let file = FileReceipt {
                source,
                hash: hash(b.as_bytes()),
                executable: !ready,
                ready,
            };
            transaction.state.files.insert(target.clone(), file.clone());
            transaction.files.push(FileChange {
                target,
                old_hash: None,
                next: Some(file),
            });
        }
        save(
            &journal_path(&lock.data),
            &Journal::Components(
                serde_json::from_value(serde_json::to_value(&transaction).unwrap()).unwrap(),
            ),
        )
        .unwrap();
        let mut count = 0;
        for change in &transaction.mods {
            if count < prefix {
                lock.apply_mod(change).unwrap();
            }
            count += 1;
        }
        for ready in [false, true] {
            for change in &transaction.files {
                if change.next.as_ref().is_some_and(|n| n.ready) == ready {
                    if count < prefix {
                        lock.apply_file(change).unwrap();
                    }
                    count += 1;
                }
            }
        }
        assert_eq!(count, 6);
        if prefix < 6 {
            assert!(!lock.data.join("kit-coord/active.json").exists());
        }
        lock.recover().unwrap();
        let recovered = lock.state().unwrap();
        lock.verify_active(&recovered).unwrap();
        assert_eq!(recovered.selection, selected(BUNDLE, &b));
        assert_eq!(
            fs::read_to_string(lock.data.join("bin/kit-second")).unwrap(),
            b
        );
        assert_eq!(
            fs::read_to_string(lock.data.join("kit-coord/active.json")).unwrap(),
            b
        );
        assert!(!journal_path(&lock.data).exists());
        lock.recover().unwrap();
    }
}

#[test]
fn component_only_selection_preserves_unowned_layout_and_rejects_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let rev = "a".repeat(40);
    let mut state = empty_state(&home);
    component_release(&lock, &mut state, &rev);
    fs::create_dir_all(home.join("layouts")).unwrap();
    fs::write(layout(&home), "owner's layout").unwrap();
    fs::write(home.join("keys.json"), "owner's keys").unwrap();
    let target = mod_path(&home, "herdr-kit-coord", &rev);
    copy_tree(
        &release(&lock.data, &rev).join("mods/herdr-kit-coord"),
        &target,
    )
    .unwrap();
    assert!(
        lock.prepare(
            &State::default(),
            state.clone(),
            selected(&["kit-coord"], &rev),
            None
        )
        .is_err()
    );
    fs::remove_dir_all(&target).unwrap();
    let installed = lock
        .activate(
            lock.prepare(
                &State::default(),
                state,
                selected(&["kit-coord"], &rev),
                None,
            )
            .unwrap(),
        )
        .unwrap();
    lock.verify_active(&installed).unwrap();
    assert_eq!(fs::read_to_string(layout(&home)).unwrap(), "owner's layout");
    assert_eq!(
        fs::read_to_string(home.join("keys.json")).unwrap(),
        "owner's keys"
    );
    assert!(!mod_path(&home, "herdr-kit-usage", &rev).exists());
    assert_eq!(installed.selection, selected(&["kit-coord"], &rev));
    fs::write(target.join("main.js"), "user edit").unwrap();
    assert!(lock.verify_active(&installed).is_err());
    assert_eq!(
        fs::read_to_string(target.join("main.js")).unwrap(),
        "user edit"
    );
}

#[test]
fn recovery_refuses_drift_before_publishing_any_new_target() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let mut first = empty_state(&home);
    component_release(&lock, &mut first, &a);
    let first = lock
        .activate(
            lock.prepare(&State::default(), first, selected(BUNDLE, &a), None)
                .unwrap(),
        )
        .unwrap();
    let mut next = first.clone();
    component_release(&lock, &mut next, &b);
    let transaction = lock
        .prepare(&first, next, selected(BUNDLE, &b), None)
        .unwrap();
    save(&journal_path(&lock.data), &Journal::Components(transaction)).unwrap();
    fs::write(layout(&home), "person edit during crash").unwrap();
    assert!(lock.recover().is_err());
    assert!(!mod_path(&home, "herdr-kit-coord", &b).exists());
    assert_eq!(fs::read_to_string(bootstrap(&lock.data)).unwrap(), a);
    assert_eq!(
        fs::read_to_string(layout(&home)).unwrap(),
        "person edit during crash"
    );
    assert!(journal_path(&lock.data).exists());
}

#[test]
fn structural_layout_projection_does_not_rewrite_protocol_keys() {
    let mut layout = json!({"id":"root","type":"row","children":[
        {"id":"coord","content":{"mod":"herdr-kit-coord","props":{"key":"herdr-kit-coord.protocol.v1"}}},
        {"id":"usage","content":{"mod":"herdr-kit-usage"}},
        {"id":"stock","content":{"mod":"spaces"}}]});
    let known = std::collections::BTreeSet::from_iter(LEGACY_MODS.iter().map(|s| s.to_string()));
    let bindings = BTreeMap::from([("herdr-kit-coord".into(), "herdr-kit-coord-pin".into())]);
    assert!(project_layout(&mut layout, &known, &bindings));
    assert_eq!(
        layout["children"][0]["content"]["mod"],
        "herdr-kit-coord-pin"
    );
    assert_eq!(
        layout["children"][0]["content"]["props"]["key"],
        "herdr-kit-coord.protocol.v1"
    );
    assert_eq!(layout["children"][1]["id"], "stock");
    assert_eq!(layout["children"].as_array().unwrap().len(), 2);
}

#[test]
fn rejects_symlink_traversal_and_excludes_second_manager_writer() {
    let temp = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(temp.path(), temp.path().join("link")).unwrap();
    assert!(no_links(&temp.path().join("link/file")).is_err());
    assert!(no_links(&temp.path().join("../outside")).is_err());
    let lock = Locked::open(temp.path()).unwrap();
    let other = OpenOptions::new()
        .write(true)
        .open(temp.path().join("manager.lock"))
        .unwrap();
    assert!(other.try_lock_exclusive().is_err());
    drop(lock);
}

#[test]
fn mismatched_shared_legacy_binary_generations_refuse_activation() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let mut next = empty_state(&temp.path().join("app"));
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    component_release(&lock, &mut next, &a);
    component_release(&lock, &mut next, &b);
    let mut selection = selected(&["kit-coord"], &a);
    selection.extend(selected(&["kit-usage"], &b));
    let error = lock
        .prepare(&State::default(), next, selection, None)
        .err()
        .unwrap();
    assert!(error.to_string().contains("share a legacy binary"));
    assert!(!bootstrap(&lock.data).exists());
}
