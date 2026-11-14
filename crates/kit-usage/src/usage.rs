//! Identity-free, bounded subscription usage collection.
mod cache;
mod parse;
mod source;
#[cfg(test)]
mod tests;
use fs2::FileExt;
use rustix::fs::{self, Mode, OFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncReadExt;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaProvider {
    Codex,
    Claude,
    Copilot,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaProviderState {
    Pending,
    Available,
    NotSignedIn,
    Unsupported,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaErrorCode {
    SourceMissing,
    NotSignedIn,
    UsageUnavailable,
    Unsupported,
    Failed,
    Timeout,
    Malformed,
    CacheUnavailable,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaUnit {
    Percent,
    Credits,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaLevel {
    Ok,
    Warning,
    Exhausted,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaLimit {
    pub id: String,
    pub window: Option<String>,
    pub tier: Option<String>,
    pub unit: QuotaUnit,
    pub used_fraction: Option<f64>,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub unlimited: bool,
    pub level: QuotaLevel,
    pub resets_at_ms: Option<u64>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaAccount {
    /// Source-reported fetch time from the corresponding OMP report.
    pub fetched_at_ms: u64,
    pub limits: Vec<QuotaLimit>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaProviderStatus {
    pub provider: QuotaProvider,
    pub state: QuotaProviderState,
    pub error: Option<QuotaErrorCode>,
    pub fetched_at_ms: Option<u64>,
    pub stale: bool,
    pub accounts: Vec<QuotaAccount>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub generated_at_ms: u64,
    pub collecting: bool,
    pub providers: Vec<QuotaProviderStatus>,
}

const CADENCE: u64 = 300_000;
const WORKING_CADENCE: u64 = 60_000;
const STALE: u64 = 900_000;
const RETAIN: u64 = 86_400_000;
const MAX_CACHE: u64 = 65_536;
const PROVIDERS: [QuotaProvider; 3] = [
    QuotaProvider::Codex,
    QuotaProvider::Claude,
    QuotaProvider::Copilot,
];

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRecord {
    attempted_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    succeeded_at_ms: Option<u64>,
    next_attempt_at_ms: u64,
    failures: u8,
    providers: Vec<ProviderRecord>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderRecord {
    provider: QuotaProvider,
    state: QuotaProviderState,
    error: Option<QuotaErrorCode>,
    accounts: Vec<QuotaAccount>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedQuota {
    schema: u32,
    omp: SourceRecord,
}
impl Default for PersistedQuota {
    fn default() -> Self {
        Self {
            schema: 2,
            omp: SourceRecord::default(),
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

/// Return the shared anonymous snapshot, collecting once when due.
pub async fn snapshot(
    cache_dir: &Path,
    agents_working: bool,
    omp_executable: Option<&Path>,
) -> anyhow::Result<Snapshot> {
    let started = now();
    let mut cache = match cache::Cache::open(cache_dir, started) {
        Ok(cache) => cache,
        Err(()) => {
            let mut value = PersistedQuota::default();
            mark_cache_failure(&mut value, started);
            return Ok(response(&value, started, false));
        }
    };
    let mut value = cache.value.take().unwrap_or_default();
    if cache.lock.is_none() {
        return Ok(response(&value, started, true));
    }
    if due(&value.omp, started, agents_working) {
        value.omp.attempted_at_ms = Some(started);
        value.omp.succeeded_at_ms = None;
        value.omp.next_attempt_at_ms = started.saturating_add(CADENCE);
        if value.omp.providers.is_empty() {
            value.omp.providers = PROVIDERS
                .iter()
                .map(|p| {
                    empty(
                        *p,
                        QuotaProviderState::Unavailable,
                        Some(QuotaErrorCode::UsageUnavailable),
                    )
                })
                .collect();
        }
        if cache.save(&value).is_err() {
            mark_cache_failure(&mut value, now());
            return Ok(response(&value, now(), false));
        }
        let result = source::collect(cache_dir, omp_executable).await;
        apply(&mut value.omp, result, &PROVIDERS, now());
        if cache.save(&value).is_err() {
            mark_cache_failure(&mut value, now());
        }
    }
    Ok(response(&value, now(), false))
}
fn due(record: &SourceRecord, now: u64, agents_working: bool) -> bool {
    record.attempted_at_ms.is_none()
        || now >= record.next_attempt_at_ms
        || record
            .attempted_at_ms
            .is_some_and(|time| time > now.saturating_add(60_000))
        || (agents_working
            && record
                .succeeded_at_ms
                .is_some_and(|time| now >= time.saturating_add(WORKING_CADENCE)))
}
fn apply(
    record: &mut SourceRecord,
    result: Result<Vec<ProviderRecord>, QuotaErrorCode>,
    providers: &[QuotaProvider],
    now: u64,
) {
    record.attempted_at_ms = Some(now);
    match result {
        Ok(mut values) => {
            for value in &mut values {
                if value.state == QuotaProviderState::Unavailable
                    && value.error == Some(QuotaErrorCode::UsageUnavailable)
                    && let Some(previous) = record.providers.iter().find(|previous| {
                        previous.provider == value.provider && !previous.accounts.is_empty()
                    })
                {
                    value.accounts = previous.accounts.clone();
                    value.state = QuotaProviderState::Available;
                }
            }
            record.providers = values;
            record.failures = 0;
            record.succeeded_at_ms = Some(now);
            record.next_attempt_at_ms = now.saturating_add(CADENCE);
        }
        Err(error) => {
            record.succeeded_at_ms = None;
            record.failures = record.failures.saturating_add(1).min(5);
            record.next_attempt_at_ms = now.saturating_add(backoff(record.failures));
            record.providers = providers
                .iter()
                .map(|provider| {
                    let mut value = record
                        .providers
                        .iter()
                        .find(|value| value.provider == *provider)
                        .cloned()
                        .unwrap_or_else(|| {
                            empty(*provider, QuotaProviderState::Unavailable, Some(error))
                        });
                    value.error = Some(error);
                    if value.accounts.is_empty() {
                        value.state = if error == QuotaErrorCode::NotSignedIn {
                            QuotaProviderState::NotSignedIn
                        } else {
                            QuotaProviderState::Unavailable
                        };
                    }
                    value
                })
                .collect();
        }
    }
}
fn backoff(failures: u8) -> u64 {
    (CADENCE << failures.saturating_sub(1)).min(3_600_000)
}
fn mark_cache_failure(snapshot: &mut PersistedQuota, now: u64) {
    apply(
        &mut snapshot.omp,
        Err(QuotaErrorCode::CacheUnavailable),
        &PROVIDERS,
        now,
    );
}
fn empty(
    provider: QuotaProvider,
    state: QuotaProviderState,
    error: Option<QuotaErrorCode>,
) -> ProviderRecord {
    ProviderRecord {
        provider,
        state,
        error,
        accounts: Vec::new(),
    }
}
fn response(snapshot: &PersistedQuota, now: u64, collecting: bool) -> Snapshot {
    let providers = PROVIDERS
        .into_iter()
        .map(|provider| {
            let source = &snapshot.omp;
            let mut value = source
                .providers
                .iter()
                .find(|value| value.provider == provider)
                .cloned()
                .unwrap_or_else(|| {
                    empty(
                        provider,
                        if source.attempted_at_ms.is_none() {
                            QuotaProviderState::Pending
                        } else {
                            QuotaProviderState::Unavailable
                        },
                        source
                            .attempted_at_ms
                            .map(|_| QuotaErrorCode::UsageUnavailable),
                    )
                });
            value.accounts.retain(|account| {
                account.fetched_at_ms <= now.saturating_add(60_000)
                    && now.saturating_sub(account.fetched_at_ms) <= RETAIN
            });
            if value.state == QuotaProviderState::Available && value.accounts.is_empty() {
                value.state = QuotaProviderState::Unavailable;
                value.error = Some(value.error.unwrap_or(QuotaErrorCode::UsageUnavailable));
            }
            let fetched_at_ms = value
                .accounts
                .iter()
                .map(|account| account.fetched_at_ms)
                .min();
            QuotaProviderStatus {
                provider,
                state: value.state,
                error: value.error,
                fetched_at_ms,
                stale: fetched_at_ms
                    .is_some_and(|time| value.error.is_some() || now.saturating_sub(time) > STALE),
                accounts: value.accounts,
            }
        })
        .collect();
    Snapshot {
        schema_version: 1,
        generated_at_ms: now,
        collecting,
        providers,
    }
}
fn valid_snapshot(snapshot: &PersistedQuota, now: u64) -> bool {
    snapshot.schema == 2 && valid_source(&snapshot.omp, &PROVIDERS, now)
}
fn valid_source(source: &SourceRecord, providers: &[QuotaProvider], now: u64) -> bool {
    if source.succeeded_at_ms.is_some()
        && (source.failures != 0 || source.succeeded_at_ms != source.attempted_at_ms)
    {
        return false;
    }
    if source.failures > 5 || source.next_attempt_at_ms > now.saturating_add(3_600_000 + 60_000) {
        return false;
    }
    if source.attempted_at_ms.is_none() {
        return source.providers.is_empty()
            && source.next_attempt_at_ms == 0
            && source.failures == 0;
    }
    source.providers.len() == providers.len()
        && source
            .providers
            .iter()
            .zip(providers)
            .all(|(value, provider)| {
                value.provider == *provider
                    && value.state != QuotaProviderState::Pending
                    && value.accounts.len() <= 8
                    && ((value.state == QuotaProviderState::Available)
                        == !value.accounts.is_empty())
                    && (value.state != QuotaProviderState::Unavailable || value.error.is_some())
                    && value.accounts.iter().all(|account| {
                        account.fetched_at_ms <= now.saturating_add(60_000)
                            && !account.limits.is_empty()
                            && account.limits.len() <= 24
                            && account
                                .limits
                                .iter()
                                .enumerate()
                                .all(|(index, limit)| parse::valid_limit(limit, *provider, index))
                    })
            })
}
