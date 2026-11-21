use crate::model::{
    AgentStateKind, BucketSummary, DeckSnapshot, InboxKind, ProviderSummary, format_percent,
    format_reset_short,
};

const BG_TOP: &str = "#0b1020";
const BG_BOT: &str = "#111829";
const PILL_BG: &str = "#172033";
const TRACK: &str = "#1e293b";
const ICE: &str = "#e5eefc";
const MUTED: &str = "#8b9bb4";
const CYAN: &str = "#22d3ee";
const MINT: &str = "#00f5a0";
const AMBER: &str = "#ffb300";
const CORAL: &str = "#ff4b5c";
const VIOLET: &str = "#a78bfa";

pub fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn svg_to_data_uri(svg: &str) -> String {
    format!(
        "data:image/svg+xml;base64,{}",
        base64_encode(svg.as_bytes())
    )
}

fn clip_chars(input: &str, max: usize) -> String {
    let trimmed = input.trim();
    let count = trimmed.chars().count();
    if count <= max {
        return trimmed.to_owned();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = trimmed.chars().take(keep).collect();
    out.push('…');
    out
}

fn wrap_lines(input: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let words: Vec<&str> = input.split_whitespace().collect();
    if words.is_empty() || max_lines == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut idx = 0;
    while idx < words.len() {
        let word = words[idx];
        let next_len = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
        if next_len <= max_chars {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            idx += 1;
        } else if current.is_empty() {
            current = clip_chars(word, max_chars);
            idx += 1;
        } else {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                break;
            }
        }
    }
    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current);
    } else if idx < words.len() && !lines.is_empty() {
        if let Some(last) = lines.last_mut() {
            *last = clip_chars(last, max_chars);
            if !last.ends_with('…') {
                *last = clip_chars(&format!("{last}…"), max_chars);
            }
        }
    }
    lines
}

fn usage_color(fraction: Option<f64>, stale: bool, available: bool) -> &'static str {
    if !available {
        return MUTED;
    }
    if stale {
        return AMBER;
    }
    match fraction {
        Some(f) if f >= 0.95 => CORAL,
        Some(f) if f >= 0.80 => AMBER,
        Some(f) if f <= 0.01 => MINT,
        Some(_) => CYAN,
        None => VIOLET,
    }
}

fn svg_card(accent: &str, highlight: bool, body: &str) -> String {
    let border_w = if highlight { "3.5" } else { "2.2" };
    let border_op = if highlight { "0.95" } else { "0.45" };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 144 144" width="144" height="144">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="{BG_TOP}"/>
      <stop offset="100%" stop-color="{BG_BOT}"/>
    </linearGradient>
    <radialGradient id="glow" cx="50%" cy="10%" r="70%">
      <stop offset="0%" stop-color="{accent}" stop-opacity="0.22"/>
      <stop offset="100%" stop-color="{accent}" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect width="144" height="144" rx="20" fill="url(#bg)"/>
  <rect width="144" height="144" rx="20" fill="url(#glow)"/>
  <rect x="3" y="3" width="138" height="138" rx="18" fill="none" stroke="{accent}" stroke-width="{border_w}" stroke-opacity="{border_op}"/>
  {body}
</svg>"#
    )
}

fn arc_point(cx: f64, cy: f64, r: f64, deg: f64) -> (f64, f64) {
    let rad = deg.to_radians();
    (cx + r * rad.cos(), cy + r * rad.sin())
}

fn arc_path(cx: f64, cy: f64, r: f64, start_deg: f64, end_deg: f64) -> String {
    let (x0, y0) = arc_point(cx, cy, r, start_deg);
    let (x1, y1) = arc_point(cx, cy, r, end_deg);
    let large_arc = if (end_deg - start_deg) > 180.0 { 1 } else { 0 };
    format!("M {x0:.1} {y0:.1} A {r:.1} {r:.1} 0 {large_arc} 1 {x1:.1} {y1:.1}")
}

fn svg_radial_gauge(
    accent: &str,
    fraction: Option<f64>,
    center_text: &str,
    label: &str,
    sublabel: &str,
    badge: Option<&str>,
    selected: bool,
) -> String {
    let cx = 72.0;
    let cy = 58.0;
    let r = 42.0;
    let track = arc_path(cx, cy, r, 150.0, 390.0);
    let fill_arc = match fraction {
        Some(f) if f > 0.005 => {
            let sweep = 240.0 * f.clamp(0.0, 1.0);
            let end = 150.0 + sweep;
            let path = arc_path(cx, cy, r, 150.0, end);
            let (dx, dy) = arc_point(cx, cy, r, end);
            format!(
                r#"<path d="{path}" fill="none" stroke="{accent}" stroke-width="10" stroke-linecap="round"/>
  <circle cx="{dx:.1}" cy="{dy:.1}" r="5.5" fill="{ICE}"/>"#
            )
        }
        _ => String::new(),
    };
    let center_size = if center_text.chars().count() <= 4 {
        29
    } else {
        22
    };
    let badge_svg = match badge {
        Some(text) if !text.is_empty() => format!(
            r#"<rect x="92" y="10" width="40" height="18" rx="6" fill="{accent}" fill-opacity="0.22" stroke="{accent}" stroke-width="1.2"/>
  <text x="112" y="23" text-anchor="middle" fill="{ICE}" font-family="monospace" font-size="11" font-weight="700">{}</text>"#,
            xml_escape(text)
        ),
        _ => String::new(),
    };
    let body = format!(
        r#"<path d="{track}" fill="none" stroke="{TRACK}" stroke-width="10" stroke-linecap="round"/>
  {fill_arc}
  {badge_svg}
  <text x="72" y="67" text-anchor="middle" fill="{ICE}" font-family="sans-serif" font-size="{center_size}" font-weight="800">{}</text>
  <text x="72" y="106" text-anchor="middle" fill="{accent}" font-family="sans-serif" font-size="16" font-weight="800">{}</text>
  <text x="72" y="128" text-anchor="middle" fill="{MUTED}" font-family="monospace" font-size="14" font-weight="700">{}</text>"#,
        xml_escape(center_text),
        xml_escape(&clip_chars(label, 10)),
        xml_escape(&clip_chars(sublabel, 12)),
    );
    svg_card(accent, selected, &body)
}

pub fn render_overview(snap: &DeckSnapshot) -> String {
    let accent = if !snap.core_connected {
        CORAL
    } else if snap.counts.needs_you > 0 {
        CORAL
    } else if snap.counts.working > 0 {
        CYAN
    } else {
        MINT
    };
    let dot_col = if snap.core_connected { MINT } else { CORAL };
    let (hero, sub) = if !snap.core_connected {
        ("OFFLINE".into(), "NO HC CORE".into())
    } else if snap.counts.needs_you > 0 {
        (
            format!("{} NEEDS", snap.counts.needs_you),
            format!(
                "{} RUN · {} DONE",
                snap.counts.working, snap.counts.unseen_done
            ),
        )
    } else if snap.counts.working > 0 {
        (
            format!("{} WORK", snap.counts.working),
            format!("{} AGENTS LIVE", snap.counts.total),
        )
    } else {
        ("CLEAR".into(), format!("{} AGENTS IDLE", snap.counts.total))
    };
    let footer = if let Some(worst_id) = snap.limits.worst_provider.as_deref() {
        if let Some(p) = snap.provider(worst_id) {
            let pct = format_percent(p.worst_fraction, p.unlimited);
            let tag = match p.id.as_str() {
                "antigravity" => "AGY",
                "claude" => "CLD",
                "codex" => "CDX",
                "copilot" => "CPL",
                _ => "LIM",
            };
            format!("{tag} {pct}")
        } else {
            "LIMITS —".into()
        }
    } else {
        "LIMITS —".into()
    };
    let hero_size = if hero.len() <= 6 { 28 } else { 24 };
    let body = format!(
        r#"<circle cx="24" cy="23" r="5" fill="{dot_col}"/>
  <text x="36" y="28" fill="{ICE}" font-family="sans-serif" font-size="14" font-weight="800">HERDR</text>
  <text x="126" y="28" text-anchor="end" fill="{MUTED}" font-family="monospace" font-size="13" font-weight="700">{}</text>
  <text x="72" y="72" text-anchor="middle" fill="{accent}" font-family="sans-serif" font-size="{hero_size}" font-weight="800">{}</text>
  <text x="72" y="96" text-anchor="middle" fill="{ICE}" font-family="monospace" font-size="13" font-weight="600">{}</text>
  <rect x="16" y="108" width="112" height="24" rx="7" fill="{PILL_BG}" stroke="{accent}" stroke-opacity="0.45" stroke-width="1.2"/>
  <text x="72" y="125" text-anchor="middle" fill="{ICE}" font-family="monospace" font-size="14" font-weight="700">{}</text>"#,
        snap.counts.total,
        xml_escape(&hero),
        xml_escape(&sub),
        xml_escape(&footer),
    );
    svg_card(accent, snap.counts.needs_you > 0, &body)
}

pub fn render_inbox(snap: &DeckSnapshot, selected_index: usize) -> String {
    let Some((item, idx)) = snap.inbox_item(selected_index) else {
        let body = format!(
            r#"<text x="72" y="28" text-anchor="middle" fill="{MUTED}" font-family="sans-serif" font-size="14" font-weight="700">INBOX</text>
  <circle cx="72" cy="68" r="22" fill="none" stroke="{MINT}" stroke-width="4"/>
  <path d="M62 68 L69 75 L84 60" fill="none" stroke="{ICE}" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round"/>
  <text x="72" y="116" text-anchor="middle" fill="{MINT}" font-family="monospace" font-size="15" font-weight="700">0 PENDING</text>"#
        );
        return svg_card(MINT, false, &body);
    };
    let accent = match item.kind {
        InboxKind::Question => AMBER,
        InboxKind::Blocked | InboxKind::Error => CORAL,
        InboxKind::Done => MINT,
    };
    let count_badge = format!("{}/{}", idx + 1, snap.inbox.len());
    let lines = wrap_lines(&item.text, 13, 2);
    let line1 = lines.first().map(String::as_str).unwrap_or("");
    let line2 = lines.get(1).map(String::as_str).unwrap_or("");
    let body = format!(
        r#"<text x="16" y="27" fill="{accent}" font-family="sans-serif" font-size="13" font-weight="800">{}</text>
  <text x="128" y="27" text-anchor="end" fill="{ICE}" font-family="monospace" font-size="13" font-weight="700">{}</text>
  <text x="72" y="54" text-anchor="middle" fill="{ICE}" font-family="sans-serif" font-size="16" font-weight="800">{}</text>
  <text x="72" y="76" text-anchor="middle" fill="{ICE}" font-family="sans-serif" font-size="15" font-weight="600">{}</text>
  <text x="72" y="96" text-anchor="middle" fill="{ICE}" font-family="sans-serif" font-size="15" font-weight="600">{}</text>
  <text x="72" y="124" text-anchor="middle" fill="{MUTED}" font-family="monospace" font-size="13" font-weight="700">{}</text>"#,
        xml_escape(item.kind.short_label()),
        xml_escape(&count_badge),
        xml_escape(&clip_chars(&item.title, 12)),
        xml_escape(line1),
        xml_escape(line2),
        xml_escape(&clip_chars(&item.source, 14)),
    );
    svg_card(accent, item.kind != InboxKind::Done, &body)
}

pub fn render_inbox_action(
    snap: &DeckSnapshot,
    selected_index: usize,
    mode: &str,
    hold_hint: bool,
) -> String {
    let active = snap.inbox_item(selected_index).map(|(item, _)| item);
    let (accent, title, sub, icon) = match mode {
        "prev" => (
            CYAN,
            "PREV",
            format!("{} ITEMS", snap.inbox.len()),
            format!(r#"<polygon points="84,42 54,60 84,78" fill="{ICE}"/>"#),
        ),
        "focus" => (
            if active.is_some() { MINT } else { MUTED },
            "FOCUS",
            active
                .and_then(|i| i.terminal.as_deref())
                .unwrap_or("NONE")
                .to_owned(),
            format!(
                r#"<circle cx="72" cy="60" r="18" fill="none" stroke="{MINT}" stroke-width="4"/><circle cx="72" cy="60" r="5" fill="{ICE}"/>"#
            ),
        ),
        "reply_1" | "reply_2" => {
            let opt_idx = if mode == "reply_1" { 0 } else { 1 };
            let opt = active.and_then(|i| i.options.get(opt_idx));
            if hold_hint {
                (
                    AMBER,
                    "HOLD!",
                    "0.65s TO SEND".into(),
                    format!(
                        r#"<circle cx="72" cy="58" r="18" fill="none" stroke="{AMBER}" stroke-width="4"/>"#
                    ),
                )
            } else if let Some(label) = opt {
                (
                    AMBER,
                    "REPLY",
                    clip_chars(label, 12),
                    format!(
                        r#"<text x="72" y="66" text-anchor="middle" fill="{ICE}" font-family="sans-serif" font-size="18" font-weight="800">{}</text>"#,
                        xml_escape(&clip_chars(label, 9))
                    ),
                )
            } else {
                (
                    MUTED,
                    "OPTION",
                    "—".into(),
                    format!(
                        r#"<text x="72" y="66" text-anchor="middle" fill="{MUTED}" font-family="sans-serif" font-size="22" font-weight="800">—</text>"#
                    ),
                )
            }
        }
        _ => (
            CYAN,
            "NEXT",
            format!("{} ITEMS", snap.inbox.len()),
            format!(r#"<polygon points="60,42 90,60 60,78" fill="{ICE}"/>"#),
        ),
    };
    let body = format!(
        r#"{icon}
  <text x="72" y="106" text-anchor="middle" fill="{accent}" font-family="sans-serif" font-size="17" font-weight="800">{}</text>
  <text x="72" y="128" text-anchor="middle" fill="{MUTED}" font-family="monospace" font-size="13" font-weight="700">{}</text>"#,
        xml_escape(title),
        xml_escape(&clip_chars(&sub, 13)),
    );
    svg_card(accent, false, &body)
}

pub fn render_agent_slot(
    snap: &DeckSnapshot,
    slot: usize,
    filter: &str,
    pin_space: Option<&str>,
) -> String {
    let Some(agent) = snap.agent_for_slot(slot, filter, pin_space) else {
        let label = pin_space
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("SLOT");
        let body = format!(
            r#"<text x="72" y="54" text-anchor="middle" fill="{MUTED}" font-family="sans-serif" font-size="15" font-weight="700">{}</text>
  <text x="72" y="84" text-anchor="middle" fill="{TRACK}" font-family="monospace" font-size="24" font-weight="800">—</text>
  <text x="72" y="122" text-anchor="middle" fill="{MUTED}" font-family="monospace" font-size="13" font-weight="700">#{slot}</text>"#,
            xml_escape(&clip_chars(label, 11)),
        );
        return svg_card(MUTED, false, &body);
    };
    let accent = match agent.status {
        AgentStateKind::Blocked | AgentStateKind::Error => CORAL,
        AgentStateKind::Working => CYAN,
        AgentStateKind::Done if agent.unseen_done => MINT,
        AgentStateKind::Done => "#34d399",
        AgentStateKind::Idle | AgentStateKind::Unknown => MUTED,
    };
    let badge = if agent.unseen_done {
        "NEW!"
    } else {
        agent.status.short_label()
    };
    let space_lines = wrap_lines(&agent.space, 11, 2);
    let line1 = space_lines.first().map(String::as_str).unwrap_or("agent");
    let line2 = space_lines.get(1).map(String::as_str).unwrap_or("");
    let body = format!(
        r#"<circle cx="20" cy="24" r="5" fill="{accent}"/>
  <text x="31" y="29" fill="{accent}" font-family="sans-serif" font-size="13" font-weight="800">{}</text>
  <text x="128" y="29" text-anchor="end" fill="{MUTED}" font-family="monospace" font-size="13" font-weight="700">T{}</text>
  <text x="72" y="68" text-anchor="middle" fill="{ICE}" font-family="sans-serif" font-size="19" font-weight="800">{}</text>
  <text x="72" y="90" text-anchor="middle" fill="{ICE}" font-family="sans-serif" font-size="17" font-weight="700">{}</text>
  <text x="72" y="124" text-anchor="middle" fill="{MUTED}" font-family="monospace" font-size="14" font-weight="700">{} · {}</text>"#,
        xml_escape(badge),
        agent.turn,
        xml_escape(line1),
        xml_escape(line2),
        xml_escape(&agent.agent_short),
        xml_escape(&agent.address),
    );
    svg_card(
        accent,
        matches!(
            agent.status,
            AgentStateKind::Blocked | AgentStateKind::Error
        ) || agent.unseen_done,
        &body,
    )
}

pub fn render_coordinator(snap: &DeckSnapshot) -> String {
    let Some(coord) = &snap.coordinator else {
        let body = format!(
            r#"<text x="72" y="28" text-anchor="middle" fill="{MUTED}" font-family="sans-serif" font-size="14" font-weight="700">COORD</text>
  <text x="72" y="76" text-anchor="middle" fill="{MUTED}" font-family="sans-serif" font-size="22" font-weight="800">N/A</text>
  <text x="72" y="122" text-anchor="middle" fill="{MUTED}" font-family="monospace" font-size="13">UNAVAILABLE</text>"#
        );
        return svg_card(MUTED, false, &body);
    };
    let accent = if coord.needs_you > 0 {
        CORAL
    } else {
        match coord.state.as_str() {
            "running" => MINT,
            "degraded" => AMBER,
            _ => MUTED,
        }
    };
    let state_word = match coord.state.as_str() {
        "running" => "RUNNING",
        "degraded" => "DEGRADED",
        "not_running" => "OFF",
        _ => "UNKNOWN",
    };
    let mid = if coord.needs_you > 0 {
        format!("{} NEEDS", coord.needs_you)
    } else if coord.state == "running" {
        format!("{} RUN", coord.running_tasks)
    } else {
        "HOLD: UP".into()
    };
    let bot = format!("WS:{} RET:{}", coord.open_workstreams, coord.retained);
    let body = format!(
        r#"<circle cx="24" cy="24" r="5.5" fill="{accent}"/>
  <text x="36" y="29" fill="{ICE}" font-family="sans-serif" font-size="14" font-weight="800">COORD</text>
  <text x="72" y="54" text-anchor="middle" fill="{accent}" font-family="monospace" font-size="14" font-weight="800">{state_word}</text>
  <text x="72" y="88" text-anchor="middle" fill="{ICE}" font-family="sans-serif" font-size="25" font-weight="800">{}</text>
  <text x="72" y="124" text-anchor="middle" fill="{MUTED}" font-family="monospace" font-size="13" font-weight="700">{}</text>"#,
        xml_escape(&mid),
        xml_escape(&bot),
    );
    svg_card(accent, coord.needs_you > 0, &body)
}

pub fn render_limit_provider(
    snap: &DeckSnapshot,
    provider_setting: &str,
    selected_provider: &str,
) -> String {
    let Some(p) = snap.resolve_provider(provider_setting, selected_provider) else {
        let (_, short) = match provider_setting {
            "codex" => ("Codex", "CODEX"),
            "claude" => ("Claude", "CLAUDE"),
            "copilot" => ("Copilot", "COPILOT"),
            "antigravity" => ("Antigravity", "ANTIGRAV"),
            _ => ("Limits", "LIMITS"),
        };
        return svg_radial_gauge(MUTED, None, "—", short, "LOADING", None, false);
    };
    render_provider_gauge(p, snap.generated_at_ms, p.id == selected_provider)
}

fn render_provider_gauge(p: &ProviderSummary, now_ms: u64, selected: bool) -> String {
    let available = p.state == "available";
    let accent = usage_color(p.worst_fraction, p.stale, available);
    let center = match p.state.as_str() {
        "not_signed_in" => "AUTH".into(),
        "unavailable" | "unsupported" => "N/A".into(),
        "pending" => "…".into(),
        _ => format_percent(p.worst_fraction, p.unlimited),
    };
    let sub = if let Some(credits) = &p.credits_text {
        credits.clone()
    } else if !available {
        p.state.to_uppercase().replace('_', " ")
    } else {
        let reset = format_reset_short(p.worst_reset_ms, now_ms);
        if reset.is_empty() { "OK".into() } else { reset }
    };
    let badge = if p.stale {
        Some("STALE")
    } else if selected {
        Some("SEL")
    } else {
        None
    };
    svg_radial_gauge(
        accent,
        p.worst_fraction,
        &center,
        &p.short_name,
        &sub,
        badge,
        selected,
    )
}

pub fn render_limit_bucket(
    snap: &DeckSnapshot,
    provider_setting: &str,
    selected_provider: &str,
    bucket_index: usize,
) -> String {
    let Some(p) = snap.resolve_provider(provider_setting, selected_provider) else {
        return svg_radial_gauge(MUTED, None, "—", "BUCKET", "—", None, false);
    };
    let Some(bucket) = p.buckets.get(bucket_index) else {
        return svg_radial_gauge(
            MUTED,
            None,
            "—",
            &p.short_name,
            &format!("SLOT {}", bucket_index + 1),
            None,
            false,
        );
    };
    render_bucket_gauge(p, bucket, snap.generated_at_ms)
}

fn render_bucket_gauge(p: &ProviderSummary, b: &BucketSummary, now_ms: u64) -> String {
    let accent = usage_color(b.used_fraction, b.stale || p.stale, p.state == "available");
    let center = format_percent(b.used_fraction, b.unlimited);
    let sub = if let Some(credits) = &b.credits_text {
        credits.clone()
    } else {
        let reset = format_reset_short(b.resets_at_ms, now_ms);
        if reset.is_empty() {
            p.short_name.clone()
        } else {
            reset
        }
    };
    let badge = if b.stale { Some("~") } else { None };
    svg_radial_gauge(
        accent,
        b.used_fraction,
        &center,
        &b.label,
        &sub,
        badge,
        false,
    )
}

pub fn render_preview_html(tiles: &[(&str, &str)]) -> String {
    let mut cards = String::new();
    for (idx, (label, file_name)) in tiles.iter().enumerate() {
        cards.push_str(&format!(
            r#"<div class="key"><img src="{}" alt="{}"/><div class="cap">#{idx} · {}</div></div>"#,
            xml_escape(file_name),
            xml_escape(label),
            xml_escape(label),
        ));
    }
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <title>kit-deck 5×3 Stream Deck MK.2 Preview</title>
  <style>
    body {{ margin: 0; padding: 32px; background: #05070e; color: #e5eefc; font-family: system-ui, sans-serif; }}
    h1 {{ font-size: 18px; margin: 0 0 18px; color: #22d3ee; }}
    .deck {{ display: grid; grid-template-columns: repeat(5, 144px); gap: 20px; background: #0b0f1b; padding: 24px; border-radius: 20px; border: 1px solid #1e293b; width: fit-content; }}
    .key {{ display: flex; flex-direction: column; align-items: center; gap: 6px; }}
    .key img {{ width: 144px; height: 144px; border-radius: 20px; display: block; }}
    .cap {{ font-size: 11px; color: #8b9bb4; font-family: monospace; }}
  </style>
</head>
<body>
  <h1>Herdr Kit — OpenDeck 5×3 Stream Deck MK.2 Preview</h1>
  <div class="deck">{cards}</div>
</body>
</html>"#
    )
}
