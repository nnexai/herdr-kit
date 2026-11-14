use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

fn report(provider: &str, fetched: u64, amount: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"provider":provider,"fetchedAt":fetched,"metadata":{"email":"identity-secret"},"limits":[{"id":"copilot:premium","scope":{"accountId":"identity-secret"},"window":{"id":"monthly","resetsAt":fetched+10000},"amount":amount}]})
}
fn parse_reports(reports: Vec<serde_json::Value>, missing: Vec<serde_json::Value>, time: u64) -> Vec<ProviderRecord> {
    parse::omp(&serde_json::to_vec(&serde_json::json!({"reports":reports,"accountsWithoutUsage":missing})).unwrap(), time).unwrap()
}

#[test]
fn anonymous_projection_and_credit_edge_cases() {
    let time = now();
    let values = parse_reports(vec![report("github-copilot", time, serde_json::json!({"unit":"credits","used":0,"limit":8000,"remaining":8000}))], vec![], time);
    let limit = &values[2].accounts[0].limits[0];
    assert_eq!(limit.used_fraction, Some(0.0));
    assert_eq!(limit.remaining, Some(8000.0));
    let text = serde_json::to_string(&values).unwrap();
    assert!(!text.contains("identity-secret"));
    let unknown = parse_reports(vec![report("github-copilot", time, serde_json::json!({"unit":"requests","used":0,"limit":0}))], vec![], time);
    assert_eq!(unknown[2].accounts[0].limits[0].used_fraction, None);
    let mut unlimited = report("github-copilot", time, serde_json::json!({"unit":"requests"}));
    unlimited["limits"][0]["notes"] = serde_json::json!(["Unlimited", "private note"]);
    let unlimited = parse_reports(vec![unlimited], vec![], time);
    assert!(unlimited[2].accounts[0].limits[0].unlimited);
    assert!(!serde_json::to_string(&unlimited).unwrap().contains("private note"));
}

#[test]
fn enterprise_metadata_is_never_exposed() {
    // OMP 18.18.8 uses the same provider key for enterprise credentials.
    let t = now();
    let mut enterprise = report("github-copilot", t, serde_json::json!({"unit":"credits","used":4,"limit":8000}));
    enterprise["metadata"]["enterpriseUrl"] = serde_json::json!("private.example");
    let values = parse_reports(vec![enterprise], vec![], t);
    assert_eq!(values[2].accounts[0].limits[0].used_fraction, Some(0.0005));
    assert!(!serde_json::to_string(&values).unwrap().contains("private.example"));
}

#[test]
fn retention_backoff_and_lease_do_not_accelerate() {
    let t = now();
    let values = parse_reports(vec![report("github-copilot", t, serde_json::json!({"unit":"credits","used":4,"limit":8000}))], vec![], t);
    let mut source = SourceRecord::default();
    apply(&mut source, Ok(values), &PROVIDERS, t);
    assert!(!due(&source, t+59_999, true));
    assert!(due(&source, t+60_000, true));
    apply(&mut source, Err(QuotaErrorCode::Failed), &PROVIDERS, t+60_000);
    assert!(!due(&source, t+120_000, true));
    let out = response(&PersistedQuota { schema:2, omp:source }, t+120_000, false);
    assert!(out.providers[2].stale);
    assert_eq!(out.providers[2].accounts[0].fetched_at_ms, t);
    assert!(response(&PersistedQuota { schema:2, omp:SourceRecord { attempted_at_ms:Some(t), succeeded_at_ms:None, next_attempt_at_ms:t+CADENCE, failures:0, providers:vec![] } }, t+RETAIN+1, false).providers.iter().all(|p| p.accounts.is_empty()));
}

#[tokio::test]
async fn private_cache_and_single_collector() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("cache");
    let exe = temp.path().join("omp");
    let calls = temp.path().join("calls");
    let payload = serde_json::json!({"reports":[report("github-copilot", now(), serde_json::json!({"unit":"credits","used":4,"limit":8000}))]});
    std::fs::write(&exe, format!("#!/bin/sh\nprintf x >> '{}'\nsleep 0.4\nprintf '%s' '{}'\n", calls.display(), payload)).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let first = snapshot(&root, false, Some(&exe));
    let second = async { tokio::time::sleep(Duration::from_millis(100)).await; snapshot(&root, false, Some(&exe)).await.unwrap() };
    let (first, second) = tokio::join!(first, second);
    assert!(second.collecting);
    assert_eq!(first.unwrap().providers[2].state, QuotaProviderState::Available);
    assert_eq!(std::fs::read(&calls).unwrap(), b"x");
    assert_eq!(std::fs::metadata(root.join("snapshot.json")).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(!std::fs::read_to_string(root.join("snapshot.json")).unwrap().contains("identity-secret"));
}

#[tokio::test]
async fn cache_rejects_symlinks_without_touching_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    std::fs::write(&target, "untouched").unwrap();
    let root = temp.path().join("cache");
    std::fs::create_dir(&root).unwrap();
    symlink(&target, root.join("collect.lock")).unwrap();
    let out = snapshot(&root, false, None).await.unwrap();
    assert_eq!(out.providers[0].error, Some(QuotaErrorCode::CacheUnavailable));
    assert_eq!(std::fs::read_to_string(target).unwrap(), "untouched");
}

#[tokio::test]
async fn excessive_output_is_bounded_and_failure_is_backed_off() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("cache");
    let exe = temp.path().join("omp");
    let calls = temp.path().join("calls");
    std::fs::write(&exe, format!("#!/bin/sh\nprintf x >> '{}'\nyes x\n", calls.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let out = snapshot(&root, false, Some(&exe)).await.unwrap();
    assert_eq!(out.providers[0].error, Some(QuotaErrorCode::Malformed));
    let out = snapshot(&root, true, Some(&exe)).await.unwrap();
    assert_eq!(out.providers[0].error, Some(QuotaErrorCode::Malformed));
    assert_eq!(std::fs::read(calls).unwrap(), b"x");
}
