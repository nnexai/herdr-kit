use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

fn report(provider: &str, fetched: u64, amount: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"provider":provider,"fetchedAt":fetched,"metadata":{"email":"identity-secret"},"limits":[{"id":"copilot:premium","scope":{"accountId":"identity-secret"},"window":{"id":"monthly","resetsAt":fetched+10000},"amount":amount}]})
}
fn parse_reports(
    reports: Vec<serde_json::Value>,
    missing: Vec<serde_json::Value>,
    time: u64,
) -> Vec<ProviderRecord> {
    parse::omp(
        &serde_json::to_vec(&serde_json::json!({"reports":reports,"accountsWithoutUsage":missing}))
            .unwrap(),
        time,
    )
    .unwrap()
}

#[test]
fn anonymous_projection_and_credit_edge_cases() {
    let time = now();
    let values = parse_reports(
        vec![report(
            "github-copilot",
            time,
            serde_json::json!({"unit":"credits","used":0,"limit":8000,"remaining":8000}),
        )],
        vec![],
        time,
    );
    let limit = &values[2].accounts[0].limits[0];
    assert_eq!(limit.used_fraction, Some(0.0));
    assert_eq!(limit.remaining, Some(8000.0));
    let text = serde_json::to_string(&values).unwrap();
    assert!(!text.contains("identity-secret"));
    let unknown = parse_reports(
        vec![report(
            "github-copilot",
            time,
            serde_json::json!({"unit":"requests","used":0,"limit":0}),
        )],
        vec![],
        time,
    );
    assert_eq!(unknown[2].accounts[0].limits[0].used_fraction, None);
    let mut unlimited = report(
        "github-copilot",
        time,
        serde_json::json!({"unit":"requests"}),
    );
    unlimited["limits"][0]["notes"] = serde_json::json!(["Unlimited", "private note"]);
    let unlimited = parse_reports(vec![unlimited], vec![], time);
    assert!(unlimited[2].accounts[0].limits[0].unlimited);
    assert!(
        !serde_json::to_string(&unlimited)
            .unwrap()
            .contains("private note")
    );
}

#[test]
fn enterprise_metadata_is_never_exposed() {
    // OMP 18.18.8 uses the same provider key for enterprise credentials.
    let t = now();
    let mut enterprise = report(
        "github-copilot",
        t,
        serde_json::json!({"unit":"credits","used":4,"limit":8000}),
    );
    enterprise["metadata"]["enterpriseUrl"] = serde_json::json!("private.example");
    let values = parse_reports(vec![enterprise], vec![], t);
    assert_eq!(values[2].accounts[0].limits[0].used_fraction, Some(0.0005));
    assert!(
        !serde_json::to_string(&values)
            .unwrap()
            .contains("private.example")
    );
}

#[test]
fn retention_backoff_and_lease_do_not_accelerate() {
    let t = now();
    let values = parse_reports(
        vec![report(
            "github-copilot",
            t,
            serde_json::json!({"unit":"credits","used":4,"limit":8000}),
        )],
        vec![],
        t,
    );
    let mut source = SourceRecord::default();
    apply(&mut source, Ok(values), &PROVIDERS, t);
    assert!(!due(&source, t + 59_999, true));
    assert!(due(&source, t + 60_000, true));
    apply(
        &mut source,
        Err(QuotaErrorCode::Failed),
        &PROVIDERS,
        t + 60_000,
    );
    assert!(!due(&source, t + 120_000, true));
    let out = response(
        &PersistedQuota {
            schema: 3,
            omp: source,
            antigravity: SourceRecord::default(),
        },
        t + 120_000,
        false,
    );
    assert!(out.providers[2].stale);
    assert_eq!(out.providers[2].accounts[0].fetched_at_ms, t);
    assert!(
        response(
            &PersistedQuota {
                schema: 3,
                omp: SourceRecord {
                    attempted_at_ms: Some(t),
                    succeeded_at_ms: None,
                    next_attempt_at_ms: t + CADENCE,
                    failures: 0,
                    providers: vec![]
                },
                antigravity: SourceRecord::default(),
            },
            t + RETAIN + 1,
            false
        )
        .providers
        .iter()
        .all(|p| p.accounts.is_empty())
    );
}

#[tokio::test]
async fn private_cache_and_single_collector() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("cache");
    let exe = temp.path().join("omp");
    let missing_agy = temp.path().join("missing-agy");
    let calls = temp.path().join("calls");
    let payload = serde_json::json!({"reports":[report("github-copilot", now(), serde_json::json!({"unit":"credits","used":4,"limit":8000}))]});
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\nprintf x >> '{}'\nsleep 0.4\nprintf '%s' '{}'\n",
            calls.display(),
            payload
        ),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let first = snapshot(&root, false, Some(&exe), Some(&missing_agy));
    let second = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        snapshot(&root, false, Some(&exe), Some(&missing_agy))
            .await
            .unwrap()
    };
    let (first, second) = tokio::join!(first, second);
    assert!(second.collecting);
    assert_eq!(
        first.unwrap().providers[2].state,
        QuotaProviderState::Available
    );
    assert_eq!(std::fs::read(&calls).unwrap(), b"x");
    assert_eq!(
        std::fs::metadata(root.join("snapshot.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        !std::fs::read_to_string(root.join("snapshot.json"))
            .unwrap()
            .contains("identity-secret")
    );
}

#[tokio::test]
async fn cache_rejects_symlinks_without_touching_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    std::fs::write(&target, "untouched").unwrap();
    let root = temp.path().join("cache");
    std::fs::create_dir(&root).unwrap();
    symlink(&target, root.join("collect.lock")).unwrap();
    let out = snapshot(&root, false, None, None).await.unwrap();
    assert_eq!(
        out.providers[0].error,
        Some(QuotaErrorCode::CacheUnavailable)
    );
    assert_eq!(
        out.providers[3].error,
        Some(QuotaErrorCode::CacheUnavailable)
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "untouched");
}

#[tokio::test]
async fn excessive_output_is_bounded_and_failure_is_backed_off() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("cache");
    let exe = temp.path().join("omp");
    let missing_agy = temp.path().join("missing-agy");
    let calls = temp.path().join("calls");
    std::fs::write(
        &exe,
        format!("#!/bin/sh\nprintf x >> '{}'\nyes x\n", calls.display()),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let out = snapshot(&root, false, Some(&exe), Some(&missing_agy))
        .await
        .unwrap();
    assert_eq!(out.providers[0].error, Some(QuotaErrorCode::Malformed));
    let out = snapshot(&root, true, Some(&exe), Some(&missing_agy))
        .await
        .unwrap();
    assert_eq!(out.providers[0].error, Some(QuotaErrorCode::Malformed));
    assert_eq!(std::fs::read(calls).unwrap(), b"x");
}

fn agy_payload() -> serde_json::Value {
    serde_json::json!({
        "conversation_id": "",
        "status": "SUCCESS",
        "response": "secret-response-banner",
        "duration_seconds": 0,
        "num_turns": 0,
        "command": {
            "name": "usage",
            "data": {
                "description": "secret-quota-description",
                "groups": [
                    {
                        "name": "Gemini Models",
                        "description": "secret-gemini-group-description",
                        "buckets": [
                            {
                                "id": "gemini-weekly",
                                "name": "Weekly Limit Remaining",
                                "description": "secret-bucket-description",
                                "window": "weekly",
                                "remaining_fraction": 0.875,
                                "reset_time": "2026-11-28T17:09:34Z"
                            },
                            {
                                "id": "gemini-5h",
                                "name": "Five Hour Limit Remaining",
                                "window": "5h",
                                "remaining_fraction": 0.6,
                                "reset_time": "2026-11-21T22:09:34Z"
                            }
                        ]
                    },
                    {
                        "name": "Claude and GPT models",
                        "buckets": [
                            {
                                "id": "3p-weekly",
                                "window": "weekly",
                                "remaining_fraction": 1,
                                "reset_time": "2026-11-28T18:26:14Z"
                            },
                            {
                                "id": "3p-5h",
                                "window": "5h",
                                "remaining_fraction": 0.25,
                                "reset_time": "2026-11-21T23:26:14Z"
                            }
                        ]
                    },
                    {
                        "name": "Gemini 4 Argon",
                        "buckets": [
                            {
                                "id": "gemini-argon-wtu-weekly",
                                "window": "weekly",
                                "remaining_fraction": 1.0,
                                "reset_time": "2026-11-28T18:26:14Z"
                            }
                        ]
                    }
                ]
            }
        }
    })
}

#[test]
fn antigravity_projection_maps_groups_buckets_and_reset_times() {
    let time = now();
    let payload = serde_json::to_vec(&agy_payload()).unwrap();
    let values = parse::antigravity(&payload, time).unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].provider, QuotaProvider::Antigravity);
    assert_eq!(values[0].state, QuotaProviderState::Available);
    let limits = &values[0].accounts[0].limits;
    assert_eq!(limits.len(), 5);
    assert_eq!(limits[0].id, "antigravity:weekly:0");
    assert_eq!(limits[0].tier.as_deref(), Some("gemini"));
    assert_eq!(limits[0].window.as_deref(), Some("weekly"));
    assert!((limits[0].used_fraction.unwrap() - 0.125).abs() < 1e-9);
    assert_eq!(
        limits[0].resets_at_ms,
        parse::parse_rfc3339_ms("2026-11-28T17:09:34Z")
    );
    assert_eq!(limits[1].id, "antigravity:5h:1");
    assert_eq!(limits[1].tier.as_deref(), Some("gemini"));
    assert!((limits[1].used_fraction.unwrap() - 0.4).abs() < 1e-9);
    assert_eq!(limits[2].id, "antigravity:weekly:2");
    assert_eq!(limits[2].tier.as_deref(), Some("claude_gpt"));
    assert_eq!(limits[2].used_fraction, Some(0.0));
    assert_eq!(limits[3].id, "antigravity:5h:3");
    assert_eq!(limits[3].tier.as_deref(), Some("claude_gpt"));
    assert!((limits[3].used_fraction.unwrap() - 0.75).abs() < 1e-9);
    assert_eq!(limits[4].id, "antigravity:weekly:4");
    assert_eq!(limits[4].tier.as_deref(), Some("gemini_argon"));
    for (idx, limit) in limits.iter().enumerate() {
        assert!(parse::valid_limit(limit, QuotaProvider::Antigravity, idx));
    }
    let serialized = serde_json::to_string(&values).unwrap();
    assert!(!serialized.contains("secret-"));
}

#[test]
fn antigravity_edge_cases_and_rfc3339_parsing() {
    let time = now();
    let not_signed = serde_json::to_vec(&serde_json::json!({
        "status": "ERROR",
        "response": "Not signed in. Please sign in to continue."
    }))
    .unwrap();
    let values = parse::antigravity(&not_signed, time).unwrap();
    assert_eq!(values[0].state, QuotaProviderState::NotSignedIn);
    assert_eq!(values[0].error, Some(QuotaErrorCode::NotSignedIn));

    let empty_groups = serde_json::to_vec(&serde_json::json!({
        "status": "SUCCESS",
        "command": { "name": "usage", "data": { "groups": [] } }
    }))
    .unwrap();
    let values = parse::antigravity(&empty_groups, time).unwrap();
    assert_eq!(values[0].state, QuotaProviderState::Unsupported);
    assert_eq!(values[0].error, Some(QuotaErrorCode::Unsupported));

    let bad_fraction = serde_json::to_vec(&serde_json::json!({
        "status": "SUCCESS",
        "command": {
            "name": "usage",
            "data": {
                "groups": [{
                    "name": "Gemini Models",
                    "buckets": [{ "id": "gemini-5h", "window": "5h", "remaining_fraction": 1.5 }]
                }]
            }
        }
    }))
    .unwrap();
    assert_eq!(
        parse::antigravity(&bad_fraction, time).err(),
        Some(QuotaErrorCode::Malformed)
    );

    assert_eq!(parse::parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
    let base = parse::parse_rfc3339_ms("2026-11-21T17:00:00Z").unwrap();
    assert_eq!(
        parse::parse_rfc3339_ms("2026-11-21T19:00:00+02:00"),
        Some(base)
    );
    assert_eq!(
        parse::parse_rfc3339_ms("2026-11-21T11:30:00-05:30"),
        Some(base)
    );
    assert_eq!(
        parse::parse_rfc3339_ms("2026-11-21T17:00:00.12Z"),
        Some(base + 120)
    );
    assert_eq!(
        parse::parse_rfc3339_ms("2026-11-21T17:00:00.123456Z"),
        Some(base + 123)
    );
    let leap = parse::parse_rfc3339_ms("2024-02-29T23:59:59Z").unwrap();
    assert_eq!(
        parse::parse_rfc3339_ms("2024-02-29T23:59:59.5Z"),
        Some(leap + 500)
    );
    assert_eq!(parse::parse_rfc3339_ms("2026-02-29T00:00:00Z"), None);
}

#[tokio::test]
async fn sources_collect_independently_and_isolate_failures() {
    let temp = tempfile::tempdir().unwrap();
    let ok_omp = temp.path().join("ok-omp");
    let ok_agy = temp.path().join("ok-agy");
    let fail_exe = temp.path().join("fail");
    let signed_out_agy = temp.path().join("signed-out-agy");

    let omp_payload = serde_json::json!({"reports":[report("github-copilot", now(), serde_json::json!({"unit":"credits","used":4,"limit":8000}))]});
    std::fs::write(&ok_omp, format!("#!/bin/sh\nprintf '%s' '{omp_payload}'\n")).unwrap();
    std::fs::write(
        &ok_agy,
        format!("#!/bin/sh\nprintf '%s' '{}'\n", agy_payload()),
    )
    .unwrap();
    std::fs::write(&fail_exe, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::write(
        &signed_out_agy,
        "#!/bin/sh\necho 'Please log in first' >&2\nexit 1\n",
    )
    .unwrap();
    for exe in [&ok_omp, &ok_agy, &fail_exe, &signed_out_agy] {
        std::fs::set_permissions(exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    let both = snapshot(&temp.path().join("c1"), false, Some(&ok_omp), Some(&ok_agy))
        .await
        .unwrap();
    assert_eq!(both.providers[2].state, QuotaProviderState::Available);
    assert_eq!(both.providers[3].provider, QuotaProvider::Antigravity);
    assert_eq!(both.providers[3].state, QuotaProviderState::Available);
    assert_eq!(both.providers[3].accounts[0].limits.len(), 5);

    let omp_fails = snapshot(
        &temp.path().join("c2"),
        false,
        Some(&fail_exe),
        Some(&ok_agy),
    )
    .await
    .unwrap();
    assert_eq!(
        omp_fails.providers[0].state,
        QuotaProviderState::Unavailable
    );
    assert_eq!(omp_fails.providers[0].error, Some(QuotaErrorCode::Failed));
    assert_eq!(omp_fails.providers[3].state, QuotaProviderState::Available);

    let agy_fails = snapshot(
        &temp.path().join("c3"),
        false,
        Some(&ok_omp),
        Some(&fail_exe),
    )
    .await
    .unwrap();
    assert_eq!(agy_fails.providers[2].state, QuotaProviderState::Available);
    assert_eq!(
        agy_fails.providers[3].state,
        QuotaProviderState::Unavailable
    );
    assert_eq!(agy_fails.providers[3].error, Some(QuotaErrorCode::Failed));

    let agy_logged_out = snapshot(
        &temp.path().join("c4"),
        false,
        Some(&ok_omp),
        Some(&signed_out_agy),
    )
    .await
    .unwrap();
    assert_eq!(
        agy_logged_out.providers[3].state,
        QuotaProviderState::NotSignedIn
    );
    assert_eq!(
        agy_logged_out.providers[3].error,
        Some(QuotaErrorCode::NotSignedIn)
    );
}
