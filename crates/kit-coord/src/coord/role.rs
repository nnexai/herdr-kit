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
}

pub const BUILTIN: [(&str, &str); 8] = [
    ("coord-main", include_str!("roles/coord-main.md")),
    ("coord-light", include_str!("roles/coord-light.md")),
    ("coord-default", include_str!("roles/coord-default.md")),
    ("coord-deep", include_str!("roles/coord-deep.md")),
    ("coord-design", include_str!("roles/coord-design.md")),
    ("codex-default", include_str!("roles/codex-default.md")),
    ("claude-default", include_str!("roles/claude-default.md")),
    ("copilot-default", include_str!("roles/copilot-default.md")),
];

pub fn builtin(name: &str) -> Option<&'static str> {
    BUILTIN
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
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
            matches!(key, "harness" | "start" | "resume" | "session" | "trust"),
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
    let harness = required("harness")?.to_owned();
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
        "none" => SessionMode::None,
        value => bail!("unknown session mode: {value}"),
    };
    Ok(Role {
        name: name.into(),
        harness,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtins_and_split() {
        for (name, text) in BUILTIN {
            parse(name, text, RoleSource::Builtin).unwrap();
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

