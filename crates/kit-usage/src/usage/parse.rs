use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Omp {
    reports: Vec<Report>,
    #[serde(default)]
    accounts_without_usage: Vec<WithoutUsage>,
}
#[derive(Deserialize)]
struct WithoutUsage {
    provider: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    provider: String,
    fetched_at: u64,
    limits: Vec<Limit>,
}
#[derive(Deserialize)]
struct Limit {
    id: Option<String>,
    #[serde(default)]
    notes: Vec<String>,
    #[serde(default)]
    scope: Scope,
    window: Option<Window>,
    amount: Amount,
    status: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Scope {
    tier: Option<String>,
    model_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Window {
    id: Option<String>,
    duration_ms: Option<u64>,
    resets_at: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Amount {
    unit: String,
    used: Option<f64>,
    limit: Option<f64>,
    remaining: Option<f64>,
    used_fraction: Option<f64>,
    remaining_fraction: Option<f64>,
}

pub(super) fn omp(bytes: &[u8], now: u64) -> Result<Vec<ProviderRecord>, QuotaErrorCode> {
    let input: Omp = serde_json::from_slice(bytes).map_err(|_| QuotaErrorCode::Malformed)?;
    let mut output = Vec::with_capacity(3);
    for (provider, source) in [
        (QuotaProvider::Codex, "openai-codex"),
        (QuotaProvider::Claude, "anthropic"),
        (QuotaProvider::Copilot, "github-copilot"),
    ] {
        let mut accounts = Vec::new();
        let mut reported = false;
        let mut retained_report = false;
        for report in input
            .reports
            .iter()
            .filter(|report| report.provider == source)
            .take(8)
        {
            reported = true;
            if report.fetched_at > now.saturating_add(60_000) {
                return Err(QuotaErrorCode::Malformed);
            }
            if now.saturating_sub(report.fetched_at) > RETAIN {
                continue;
            }
            retained_report = true;
            let mut limits = Vec::new();
            for input in &report.limits {
                let credits = provider == QuotaProvider::Copilot;
                let supported = if credits {
                    input.id.as_deref() == Some("copilot:premium")
                        && matches!(input.amount.unit.as_str(), "requests" | "credits")
                } else {
                    input.amount.unit == "percent"
                };
                if !supported {
                    continue;
                }
                if limits.len() == 24 {
                    break;
                }
                limits.push(normalize_omp_limit(provider, credits, input, limits.len())?);
            }
            if !limits.is_empty() {
                accounts.push(QuotaAccount {
                    fetched_at_ms: report.fetched_at,
                    limits,
                });
            }
        }
        output.push(if !accounts.is_empty() {
            ProviderRecord {
                provider,
                state: QuotaProviderState::Available,
                error: None,
                accounts,
            }
        } else if retained_report {
            empty(
                provider,
                QuotaProviderState::Unsupported,
                Some(QuotaErrorCode::Unsupported),
            )
        } else if reported
            || input
                .accounts_without_usage
                .iter()
                .any(|account| account.provider == source)
        {
            empty(
                provider,
                QuotaProviderState::Unavailable,
                Some(QuotaErrorCode::UsageUnavailable),
            )
        } else {
            empty(
                provider,
                QuotaProviderState::NotSignedIn,
                Some(QuotaErrorCode::NotSignedIn),
            )
        });
    }
    Ok(output)
}

fn normalize_omp_limit(
    provider: QuotaProvider,
    credits: bool,
    input: &Limit,
    slot: usize,
) -> Result<QuotaLimit, QuotaErrorCode> {
    let amount = &input.amount;
    if credits
        && [amount.used, amount.limit, amount.remaining]
            .into_iter()
            .any(|value| value.is_some_and(|value| !number(value, 1e9)))
    {
        return Err(QuotaErrorCode::Malformed);
    }
    let unlimited = credits
        && amount.used.is_none()
        && amount.limit.is_none()
        && amount.remaining.is_none()
        && input.notes.iter().any(|note| note == "Unlimited");
    let fraction = if unlimited {
        None
    } else {
        amount
            .used_fraction
            .or_else(|| {
                amount
                    .used
                    .zip(amount.limit)
                    .filter(|(_, total)| *total > 0.0)
                    .map(|(used, total)| used / total)
            })
            .or_else(|| {
                if credits {
                    None
                } else {
                    amount.used.map(|used| used / 100.0)
                }
            })
            .or_else(|| amount.remaining_fraction.map(|remaining| 1.0 - remaining))
    };
    if fraction.is_some_and(|value| !number(value, 10.0)) {
        return Err(QuotaErrorCode::Malformed);
    }
    let window = input
        .window
        .as_ref()
        .and_then(|window| normalize_window(window.id.as_deref(), window.duration_ms));
    let tier = if credits {
        None
    } else {
        input
            .scope
            .tier
            .as_deref()
            .filter(|tier| known_tier(tier))
            .or_else(|| {
                input
                    .scope
                    .model_id
                    .as_deref()
                    .and_then(|model| model.split('-').find(|part| known_tier(part)))
            })
            .map(str::to_owned)
    };
    let level = match input.status.as_deref() {
        Some("ok") => QuotaLevel::Ok,
        Some("warning") => QuotaLevel::Warning,
        Some("exhausted") => QuotaLevel::Exhausted,
        Some("unknown") => QuotaLevel::Unknown,
        _ => level(fraction),
    };
    Ok(QuotaLimit {
        id: id(provider, window.as_deref(), slot),
        window,
        tier,
        // OMP Copilot counters are surfaced as credits without rescaling,
        // including reports carrying legacy source unit labels.
        unit: if credits {
            QuotaUnit::Credits
        } else {
            QuotaUnit::Percent
        },
        used_fraction: fraction,
        used: credits.then_some(amount.used).flatten(),
        limit: credits.then_some(amount.limit).flatten(),
        remaining: credits.then_some(amount.remaining).flatten(),
        unlimited,
        level,
        resets_at_ms: input
            .window
            .as_ref()
            .and_then(|window| window.resets_at)
            .filter(|time| *time <= 9_007_199_254_740_991),
    })
}

#[derive(Deserialize)]
struct AgyOutput {
    status: Option<String>,
    response: Option<String>,
    error: Option<serde_json::Value>,
    command: Option<AgyCommand>,
}
#[derive(Deserialize)]
struct AgyCommand {
    name: Option<String>,
    data: Option<AgyUsageData>,
}
#[derive(Deserialize)]
struct AgyUsageData {
    #[serde(default)]
    groups: Vec<AgyGroup>,
}
#[derive(Deserialize)]
struct AgyGroup {
    name: Option<String>,
    #[serde(default)]
    buckets: Vec<AgyBucket>,
}
#[derive(Deserialize)]
struct AgyBucket {
    id: Option<String>,
    window: Option<String>,
    remaining_fraction: Option<f64>,
    used_fraction: Option<f64>,
    reset_time: Option<String>,
}

pub(super) fn looks_not_signed_in(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    looks_not_signed_in_str(&text)
}

fn looks_not_signed_in_str(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "not signed in",
        "not logged in",
        "unauthenticated",
        "login required",
        "please log in",
        "please sign in",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(super) fn antigravity(bytes: &[u8], now: u64) -> Result<Vec<ProviderRecord>, QuotaErrorCode> {
    let input: AgyOutput = serde_json::from_slice(bytes).map_err(|_| QuotaErrorCode::Malformed)?;
    let auth_error = input
        .response
        .as_deref()
        .is_some_and(looks_not_signed_in_str)
        || input
            .error
            .as_ref()
            .is_some_and(|value| looks_not_signed_in_str(&value.to_string()));
    if input.status.as_deref() != Some("SUCCESS") {
        if auth_error {
            return Ok(vec![empty(
                QuotaProvider::Antigravity,
                QuotaProviderState::NotSignedIn,
                Some(QuotaErrorCode::NotSignedIn),
            )]);
        }
        return Err(QuotaErrorCode::Failed);
    }
    let Some(command) = input.command else {
        return Err(QuotaErrorCode::Malformed);
    };
    if command.name.as_deref() != Some("usage") {
        return Err(QuotaErrorCode::Malformed);
    }
    let Some(data) = command.data else {
        return Err(QuotaErrorCode::Malformed);
    };
    let mut limits = Vec::new();
    for group in &data.groups {
        for bucket in &group.buckets {
            if limits.len() == 24 {
                break;
            }
            let fraction = if let Some(used) = bucket.used_fraction {
                if !number(used, 10.0) {
                    return Err(QuotaErrorCode::Malformed);
                }
                Some(used)
            } else if let Some(remaining) = bucket.remaining_fraction {
                if !remaining.is_finite() || !(-0.001..=1.001).contains(&remaining) {
                    return Err(QuotaErrorCode::Malformed);
                }
                Some((1.0 - remaining).clamp(0.0, 1.0))
            } else {
                None
            };
            let window = match bucket.window.as_deref() {
                Some("daily") => Some("1d".into()),
                Some("hourly") => Some("1h".into()),
                other => normalize_window(other, None),
            };
            let tier = antigravity_tier(bucket.id.as_deref(), group.name.as_deref());
            let resets_at_ms = bucket.reset_time.as_deref().and_then(parse_rfc3339_ms);
            limits.push(QuotaLimit {
                id: id(QuotaProvider::Antigravity, window.as_deref(), limits.len()),
                window,
                tier,
                unit: QuotaUnit::Percent,
                used_fraction: fraction,
                used: None,
                limit: None,
                remaining: None,
                unlimited: false,
                level: level(fraction),
                resets_at_ms,
            });
        }
        if limits.len() == 24 {
            break;
        }
    }
    Ok(vec![if limits.is_empty() {
        empty(
            QuotaProvider::Antigravity,
            QuotaProviderState::Unsupported,
            Some(QuotaErrorCode::Unsupported),
        )
    } else {
        ProviderRecord {
            provider: QuotaProvider::Antigravity,
            state: QuotaProviderState::Available,
            error: None,
            accounts: vec![QuotaAccount {
                fetched_at_ms: now,
                limits,
            }],
        }
    }])
}

fn antigravity_tier(bucket_id: Option<&str>, group_name: Option<&str>) -> Option<String> {
    let id = bucket_id.unwrap_or("").to_ascii_lowercase();
    let group = group_name.unwrap_or("").to_ascii_lowercase();
    if id.starts_with("gemini-argon") || id.contains("argon") || group.contains("argon") {
        Some("gemini_argon".into())
    } else if id.starts_with("gemini-pro") || (group.contains("gemini") && group.contains("pro")) {
        Some("gemini_pro".into())
    } else if id.starts_with("gemini-flash")
        || (group.contains("gemini") && group.contains("flash"))
    {
        Some("gemini_flash".into())
    } else if id.starts_with("gemini") || group.contains("gemini") {
        Some("gemini".into())
    } else if id.starts_with("3p") || group.contains("claude") || group.contains("gpt") {
        Some("claude_gpt".into())
    } else {
        None
    }
}

pub(super) fn parse_rfc3339_ms(input: &str) -> Option<u64> {
    let bytes = input.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = parse_digits(&bytes[0..4])? as i64;
    let month = parse_digits(&bytes[5..7])? as u32;
    let day = parse_digits(&bytes[8..10])? as u32;
    let hour = parse_digits(&bytes[11..13])? as i64;
    let minute = parse_digits(&bytes[14..16])? as i64;
    let second = parse_digits(&bytes[17..19])? as i64;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let second = second.min(59);
    let mut rest = &bytes[19..];
    let mut ms = 0_u64;
    if rest.first() == Some(&b'.') {
        rest = &rest[1..];
        let frac_len = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=9).contains(&frac_len) {
            return None;
        }
        for i in 0..3 {
            let digit = if i < frac_len {
                u64::from(rest[i] - b'0')
            } else {
                0
            };
            ms = ms * 10 + digit;
        }
        rest = &rest[frac_len..];
    }
    let offset_secs: i64 = if rest == b"Z" || rest == b"z" {
        0
    } else if rest.len() == 6 && matches!(rest[0], b'+' | b'-') && rest[3] == b':' {
        let off_h = parse_digits(&rest[1..3])? as i64;
        let off_m = parse_digits(&rest[4..6])? as i64;
        if off_h > 23 || off_m > 59 {
            return None;
        }
        let abs = off_h * 3600 + off_m * 60;
        if rest[0] == b'+' { abs } else { -abs }
    } else {
        return None;
    };
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let epoch_secs = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_secs;
    if epoch_secs < 0 {
        return None;
    }
    let total_ms = (epoch_secs as u64).checked_mul(1000)?.checked_add(ms)?;
    (total_ms <= 9_007_199_254_740_991).then_some(total_ms)
}

fn parse_digits(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value = 0_u64;
    for &b in bytes {
        value = value.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
    }
    Some(value)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn number(value: f64, max: f64) -> bool {
    value.is_finite() && (0.0..=max).contains(&value)
}
fn level(fraction: Option<f64>) -> QuotaLevel {
    match fraction {
        Some(value) if value >= 1.0 => QuotaLevel::Exhausted,
        Some(value) if value >= 0.8 => QuotaLevel::Warning,
        Some(_) => QuotaLevel::Ok,
        None => QuotaLevel::Unknown,
    }
}
fn known_tier(tier: &str) -> bool {
    matches!(
        tier,
        "opus"
            | "sonnet"
            | "haiku"
            | "fable"
            | "gemini"
            | "gemini_argon"
            | "gemini_pro"
            | "gemini_flash"
            | "claude_gpt"
    )
}
fn valid_window(window: &str) -> bool {
    if matches!(window, "monthly" | "weekly") {
        return true;
    }
    let bytes = window.as_bytes();
    (2..=5).contains(&bytes.len())
        && matches!(bytes[bytes.len() - 1], b's' | b'm' | b'h' | b'd' | b'w')
        && bytes[..bytes.len() - 1].iter().all(u8::is_ascii_digit)
}
fn normalize_window(window: Option<&str>, duration: Option<u64>) -> Option<String> {
    if let Some(window) = window.filter(|window| valid_window(window)) {
        return Some(window.into());
    }
    let duration = duration.filter(|duration| *duration > 0)?;
    for (unit, millis) in [
        ("w", 604_800_000),
        ("d", 86_400_000),
        ("h", 3_600_000),
        ("m", 60_000),
        ("s", 1_000),
    ] {
        if duration % millis == 0 && duration / millis <= 9999 {
            return Some(format!("{}{unit}", duration / millis));
        }
    }
    None
}
fn id(provider: QuotaProvider, window: Option<&str>, index: usize) -> String {
    let provider = match provider {
        QuotaProvider::Codex => "codex",
        QuotaProvider::Claude => "claude",
        QuotaProvider::Copilot => "copilot",
        QuotaProvider::Antigravity => "antigravity",
    };
    format!("{provider}:{}:{index}", window.unwrap_or("none"))
}
pub(super) fn valid_limit(limit: &QuotaLimit, provider: QuotaProvider, index: usize) -> bool {
    let credits = provider == QuotaProvider::Copilot;
    limit.id == id(provider, limit.window.as_deref(), index)
        && limit.window.as_deref().is_none_or(valid_window)
        && limit.tier.as_deref().is_none_or(known_tier)
        && (limit.unit
            == if credits {
                QuotaUnit::Credits
            } else {
                QuotaUnit::Percent
            })
        && limit.used_fraction.is_none_or(|value| number(value, 10.0))
        && [limit.used, limit.limit, limit.remaining]
            .into_iter()
            .all(|value| value.is_none_or(|value| credits && number(value, 1e9)))
        && (!limit.unlimited
            || (credits
                && limit.used_fraction.is_none()
                && limit.used.is_none()
                && limit.limit.is_none()
                && limit.remaining.is_none()))
        && limit
            .resets_at_ms
            .is_none_or(|time| time <= 9_007_199_254_740_991)
}
