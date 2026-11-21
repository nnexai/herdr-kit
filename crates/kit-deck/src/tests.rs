use crate::actions::{build_answer_question_cmd, build_start_coordinator_cmd};
use crate::bundle;
use crate::model::{
    AgentStateKind, EventEffect, HcState, InboxKind, LIMITS_STALE_AFTER_MS, build_snapshot,
    fixture_snapshot, parse_coordinator_status,
};
use crate::plugin::{
    AgentSettings, InboxActionSettings, PressTarget, StoredSettings, UUID_AGENT,
    UUID_INBOX_ACTION, capture_press_target, resolve_agent_hold_interrupt,
    resolve_agent_tap_focus, resolve_reply_hold,
};
use crate::render::{
    base64_encode, clip_to_width, measure_text, render_agent_slot, render_coordinator,
    render_inbox, render_inbox_action, render_limit_bucket, render_limit_provider, render_overview,
    svg_to_data_uri, wrap_lines_by_width, xml_escape,
};
use crate::sources::{SharedRawState, SourceConfig, run_bounded, start_background};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::process::Command;

// Serialize tests that write executable scripts and/or spawn subprocesses within the
// shared test process. On Linux, a sibling test thread calling fork() while another
// thread holds a writable FD (even O_CLOEXEC) to a newly created script temporarily
// inherits that writable FD until execve(), causing execve() of the script to fail
// with ETXTBSY (os error 26).
static SUBPROCESS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn hc_events_fold_filters_shells_and_sorts_by_urgency() {
    let mut hc = HcState::default();
    let initial = json!({
        "seq": 1,
        "terminals": [
            {
                "address": "t1",
                "state": "live",
                "status": null,
                "turn": 0,
                "cwd": "/tmp",
                "labels": { "where": "home / 1" }
            },
            {
                "address": "t2",
                "state": "live",
                "status": null,
                "turn": 0,
                "cwd": "/tmp",
                "name": "coord-watcher",
                "labels": { "coord.role": "watcher", "where": "coordinator / watcher" }
            },
            {
                "address": "t3",
                "state": "live",
                "status": { "app": "omp", "state": "idle" },
                "status_ms": 100,
                "turn": 1,
                "cwd": "/prj/app",
                "labels": { "where": "app / 1", "herdr.seen": "1" }
            },
            {
                "address": "t4",
                "state": "live",
                "status": { "app": "antigravity", "state": "working" },
                "status_ms": 200,
                "turn": 2,
                "cwd": "/prj/kit",
                "labels": { "where": "kit / Antigravity", "herdr.seen": "2" }
            },
            {
                "address": "t5",
                "state": "live",
                "status": { "app": "omp", "state": "done" },
                "status_ms": 300,
                "turn": 5,
                "cwd": "/prj/demo",
                "labels": { "where": "demo / 1", "herdr.seen": "4" }
            }
        ]
    });
    assert!(matches!(
        hc.apply_event_line(&initial.to_string()),
        EventEffect::Updated {
            wake_coord: true,
            ..
        }
    ));
    let agents = hc.agents();
    assert_eq!(agents.len(), 3);
    assert_eq!(agents[0].address, "t4"); // working
    assert_eq!(agents[0].agent_short, "AGY");
    assert_eq!(agents[1].address, "t5"); // unseen done
    assert!(agents[1].unseen_done);
    assert_eq!(agents[2].address, "t3"); // idle

    let blocked_ev = json!({
        "jsonrpc": "2.0",
        "method": "event",
        "params": {
            "terminal": "t3",
            "type": "status",
            "data": { "status": { "app": "omp", "state": "blocked", "msg": "Need input" }, "status_ms": 500 }
        }
    });
    hc.apply_event_line(&blocked_ev.to_string());
    let agents = hc.agents();
    assert_eq!(agents[0].address, "t3");
    assert_eq!(agents[0].status, AgentStateKind::Blocked);

    let remove_ev = json!({
        "jsonrpc": "2.0",
        "method": "event",
        "params": { "terminal": "t4", "type": "removed" }
    });
    hc.apply_event_line(&remove_ev.to_string());
    assert_eq!(hc.agents().len(), 2);
}

#[test]
fn inbox_deduplicates_coordinator_and_agent_blockers() {
    let now = 2_000_000;
    let mut hc = HcState::default();
    let initial = json!({
        "terminals": [
            {
                "address": "t1",
                "state": "live",
                "status": { "app": "omp", "state": "blocked", "msg": "Waiting" },
                "status_ms": now - 10_000,
                "turn": 1,
                "cwd": "/prj/a",
                "labels": { "where": "a / 1" }
            },
            {
                "address": "t2",
                "state": "live",
                "status": { "app": "claude", "state": "blocked", "msg": "Approve bash" },
                "status_ms": now - 5_000,
                "turn": 2,
                "cwd": "/prj/b",
                "labels": { "where": "b / 1" }
            },
            {
                "address": "t3",
                "state": "live",
                "status": { "app": "omp", "state": "done" },
                "status_ms": now - 1_000,
                "turn": 3,
                "cwd": "/prj/c",
                "labels": { "where": "c / 1", "herdr.seen": "2" }
            }
        ]
    });
    hc.apply_event_line(&initial.to_string());
    let coord = parse_coordinator_status(
        &json!({
            "state": "running",
            "enabled": true,
            "coordinator": { "terminal": "t4y" },
            "counts": { "running": 0, "needs_you": 1, "retained": 0, "open_workstreams": 1 }
        })
        .to_string()
        .into_bytes(),
    );
    let board = json!({
        "open_questions": [{
            "qid": "q-9",
            "text": "Proceed with deploy?",
            "options": ["Yes", "No"],
            "ws": "ws-1",
            "task": "ws-1-t1",
            "since_ms": now - 12_000
        }],
        "attention": [{
            "id": "blocked-4",
            "kind": "task_question",
            "ws": "ws-1",
            "task": "ws-1-t1",
            "since_ms": now - 12_000,
            "text": "Duplicate task question",
            "qid": "q-9"
        }],
        "workstreams": [{
            "id": "ws-1",
            "title": "Deploy",
            "space": { "name": "a" },
            "tasks": [{ "id": "ws-1-t1", "title": "Ship", "terminal": "t1", "harness": "omp" }]
        }]
    });
    let snap = build_snapshot(
        now,
        &hc,
        coord.as_ref(),
        Some(&board.to_string().into_bytes()),
        None,
    );
    // t1 is already covered by q-9 so it should not appear twice.
    assert_eq!(snap.inbox.len(), 3);
    assert_eq!(snap.inbox[0].kind, InboxKind::Question);
    assert_eq!(snap.inbox[0].terminal.as_deref(), Some("t1"));
    assert_eq!(snap.inbox[1].kind, InboxKind::Blocked);
    assert_eq!(snap.inbox[1].terminal.as_deref(), Some("t2"));
    assert_eq!(snap.inbox[2].kind, InboxKind::Done);
    assert_eq!(snap.inbox[2].terminal.as_deref(), Some("t3"));
    assert_eq!(snap.counts.needs_you, 2);
}

#[test]
fn limits_and_svg_rendering_produce_valid_escaped_data_uris() {
    let snap = fixture_snapshot(1_795_286_726_000);
    assert!(snap.limits.available);
    assert_eq!(snap.limits.providers.len(), 4);
    assert_eq!(snap.limits.worst_provider.as_deref(), Some("antigravity"));
    let agy = snap.provider("antigravity").unwrap();
    assert_eq!(agy.buckets.len(), 5);
    assert_eq!(agy.buckets[0].label, "GEM WK");
    assert_eq!(agy.buckets[1].label, "GEM 5H");
    assert_eq!(agy.buckets[4].label, "ARGON");

    let svgs = [
        render_overview(&snap),
        render_inbox(&snap, 0),
        render_inbox_action(&snap, 0, "reply_1", false),
        render_agent_slot(&snap, 1, "all", None),
        render_coordinator(&snap),
        render_limit_provider(&snap, "antigravity", "antigravity"),
        render_limit_bucket(&snap, "antigravity", "antigravity", 1),
    ];
    for svg in svgs {
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
        let uri = svg_to_data_uri(&svg);
        assert!(uri.starts_with("data:image/svg+xml;base64,"));
    }
    assert_eq!(
        xml_escape("A & <B> \"C\""),
        "A &amp; &lt;B&gt; &quot;C&quot;"
    );
    assert_eq!(base64_encode(b"hi!"), "aGkh");
}

#[test]
fn bundle_install_and_uninstall_stage_complete_sdplugin() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let plugins_dir = root.join("plugins");
    let fake_bin = root.join("kit-deck");
    std::fs::write(&fake_bin, b"#!/bin/sh\necho ok\n").unwrap();
    let data_dir = root.join("data");

    let report = bundle::install(&plugins_dir, &fake_bin, Some(&data_dir), false).unwrap();
    assert!(report.installed);
    assert_eq!(report.actions, 7);
    assert!(report.plugin_dir.join("manifest.json").is_file());
    assert!(report.plugin_dir.join("icons/plugin.svg").is_file());
    assert!(report.plugin_dir.join("pi/agent.html").is_file());
    assert!(report.plugin_dir.join("config.json").is_file());
    let mode = std::fs::metadata(&report.binary)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755);

    let removed = bundle::uninstall(&plugins_dir).unwrap();
    assert!(removed.removed);
    assert!(!report.plugin_dir.exists());
}

#[tokio::test]
async fn coordinator_commands_preserve_hc_terminal_id_caller_identity() {
    let _guard = SUBPROCESS_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let fake_coord = temp.path().join("fake-kit-coord");
    std::fs::write(
        &fake_coord,
        "#!/bin/sh\nif [ -n \"$HC_TERMINAL_ID\" ]; then\n  printf 'forbidden_caller:%s' \"$HC_TERMINAL_ID\" >&2\n  exit 1\nfi\nprintf 'ok'\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake_coord, std::fs::Permissions::from_mode(0o755)).unwrap();

    let cfg = SourceConfig {
        data_dir: temp.path().join("data"),
        hc: PathBuf::from("hc"),
        herdr_app: PathBuf::from("herdr-app"),
        kit_coord: fake_coord,
        kit_usage: PathBuf::from("kit-usage"),
    };

    let mut up_cmd = build_start_coordinator_cmd(&cfg);
    up_cmd.env("HC_TERMINAL_ID", "t-delegate-42");
    let up_err = run_bounded(up_cmd, Duration::from_secs(5), 4096)
        .await
        .unwrap_err();
    assert!(
        up_err.to_string().contains("forbidden_caller:t-delegate-42"),
        "expected caller identity refusal for up, got: {up_err}"
    );

    let mut say_cmd = build_answer_question_cmd(&cfg, "q-1", "Yes");
    say_cmd.env("HC_TERMINAL_ID", "t-coord-9");
    let say_err = run_bounded(say_cmd, Duration::from_secs(5), 4096)
        .await
        .unwrap_err();
    assert!(
        say_err.to_string().contains("forbidden_caller:t-coord-9"),
        "expected caller identity refusal for say, got: {say_err}"
    );
}

#[tokio::test]
async fn oversized_command_output_fails_fast_instead_of_timing_out() {
    let _guard = SUBPROCESS_TEST_LOCK.lock().await;
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "yes x"]);
    let started = Instant::now();
    let err = run_bounded(cmd, Duration::from_secs(10), 1024)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("exceeded"),
        "expected output-limit error, got: {err}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "oversized output waited too long: {:?}",
        started.elapsed()
    );
}

#[test]
fn hold_reply_and_interrupt_bind_to_key_down_identity() {
    let now = 3_000_000;
    let mut hc = HcState::default();
    hc.apply_event_line(
        &json!({
            "terminals": [
                {
                    "address": "t1",
                    "state": "live",
                    "status": { "app": "omp", "state": "working" },
                    "status_ms": now - 1_000,
                    "turn": 1,
                    "cwd": "/prj/a",
                    "labels": { "where": "alpha / 1" }
                },
                {
                    "address": "t2",
                    "state": "live",
                    "status": { "app": "omp", "state": "working" },
                    "status_ms": now - 5_000,
                    "turn": 2,
                    "cwd": "/prj/b",
                    "labels": { "where": "beta / 1" }
                }
            ]
        })
        .to_string(),
    );
    let board_initial = json!({
        "open_questions": [
            { "qid": "q-1", "text": "Approve alpha?", "options": ["Yes", "No"], "since_ms": now - 20_000 },
            { "qid": "q-2", "text": "Delete beta?", "options": ["Delete", "Keep"], "since_ms": now - 10_000 }
        ],
        "attention": [],
        "workstreams": []
    });
    let snap_down = build_snapshot(
        now,
        &hc,
        None,
        Some(&board_initial.to_string().into_bytes()),
        None,
    );

    // 1) Key-down on reply_1 at inbox index 0 binds to q-1 / "Yes".
    let reply_settings = StoredSettings::InboxAction(InboxActionSettings {
        mode: "reply_1".into(),
    });
    let captured_reply = capture_press_target(UUID_INBOX_ACTION, &reply_settings, &snap_down, 0);
    assert_eq!(
        captured_reply,
        PressTarget::InboxReply {
            qid: "q-1".into(),
            option_index: 0,
            text: "Yes".into(),
        }
    );

    // If q-1 closes and q-2 slides into index 0, key-up must NOT answer q-2.
    let board_closed = json!({
        "open_questions": [
            { "qid": "q-2", "text": "Delete beta?", "options": ["Delete", "Keep"], "since_ms": now - 10_000 }
        ],
        "attention": [],
        "workstreams": []
    });
    let snap_closed = build_snapshot(
        now + 700,
        &hc,
        None,
        Some(&board_closed.to_string().into_bytes()),
        None,
    );
    assert_eq!(resolve_reply_hold(&captured_reply, &snap_closed), None);

    // If a new earlier question shifts q-1 to index 1, key-up still answers q-1.
    let board_shifted = json!({
        "open_questions": [
            { "qid": "q-0", "text": "Urgent?", "options": ["Now", "Later"], "since_ms": now - 30_000 },
            { "qid": "q-1", "text": "Approve alpha?", "options": ["Yes", "No"], "since_ms": now - 20_000 }
        ],
        "attention": [],
        "workstreams": []
    });
    let snap_shifted = build_snapshot(
        now + 700,
        &hc,
        None,
        Some(&board_shifted.to_string().into_bytes()),
        None,
    );
    assert_eq!(
        resolve_reply_hold(&captured_reply, &snap_shifted),
        Some(("q-1".into(), "Yes".into()))
    );

    // 2) Key-down on agent slot 1 binds to t1 (working).
    let slot_settings = StoredSettings::Agent(AgentSettings {
        slot: 1,
        filter: "all".into(),
        pin_space: None,
    });
    let captured_agent = capture_press_target(UUID_AGENT, &slot_settings, &snap_down, 0);
    assert_eq!(
        captured_agent,
        PressTarget::AgentSlot {
            terminal: "t1".into(),
            was_working: true,
        }
    );

    // If t2 overtakes slot 1 while t1 is still working in slot 2, hold still interrupts t1.
    let mut hc_reordered = HcState::default();
    hc_reordered.apply_event_line(
        &json!({
            "terminals": [
                {
                    "address": "t2",
                    "state": "live",
                    "status": { "app": "omp", "state": "working" },
                    "status_ms": now + 500,
                    "turn": 2,
                    "cwd": "/prj/b",
                    "labels": { "where": "beta / 1" }
                },
                {
                    "address": "t1",
                    "state": "live",
                    "status": { "app": "omp", "state": "working" },
                    "status_ms": now - 1_000,
                    "turn": 1,
                    "cwd": "/prj/a",
                    "labels": { "where": "alpha / 1" }
                }
            ]
        })
        .to_string(),
    );
    let snap_reordered = build_snapshot(now + 700, &hc_reordered, None, None, None);
    assert_eq!(
        snap_reordered.agent_for_slot(1, "all", None).unwrap().address,
        "t2"
    );
    assert_eq!(
        resolve_agent_hold_interrupt(&captured_agent, &snap_reordered),
        Some("t1".into())
    );
    assert_eq!(
        resolve_agent_tap_focus(&captured_agent, &snap_reordered),
        Some("t1".into())
    );

    // If t1 finishes while t2 takes slot 1 (working), hold must NOT interrupt t2.
    let mut hc_finished = HcState::default();
    hc_finished.apply_event_line(
        &json!({
            "terminals": [
                {
                    "address": "t2",
                    "state": "live",
                    "status": { "app": "omp", "state": "working" },
                    "status_ms": now + 500,
                    "turn": 2,
                    "cwd": "/prj/b",
                    "labels": { "where": "beta / 1" }
                },
                {
                    "address": "t1",
                    "state": "live",
                    "status": { "app": "omp", "state": "done" },
                    "status_ms": now + 600,
                    "turn": 1,
                    "cwd": "/prj/a",
                    "labels": { "where": "alpha / 1" }
                }
            ]
        })
        .to_string(),
    );
    let snap_finished = build_snapshot(now + 700, &hc_finished, None, None, None);
    assert_eq!(
        resolve_agent_hold_interrupt(&captured_agent, &snap_finished),
        None
    );
}

#[tokio::test]
async fn data_dir_resolution_honors_env_and_wrapper_config_json() {
    let _guard = SUBPROCESS_TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let plugins_dir = root.join("plugins");
    let data_from_cfg = root.join("cfg-data");
    let data_from_env = root.join("env-data");
    let fake_bin = root.join("real-target/kit-deck");
    std::fs::create_dir_all(fake_bin.parent().unwrap()).unwrap();
    std::fs::write(&fake_bin, "#!/bin/sh\nprintf '%s' \"$KIT_DECK_PLUGIN_DIR\"\n").unwrap();
    std::fs::set_permissions(&fake_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Install in wrapper mode with an explicit data dir.
    let report = bundle::install(&plugins_dir, &fake_bin, Some(&data_from_cfg), true).unwrap();
    assert_eq!(report.mode, "wrapper");

    // Execute the installed wrapper script and verify it sets KIT_DECK_PLUGIN_DIR to the plugin dir.
    let mut wrapper_cmd = Command::new(&report.binary);
    wrapper_cmd.args(["-port", "1234", "-pluginUUID", "abc"]);
    let out = run_bounded(wrapper_cmd, Duration::from_secs(5), 4096)
        .await
        .unwrap();
    let reported_plugin_dir = PathBuf::from(String::from_utf8(out).unwrap());
    assert_eq!(reported_plugin_dir, report.plugin_dir);

    // Wrapper mode resolution: exe_path is outside the plugin bundle, plugin_dir locates config.json.
    let cfg_wrapper = SourceConfig::resolve_with(
        None,
        None,
        Some(&reported_plugin_dir),
        Some(&fake_bin),
    )
    .unwrap();
    assert_eq!(cfg_wrapper.data_dir, data_from_cfg);

    // OpenAction launch with KIT_DECK_DATA_DIR overrides bundle config.json even when cli_data_dir is None.
    let cfg_env = SourceConfig::resolve_with(
        None,
        Some(data_from_env.clone()),
        Some(&reported_plugin_dir),
        Some(&fake_bin),
    )
    .unwrap();
    assert_eq!(cfg_env.data_dir, data_from_env);

    // Copy mode resolution: plugin_dir is None, exe_path inside plugin/bin locates config.json.
    let cfg_copy =
        SourceConfig::resolve_with(None, None, None, Some(&report.binary)).unwrap();
    assert_eq!(cfg_copy.data_dir, data_from_cfg);
}

#[test]
fn failed_usage_poll_and_local_age_mark_gauges_stale() {
    let now = 5_000_000;
    let fixture = fixture_snapshot(now);
    assert!(fixture.limits.providers.iter().all(|p| !p.stale));
    assert!(
        fixture
            .limits
            .providers
            .iter()
            .flat_map(|p| p.buckets.iter())
            .all(|b| !b.stale)
    );

    let mut shared = SharedRawState::default();
    assert!(shared.apply_usage_poll(Some(fixture.limits.clone())));
    assert!(!shared.limits.as_ref().unwrap().providers[0].stale);

    // Failed poll marks cached providers and buckets stale and requests a broadcast.
    assert!(shared.apply_usage_poll(None));
    let stale_limits = shared.limits.as_ref().unwrap();
    assert!(!stale_limits.collecting);
    assert!(stale_limits.providers.iter().all(|p| p.stale));
    assert!(
        stale_limits
            .providers
            .iter()
            .flat_map(|p| p.buckets.iter())
            .all(|b| b.stale)
    );

    let stale_snap = shared.snapshot();
    let provider_svg = render_limit_provider(&stale_snap, "antigravity", "antigravity");
    let bucket_svg = render_limit_bucket(&stale_snap, "antigravity", "antigravity", 1);
    assert!(provider_svg.contains(">~</text>"));
    assert!(bucket_svg.contains(">~</text>"));

    // Local staleness aging also marks an old snapshot stale when now_ms advances past the threshold.
    let aged = build_snapshot(
        now + LIMITS_STALE_AFTER_MS + 1,
        &HcState::default(),
        None,
        None,
        Some(fixture.limits),
    );
    assert!(aged.limits.providers.iter().all(|p| p.stale));
    assert!(
        aged.limits
            .providers
            .iter()
            .flat_map(|p| p.buckets.iter())
            .all(|b| b.stale)
    );
}

#[test]
fn width_measured_wrapping_prevents_wide_label_clipping() {
    let max_width = 122.0;

    // Exactly 12 chars, but wide glyphs exceed 122 SVG units at 21px -> must wrap to 2 lines.
    let ww = wrap_lines_by_width("WW Workspace", 21.0, max_width, 2);
    assert_eq!(ww, vec!["WW", "Workspace"]);
    for line in &ww {
        assert!(
            measure_text(line, 21.0) <= max_width,
            "line {line:?} exceeded {max_width}: {}",
            measure_text(line, 21.0)
        );
    }

    let wide = wrap_lines_by_width("Wide Workspace", 21.0, max_width, 2);
    assert_eq!(wide, vec!["Wide", "Workspace"]);

    // Unbreakable wide word and overflowing multi-word labels stay within width budget.
    for sample in [
        "WWWWWWWWWWWW",
        "MMMMMMMMMMMM Workspace",
        "WW Workspace Confirm migration details right now",
    ] {
        let lines = wrap_lines_by_width(sample, 21.0, max_width, 2);
        assert!(!lines.is_empty() && lines.len() <= 2);
        for line in &lines {
            assert!(
                measure_text(line, 21.0) <= max_width,
                "wrapped line {line:?} from {sample:?} exceeded {max_width}: {}",
                measure_text(line, 21.0)
            );
        }
    }
    let clipped = clip_to_width("WWWWWWWWWWWW", 24.0, max_width);
    assert!(clipped.ends_with('…'));
    assert!(measure_text(&clipped, 24.0) <= max_width);

    // End-to-end agent tile check for "WW Workspace" renders two lines instead of one clipped line.
    let mut hc = HcState::default();
    hc.apply_event_line(
        &json!({
            "terminals": [{
                "address": "t9",
                "state": "live",
                "status": { "app": "omp", "state": "working" },
                "status_ms": 100,
                "turn": 1,
                "cwd": "/prj/ww",
                "labels": { "where": "WW Workspace / 1" }
            }]
        })
        .to_string(),
    );
    let snap = build_snapshot(1000, &hc, None, None, None);
    let svg = render_agent_slot(&snap, 1, "all", None);
    assert!(svg.contains(">WW</text>"), "expected wrapped line 1 in {svg}");
    assert!(
        svg.contains(">Workspace</text>"),
        "expected wrapped line 2 in {svg}"
    );

    // Multi-word and single-word wide Unicode names must wrap/clip inside the 144-unit viewport.
    let cyrillic = wrap_lines_by_width("ЖЖЖЖ ЖЖЖЖ", 21.0, max_width, 2);
    assert_eq!(cyrillic, vec!["ЖЖЖЖ", "ЖЖЖЖ"]);
    for line in &cyrillic {
        assert!(
            measure_text(line, 21.0) <= max_width,
            "Unicode line {line:?} exceeded {max_width}: {}",
            measure_text(line, 21.0)
        );
    }
    for sample in ["ЖЖЖЖЖЖЖЖ", "ЩЩЩЩЩЩ", "ЖЖЖЖ ЖЖЖЖ ЖЖЖЖ"] {
        let lines = wrap_lines_by_width(sample, 21.0, max_width, 2);
        assert!(!lines.is_empty() && lines.len() <= 2);
        for line in &lines {
            assert!(
                measure_text(line, 21.0) <= max_width,
                "wrapped Unicode line {line:?} from {sample:?} exceeded {max_width}: {}",
                measure_text(line, 21.0)
            );
        }
    }
    let mut hc_unicode = HcState::default();
    hc_unicode.apply_event_line(
        &json!({
            "terminals": [{
                "address": "t10",
                "state": "live",
                "status": { "app": "omp", "state": "working" },
                "status_ms": 100,
                "turn": 1,
                "cwd": "/prj/unicode",
                "labels": { "where": "ЖЖЖЖ ЖЖЖЖ / 1" }
            }]
        })
        .to_string(),
    );
    let snap_unicode = build_snapshot(1000, &hc_unicode, None, None, None);
    let svg_unicode = render_agent_slot(&snap_unicode, 1, "all", None);
    assert!(
        !svg_unicode.contains(">ЖЖЖЖ ЖЖЖЖ</text>"),
        "expected wide Unicode name to wrap instead of staying on one line in {svg_unicode}"
    );
    assert_eq!(
        svg_unicode.matches(">ЖЖЖЖ</text>").count(),
        2,
        "expected two wrapped lines for 'ЖЖЖЖ ЖЖЖЖ' in {svg_unicode}"
    );
}

#[test]
fn preview_fixture_uses_neutral_placeholder_project_names() {
    let snap = fixture_snapshot(1_795_286_726_000);
    let spaces: Vec<&str> = snap.agents.iter().map(|a| a.space.as_str()).collect();
    assert_eq!(spaces, vec!["api-service", "herdr-kit", "web-app", "sample-app"]);
    let serialized = serde_json::to_string(&snap).unwrap();
    assert!(serialized.contains("api-service"));
    assert!(serialized.contains("web-app"));
    assert!(serialized.contains("sample-app"));
    assert!(!serialized.contains("/prj/"));
}

#[test]
fn usage_poller_releases_shared_mutex_during_wait() {
    let _guard = SUBPROCESS_TEST_LOCK.blocking_lock();
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("usage-polled");
    let fake_usage = temp.path().join("fake-kit-usage");
    let fake_coord = temp.path().join("fake-kit-coord");
    let fake_hc = temp.path().join("fake-hc");

    std::fs::write(
        &fake_usage,
        format!(
            "#!/bin/sh\nprintf '{{\"collecting\":false,\"generated_at_ms\":1000,\"providers\":[]}}\\n'\ntouch '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake_usage, std::fs::Permissions::from_mode(0o755)).unwrap();

    std::fs::write(&fake_coord, "#!/bin/sh\nprintf '{{}}\\n'\n").unwrap();
    std::fs::set_permissions(&fake_coord, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Emit an initial idle agent, wait until the usage poller has completed its non-collecting
    // poll and entered its 60s wait, then emit a status update transitioning the agent to working.
    // If the usage poller kept the shared mutex across its wait, this update would stall for 60s.
    let initial_line = json!({
        "seq": 1,
        "terminals": [{
            "address": "t1",
            "state": "live",
            "status": { "app": "omp", "state": "idle" },
            "status_ms": 100,
            "turn": 1,
            "cwd": "/home/user/projects/api-service",
            "labels": { "where": "api-service / 1" }
        }]
    })
    .to_string();
    let working_line = json!({
        "jsonrpc": "2.0",
        "method": "event",
        "params": {
            "terminal": "t1",
            "type": "status",
            "data": { "status": { "app": "omp", "state": "working" }, "status_ms": 200 }
        }
    })
    .to_string();
    std::fs::write(
        &fake_hc,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\nwhile [ ! -f '{}' ]; do sleep 0.02; done\nsleep 0.08\nprintf '%s\\n' '{}'\nexec sleep 10\n",
            initial_line,
            marker.display(),
            working_line
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake_hc, std::fs::Permissions::from_mode(0o755)).unwrap();

    let cfg = SourceConfig {
        data_dir: temp.path().join("data"),
        hc: fake_hc,
        herdr_app: PathBuf::from("herdr-app"),
        kit_coord: fake_coord,
        kit_usage: fake_usage,
    };

    // Use a dedicated runtime so all background poller tasks are shut down before
    // SUBPROCESS_TEST_LOCK is released.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (mut rx, _refresh) = start_background(cfg);
        let updated = tokio::time::timeout(
            Duration::from_secs(3),
            rx.wait_for(|snap| snap.limits.available && snap.counts.working == 1),
        )
        .await
        .expect("HC event update stalled while usage poller was waiting")
        .expect("snapshot channel closed unexpectedly");
        assert!(updated.limits.available);
        assert_eq!(updated.counts.working, 1);
    });
    drop(rt);
}
