use super::*;

pub(super) fn component_release(lock: &Locked, state: &mut State, rev: &str) {
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
pub(super) fn selected(ids: &[&str], rev: &str) -> BTreeMap<String, String> {
    ids.iter()
        .map(|id| (id.to_string(), rev.to_owned()))
        .collect()
}
pub(super) const BUNDLE: &[&str] = &[
    "kit-manager",
    "kit-coord",
    "kit-coord-ui",
    "kit-usage",
    "herdr-kit",
];
pub(super) fn empty_state(home: &Path) -> State {
    fs::create_dir_all(home).unwrap();
    State {
        app_home: Some(home.to_path_buf()),
        ..State::default()
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
    copy_tree(&release(&lock.data, &rev).join("mods/kit-coord"), &target).unwrap();
    assert!(
        lock.prepare(
            &State::default(),
            state.clone(),
            selected(&["kit-coord"], &rev),
        )
        .is_err()
    );
    fs::remove_dir_all(&target).unwrap();
    let installed = lock
        .activate(
            lock.prepare(&State::default(), state, selected(&["kit-coord"], &rev))
                .unwrap(),
        )
        .unwrap();
    lock.verify_active(&installed).unwrap();
    assert_eq!(fs::read_to_string(layout(&home)).unwrap(), "owner's layout");
    assert_eq!(
        fs::read_to_string(home.join("keys.json")).unwrap(),
        "owner's keys"
    );
    assert!(!mod_path(&home, "kit-usage", &rev).exists());
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
            lock.prepare(&State::default(), first, selected(BUNDLE, &a))
                .unwrap(),
        )
        .unwrap();
    let mut next = first.clone();
    component_release(&lock, &mut next, &b);
    let transaction = lock.prepare(&first, next, selected(BUNDLE, &b)).unwrap();
    save(&journal_path(&lock.data), &transaction).unwrap();
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
        {"id":"coord","content":{"mod":"kit-coord","props":{"key":"kit-coord.protocol.v1"}}},
        {"id":"usage","content":{"mod":"kit-usage"}},
        {"id":"bar","content":{"mod":"herdr-kit-bar"}},
        {"id":"stock","content":{"mod":"spaces"}}]});
    let known = std::collections::BTreeSet::from_iter(
        ["kit-coord", "kit-coord-ui", "kit-usage", "herdr-kit-bar"]
            .iter()
            .map(|s| s.to_string()),
    );
    let bindings = BTreeMap::from([
        ("kit-coord".into(), "kit-coord-pin".into()),
        ("kit-coord-ui".into(), "kit-coord-ui-pin".into()),
        ("herdr-kit-bar".into(), "herdr-kit-bar-pin".into()),
    ]);
    assert!(project_layout(
        &mut layout,
        &known,
        &bindings,
        None,
        Path::new("/fixture")
    ));
    assert_eq!(layout["children"][0]["content"]["mod"], "kit-coord-pin");
    assert_eq!(
        layout["children"][0]["content"]["props"]["key"],
        "kit-coord.protocol.v1"
    );
    assert_eq!(
        layout["children"][1]["content"]["props"]["coord_mod"],
        "kit-coord-pin"
    );
    assert_eq!(
        layout["children"][1]["content"]["props"]["coord_ui_mod"],
        "kit-coord-ui-pin"
    );
    assert_eq!(layout["children"][2]["id"], "stock");
    assert_eq!(layout["children"].as_array().unwrap().len(), 3);
}

#[test]
fn rejects_symlink_traversal_and_excludes_second_manager_writer() {
    let temp = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(temp.path(), temp.path().join("link")).unwrap();
    assert!(no_links(&temp.path().join("link/file"), SymlinkMessage::Generic).is_err());
    assert!(no_links(&temp.path().join("../outside"), SymlinkMessage::Generic).is_err());
    let lock = Locked::open(temp.path()).unwrap();
    let other = OpenOptions::new()
        .write(true)
        .open(temp.path().join("manager.lock"))
        .unwrap();
    assert!(other.try_lock_exclusive().is_err());
    drop(lock);
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
            lock.prepare(&State::default(), first, selected(&["kit-coord"], &a))
                .unwrap(),
        )
        .unwrap();
    // An independently owned layout still pins this release after deselection.
    save(
        &first
            .app_home
            .as_ref()
            .unwrap()
            .join("layouts/personal.json"),
        &json!({"content":{"mod":name("kit-coord", &a)}}),
    )
    .unwrap();
    let mut second = first.clone();
    component_release(&lock, &mut second, &b);
    let mut second = lock
        .activate(
            lock.prepare(&first, second, selected(&["kit-manager"], &b))
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
        .prepare(&State::default(), state.clone(), mismatched)
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
            )
            .unwrap(),
        )
        .unwrap();
    lock.verify_active(&installed).unwrap();
    assert!(mod_path(&home, "kit-coord-ui", &a).exists());
    assert!(!mod_path(&home, "kit-usage", &a).exists());
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
    let first = lock
        .activate(
            lock.prepare(
                &State::default(),
                state,
                selected(&["kit-coord", "kit-usage"], &a),
            )
            .unwrap(),
        )
        .unwrap();
    let descriptor = lock.data.join("kit-coord/active.json");
    let before = fs::read(&descriptor).unwrap();
    let mut selection = selected(&["kit-coord"], &a);
    selection.extend(selected(&["kit-usage"], &b));
    let next = lock
        .activate(lock.prepare(&first, first.clone(), selection).unwrap())
        .unwrap();
    lock.verify_active(&next).unwrap();
    assert_eq!(fs::read(&descriptor).unwrap(), before);
    assert_eq!(
        fs::read_to_string(lock.data.join("bin/kit-coord")).unwrap(),
        a
    );
    assert_eq!(
        fs::read_to_string(lock.data.join("bin/kit-usage")).unwrap(),
        b
    );
    assert_eq!(next.selection["kit-coord"], a);
    assert!(!next.selection.contains_key("kit-manager"));
}

#[test]
fn activation_removes_only_superseded_owned_deployments_and_replays_cleanup() {
    for extra_file in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let lock = Locked::open(&temp.path().join("data")).unwrap();
        let home = temp.path().join("app");
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let mut first = empty_state(&home);
        component_release(&lock, &mut first, &a);
        let first = lock
            .activate(
                lock.prepare(&State::default(), first, selected(BUNDLE, &a))
                    .unwrap(),
            )
            .unwrap();
        let old_source = first.files[&layout(&home)].source.clone();
        let extra = old_source.parent().unwrap().join("personal.txt");
        if extra_file {
            fs::write(&extra, "unowned").unwrap();
        }
        let mut next = first.clone();
        component_release(&lock, &mut next, &b);
        let transaction = lock.prepare(&first, next, selected(BUNDLE, &b)).unwrap();
        assert!(transaction.obsolete_deployments.contains_key(&old_source));
        save(&journal_path(&lock.data), &transaction).unwrap();
        fs::write(&old_source, "edited deployment").unwrap();
        assert!(lock.recover().is_err());
        assert!(!mod_path(&home, "kit-coord", &b).exists());
        fs::copy(layout(&home), &old_source).unwrap();
        lock.recover_with_processes(|| Ok(Vec::new())).unwrap();
        // Replay the crash prefix after cleanup, before journal removal.
        save(&journal_path(&lock.data), &transaction).unwrap();
        lock.recover_with_processes(|| Ok(Vec::new())).unwrap();
        lock.verify_active(&lock.state().unwrap()).unwrap();
        for path in transaction.obsolete_deployments.keys() {
            assert!(!path.exists());
        }
        for file in transaction.state.files.values() {
            assert!(file.source.exists());
        }
        if extra_file {
            assert_eq!(fs::read_to_string(extra).unwrap(), "unowned");
        } else {
            assert!(!old_source.parent().unwrap().exists());
        }
        assert!(!release(&lock.data, &a).exists());
        assert!(!mod_path(&home, "kit-coord", &a).exists());
    }
}
