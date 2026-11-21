use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

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
}

pub const BUILTIN: [(&str, &str); 10] = [
    ("coord-main", include_str!("roles/coord-main.md")),
    ("coord-light", include_str!("roles/coord-light.md")),
    ("coord-default", include_str!("roles/coord-default.md")),
    ("coord-deep", include_str!("roles/coord-deep.md")),
    ("coord-design", include_str!("roles/coord-design.md")),
    ("agy-default", include_str!("roles/agy-default.md")),
    ("claude-default", include_str!("roles/claude-default.md")),
    ("claude-opus", include_str!("roles/claude-opus.md")),
    ("codex-default", include_str!("roles/codex-default.md")),
    ("copilot-default", include_str!("roles/copilot-default.md")),
];

pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
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
    harness == "omp"
}

pub fn app_supports_steering(app: Option<&str>) -> bool {
    app == Some("omp")
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
        harness_supports_steering(&self.harness)
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
        expand(
            &self.start,
            &effective,
            (!injected.is_empty()).then_some(injected.as_slice()),
        )
    }

    pub fn expand_resume(&self, vars: &Vars<'_>, trusted: bool) -> Result<Vec<String>> {
        let template = self
            .resume
            .as_deref()
            .context("role has no resume command")?;
        let effective = Vars {
            model: vars.model.or(self.model.as_deref()),
            effort: vars.effort.or(self.effort.as_deref()),
            ..*vars
        };
        expand(
            template,
            &effective,
            trusted.then_some(self.trust.as_slice()),
        )
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
                "harness" | "model" | "effort" | "start" | "resume" | "session" | "trust"
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
    }
    let trust = fields
        .get("trust")
        .map(|s| split_words(s))
        .transpose()?
        .unwrap_or_default();
    let session = match required("session")? {
        "dir" => SessionMode::Dir,
        "preset" => SessionMode::Preset,
        "codex-rollout" => SessionMode::CodexRollout,
        "agy-conversation" => SessionMode::AgyConversation,
        "none" => SessionMode::None,
        value => bail!("unknown session mode: {value}"),
    };
    let has_model_placeholder = start.iter().any(|arg| arg.contains("{model}"));
    let has_effort_placeholder = start.iter().any(|arg| arg.contains("{effort}"));
    if model.is_some() && !has_model_placeholder {
        ensure!(
            model_of(&start).is_none(),
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
                effort_of(&start).is_none(),
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
        body: lines.collect::<String>().trim().to_owned(),
        source,
    })
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
            "model" => vars.model.map(str::to_owned),
            "effort" => vars.effort.map(str::to_owned),
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

pub fn model_of(argv: &[String]) -> Option<String> {
    argv.windows(2)
        .find(|pair| pair[0] == "--model" || pair[0] == "-m")
        .map(|pair| pair[1].clone())
}

pub fn effort_of(argv: &[String]) -> Option<String> {
    argv.windows(2)
        .find(|pair| pair[0] == "--effort" || pair[0] == "--thinking")
        .map(|pair| pair[1].clone())
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
        assert!(!agy.supports_steering());
        assert_eq!(
            agy.expand_start(&vars, false).unwrap(),
            ["agy", "--model", "gemini-4-neon-high", "-i", "do the work"]
        );
        assert_eq!(
            agy.expand_resume(&vars, false).unwrap(),
            [
                "agy",
                "--conversation",
                "11111111-2222-4333-8444-555555555555",
                "-i",
                "do the work"
            ]
        );

        let agy_effort = parse(
            "agy-xhigh",
            "---\nharness: agy\nmodel: gemini-4-neon-high\neffort: xhigh\nstart: agy -i {brief}\nresume: agy --conversation {session_id} -i {brief}\nsession: agy-conversation\n---\nbody\n",
            RoleSource::Builtin,
        )
        .unwrap();
        let argv = agy_effort.expand_start(&vars, false).unwrap();
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

        let opus = parse(
            "claude-opus",
            builtin("claude-opus").unwrap(),
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

        let sonnet = parse(
            "claude-default",
            builtin("claude-default").unwrap(),
            RoleSource::Builtin,
        )
        .unwrap();
        assert_eq!(
            sonnet.expand_start(&vars, false).unwrap(),
            [
                "claude",
                "--model",
                "sonnet",
                "--session-id",
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
}
