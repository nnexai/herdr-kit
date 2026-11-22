use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use kit_core::paths::{SymlinkMessage, no_links, write_atomic};
use serde_json::{Value, json};

use super::cli::{RoleCreate, RoleEdit};
use super::error;
pub use super::kinds::SessionMode;
use super::paths::CoordPaths;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleSource {
    Builtin,
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Role {
    pub name: String,
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub start: Vec<String>,
    pub resume: Option<Vec<String>>,
    pub session: SessionMode,
    pub trust: Vec<String>,
    pub steering: Option<bool>,
    pub body: String,
    pub source: RoleSource,
}

pub struct Vars<'a> {
    pub brief: &'a str,
    pub session_dir: Option<&'a Path>,
    pub session_id: Option<&'a str>,
    pub repo_root: Option<&'a Path>,
    pub cwd: &'a Path,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub computer_config: Option<&'a Path>,
}

pub const BUILTIN: [(&str, &str); 8] = [
    ("coord-main", include_str!("roles/coord-main.md")),
    ("coord-light", include_str!("roles/coord-light.md")),
    ("coord-default", include_str!("roles/coord-default.md")),
    ("coord-deep", include_str!("roles/coord-deep.md")),
    ("coord-design", include_str!("roles/coord-design.md")),
    ("coord-review", include_str!("roles/coord-review.md")),
    ("coord-computer", include_str!("roles/coord-computer.md")),
    ("agy-default", include_str!("roles/agy-default.md")),
];

pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

pub const KNOWN_HARNESSES: &[&str] = &["omp", "agy", "claude", "codex", "copilot"];

pub struct HarnessDefaults {
    pub start: &'static str,
    pub resume: Option<&'static str>,
    pub session: SessionMode,
    pub trust: Option<&'static str>,
    pub steering: Option<bool>,
}

pub fn harness_defaults(harness: &str) -> Option<HarnessDefaults> {
    match harness {
        "omp" => Some(HarnessDefaults {
            start: "omp --session-dir {session_dir} {brief}",
            resume: Some("omp --session-dir {session_dir} --resume {session_id} {brief}"),
            session: SessionMode::Dir,
            trust: None,
            steering: None,
        }),
        "agy" => Some(HarnessDefaults {
            start: "agy -i {brief}",
            resume: Some("agy --conversation {session_id} -i {brief}"),
            session: SessionMode::AgyConversation,
            trust: Some(r#"--settings '{"trustedWorkspaces":[{cwd_json}]}'"#),
            steering: Some(true),
        }),
        "claude" => Some(HarnessDefaults {
            start: "claude --session-id {session_id} {brief}",
            resume: Some("claude --resume {session_id} {brief}"),
            session: SessionMode::Preset,
            trust: None,
            steering: None,
        }),
        "codex" => Some(HarnessDefaults {
            start: "codex {brief}",
            resume: Some("codex resume {session_id} {brief}"),
            session: SessionMode::CodexRollout,
            trust: Some(r#"-c 'projects={ "{repo_root}" = { trust_level = "trusted" } }'"#),
            steering: None,
        }),
        "copilot" => Some(HarnessDefaults {
            start: "copilot --session-id {session_id} -i {brief}",
            resume: Some("copilot --resume {session_id} -i {brief}"),
            session: SessionMode::Preset,
            trust: None,
            steering: None,
        }),
        _ => None,
    }
}

pub fn session_mode_str(mode: SessionMode) -> &'static str {
    match mode {
        SessionMode::Dir => "dir",
        SessionMode::Preset => "preset",
        SessionMode::CodexRollout => "codex-rollout",
        SessionMode::AgyConversation => "agy-conversation",
        SessionMode::None => "none",
    }
}

pub fn parse_session_mode(value: &str) -> Result<SessionMode> {
    match value {
        "dir" => Ok(SessionMode::Dir),
        "preset" => Ok(SessionMode::Preset),
        "codex-rollout" => Ok(SessionMode::CodexRollout),
        "agy-conversation" => Ok(SessionMode::AgyConversation),
        "none" => Ok(SessionMode::None),
        _ => bail!(
            "unknown session mode: {value} (valid: dir, preset, codex-rollout, agy-conversation, none)"
        ),
    }
}

pub fn model_flag(harness: &str) -> Option<&'static str> {
    match harness {
        "omp" | "agy" | "claude" | "copilot" => Some("--model"),
        "codex" => Some("-m"),
        _ => None,
    }
}

pub fn effort_flag(harness: &str) -> Option<&'static str> {
    match harness {
        "agy" | "claude" => Some("--effort"),
        "omp" => Some("--thinking"),
        _ => None,
    }
}

pub fn valid_effort(harness: &str, value: &str) -> bool {
    match harness {
        "agy" | "claude" => matches!(value, "low" | "medium" | "high" | "xhigh" | "max"),
        "omp" => matches!(
            value,
            "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "auto"
        ),
        _ => !value.is_empty(),
    }
}

pub fn harness_supports_steering(harness: &str) -> bool {
    matches!(harness, "omp" | "agy")
}

pub fn app_supports_steering(app: Option<&str>) -> bool {
    matches!(app, Some("omp" | "antigravity" | "agy"))
}

const AGY_STEER_SETTINGS: &str = r#"{"queuedMessages":"send-immediately"}"#;

/// Merge multiple `--settings <json-object>` flags for `agy` into one `--settings`
/// flag, because `agy`'s `--settings` flag is last-wins rather than repeatable.
fn merge_agy_settings(argv: Vec<String>) -> Result<Vec<String>> {
    let mut merged = serde_json::Map::new();
    let mut json_positions: Vec<(usize, bool)> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        if argv[i] == "--" {
            break;
        }
        if argv[i] == "--settings" {
            if let Some(val) = argv.get(i + 1)
                && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(val)
            {
                for (k, v) in obj {
                    merged.insert(k, v);
                }
                json_positions.push((i, false));
                i += 2;
                continue;
            }
        } else if let Some(val) = argv[i].strip_prefix("--settings=")
            && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(val)
        {
            for (k, v) in obj {
                merged.insert(k, v);
            }
            json_positions.push((i, true));
            i += 1;
            continue;
        }
        i += 1;
    }
    if json_positions.len() < 2 {
        return Ok(argv);
    }
    let combined = serde_json::to_string(&Value::Object(merged))?;
    let first = json_positions[0].0;
    let mut out = Vec::with_capacity(argv.len());
    let mut idx = 0;
    while idx < argv.len() {
        if let Some(&(_, inline)) = json_positions.iter().find(|&&(pos, _)| pos == idx) {
            if idx == first {
                out.push("--settings".into());
                out.push(combined.clone());
            }
            idx += if inline { 1 } else { 2 };
            continue;
        }
        out.push(argv[idx].clone());
        idx += 1;
    }
    Ok(out)
}

impl Role {
    pub fn effective_model(&self) -> Option<String> {
        self.model.clone().or_else(|| model_of(&self.start))
    }

    pub fn effective_effort(&self) -> Option<String> {
        self.effort.clone().or_else(|| effort_of(&self.start))
    }

    pub fn resumable(&self) -> bool {
        self.resume.is_some() && self.session != SessionMode::None
    }

    pub fn supports_steering(&self) -> bool {
        self.steering
            .unwrap_or_else(|| harness_supports_steering(&self.harness))
    }

    pub fn needs_computer_config(&self) -> bool {
        self.start.iter().any(|a| a.contains("{computer_config}"))
            || self
                .resume
                .as_deref()
                .is_some_and(|r| r.iter().any(|a| a.contains("{computer_config}")))
    }

    pub fn expand_start(&self, vars: &Vars<'_>, trusted: bool) -> Result<Vec<String>> {
        let model = vars.model.or(self.model.as_deref());
        let effort = vars.effort.or(self.effort.as_deref());
        let effective = Vars {
            model,
            effort,
            ..*vars
        };
        let mut injected = Vec::new();
        if self.harness == "agy" && self.supports_steering() {
            injected.push("--settings".into());
            injected.push(AGY_STEER_SETTINGS.into());
        }
        if trusted {
            injected.extend(self.trust.iter().cloned());
        }
        if let Some(m) = model {
            if !self.start.iter().any(|arg| arg.contains("{model}")) {
                let flag = model_flag(&self.harness).with_context(|| {
                    format!(
                        "harness {} requires {{model}} in start when model is set",
                        self.harness
                    )
                })?;
                injected.push(flag.into());
                injected.push(m.into());
            }
        }
        if let Some(e) = effort {
            if !self.start.iter().any(|arg| arg.contains("{effort}")) {
                let flag = effort_flag(&self.harness).with_context(|| {
                    format!(
                        "harness {} requires {{effort}} in start when effort is set",
                        self.harness
                    )
                })?;
                injected.push(flag.into());
                injected.push(e.into());
            }
        }
        let expanded = expand(
            &self.start,
            &effective,
            (!injected.is_empty()).then_some(injected.as_slice()),
        )?;
        if self.harness == "agy" {
            merge_agy_settings(expanded)
        } else {
            Ok(expanded)
        }
    }

    pub fn expand_resume(&self, vars: &Vars<'_>, trusted: bool) -> Result<Vec<String>> {
        let template = self
            .resume
            .as_deref()
            .context("role has no resume command")?;
        ensure!(
            !template
                .iter()
                .any(|arg| arg.contains("{model}") || arg.contains("{effort}")),
            "role resume command must not reference {{model}} or {{effort}}"
        );
        let effective = Vars {
            model: None,
            effort: None,
            ..*vars
        };
        let mut injected = Vec::new();
        if self.harness == "agy" && self.supports_steering() {
            injected.push("--settings".into());
            injected.push(AGY_STEER_SETTINGS.into());
        }
        if trusted {
            injected.extend(self.trust.iter().cloned());
        }
        let expanded = expand(
            template,
            &effective,
            (!injected.is_empty()).then_some(injected.as_slice()),
        )?;
        if self.harness == "agy" {
            merge_agy_settings(expanded)
        } else {
            Ok(expanded)
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_writable_name(name: &str) -> bool {
    valid_name(name) && !matches!(name, "create" | "edit" | "delete" | "reset")
}

pub fn source_label(source: &RoleSource, overrides_builtin: bool) -> &'static str {
    match source {
        RoleSource::Builtin => "builtin",
        RoleSource::File(_) if overrides_builtin => "override",
        RoleSource::File(_) => "file",
    }
}

pub fn to_json(role: &Role) -> Value {
    let overrides = matches!(role.source, RoleSource::File(_)) && builtin(&role.name).is_some();
    let source = source_label(&role.source, overrides);
    let path = match &role.source {
        RoleSource::Builtin => None,
        RoleSource::File(p) => Some(p.to_string_lossy().into_owned()),
    };
    json!({
        "name": role.name,
        "source": source,
        "overrides_builtin": overrides,
        "path": path,
        "harness": role.harness,
        "model": role.effective_model(),
        "effort": role.effective_effort(),
        "session": role.session,
        "start": format_words(&role.start),
        "start_argv": role.start,
        "resume": role.resume.as_deref().map(format_words),
        "resume_argv": role.resume,
        "trust": (!role.trust.is_empty()).then(|| format_words(&role.trust)),
        "trust_argv": role.trust,
        "resumable": role.resumable(),
        "steering": role.supports_steering(),
        "body": role.body,
    })
}

pub fn to_json_with_usage(
    role: &Role,
    snapshot: Option<&kit_core::usage::Snapshot>,
    omp_config: Option<&Path>,
    now_ms: u64,
) -> Value {
    let mut value = to_json(role);
    if let (Some(snap), Some(map)) = (snapshot, value.as_object_mut()) {
        let (resolved_model, usage) = kit_core::usage::resolve::evaluate_role(
            snap,
            &role.harness,
            role.effective_model().as_deref(),
            omp_config,
            now_ms,
        );
        map.insert("resolved_model".into(), json!(resolved_model));
        map.insert("usage".into(), json!(usage));
    }
    value
}

pub fn load(paths: &CoordPaths, name: &str) -> Result<Role> {
    ensure!(valid_name(name), "invalid role name: {name}");
    let path = paths.roles.join(format!("{name}.md"));
    match fs::read_to_string(&path) {
        Ok(text) => parse(name, &text, RoleSource::File(path)),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let text = builtin(name).with_context(|| format!("unknown role: {name}"))?;
            parse(name, text, RoleSource::Builtin)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn list(paths: &CoordPaths) -> Result<Vec<(String, RoleSource, bool)>> {
    let mut roles = BTreeMap::new();
    for (name, _) in BUILTIN {
        roles.insert(name.to_owned(), (RoleSource::Builtin, false));
    }
    match fs::read_dir(&paths.roles) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("md")
                    || !entry.file_type()?.is_file()
                {
                    continue;
                }
                if let Some(name) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|s| valid_name(s))
                {
                    roles.insert(
                        name.to_owned(),
                        (RoleSource::File(path.clone()), builtin(name).is_some()),
                    );
                }
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(roles
        .into_iter()
        .map(|(name, (source, overrides))| (name, source, overrides))
        .collect())
}

pub fn parse(name: &str, text: &str, source: RoleSource) -> Result<Role> {
    let mut lines = text.split_inclusive('\n');
    ensure!(
        lines.next().map(str::trim) == Some("---"),
        "missing role front matter"
    );
    let mut fields = BTreeMap::new();
    let mut closed = false;
    for line in lines.by_ref() {
        let line = line.trim();
        if line == "---" {
            closed = true;
            break;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once(':').context("invalid role front matter")?;
        let key = key.trim();
        ensure!(
            matches!(
                key,
                "harness"
                    | "model"
                    | "effort"
                    | "start"
                    | "resume"
                    | "session"
                    | "trust"
                    | "steering"
            ),
            "unknown role key: {key}"
        );
        let value = value.split_once(" #").map_or(value, |(v, _)| v).trim();
        ensure!(
            fields.insert(key.to_owned(), value.to_owned()).is_none(),
            "duplicate role key: {key}"
        );
    }
    ensure!(closed, "unterminated role front matter");
    let required = |key: &str| -> Result<&str> {
        fields
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
            .with_context(|| format!("missing role {key}"))
    };
    let optional_nonempty = |key: &str| -> Result<Option<String>> {
        match fields.get(key) {
            Some(v) if v.is_empty() => bail!("empty role {key}"),
            Some(v) => Ok(Some(v.clone())),
            None => Ok(None),
        }
    };
    let harness = required("harness")?.to_owned();
    let model = optional_nonempty("model")?;
    let effort = optional_nonempty("effort")?;
    let start = split_words(required("start")?)?;
    ensure!(
        !start.is_empty() && !start[0].is_empty(),
        "empty start command"
    );
    let resume = fields.get("resume").map(|s| split_words(s)).transpose()?;
    if let Some(argv) = &resume {
        ensure!(
            !argv.is_empty() && !argv[0].is_empty(),
            "empty resume command"
        );
        ensure!(
            !argv
                .iter()
                .any(|arg| arg.contains("{model}") || arg.contains("{effort}")),
            "role resume command must not reference {{model}} or {{effort}}; resumed sessions keep their original model"
        );
    }
    let trust = fields
        .get("trust")
        .map(|s| split_words(s))
        .transpose()?
        .unwrap_or_default();
    let steering = match fields.get("steering").map(String::as_str) {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(other) => bail!("invalid role steering: {other} (valid: true, false)"),
    };
    if steering == Some(true) {
        ensure!(
            harness_supports_steering(&harness),
            "harness {harness} does not support mid-turn steering"
        );
    }
    let session = parse_session_mode(required("session")?)?;
    let start_placeholders = collect_placeholders(&start)?;
    ensure!(
        start_placeholders.contains("brief"),
        "role start command must reference {{brief}}"
    );
    match session {
        SessionMode::Dir => ensure!(
            start_placeholders.contains("session_dir"),
            "session mode dir requires {{session_dir}} in start command"
        ),
        _ => ensure!(
            !start_placeholders.contains("session_dir"),
            "placeholder {{session_dir}} requires session mode dir"
        ),
    }
    match session {
        SessionMode::Preset => ensure!(
            start_placeholders.contains("session_id"),
            "session mode preset requires {{session_id}} in start command"
        ),
        _ => ensure!(
            !start_placeholders.contains("session_id"),
            "placeholder {{session_id}} in start requires session mode preset"
        ),
    }
    if let Some(argv) = &resume {
        let resume_placeholders = collect_placeholders(argv)?;
        ensure!(
            resume_placeholders.contains("brief"),
            "role resume command must reference {{brief}}"
        );
        ensure!(
            resume_placeholders.contains("session_id"),
            "role resume command must reference {{session_id}}"
        );
        match session {
            SessionMode::Dir => ensure!(
                resume_placeholders.contains("session_dir"),
                "session mode dir requires {{session_dir}} in resume command"
            ),
            _ => ensure!(
                !resume_placeholders.contains("session_dir"),
                "placeholder {{session_dir}} in resume requires session mode dir"
            ),
        }
    }
    let trust_placeholders = collect_placeholders(&trust)?;
    ensure!(
        !trust_placeholders.contains("model") && !trust_placeholders.contains("effort"),
        "role trust flags must not reference {{model}} or {{effort}}"
    );
    let has_model_placeholder = start_placeholders.contains("model");
    let has_effort_placeholder = start_placeholders.contains("effort");
    if has_model_placeholder {
        ensure!(
            model.is_some(),
            "start command references {{model}} but role has no model"
        );
    }
    if has_effort_placeholder {
        ensure!(
            effort.is_some(),
            "start command references {{effort}} but role has no effort"
        );
    }
    if model.is_some() && !has_model_placeholder {
        ensure!(
            !has_flag(&start, MODEL_FLAGS),
            "role specifies model in both front matter and start command"
        );
        ensure!(
            model_flag(&harness).is_some(),
            "harness {harness} requires {{model}} in start when model is set"
        );
    }
    if let Some(value) = &effort {
        ensure!(
            valid_effort(&harness, value),
            "invalid effort for {harness}: {value}"
        );
        if !has_effort_placeholder {
            ensure!(
                !has_flag(&start, EFFORT_FLAGS),
                "role specifies effort in both front matter and start command"
            );
            ensure!(
                effort_flag(&harness).is_some(),
                "harness {harness} requires {{effort}} in start when effort is set"
            );
        }
    }
    Ok(Role {
        name: name.into(),
        harness,
        model,
        effort,
        start,
        resume,
        session,
        trust,
        steering,
        body: lines.collect::<String>().trim().to_owned(),
        source,
    })
}

fn collect_placeholders(argv: &[String]) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    for arg in argv {
        let mut rest = arg.as_str();
        while let Some(pos) = rest.find('{') {
            rest = &rest[pos + 1..];
            let Some(end) = rest.find('}') else {
                break;
            };
            let name = &rest[..end];
            if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                continue;
            }
            ensure!(
                matches!(
                    name,
                    "brief"
                        | "session_dir"
                        | "session_id"
                        | "repo_root"
                        | "cwd"
                        | "cwd_json"
                        | "model"
                        | "effort"
                        | "computer_config"
                ),
                "unknown placeholder: {{{name}}}"
            );
            found.insert(name.to_owned());
            rest = &rest[end + 1..];
        }
    }
    Ok(found)
}

pub fn format_words(words: &[String]) -> String {
    words
        .iter()
        .enumerate()
        .map(|(idx, word)| {
            if word.is_empty() {
                return "\"\"".to_owned();
            }
            let safe = word
                .chars()
                .all(|c| !c.is_whitespace() && !matches!(c, '\'' | '"' | '\\'))
                && !(idx > 0 && word.starts_with('#'));
            if safe {
                word.clone()
            } else if !word.contains('\'') && !word.contains(" #") {
                format!("'{word}'")
            } else {
                let mut escaped = String::with_capacity(word.len() + 2);
                escaped.push('"');
                for c in word.chars() {
                    if matches!(c, '"' | '\\') {
                        escaped.push('\\');
                    }
                    escaped.push(c);
                }
                escaped.push('"');
                escaped
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn check_frontmatter_value(key: &str, value: &str) -> Result<()> {
    ensure!(
        !value.contains('\n') && !value.contains('\r'),
        "role {key} must be a single line"
    );
    ensure!(
        !value.contains(" #"),
        "role {key} must not contain ' #' (inline comment delimiter)"
    );
    Ok(())
}

pub fn format_markdown(role: &Role) -> Result<String> {
    check_frontmatter_value("harness", &role.harness)?;
    if let Some(m) = &role.model {
        check_frontmatter_value("model", m)?;
    }
    if let Some(e) = &role.effort {
        check_frontmatter_value("effort", e)?;
    }
    let start_line = format_words(&role.start);
    check_frontmatter_value("start", &start_line)?;
    let resume_line = role.resume.as_deref().map(format_words);
    if let Some(r) = &resume_line {
        check_frontmatter_value("resume", r)?;
    }
    let trust_line = (!role.trust.is_empty()).then(|| format_words(&role.trust));
    if let Some(t) = &trust_line {
        check_frontmatter_value("trust", t)?;
    }

    let mut out = String::from("---\n");
    out.push_str(&format!("harness: {}\n", role.harness));
    if let Some(m) = &role.model {
        out.push_str(&format!("model: {m}\n"));
    }
    if let Some(e) = &role.effort {
        out.push_str(&format!("effort: {e}\n"));
    }
    out.push_str(&format!("start: {start_line}\n"));
    if let Some(r) = &resume_line {
        out.push_str(&format!("resume: {r}\n"));
    }
    out.push_str(&format!("session: {}\n", session_mode_str(role.session)));
    if let Some(t) = &trust_line {
        out.push_str(&format!("trust: {t}\n"));
    }
    if let Some(s) = role.steering {
        out.push_str(&format!("steering: {s}\n"));
    }
    out.push_str("---\n");
    let body = role.body.trim();
    if !body.is_empty() {
        out.push_str(body);
        out.push('\n');
    }

    // Validate through the exact same parser the loader uses, and assert lossless round-trip.
    let parsed = parse(&role.name, &out, role.source.clone())?;
    ensure!(
        parsed.harness == role.harness
            && parsed.model == role.model
            && parsed.effort == role.effort
            && parsed.start == role.start
            && parsed.resume == role.resume
            && parsed.session == role.session
            && parsed.trust == role.trust
            && parsed.steering == role.steering
            && parsed.body == body,
        "formatted role front matter did not round-trip cleanly"
    );
    Ok(out)
}

fn read_prompt(prompt: Option<&str>, prompt_file: Option<&Path>) -> Result<Option<String>> {
    if let Some(text) = prompt {
        return Ok(Some(text.to_owned()));
    }
    let Some(path) = prompt_file else {
        return Ok(None);
    };
    let mut text = String::new();
    if path == Path::new("-") {
        std::io::stdin()
            .take(1_048_577)
            .read_to_string(&mut text)
            .context("reading prompt from stdin")?;
    } else {
        fs::File::open(path)
            .with_context(|| format!("opening prompt file {}", path.display()))?
            .take(1_048_577)
            .read_to_string(&mut text)
            .with_context(|| format!("reading prompt file {}", path.display()))?;
    }
    ensure!(text.len() <= 1_048_576, "prompt exceeds 1 MiB");
    Ok(Some(text))
}

fn strip_flag(argv: &[String], flags: &[&str]) -> Vec<String> {
    if argv.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(argv.len());
    out.push(argv[0].clone());
    let mut iter = argv.iter().skip(1);
    let mut after_sep = false;
    while let Some(arg) = iter.next() {
        if after_sep {
            out.push(arg.clone());
            continue;
        }
        if arg == "--" {
            after_sep = true;
            out.push(arg.clone());
            continue;
        }
        if flags.contains(&arg.as_str()) {
            let _ = iter.next();
            continue;
        }
        let matches_inline = flags.iter().any(|flag| {
            arg.strip_prefix(flag).is_some_and(|rest| {
                rest.starts_with('=') || (is_short_flag(flag) && !rest.is_empty())
            })
        });
        if matches_inline {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn write_validated(paths: &CoordPaths, role: &Role) -> Result<Role> {
    let text = format_markdown(role).map_err(|e| error::refused("role_invalid", e.to_string()))?;
    paths.ensure()?;
    let path = paths.roles.join(format!("{}.md", role.name));
    write_atomic(&path, text.as_bytes(), SymlinkMessage::WithPath)?;
    parse(&role.name, &text, RoleSource::File(path))
        .map_err(|e| error::refused("role_invalid", e.to_string()))
}

pub fn load_or_not_found(paths: &CoordPaths, name: &str) -> Result<Role> {
    if !valid_name(name) {
        return Err(error::refused(
            "invalid_role_name",
            format!("invalid role name: {name}"),
        ));
    }
    let path = paths.roles.join(format!("{name}.md"));
    no_links(&path, SymlinkMessage::WithPath)?;
    match fs::read_to_string(&path) {
        Ok(text) => parse(name, &text, RoleSource::File(path))
            .map_err(|e| error::refused("role_invalid", e.to_string())),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let text = builtin(name).ok_or_else(|| {
                error::refused("role_not_found", format!("unknown role: {name}"))
            })?;
            parse(name, text, RoleSource::Builtin)
                .map_err(|e| error::refused("role_invalid", e.to_string()))
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn create(paths: &CoordPaths, args: &RoleCreate) -> Result<Role> {
    if !valid_writable_name(&args.name) {
        return Err(error::refused(
            "invalid_role_name",
            format!(
                "invalid role name '{}': use ASCII letters, digits, hyphens, or underscores (and not create/edit/delete/reset)",
                args.name
            ),
        ));
    }
    let target = paths.roles.join(format!("{}.md", args.name));
    no_links(&target, SymlinkMessage::WithPath)?;
    if target.exists() {
        return Err(error::refused(
            "role_exists",
            format!(
                "role '{}' already exists; use 'roles edit {}' to modify it",
                args.name, args.name
            ),
        ));
    }
    if builtin(&args.name).is_some() {
        return Err(error::refused(
            "role_exists",
            format!(
                "built-in role '{}' already exists; use 'roles edit {}' to override it",
                args.name, args.name
            ),
        ));
    }

    let mut role = if let Some(from) = &args.from {
        let mut base = load_or_not_found(paths, from)?;
        base.name = args.name.clone();
        base.source = RoleSource::File(target.clone());
        base
    } else {
        let harness = args
            .harness
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                error::refused(
                    "role_invalid",
                    "missing --harness (or specify --from <existing-role>)",
                )
            })?
            .to_owned();
        let defaults = harness_defaults(&harness);
        let start_str = match (&args.start, &defaults) {
            (Some(s), _) => s.as_str(),
            (None, Some(d)) => d.start,
            (None, None) => {
                return Err(error::refused(
                    "role_invalid",
                    format!(
                        "unknown harness '{harness}' (known: {}); specify --start and --session for a custom harness",
                        KNOWN_HARNESSES.join(", ")
                    ),
                ));
            }
        };
        let session = match (&args.session, &defaults) {
            (Some(s), _) => {
                parse_session_mode(s).map_err(|e| error::refused("role_invalid", e.to_string()))?
            }
            (None, Some(d)) => d.session,
            (None, None) => {
                return Err(error::refused(
                    "role_invalid",
                    format!(
                        "unknown harness '{harness}' (known: {}); specify --start and --session for a custom harness",
                        KNOWN_HARNESSES.join(", ")
                    ),
                ));
            }
        };
        let start = split_words(start_str)
            .map_err(|e| error::refused("role_invalid", format!("invalid start command: {e}")))?;
        let resume = if args.clear_resume {
            None
        } else if let Some(r) = &args.resume {
            Some(
                split_words(r).map_err(|e| {
                    error::refused("role_invalid", format!("invalid resume command: {e}"))
                })?,
            )
        } else {
            defaults.as_ref().and_then(|d| d.resume).map(|r| {
                split_words(r).expect("built-in default resume command must split cleanly")
            })
        };
        let trust = if args.clear_trust {
            Vec::new()
        } else if let Some(t) = &args.trust {
            split_words(t)
                .map_err(|e| error::refused("role_invalid", format!("invalid trust flags: {e}")))?
        } else {
            defaults
                .as_ref()
                .and_then(|d| d.trust)
                .map(|t| split_words(t).expect("built-in default trust flags must split cleanly"))
                .unwrap_or_default()
        };
        Role {
            name: args.name.clone(),
            harness,
            model: None,
            effort: None,
            start,
            resume,
            session,
            trust,
            steering: defaults.as_ref().and_then(|d| d.steering),
            body: String::new(),
            source: RoleSource::File(target.clone()),
        }
    };

    if let Some(s) = args.steering {
        role.steering = Some(s);
    }

    if args.from.is_some() {
        if let Some(h) = &args.harness {
            role.harness = h.trim().to_owned();
        }
        if let Some(s) = &args.start {
            role.start = split_words(s).map_err(|e| {
                error::refused("role_invalid", format!("invalid start command: {e}"))
            })?;
        }
        if args.clear_resume {
            role.resume = None;
        } else if let Some(r) = &args.resume {
            role.resume = Some(split_words(r).map_err(|e| {
                error::refused("role_invalid", format!("invalid resume command: {e}"))
            })?);
        }
        if let Some(s) = &args.session {
            role.session =
                parse_session_mode(s).map_err(|e| error::refused("role_invalid", e.to_string()))?;
        }
        if args.clear_trust {
            role.trust.clear();
        } else if let Some(t) = &args.trust {
            role.trust = split_words(t)
                .map_err(|e| error::refused("role_invalid", format!("invalid trust flags: {e}")))?;
        }
    }

    if args.clear_model {
        role.model = None;
        if args.start.is_none() {
            role.start = strip_flag(&role.start, MODEL_FLAGS);
        }
    } else if let Some(m) = &args.model {
        let trimmed = m.trim();
        if trimmed.is_empty() {
            return Err(error::refused("role_invalid", "empty role model"));
        }
        if args.start.is_none() && !role.start.iter().any(|a| a.contains("{model}")) {
            role.start = strip_flag(&role.start, MODEL_FLAGS);
        }
        role.model = Some(trimmed.to_owned());
    }

    if args.clear_effort {
        role.effort = None;
        if args.start.is_none() {
            role.start = strip_flag(&role.start, EFFORT_FLAGS);
        }
    } else if let Some(e) = &args.effort {
        let trimmed = e.trim();
        if trimmed.is_empty() {
            return Err(error::refused("role_invalid", "empty role effort"));
        }
        if args.start.is_none() && !role.start.iter().any(|a| a.contains("{effort}")) {
            role.start = strip_flag(&role.start, EFFORT_FLAGS);
        }
        role.effort = Some(trimmed.to_owned());
    }

    if let Some(body) = read_prompt(args.prompt.as_deref(), args.prompt_file.as_deref())? {
        role.body = body.trim().to_owned();
    }

    write_validated(paths, &role)
}

pub fn edit(paths: &CoordPaths, args: &RoleEdit) -> Result<Role> {
    let any_change = args.harness.is_some()
        || args.model.is_some()
        || args.clear_model
        || args.effort.is_some()
        || args.clear_effort
        || args.start.is_some()
        || args.resume.is_some()
        || args.clear_resume
        || args.session.is_some()
        || args.trust.is_some()
        || args.clear_trust
        || args.steering.is_some()
        || args.prompt.is_some()
        || args.prompt_file.is_some();
    if !any_change {
        return Err(error::refused(
            "invalid_arguments",
            "no fields specified to edit",
        ));
    }

    let mut role = load_or_not_found(paths, &args.name)?;
    let target = paths.roles.join(format!("{}.md", args.name));
    role.source = RoleSource::File(target);

    if let Some(h) = &args.harness {
        role.harness = h.trim().to_owned();
    }
    if let Some(s) = &args.start {
        role.start = split_words(s)
            .map_err(|e| error::refused("role_invalid", format!("invalid start command: {e}")))?;
    }
    if args.clear_resume {
        role.resume = None;
    } else if let Some(r) = &args.resume {
        role.resume = Some(
            split_words(r).map_err(|e| {
                error::refused("role_invalid", format!("invalid resume command: {e}"))
            })?,
        );
    }
    if let Some(s) = &args.session {
        role.session =
            parse_session_mode(s).map_err(|e| error::refused("role_invalid", e.to_string()))?;
    }
    if args.clear_trust {
        role.trust.clear();
    } else if let Some(t) = &args.trust {
        role.trust = split_words(t)
            .map_err(|e| error::refused("role_invalid", format!("invalid trust flags: {e}")))?;
    }
    if let Some(s) = args.steering {
        role.steering = Some(s);
    }

    if args.clear_model {
        role.model = None;
        if args.start.is_none() {
            role.start = strip_flag(&role.start, MODEL_FLAGS);
        }
    } else if let Some(m) = &args.model {
        let trimmed = m.trim();
        if trimmed.is_empty() {
            return Err(error::refused("role_invalid", "empty role model"));
        }
        if args.start.is_none() && !role.start.iter().any(|a| a.contains("{model}")) {
            role.start = strip_flag(&role.start, MODEL_FLAGS);
        }
        role.model = Some(trimmed.to_owned());
    }

    if args.clear_effort {
        role.effort = None;
        if args.start.is_none() {
            role.start = strip_flag(&role.start, EFFORT_FLAGS);
        }
    } else if let Some(e) = &args.effort {
        let trimmed = e.trim();
        if trimmed.is_empty() {
            return Err(error::refused("role_invalid", "empty role effort"));
        }
        if args.start.is_none() && !role.start.iter().any(|a| a.contains("{effort}")) {
            role.start = strip_flag(&role.start, EFFORT_FLAGS);
        }
        role.effort = Some(trimmed.to_owned());
    }

    if let Some(body) = read_prompt(args.prompt.as_deref(), args.prompt_file.as_deref())? {
        role.body = body.trim().to_owned();
    }

    write_validated(paths, &role)
}

pub fn delete(paths: &CoordPaths, name: &str) -> Result<()> {
    if !valid_name(name) {
        return Err(error::refused(
            "invalid_role_name",
            format!("invalid role name: {name}"),
        ));
    }
    let path = paths.roles.join(format!("{name}.md"));
    no_links(&path, SymlinkMessage::WithPath)?;
    if builtin(name).is_some() {
        if path.exists() {
            return Err(error::refused(
                "cannot_delete_builtin",
                format!(
                    "role '{name}' is a built-in override; use 'roles reset {name}' to revert it to default"
                ),
            ));
        }
        return Err(error::refused(
            "cannot_delete_builtin",
            format!("built-in role '{name}' cannot be deleted"),
        ));
    }
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(error::refused(
            "role_not_found",
            format!("unknown role: {name}"),
        )),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

pub fn reset(paths: &CoordPaths, name: &str) -> Result<Role> {
    if !valid_name(name) {
        return Err(error::refused(
            "invalid_role_name",
            format!("invalid role name: {name}"),
        ));
    }
    let Some(default_text) = builtin(name) else {
        return Err(error::refused(
            "not_a_builtin",
            format!("role '{name}' is not a built-in; use 'roles delete {name}' to remove it"),
        ));
    };
    let path = paths.roles.join(format!("{name}.md"));
    no_links(&path, SymlinkMessage::WithPath)?;
    match fs::remove_file(&path) {
        Ok(()) => parse(name, default_text, RoleSource::Builtin),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(error::refused(
            "role_not_overridden",
            format!("built-in role '{name}' has no override to reset"),
        )),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

pub fn split_words(s: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = s.chars();
    let mut quote = None;
    let mut started = false;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (Some(q), c) if q == c => quote = None,
            (None, '\\') => {
                word.push(chars.next().context("trailing escape")?);
                started = true;
            }
            (Some('"'), '\\') => {
                let next = chars.next().context("trailing escape in double quote")?;
                if next != '"' && next != '\\' {
                    word.push('\\');
                }
                word.push(next);
            }
            _ => {
                word.push(c);
                started = true;
            }
        }
    }
    ensure!(quote.is_none(), "unterminated quote");
    if started {
        words.push(word);
    }
    Ok(words)
}

fn expand_one(arg: &str, vars: &Vars<'_>) -> Result<String> {
    let mut out = String::new();
    let mut rest = arg;
    while let Some(pos) = rest.find('{') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos + 1..];
        let Some(end) = rest.find('}') else {
            out.push('{');
            break;
        };
        let name = &rest[..end];
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            out.push('{');
            continue;
        }
        let value = match name {
            "brief" => Some(vars.brief.to_owned()),
            "session_dir" => vars.session_dir.map(|p| p.to_string_lossy().into_owned()),
            "session_id" => vars.session_id.map(str::to_owned),
            "repo_root" => vars.repo_root.map(|p| p.to_string_lossy().into_owned()),
            "cwd" => Some(vars.cwd.to_string_lossy().into_owned()),
            "cwd_json" => serde_json::to_string(&vars.cwd.to_string_lossy()).ok(),
            "model" => vars.model.map(str::to_owned),
            "effort" => vars.effort.map(str::to_owned),
            "computer_config" => vars
                .computer_config
                .map(|p| p.to_string_lossy().into_owned()),
            _ => bail!("unknown placeholder: {{{name}}}"),
        }
        .filter(|v| !v.is_empty())
        .with_context(|| format!("missing value for {{{name}}}"))?;
        out.push_str(&value);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

pub fn expand(
    template: &[String],
    vars: &Vars<'_>,
    trust: Option<&[String]>,
) -> Result<Vec<String>> {
    ensure!(!template.is_empty(), "empty command");
    let mut result = Vec::with_capacity(template.len() + trust.map_or(0, |t| t.len()));
    result.push(expand_one(&template[0], vars)?);
    ensure!(!result[0].is_empty(), "empty command");
    if let Some(trust) = trust {
        for arg in trust {
            result.push(expand_one(arg, vars)?);
        }
    }
    for arg in &template[1..] {
        result.push(expand_one(arg, vars)?);
    }
    Ok(result)
}

const MODEL_FLAGS: &[&str] = &["--model", "-m"];
const EFFORT_FLAGS: &[&str] = &["--effort", "--thinking"];

fn is_short_flag(flag: &str) -> bool {
    flag.starts_with('-') && !flag.starts_with("--") && flag.len() == 2
}

fn has_flag(argv: &[String], flags: &[&str]) -> bool {
    for arg in argv.iter().skip(1) {
        if arg == "--" {
            break;
        }
        if flags.contains(&arg.as_str())
            || flags.iter().any(|flag| {
                arg.strip_prefix(flag).is_some_and(|rest| {
                    rest.starts_with('=') || (is_short_flag(flag) && !rest.is_empty())
                })
            })
        {
            return true;
        }
    }
    false
}

fn flag_value(argv: &[String], flags: &[&str]) -> Option<String> {
    let mut iter = argv.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if flags.contains(&arg.as_str()) {
            return iter.next().filter(|v| !v.is_empty()).cloned();
        }
        for flag in flags {
            if let Some(rest) = arg.strip_prefix(flag) {
                let value = if let Some(v) = rest.strip_prefix('=') {
                    Some(v)
                } else if is_short_flag(flag) && !rest.is_empty() {
                    Some(rest)
                } else {
                    None
                };
                if let Some(value) = value.filter(|v| !v.is_empty()) {
                    return Some(value.to_owned());
                }
            }
        }
    }
    None
}

pub fn model_of(argv: &[String]) -> Option<String> {
    flag_value(argv, MODEL_FLAGS)
}

pub fn effort_of(argv: &[String]) -> Option<String> {
    flag_value(argv, EFFORT_FLAGS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtins_and_split() {
        for (name, text) in BUILTIN {
            let role = parse(name, text, RoleSource::Builtin).unwrap();
            assert!(role.effective_model().is_some(), "{name} missing model");
        }
        let trust = split_words(r#"-c 'projects={ "{repo_root}" = { trust_level = "trusted" } }'"#)
            .unwrap();
        assert_eq!(
            trust,
            [
                "-c",
                r#"projects={ "{repo_root}" = { trust_level = "trusted" } }"#
            ]
        );
        assert_eq!(
            split_words(r#"a\ b "" 'x y' "x\"y\\z""#).unwrap(),
            ["a b", "", "x y", "x\"y\\z"]
        );
        assert!(split_words("'bad").is_err());
        assert!(split_words("bad\\").is_err());
    }
    #[test]
    fn expansion_and_model() {
        let vars = Vars {
            brief: "hello world",
            session_dir: None,
            session_id: None,
            repo_root: Some(Path::new("/repo space")),
            cwd: Path::new("/cwd"),
            model: None,
            effort: None,
            computer_config: None,
        };
        let start = vec![
            "codex".into(),
            "-m".into(),
            "@task".into(),
            "msg={brief}".into(),
        ];
        let trust = split_words(r#"-c 'projects={ "{repo_root}" = { trust_level = "trusted" } }'"#)
            .unwrap();
        let result = expand(&start, &vars, Some(&trust)).unwrap();
        assert_eq!(
            result,
            [
                "codex",
                "-c",
                r#"projects={ "/repo space" = { trust_level = "trusted" } }"#,
                "-m",
                "@task",
                "msg=hello world"
            ]
        );
        assert_eq!(model_of(&result).as_deref(), Some("@task"));
        assert!(expand(&["x".into(), "{session_id}".into()], &vars, None).is_err());
        assert!(expand(&["x".into(), "{unknown}".into()], &vars, None).is_err());
        assert!(expand(&["x".into(), "{}".into()], &vars, None).is_err());
    }
    #[test]
    fn agy_and_claude_roles_expand_start_and_resume() {
        let vars = Vars {
            brief: "do the work",
            session_dir: Some(Path::new("/sessions/t1")),
            session_id: Some("11111111-2222-4333-8444-555555555555"),
            repo_root: Some(Path::new("/repo")),
            cwd: Path::new("/cwd"),
            model: None,
            effort: None,
            computer_config: Some(Path::new("/coord/omp-computer.yml")),
        };
        let agy = parse(
            "agy-default",
            builtin("agy-default").unwrap(),
            RoleSource::Builtin,
        )
        .unwrap();
        assert_eq!(agy.harness, "agy");
        assert_eq!(agy.session, SessionMode::AgyConversation);
        assert!(agy.resumable());
        assert!(agy.supports_steering());
        assert_eq!(
            agy.expand_start(&vars, false).unwrap(),
            [
                "agy",
                "--settings",
                r#"{"queuedMessages":"send-immediately"}"#,
                "--model",
                "gemini-4-neon-high",
                "-i",
                "do the work"
            ]
        );
        let trusted_start = agy.expand_start(&vars, true).unwrap();
        assert_eq!(trusted_start[0], "agy");
        assert_eq!(trusted_start[1], "--settings");
        assert_eq!(
            serde_json::from_str::<Value>(&trusted_start[2]).unwrap(),
            json!({"queuedMessages": "send-immediately", "trustedWorkspaces": ["/cwd"]})
        );
        assert_eq!(
            &trusted_start[3..],
            &["--model", "gemini-4-neon-high", "-i", "do the work"]
        );
        assert_eq!(
            agy.expand_resume(&vars, false).unwrap(),
            [
                "agy",
                "--settings",
                r#"{"queuedMessages":"send-immediately"}"#,
                "--conversation",
                "11111111-2222-4333-8444-555555555555",
                "-i",
                "do the work"
            ]
        );
        let trusted_resume = agy.expand_resume(&vars, true).unwrap();
        assert_eq!(trusted_resume[1], "--settings");
        assert_eq!(
            serde_json::from_str::<Value>(&trusted_resume[2]).unwrap(),
            json!({"queuedMessages": "send-immediately", "trustedWorkspaces": ["/cwd"]})
        );
        assert_eq!(
            &trusted_resume[3..],
            &[
                "--conversation",
                "11111111-2222-4333-8444-555555555555",
                "-i",
                "do the work"
            ]
        );
        let weird_cwd = Path::new(r#"/work/dir "quoted"\slash"#);
        let weird_vars = Vars {
            cwd: weird_cwd,
            ..vars
        };
        let weird_argv = agy.expand_start(&weird_vars, true).unwrap();
        assert_eq!(weird_argv[1], "--settings");
        let parsed: serde_json::Value = serde_json::from_str(&weird_argv[2]).unwrap();
        assert_eq!(
            parsed,
            serde_json::json!({
                "queuedMessages": "send-immediately",
                "trustedWorkspaces": [r#"/work/dir "quoted"\slash"#]
            })
        );

        let agy_no_steer = parse(
            "agy-no-steer",
            "---\nharness: agy\nmodel: gemini-4-neon-high\neffort: xhigh\nstart: agy -i {brief}\nresume: agy --conversation {session_id} -i {brief}\nsession: agy-conversation\nsteering: false\n---\nbody\n",
            RoleSource::Builtin,
        )
        .unwrap();
        assert!(!agy_no_steer.supports_steering());
        let argv = agy_no_steer.expand_start(&vars, false).unwrap();
        assert_eq!(
            argv,
            [
                "agy",
                "--model",
                "gemini-4-neon-high",
                "--effort",
                "xhigh",
                "-i",
                "do the work"
            ]
        );
        assert_eq!(model_of(&argv).as_deref(), Some("gemini-4-neon-high"));
        assert_eq!(effort_of(&argv).as_deref(), Some("xhigh"));

        let design = parse(
            "coord-design",
            builtin("coord-design").unwrap(),
            RoleSource::Builtin,
        )
        .unwrap();
        assert_eq!(design.harness, "agy");
        assert_eq!(design.effective_model().as_deref(), Some("gemini-4-neon-high"));
        assert!(design.supports_steering());

        let computer = parse(
            "coord-computer",
            builtin("coord-computer").unwrap(),
            RoleSource::Builtin,
        )
        .unwrap();
        assert!(computer.needs_computer_config());
        assert_eq!(
            computer.expand_start(&vars, false).unwrap(),
            [
                "omp",
                "--model",
                "@design",
                "--config",
                "/coord/omp-computer.yml",
                "--session-dir",
                "/sessions/t1",
                "do the work"
            ]
        );

        let opus = parse(
            "claude-opus",
            "---\nharness: claude\nmodel: opus\neffort: high\nstart: claude --session-id {session_id} {brief}\nresume: claude --resume {session_id} {brief}\nsession: preset\n---\nbody\n",
            RoleSource::Builtin,
        )
        .unwrap();
        assert_eq!(opus.harness, "claude");
        assert_eq!(opus.session, SessionMode::Preset);
        assert!(opus.resumable());
        assert!(!opus.supports_steering());
        assert_eq!(
            opus.expand_start(&vars, false).unwrap(),
            [
                "claude",
                "--model",
                "opus",
                "--effort",
                "high",
                "--session-id",
                "11111111-2222-4333-8444-555555555555",
                "do the work"
            ]
        );
        assert_eq!(
            opus.expand_resume(&vars, false).unwrap(),
            [
                "claude",
                "--resume",
                "11111111-2222-4333-8444-555555555555",
                "do the work"
            ]
        );

        let omp = parse(
            "coord-default",
            builtin("coord-default").unwrap(),
            RoleSource::Builtin,
        )
        .unwrap();
        assert!(omp.supports_steering());
        assert_eq!(
            omp.expand_start(&vars, false).unwrap(),
            [
                "omp",
                "--model",
                "@task",
                "--session-dir",
                "/sessions/t1",
                "do the work"
            ]
        );
        assert_eq!(
            omp.expand_resume(&vars, false).unwrap(),
            [
                "omp",
                "--session-dir",
                "/sessions/t1",
                "--resume",
                "11111111-2222-4333-8444-555555555555",
                "do the work"
            ]
        );
    }
    #[test]
    fn model_and_effort_validation_rejects_conflicts_and_bad_values() {
        assert!(
            parse(
                "dup-model",
                "---\nharness: agy\nmodel: gemini-4-neon-high\nstart: agy --model other -i {brief}\nsession: none\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert!(
            parse(
                "dup-model-equals",
                "---\nharness: claude\nmodel: opus\nstart: claude --model=haiku --session-id {session_id} {brief}\nsession: preset\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert!(
            parse(
                "dup-codex-compact-short",
                "---\nharness: codex\nmodel: gpt-6-luna\nstart: codex -mother-model {brief}\nsession: codex-rollout\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert!(
            parse(
                "dup-codex-short-equals",
                "---\nharness: codex\nmodel: gpt-6-luna\nstart: codex -m=other-model {brief}\nsession: codex-rollout\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        let codex_compact_only = parse(
            "codex-compact-only",
            "---\nharness: codex\nstart: codex -mgpt-6-luna {brief}\nsession: codex-rollout\n---\nbody\n",
            RoleSource::Builtin,
        )
        .unwrap();
        assert_eq!(
            codex_compact_only.effective_model().as_deref(),
            Some("gpt-6-luna")
        );
        let codex_after_sep = parse(
            "codex-after-sep",
            "---\nharness: codex\nmodel: gpt-6-luna\nstart: codex -- -mnot-a-flag {brief}\nsession: codex-rollout\n---\nbody\n",
            RoleSource::Builtin,
        )
        .unwrap();
        assert_eq!(
            codex_after_sep.effective_model().as_deref(),
            Some("gpt-6-luna")
        );
        let omp_models_flag = parse(
            "omp-models-flag",
            "---\nharness: omp\nmodel: @task\nstart: omp --models=a,b --session-dir {session_dir} {brief}\nsession: dir\n---\nbody\n",
            RoleSource::Builtin,
        )
        .unwrap();
        assert_eq!(omp_models_flag.effective_model().as_deref(), Some("@task"));
        assert!(
            parse(
                "dup-effort-equals",
                "---\nharness: claude\nmodel: opus\neffort: high\nstart: claude --effort=low --session-id {session_id} {brief}\nsession: preset\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert!(
            parse(
                "dup-thinking-equals",
                "---\nharness: omp\nmodel: @task\neffort: high\nstart: omp --thinking=low {brief}\nsession: none\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert!(
            parse(
                "resume-model-placeholder",
                "---\nharness: custom\nmodel: m1\nstart: custom --llm {model} {brief}\nresume: custom --llm {model} --resume {session_id} {brief}\nsession: preset\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert!(
            parse(
                "resume-effort-placeholder",
                "---\nharness: custom\neffort: deep\nstart: custom --lvl {effort} {brief}\nresume: custom --lvl {effort} --resume {session_id} {brief}\nsession: preset\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert_eq!(
            model_of(&["claude".into(), "--model=haiku".into()]).as_deref(),
            Some("haiku")
        );
        assert_eq!(
            model_of(&["codex".into(), "-m=gpt-6-luna".into()]).as_deref(),
            Some("gpt-6-luna")
        );
        assert_eq!(
            model_of(&["codex".into(), "-mgpt-6-luna".into()]).as_deref(),
            Some("gpt-6-luna")
        );
        assert_eq!(
            model_of(&["codex".into(), "--".into(), "-mgpt-6-luna".into()]),
            None
        );
        assert_eq!(model_of(&["omp".into(), "--models=a,b".into()]), None);
        assert_eq!(
            effort_of(&["claude".into(), "--effort=high".into()]).as_deref(),
            Some("high")
        );
        assert_eq!(
            effort_of(&["omp".into(), "--thinking=medium".into()]).as_deref(),
            Some("medium")
        );
        assert!(
            parse(
                "bad-effort",
                "---\nharness: claude\nmodel: opus\neffort: turbo\nstart: claude --session-id {session_id} {brief}\nsession: preset\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert!(
            parse(
                "unsupported-effort",
                "---\nharness: codex\nmodel: gpt-6-luna\neffort: high\nstart: codex {brief}\nsession: codex-rollout\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        assert!(
            parse(
                "custom-without-placeholder",
                "---\nharness: custom\nmodel: m1\nstart: custom {brief}\nsession: none\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        let custom = parse(
            "custom-with-placeholder",
            "---\nharness: custom\nmodel: m1\neffort: deep\nstart: custom --llm {model} --lvl {effort} {brief}\nsession: none\n---\nbody\n",
            RoleSource::Builtin,
        )
        .unwrap();
        let vars = Vars {
            brief: "hi",
            session_dir: None,
            session_id: None,
            repo_root: None,
            cwd: Path::new("/cwd"),
            model: None,
            effort: None,
            computer_config: None,
        };
        assert_eq!(
            custom.expand_start(&vars, false).unwrap(),
            ["custom", "--llm", "m1", "--lvl", "deep", "hi"]
        );
    }
    #[test]
    fn override_wins() {
        let dir = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(dir.path());
        fs::create_dir_all(&paths.roles).unwrap();
        fs::write(
            paths.roles.join("coord-main.md"),
            "---\nharness: custom\nstart: custom {brief}\nsession: none\n---\nbody\n",
        )
        .unwrap();
        let role = load(&paths, "coord-main").unwrap();
        assert_eq!(role.harness, "custom");
        assert!(matches!(role.source, RoleSource::File(_)));
        assert!(
            list(&paths)
                .unwrap()
                .iter()
                .any(|(n, _, over)| n == "coord-main" && *over)
        );
        assert!(load(&paths, "../escape").is_err());
    }

    #[test]
    fn format_roundtrip_and_placeholder_validation() {
        for (name, text) in BUILTIN {
            let role = parse(name, text, RoleSource::Builtin).unwrap();
            let formatted = format_markdown(&role).unwrap();
            let reparsed = parse(name, &formatted, RoleSource::Builtin).unwrap();
            assert_eq!(reparsed.harness, role.harness);
            assert_eq!(reparsed.model, role.model);
            assert_eq!(reparsed.effort, role.effort);
            assert_eq!(reparsed.start, role.start);
            assert_eq!(reparsed.resume, role.resume);
            assert_eq!(reparsed.session, role.session);
            assert_eq!(reparsed.trust, role.trust);
            assert_eq!(reparsed.body, role.body);
        }
        // Missing {brief} in start
        assert!(
            parse(
                "no-brief",
                "---\nharness: omp\nstart: omp --session-dir {session_dir}\nsession: dir\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        // Missing {session_dir} for dir mode
        assert!(
            parse(
                "no-session-dir",
                "---\nharness: omp\nstart: omp {brief}\nsession: dir\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        // Missing {session_id} for preset mode
        assert!(
            parse(
                "no-session-id",
                "---\nharness: claude\nstart: claude {brief}\nsession: preset\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        // Unknown placeholder
        assert!(
            parse(
                "bad-ph",
                "---\nharness: agy\nstart: agy {bogus} {brief}\nsession: none\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        // Resume missing {session_id}
        assert!(
            parse(
                "bad-resume",
                "---\nharness: agy\nstart: agy -i {brief}\nresume: agy -i {brief}\nsession: agy-conversation\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        // {model} placeholder without model field
        assert!(
            parse(
                "missing-model-val",
                "---\nharness: custom\nstart: custom --llm {model} {brief}\nsession: none\n---\nbody\n",
                RoleSource::Builtin,
            )
            .is_err()
        );
        // Inline comment delimiter rejected by format_markdown
        let bad_role = Role {
            name: "bad-comment".into(),
            harness: "omp".into(),
            model: Some("foo # bar".into()),
            effort: None,
            start: vec![
                "omp".into(),
                "--session-dir".into(),
                "{session_dir}".into(),
                "{brief}".into(),
            ],
            resume: None,
            session: SessionMode::Dir,
            trust: Vec::new(),
            steering: None,
            body: "body".into(),
            source: RoleSource::Builtin,
        };
        assert!(format_markdown(&bad_role).is_err());
    }
}
