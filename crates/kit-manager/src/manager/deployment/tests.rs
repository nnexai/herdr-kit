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
            activated: false,
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
const BUNDLE: &[&str] = &[
    "kit-manager",
    "kit-coord",
    "kit-coord-ui",
    "kit-usage",
    "herdr-kit",
];
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
    assert_eq!(read_json::<Value>(&lock.data.join("kit-coord/active.json")).unwrap().unwrap()["revision"], b);
    let back = lock
        .activate(
            lock.prepare(&next, next.clone(), BTreeMap::new(), Some(&a))
                .unwrap(),
        )
        .unwrap();
    lock.verify_active(&back).unwrap();
    assert_eq!(fs::read_to_string(bootstrap(&lock.data)).unwrap(), a);
    assert!(!lock.data.join("kit-coord/active.json").exists());
    assert!(!lock.data.join("bin/kit-coord").exists());
    assert_eq!(
        fs::read_to_string(layout(&home)).unwrap(),
        name(LEGACY_MOD, &a)
    );
    assert!(mod_path(&home, "kit-coord", &b).exists());
    assert_eq!(back.previous_selection.unwrap(), selected(BUNDLE, &b));
}

#[test]
fn every_multi_artifact_crash_prefix_recovers_complete_selection() {
    let fixture = || {
        let temp = tempfile::tempdir().unwrap();
        let lock = Locked::open(&temp.path().join("data")).unwrap();
        let home = temp.path().join("app");
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let mut first = empty_state(&home);
        component_release(&lock, &mut first, &a);
        let first = lock.activate(lock.prepare(
            &State::default(), first, selected(BUNDLE, &a), None,
        ).unwrap()).unwrap();
        let mut second = first.clone();
        component_release(&lock, &mut second, &b);
        let transaction = lock.prepare(&first, second, selected(BUNDLE, &b), None).unwrap();
        (temp, lock, transaction, a, b)
    };
    let probe = fixture();
    let publications = probe.2.mods.len() + probe.2.files.len();
    drop(probe);
    for prefix in 0..=publications + 2 {
        let (_temp, lock, transaction, a, b) = fixture();
        save(&journal_path(&lock.data), &Journal::Components(
            serde_json::from_value(serde_json::to_value(&transaction).unwrap()).unwrap(),
        )).unwrap();
        let mut count = 0;
        for change in &transaction.mods {
            if count < prefix { lock.apply_mod(change).unwrap(); }
            count += 1;
        }
        for ready in [false, true] {
            for change in &transaction.files {
                if change.next.as_ref().is_some_and(|file| file.ready) == ready {
                    if count < prefix { lock.apply_file(change).unwrap(); }
                    count += 1;
                }
            }
        }
        let descriptor: Value = read_json(&lock.data.join("kit-coord/active.json")).unwrap().unwrap();
        assert_eq!(descriptor["revision"], if prefix < publications { a.as_str() } else { b.as_str() });
        if prefix == publications + 1 { state::store_history(&lock.data, &transaction.state).unwrap(); }
        if prefix == publications + 2 { lock.save(&transaction.state).unwrap(); }
        lock.recover().unwrap();
        let recovered = lock.state().unwrap();
        lock.verify_active(&recovered).unwrap();
        assert_eq!(recovered.selection, selected(BUNDLE, &b));
        assert_eq!(fs::read_to_string(lock.data.join("bin/kit-coord")).unwrap(), b);
        assert_eq!(fs::read_to_string(bootstrap(&lock.data)).unwrap(), b);
        let descriptor: Value = read_json(&lock.data.join("kit-coord/active.json")).unwrap().unwrap();
        assert_eq!(descriptor["revision"], b);
        assert_eq!(descriptor["executable"], release(&lock.data, &b).join("kit-coord").to_str().unwrap());
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
    let target = mod_path(&home, "kit-coord", &rev);
    copy_tree(
        &release(&lock.data, &rev).join("mods/kit-coord"),
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
    assert!(!mod_path(&home, "kit-coord", &b).exists());
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
    let mut selection = selected(&["kit-manager"], &a);
    selection.extend(selected(&["kit-usage"], &b));
    let error = lock
        .prepare(&State::default(), next, selection, None)
        .err()
        .unwrap();
    assert!(error.to_string().contains("share a legacy binary"));
    assert!(!bootstrap(&lock.data).exists());
}

#[test]
fn retained_activated_release_cannot_be_rebuilt_as_incomplete_staging() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let mut first = empty_state(&temp.path().join("app"));
    component_release(&lock, &mut first, &a);
    let first = lock
        .activate(
            lock.prepare(&State::default(), first, selected(&["kit-coord"], &a), None)
                .unwrap(),
        )
        .unwrap();
    let mut second = first.clone();
    component_release(&lock, &mut second, &b);
    let mut second = lock
        .activate(
            lock.prepare(&first, second, selected(&["kit-manager"], &b), None)
                .unwrap(),
        )
        .unwrap();
    let immutable = release(&lock.data, &a).join("kit-coord");
    fs::remove_file(&immutable).unwrap();
    let error = lock
        .build(&mut second, &a, &["kit-coord".into()])
        .unwrap_err();
    assert!(
        error.to_string().contains("release binary drift"),
        "{error}"
    );
    assert!(!immutable.exists());
    assert_eq!(fs::read_to_string(bootstrap(&lock.data)).unwrap(), b);
}

#[test]
fn legacy_release_is_never_augmented_during_component_migration() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let mut state = empty_state(&temp.path().join("app"));
    let rev = "a".repeat(40);
    legacy_release(&lock, &mut state, &rev);
    let before = tree(&release(&lock.data, &rev)).unwrap();
    let error = lock
        .build(&mut state, &rev, &["kit-coord".into()])
        .unwrap_err();
    assert!(
        error.to_string().contains("legacy release is immutable"),
        "{error}"
    );
    assert_eq!(tree(&release(&lock.data, &rev)).unwrap(), before);
    assert!(
        !release(&lock.data, &rev)
            .join("layout-source.json")
            .exists()
    );
}

#[test]
fn legacy_state_writer_cannot_discard_package_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let rev = "a".repeat(40);
    let mut next = empty_state(&temp.path().join("app"));
    component_release(&lock, &mut next, &rev);
    lock.activate(
        lock.prepare(
            &State::default(),
            next,
            selected(&["kit-coord"], &rev),
            None,
        )
        .unwrap(),
    )
    .unwrap();
    // An actual old writer only deserializes and republishes these legacy
    // fields. The canonical package snapshot must survive that round trip.
    let mut old_writer: State = read_json(&state_path(&lock.data)).unwrap().unwrap();
    old_writer.next_check = 777;
    state::store_legacy(&lock.data, &old_writer).unwrap();
    let recovered = lock.state().unwrap();
    lock.verify_active(&recovered).unwrap();
    assert_eq!(recovered.next_check, 777);
    assert_eq!(read_json::<Value>(&lock.data.join("kit-coord/active.json")).unwrap().unwrap()["revision"], rev);
    let next = lock
        .activate(
            lock.prepare(
                &recovered,
                recovered.clone(),
                selected(&["kit-coord", "kit-manager"], &rev),
                None,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        next.selection,
        selected(&["kit-coord", "kit-manager"], &rev)
    );
    assert_eq!(fs::read_to_string(bootstrap(&lock.data)).unwrap(), rev);
}

#[test]
fn legacy_journal_advancement_retains_new_package_rollback_assets() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let c = "c".repeat(40);
    let mut first = empty_state(&home);
    legacy_release(&lock, &mut first, &b);
    first.current = Some(b.clone());
    first.managed_mods.push(b.clone());
    save(
        &journal_path(&lock.data),
        &Journal::Legacy(LegacyJournal {
            state: first,
            old_layout: None,
            old_binary: None,
        }),
    )
    .unwrap();
    lock.recover().unwrap();
    let first = lock.state().unwrap();
    let mut next = first.clone();
    component_release(&lock, &mut next, &a);
    let next = lock
        .activate(
            lock.prepare(&first, next, selected(BUNDLE, &a), None)
                .unwrap(),
        )
        .unwrap();
    let back = lock
        .activate(
            lock.prepare(&next, next.clone(), BTreeMap::new(), Some(&b))
                .unwrap(),
        )
        .unwrap();
    let mut old_writer: State = read_json(&state_path(&lock.data)).unwrap().unwrap();
    legacy_release(&lock, &mut old_writer, &c);
    old_writer.previous = Some(b.clone());
    old_writer.current = Some(c.clone());
    old_writer.managed_mods.push(c.clone());
    save(
        &journal_path(&lock.data),
        &Journal::Legacy(LegacyJournal {
            state: old_writer,
            old_layout: file_hash(&layout(&home)).unwrap(),
            old_binary: file_hash(&bootstrap(&lock.data)).unwrap(),
        }),
    )
    .unwrap();
    lock.recover().unwrap();
    let recovered = lock.state().unwrap();
    lock.verify_active(&recovered).unwrap();
    assert_eq!(recovered.current.as_deref(), Some(c.as_str()));
    let restored = lock
        .activate(
            lock.prepare(&recovered, recovered.clone(), selected(BUNDLE, &a), None)
                .unwrap(),
        )
        .unwrap();
    lock.verify_active(&restored).unwrap();
    assert_eq!(restored.selection, selected(BUNDLE, &a));
    assert_eq!(fs::read_to_string(bootstrap(&lock.data)).unwrap(), a);
    assert_eq!(back.previous_selection.unwrap(), selected(BUNDLE, &a));
}

#[test]
fn chat_package_requires_matching_coordinator_without_usage() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let home = temp.path().join("app");
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let mut state = empty_state(&home);
    component_release(&lock, &mut state, &a);
    component_release(&lock, &mut state, &b);
    let mut mismatched = selected(&["kit-coord-ui"], &b);
    mismatched.extend(selected(&["kit-coord"], &a));
    let error = lock
        .prepare(&State::default(), state.clone(), mismatched, None)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("dependent component pins"),
        "{error}"
    );
    assert!(!mod_path(&home, "kit-coord-ui", &b).exists());
    let installed = lock
        .activate(
            lock.prepare(
                &State::default(),
                state,
                selected(&["kit-coord", "kit-coord-ui"], &a),
                None,
            )
            .unwrap(),
        )
        .unwrap();
    lock.verify_active(&installed).unwrap();
    assert!(mod_path(&home, "kit-coord-ui", &a).exists());
    assert!(!mod_path(&home, "herdr-kit-usage", &a).exists());
    assert!(!layout(&home).exists());
}

#[test]
fn independent_usage_update_keeps_coordinator_readiness_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let lock = Locked::open(&temp.path().join("data")).unwrap();
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let mut state = empty_state(&temp.path().join("app"));
    component_release(&lock, &mut state, &a);
    component_release(&lock, &mut state, &b);
    let first = lock.activate(lock.prepare(
        &State::default(), state, selected(&["kit-coord", "kit-usage"], &a), None,
    ).unwrap()).unwrap();
    let descriptor = lock.data.join("kit-coord/active.json");
    let before = fs::read(&descriptor).unwrap();
    let mut selection = selected(&["kit-coord"], &a);
    selection.extend(selected(&["kit-usage"], &b));
    let next = lock.activate(lock.prepare(&first, first.clone(), selection, None).unwrap()).unwrap();
    lock.verify_active(&next).unwrap();
    assert_eq!(fs::read(&descriptor).unwrap(), before);
    assert_eq!(fs::read_to_string(lock.data.join("bin/kit-coord")).unwrap(), a);
    assert_eq!(fs::read_to_string(bootstrap(&lock.data)).unwrap(), b);
    assert_eq!(next.selection["kit-coord"], a);
    assert!(!next.selection.contains_key("kit-manager"));
}
