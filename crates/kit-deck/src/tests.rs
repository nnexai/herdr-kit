use crate::bundle;
use crate::model::{
    AgentStateKind, EventEffect, HcState, InboxKind, build_snapshot, fixture_snapshot,
    parse_coordinator_status,
};
use crate::render::{
    base64_encode, render_agent_slot, render_coordinator, render_inbox, render_inbox_action,
    render_limit_bucket, render_limit_provider, render_overview, svg_to_data_uri, xml_escape,
};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

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
