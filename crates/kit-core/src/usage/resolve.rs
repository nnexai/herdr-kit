//! Role-to-subscription usage resolution against a `kit_core::usage::Snapshot`.
use super::{QuotaLevel, QuotaLimit, QuotaProvider, QuotaProviderState, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_OMP_CONFIG: u64 = 262_144;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleUsageStatus {
    Ok,
    Warning,
    Exhausted,
    NotSignedIn,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoleLimitSummary {
    pub window: Option<String>,
    pub tier: Option<String>,
    pub used_fraction: Option<f64>,
    pub level: QuotaLevel,
    pub resets_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoleUsage {
    pub provider: Option<QuotaProvider>,
    pub tier: Option<String>,
    pub status: RoleUsageStatus,
    pub stale: bool,
    pub max_used_fraction: Option<f64>,
    pub blocking_window: Option<String>,
    pub resets_at_ms: Option<u64>,
    pub resets_in_s: Option<u64>,
    pub limits: Vec<RoleLimitSummary>,
}

impl RoleUsage {
    fn unknown(provider: Option<QuotaProvider>, tier: Option<String>, stale: bool) -> Self {
        Self {
            provider,
            tier,
            status: RoleUsageStatus::Unknown,
            stale,
            max_used_fraction: None,
            blocking_window: None,
            resets_at_ms: None,
            resets_in_s: None,
            limits: Vec::new(),
        }
    }
}

/// Default path to OMP's `config.yml`, honoring `KIT_COORD_OMP_CONFIG` and `PI_CODING_AGENT_DIR`.
pub fn default_omp_config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("KIT_COORD_OMP_CONFIG").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(path));
    }
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir).join("config.yml"));
    }
    crate::paths::home()
        .ok()
        .map(|h| h.join(".omp/agent/config.yml"))
}

/// Read the `modelRoles:` mapping from an OMP `config.yml` without external YAML dependencies.
pub fn read_omp_model_roles(path: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Ok(file) = File::open(path) else {
        return map;
    };
    let mut text = String::new();
    if file.take(MAX_OMP_CONFIG).read_to_string(&mut text).is_err() {
        return map;
    }
    let mut in_roles = false;
    let mut indent_level: Option<usize> = None;
    for raw_line in text.lines() {
        let trimmed_end = raw_line.trim_end();
        let trimmed = trimmed_end.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = trimmed_end.len() - trimmed.len();
        if !in_roles {
            if indent == 0 && (trimmed == "modelRoles:" || trimmed.starts_with("modelRoles: #")) {
                in_roles = true;
            }
            continue;
        }
        if indent == 0 {
            break;
        }
        let expected = *indent_level.get_or_insert(indent);
        if indent != expected {
            continue;
        }
        let Some((key, rest)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let mut val = rest.trim();
        if let Some((before, _)) = val.split_once(" #") {
            val = before.trim();
        }
        val = val
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| val.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(val)
            .trim();
        if !key.is_empty() && !val.is_empty() {
            map.insert(key.to_owned(), val.to_owned());
        }
    }
    map
}

/// Resolve a role's configured model selector (e.g., OMP `@plan`, `@task`, `@advisor`)
/// to its underlying provider/model identifier.
pub fn resolve_model(
    harness: &str,
    model: Option<&str>,
    omp_config: Option<&Path>,
) -> Option<String> {
    let model = model?.trim();
    if model.is_empty() {
        return None;
    }
    if harness == "omp" && model.starts_with('@') {
        let roles = omp_config.map(read_omp_model_roles).unwrap_or_default();
        let mut current = model.to_owned();
        for _ in 0..4 {
            let Some(selector) = current.strip_prefix('@') else {
                return Some(current);
            };
            let next = roles.get(selector)?;
            current = next.clone();
        }
        return (!current.starts_with('@')).then_some(current);
    }
    Some(model.to_owned())
}

fn claude_tier(model_lower: &str) -> Option<&'static str> {
    if model_lower.contains("opus") {
        Some("opus")
    } else if model_lower.contains("sonnet") {
        Some("sonnet")
    } else if model_lower.contains("haiku") {
        Some("haiku")
    } else if model_lower.contains("fable") {
        Some("fable")
    } else {
        None
    }
}

fn agy_tier(model_lower: &str) -> Option<&'static str> {
    if model_lower.contains("argon") {
        Some("gemini_argon")
    } else if model_lower.contains("gemini-pro") {
        Some("gemini_pro")
    } else if model_lower.contains("flash") {
        Some("gemini_flash")
    } else if model_lower.contains("gemini") {
        Some("gemini")
    } else if model_lower.contains("claude") || model_lower.contains("gpt") {
        Some("claude_gpt")
    } else {
        None
    }
}

/// Map a harness and resolved model to its `QuotaProvider` and optional quota tier.
pub fn provider_for(
    harness: &str,
    resolved_model: Option<&str>,
) -> Option<(QuotaProvider, Option<&'static str>)> {
    let base_lower = resolved_model
        .map(|m| m.split(':').next().unwrap_or(m).trim().to_ascii_lowercase())
        .unwrap_or_default();
    match harness {
        "agy" => {
            let tier = if base_lower.is_empty() {
                None
            } else {
                agy_tier(&base_lower)
            };
            Some((QuotaProvider::Antigravity, tier))
        }
        "claude" => Some((QuotaProvider::Claude, claude_tier(&base_lower))),
        "codex" => Some((QuotaProvider::Codex, None)),
        "copilot" => Some((QuotaProvider::Copilot, None)),
        "omp" => {
            if base_lower.is_empty() {
                return None;
            }
            if base_lower.starts_with("openai-codex/") {
                Some((QuotaProvider::Codex, None))
            } else if let Some(rest) = base_lower.strip_prefix("anthropic/") {
                Some((QuotaProvider::Claude, claude_tier(rest)))
            } else if base_lower.starts_with("github-copilot/") {
                Some((QuotaProvider::Copilot, None))
            } else if let Some(rest) = base_lower.strip_prefix("google-antigravity/") {
                Some((QuotaProvider::Antigravity, agy_tier(rest)))
            } else if base_lower.contains("claude")
                || matches!(base_lower.as_str(), "opus" | "sonnet" | "haiku")
            {
                Some((QuotaProvider::Claude, claude_tier(&base_lower)))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn limit_is_exhausted(limit: &QuotaLimit) -> bool {
    !limit.unlimited
        && (limit.level == QuotaLevel::Exhausted || limit.used_fraction.is_some_and(|f| f >= 1.0))
}

fn limit_is_warning(limit: &QuotaLimit) -> bool {
    !limit.unlimited
        && (limit.level == QuotaLevel::Warning || limit.used_fraction.is_some_and(|f| f >= 0.8))
}

/// Evaluate subscription headroom for a role against a usage snapshot.
pub fn evaluate_role(
    snapshot: &Snapshot,
    harness: &str,
    configured_model: Option<&str>,
    omp_config: Option<&Path>,
    now_ms: u64,
) -> (Option<String>, RoleUsage) {
    let resolved = resolve_model(harness, configured_model, omp_config);
    let Some((provider, tier)) = provider_for(harness, resolved.as_deref()) else {
        return (resolved, RoleUsage::unknown(None, None, false));
    };
    let tier_str = tier.map(str::to_owned);
    let Some(ps) = snapshot.providers.iter().find(|p| p.provider == provider) else {
        return (resolved, RoleUsage::unknown(Some(provider), tier_str, false));
    };
    match ps.state {
        QuotaProviderState::NotSignedIn => {
            return (
                resolved,
                RoleUsage {
                    provider: Some(provider),
                    tier: tier_str,
                    status: RoleUsageStatus::NotSignedIn,
                    stale: ps.stale,
                    max_used_fraction: None,
                    blocking_window: None,
                    resets_at_ms: None,
                    resets_in_s: None,
                    limits: Vec::new(),
                },
            );
        }
        QuotaProviderState::Available => {}
        QuotaProviderState::Pending
        | QuotaProviderState::Unsupported
        | QuotaProviderState::Unavailable => {
            return (
                resolved,
                RoleUsage::unknown(Some(provider), tier_str, ps.stale),
            );
        }
    }

    let matching: Vec<&QuotaLimit> = ps
        .accounts
        .iter()
        .flat_map(|a| a.limits.iter())
        .filter(|limit| limit.tier.is_none() || limit.tier.as_deref() == tier)
        .collect();

    let mut stale = ps.stale;
    let mut active: Vec<&QuotaLimit> = Vec::new();
    for limit in matching {
        let reset_passed = limit.resets_at_ms.is_some_and(|r| r <= now_ms);
        if reset_passed && limit_is_exhausted(limit) {
            // A cached exhausted window whose reset timestamp is already in the past
            // should not wedge launches permanently; treat as stale/reset.
            stale = true;
            continue;
        }
        active.push(limit);
    }

    let summaries: Vec<RoleLimitSummary> = active
        .iter()
        .map(|l| RoleLimitSummary {
            window: l.window.clone(),
            tier: l.tier.clone(),
            used_fraction: l.used_fraction,
            level: l.level,
            resets_at_ms: l.resets_at_ms,
        })
        .collect();

    let max_used_fraction = active
        .iter()
        .filter_map(|l| l.used_fraction)
        .fold(None, |acc: Option<f64>, v| {
            Some(acc.map_or(v, |prev| prev.max(v)))
        });

    let exhausted: Vec<&QuotaLimit> = active
        .iter()
        .copied()
        .filter(|l| limit_is_exhausted(l))
        .collect();
    if !exhausted.is_empty() {
        // Every exhausted window must reset before the role is usable again.
        let latest = exhausted
            .iter()
            .copied()
            .max_by_key(|l| l.resets_at_ms.unwrap_or(0))
            .unwrap();
        let resets_at_ms = latest.resets_at_ms;
        return (
            resolved,
            RoleUsage {
                provider: Some(provider),
                tier: tier_str,
                status: RoleUsageStatus::Exhausted,
                stale,
                max_used_fraction,
                blocking_window: latest.window.clone(),
                resets_at_ms,
                resets_in_s: resets_at_ms.map(|r| r.saturating_sub(now_ms) / 1000),
                limits: summaries,
            },
        );
    }

    let warning: Vec<&QuotaLimit> = active
        .iter()
        .copied()
        .filter(|l| limit_is_warning(l))
        .collect();
    if !warning.is_empty() {
        let top = warning
            .iter()
            .copied()
            .max_by(|a, b| {
                a.used_fraction
                    .unwrap_or(0.0)
                    .total_cmp(&b.used_fraction.unwrap_or(0.0))
            })
            .unwrap();
        let resets_at_ms = warning
            .iter()
            .filter_map(|l| l.resets_at_ms)
            .min()
            .or(top.resets_at_ms);
        return (
            resolved,
            RoleUsage {
                provider: Some(provider),
                tier: tier_str,
                status: RoleUsageStatus::Warning,
                stale,
                max_used_fraction,
                blocking_window: top.window.clone(),
                resets_at_ms,
                resets_in_s: resets_at_ms.map(|r| r.saturating_sub(now_ms) / 1000),
                limits: summaries,
            },
        );
    }

    let has_known = active
        .iter()
        .any(|l| l.unlimited || l.level == QuotaLevel::Ok || l.used_fraction.is_some());
    if has_known {
        let resets_at_ms = active.iter().filter_map(|l| l.resets_at_ms).min();
        return (
            resolved,
            RoleUsage {
                provider: Some(provider),
                tier: tier_str,
                status: RoleUsageStatus::Ok,
                stale,
                max_used_fraction,
                blocking_window: None,
                resets_at_ms,
                resets_in_s: resets_at_ms.map(|r| r.saturating_sub(now_ms) / 1000),
                limits: summaries,
            },
        );
    }

    (
        resolved,
        RoleUsage {
            provider: Some(provider),
            tier: tier_str,
            status: RoleUsageStatus::Unknown,
            stale,
            max_used_fraction,
            blocking_window: None,
            resets_at_ms: None,
            resets_in_s: None,
            limits: summaries,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{QuotaAccount, QuotaProviderStatus, QuotaUnit};

    fn limit(
        id: &str,
        window: &str,
        tier: Option<&str>,
        used: f64,
        level: QuotaLevel,
        resets_at_ms: Option<u64>,
    ) -> QuotaLimit {
        QuotaLimit {
            id: id.into(),
            window: Some(window.into()),
            tier: tier.map(str::to_owned),
            unit: QuotaUnit::Percent,
            used_fraction: Some(used),
            used: None,
            limit: None,
            remaining: None,
            unlimited: false,
            level,
            resets_at_ms,
        }
    }

    fn snapshot_with(providers: Vec<QuotaProviderStatus>, now: u64) -> Snapshot {
        Snapshot {
            schema_version: 1,
            generated_at_ms: now,
            collecting: false,
            providers,
        }
    }

    #[test]
    fn parses_omp_model_roles_and_resolves_selectors() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.yml");
        std::fs::write(
            &cfg,
            "modelRoles:\n  plan: anthropic/claude-opus-5-5:high # comment\n  task: \"openai-codex/gpt-6.1-terra:medium\"\n  advisor: '@slow'\n  slow: openai-codex/gpt-6.2-sol:medium\ntheme:\n  dark: titanium\n",
        )
        .unwrap();
        assert_eq!(
            resolve_model("omp", Some("@plan"), Some(&cfg)).as_deref(),
            Some("anthropic/claude-opus-5-5:high")
        );
        assert_eq!(
            resolve_model("omp", Some("@task"), Some(&cfg)).as_deref(),
            Some("openai-codex/gpt-6.1-terra:medium")
        );
        assert_eq!(
            resolve_model("omp", Some("@advisor"), Some(&cfg)).as_deref(),
            Some("openai-codex/gpt-6.2-sol:medium")
        );
        assert_eq!(resolve_model("omp", Some("@missing"), Some(&cfg)), None);
        assert_eq!(
            resolve_model("agy", Some("gemini-4-neon-high"), Some(&cfg)).as_deref(),
            Some("gemini-4-neon-high")
        );
    }

    #[test]
    fn evaluates_exhausted_warning_tier_filtering_and_past_reset() {
        let now = 1_000_000_u64;
        let snap = snapshot_with(
            vec![
                QuotaProviderStatus {
                    provider: QuotaProvider::Claude,
                    state: QuotaProviderState::Available,
                    error: None,
                    fetched_at_ms: Some(now),
                    stale: false,
                    accounts: vec![QuotaAccount {
                        fetched_at_ms: now,
                        limits: vec![
                            limit(
                                "claude:5h:0",
                                "5h",
                                None,
                                1.0,
                                QuotaLevel::Exhausted,
                                Some(now + 600_000),
                            ),
                            limit(
                                "claude:7d:1",
                                "7d",
                                None,
                                1.0,
                                QuotaLevel::Exhausted,
                                Some(now + 86_400_000),
                            ),
                        ],
                    }],
                },
                QuotaProviderStatus {
                    provider: QuotaProvider::Antigravity,
                    state: QuotaProviderState::Available,
                    error: None,
                    fetched_at_ms: Some(now),
                    stale: false,
                    accounts: vec![QuotaAccount {
                        fetched_at_ms: now,
                        limits: vec![
                            limit(
                                "antigravity:5h:0",
                                "5h",
                                Some("gemini"),
                                0.4,
                                QuotaLevel::Ok,
                                Some(now + 300_000),
                            ),
                            limit(
                                "antigravity:5h:1",
                                "5h",
                                Some("claude_gpt"),
                                1.0,
                                QuotaLevel::Exhausted,
                                Some(now + 300_000),
                            ),
                        ],
                    }],
                },
                QuotaProviderStatus {
                    provider: QuotaProvider::Codex,
                    state: QuotaProviderState::Available,
                    error: None,
                    fetched_at_ms: Some(now),
                    stale: false,
                    accounts: vec![QuotaAccount {
                        fetched_at_ms: now,
                        limits: vec![limit(
                            "codex:7d:0",
                            "7d",
                            None,
                            1.0,
                            QuotaLevel::Exhausted,
                            Some(now - 1_000),
                        )],
                    }],
                },
            ],
            now,
        );

        // Both 5h and 7d exhausted -> latest reset wins (7d).
        let (_, claude) =
            evaluate_role(&snap, "omp", Some("anthropic/claude-opus-5-5:high"), None, now);
        assert_eq!(claude.status, RoleUsageStatus::Exhausted);
        assert_eq!(claude.blocking_window.as_deref(), Some("7d"));
        assert_eq!(claude.resets_in_s, Some(86_400));

        // Antigravity gemini tier is ok even when claude_gpt tier is exhausted.
        let (_, neon) = evaluate_role(&snap, "agy", Some("gemini-4-neon-high"), None, now);
        assert_eq!(neon.status, RoleUsageStatus::Ok);
        assert_eq!(neon.tier.as_deref(), Some("gemini"));

        // Past-reset exhausted limit does not falsely block launches.
        let (_, codex) = evaluate_role(&snap, "omp", Some("openai-codex/gpt-6-luna:medium"), None, now);
        assert_eq!(codex.status, RoleUsageStatus::Unknown);
        assert!(codex.stale);
    }
}
