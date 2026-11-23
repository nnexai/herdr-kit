//! Report attachments: bounded atomic local staging, lazy availability checks,
//! inert plain-text slicing, bounded raster image preview validation, and explicit cleanup.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail, ensure};
use kit_core::paths::{SymlinkMessage, no_links};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Ctx;
use super::caller::Caller;
use super::error;
use super::ids::{TaskId, WsId};
use super::kinds::{AttachmentKind, AttachmentMode, AttachmentRef, Kind};
use super::paths::CoordPaths;
use super::state::{AttachmentRec, State, WsState};
use super::stats::parse_since;

pub const MAX_ATTACHMENTS_PER_REPORT: usize = 8;
pub const MAX_STAGE_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_STAGE_WS_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_STAGE_TOTAL_BYTES: u64 = 500 * 1024 * 1024;
pub const MAX_READ_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_PREVIEW_DIM: u32 = 4096;
pub const MAX_PREVIEW_PIXELS: u64 = 12_000_000;

#[derive(Debug, Default)]
pub struct StagedReport {
    pub attachments: Vec<AttachmentRef>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
}

impl RasterFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
            Self::Gif => "gif",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Gif => "gif",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterInfo {
    pub format: RasterFormat,
    pub width: u32,
    pub height: u32,
}

pub fn classify_kind(name: &str) -> AttachmentKind {
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "webp" | "gif") => AttachmentKind::Image,
        Some("html" | "htm") => AttachmentKind::Html,
        Some("log") => AttachmentKind::Log,
        Some("md" | "markdown") => AttachmentKind::Markdown,
        Some(
            "txt" | "diff" | "patch" | "csv" | "json" | "toml" | "yaml" | "yml" | "rs" | "js"
            | "ts" | "mjs" | "cjs" | "py" | "sh" | "bash" | "zsh" | "svg" | "xml" | "css" | "jsonl",
        ) => AttachmentKind::Text,
        _ => AttachmentKind::File,
    }
}

pub fn sanitize_filename(name: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
            prev_sep = false;
        } else if !prev_sep && !out.is_empty() {
            out.push('_');
            prev_sep = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '.' || c == '_');
    let mut clean = if trimmed.is_empty() {
        "attachment".to_owned()
    } else {
        trimmed.to_owned()
    };
    if clean.len() > 64 {
        // Keep extension if present and short.
        if let Some(dot) = clean.rfind('.') {
            let ext = &clean[dot..];
            if ext.len() <= 12 && dot > 0 {
                let keep = 64 - ext.len();
                clean = format!("{}{}", &clean[..keep], ext);
            } else {
                clean.truncate(64);
            }
        } else {
            clean.truncate(64);
        }
    }
    clean
}

fn lexically_normalize(path: &Path) -> Result<PathBuf> {
    let raw = path.to_string_lossy();
    ensure!(!raw.is_empty(), "attachment path is empty");
    ensure!(!raw.contains('\0'), "attachment path contains NUL");
    ensure!(
        !raw.starts_with("file://"),
        "file:// URLs are not accepted as attachment paths"
    );
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("current directory unavailable")?
            .join(path)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    bail!("attachment path escapes root");
                }
            }
            Component::Normal(s) => out.push(s),
        }
    }
    ensure!(
        out.is_absolute(),
        "attachment path must resolve to absolute"
    );
    Ok(out)
}

fn is_sensitive_path(path: &Path, coord_paths: &CoordPaths) -> bool {
    let s = path.to_string_lossy();
    for sys in ["/etc", "/proc", "/sys", "/dev"] {
        if path == Path::new(sys) || path.starts_with(sys) {
            return true;
        }
    }
    for managed in [
        &coord_paths.sessions,
        &coord_paths.journal,
        &coord_paths.artifacts,
        &coord_paths.lock,
        &coord_paths.up_lock,
        &coord_paths.watcher_lock,
    ] {
        if path == managed || path.starts_with(managed) {
            return true;
        }
        if let Ok(canon) = fs::canonicalize(managed) {
            if path == canon || path.starts_with(&canon) {
                return true;
            }
        }
    }
    if let Ok(home) = kit_core::paths::home() {
        let mut homes = vec![home.clone()];
        if let Ok(canon) = fs::canonicalize(&home) {
            if canon != home {
                homes.push(canon);
            }
        }
        for h in homes {
            for rel in [
                ".ssh",
                ".gnupg",
                ".aws",
                ".kube",
                ".config/gcloud",
                ".config/gh",
                ".netrc",
                ".npmrc",
                ".pypirc",
            ] {
                let p = h.join(rel);
                if path == p || path.starts_with(&p) {
                    return true;
                }
            }
        }
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let lower = name.to_ascii_lowercase();
        if lower == ".env"
            || lower.starts_with(".env.")
            || lower.starts_with("id_rsa")
            || lower.starts_with("id_ed25519")
            || lower.contains("credentials")
            || lower.ends_with(".pem")
            || lower.ends_with(".key")
            || lower.ends_with(".p12")
            || lower.ends_with(".pfx")
        {
            return true;
        }
    }
    s.contains("/.ssh/") || s.contains("/.gnupg/") || s.contains("/.aws/")
}

struct ValidatedLocal {
    normalized: PathBuf,
    canonical_file: PathBuf,
    label: String,
    bytes: u64,
}

fn validate_local_regular_file(raw: &Path, coord_paths: &CoordPaths) -> Result<ValidatedLocal> {
    let normalized = lexically_normalize(raw)?;
    if is_sensitive_path(&normalized, coord_paths) {
        return Err(error::refused(
            "unsafe_attachment_path",
            format!("refusing sensitive attachment path: {}", raw.display()),
        ));
    }
    let meta = fs::symlink_metadata(&normalized).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            error::refused(
                "attachment_not_found",
                format!("attachment not found: {}", raw.display()),
            )
        } else {
            anyhow::Error::from(e)
        }
    })?;
    if meta.file_type().is_symlink() {
        return Err(error::refused(
            "symlink_attachment",
            format!("refusing symlink attachment: {}", raw.display()),
        ));
    }
    if !meta.file_type().is_file() {
        return Err(error::refused(
            "non_regular_attachment",
            format!("attachment must be a regular file: {}", raw.display()),
        ));
    }
    let parent = normalized
        .parent()
        .context("attachment path has no parent")?;
    let file_name = normalized
        .file_name()
        .context("attachment path has no file name")?;
    let canon_parent = fs::canonicalize(parent)?;
    let canonical_file = canon_parent.join(file_name);
    if is_sensitive_path(&canonical_file, coord_paths) {
        return Err(error::refused(
            "unsafe_attachment_path",
            format!("refusing sensitive attachment path: {}", raw.display()),
        ));
    }
    let label = file_name.to_string_lossy().into_owned();
    Ok(ValidatedLocal {
        normalized,
        canonical_file,
        label,
        bytes: meta.len(),
    })
}

fn dir_bytes(root: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.file_type().is_dir() {
            total = total.saturating_add(dir_bytes(&path));
        } else if meta.file_type().is_file() {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

fn ensure_dir_private(path: &Path) -> Result<()> {
    no_links(path, SymlinkMessage::WithPath)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    no_links(path, SymlinkMessage::WithPath)?;
    ensure!(
        fs::metadata(path)?.is_dir(),
        "expected directory: {}",
        path.display()
    );
    Ok(())
}

/// Validate and stage attachments for a single delegate `report` call.
/// Oversized or over-budget `--attach` files degrade cleanly to `mode: "ref"` with a warning.
pub fn stage_for_report(
    paths: &CoordPaths,
    ws: &WsId,
    task: &TaskId,
    starting_index: usize,
    attach: &[PathBuf],
    attach_ref: &[PathBuf],
    remote_machine: Option<&str>,
) -> Result<StagedReport> {
    let total_requested = attach.len().saturating_add(attach_ref.len());
    if total_requested == 0 {
        return Ok(StagedReport::default());
    }
    if total_requested > MAX_ATTACHMENTS_PER_REPORT {
        return Err(error::refused(
            "too_many_attachments",
            format!("at most {MAX_ATTACHMENTS_PER_REPORT} attachments are allowed per report"),
        ));
    }

    paths.ensure()?;
    let mut ws_used = dir_bytes(&paths.artifacts_ws(ws));
    let mut total_used = dir_bytes(&paths.artifacts);
    let mut out = StagedReport::default();
    let mut idx = starting_index;

    for raw in attach {
        let id = format!("{}-a{idx}", task.as_str());
        idx += 1;
        if let Some(machine) = remote_machine {
            let raw_str = raw.to_string_lossy().into_owned();
            ensure!(
                !raw_str.is_empty() && !raw_str.contains('\0') && !raw_str.starts_with("file://"),
                "invalid remote attachment path"
            );
            let label = raw
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "attachment".into());
            let kind = classify_kind(&label);
            out.attachments.push(AttachmentRef {
                id,
                label,
                kind,
                mode: AttachmentMode::Ref,
                stored_path: None,
                source_path: raw_str,
                bytes: None,
                sha256_12: None,
                reason: Some("remote_host".into()),
                machine: Some(machine.to_owned()),
            });
            continue;
        }

        let valid = validate_local_regular_file(raw, paths)?;
        let kind = classify_kind(&valid.label);
        let source_display = valid.canonical_file.to_string_lossy().into_owned();

        if valid.bytes > MAX_STAGE_FILE_BYTES {
            out.warnings.push(format!(
                "{} exceeds 10 MiB stage limit ({} bytes); recorded as reference only",
                valid.label, valid.bytes
            ));
            out.attachments.push(AttachmentRef {
                id,
                label: valid.label,
                kind,
                mode: AttachmentMode::Ref,
                stored_path: None,
                source_path: source_display,
                bytes: Some(valid.bytes),
                sha256_12: None,
                reason: Some("exceeds_stage_limit".into()),
                machine: None,
            });
            continue;
        }
        if ws_used.saturating_add(valid.bytes) > MAX_STAGE_WS_BYTES
            || total_used.saturating_add(valid.bytes) > MAX_STAGE_TOTAL_BYTES
        {
            out.warnings.push(format!(
                "{} exceeds staging storage budget; recorded as reference only",
                valid.label
            ));
            out.attachments.push(AttachmentRef {
                id,
                label: valid.label,
                kind,
                mode: AttachmentMode::Ref,
                stored_path: None,
                source_path: source_display,
                bytes: Some(valid.bytes),
                sha256_12: None,
                reason: Some("budget_exceeded".into()),
                machine: None,
            });
            continue;
        }

        let task_dir = paths.artifacts_task(ws, task);
        ensure_dir_private(&paths.artifacts)?;
        ensure_dir_private(&paths.artifacts_ws(ws))?;
        ensure_dir_private(&task_dir)?;

        let clean = sanitize_filename(&valid.label);
        let file_name = format!("a{}-{clean}", idx - 1);
        let target = task_dir.join(&file_name);
        no_links(&target, SymlinkMessage::WithPath)?;

        let mut src = File::open(&valid.normalized)?;
        // Verify fd is still a regular file and copy bounded bytes while hashing.
        ensure!(
            src.metadata()?.file_type().is_file(),
            "attachment source is not a regular file"
        );
        let mut tmp = tempfile::NamedTempFile::new_in(&task_dir)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 32 * 1024];
        let mut copied = 0u64;
        let mut overflowed = false;
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            copied = copied.saturating_add(n as u64);
            if copied > MAX_STAGE_FILE_BYTES
                || ws_used.saturating_add(copied) > MAX_STAGE_WS_BYTES
                || total_used.saturating_add(copied) > MAX_STAGE_TOTAL_BYTES
            {
                overflowed = true;
                break;
            }
            hasher.update(&buf[..n]);
            tmp.write_all(&buf[..n])?;
        }
        if overflowed {
            drop(tmp);
            out.warnings.push(format!(
                "{} grew past staging limit while copying; recorded as reference only",
                valid.label
            ));
            out.attachments.push(AttachmentRef {
                id,
                label: valid.label,
                kind,
                mode: AttachmentMode::Ref,
                stored_path: None,
                source_path: source_display,
                bytes: Some(copied),
                sha256_12: None,
                reason: Some("exceeds_stage_limit".into()),
                machine: None,
            });
            continue;
        }
        tmp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        tmp.as_file().sync_all()?;
        tmp.persist_noclobber(&target).map_err(|e| e.error)?;
        File::open(&task_dir)?.sync_all()?;

        let digest = hasher.finalize();
        let sha256_12: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
        ws_used = ws_used.saturating_add(copied);
        total_used = total_used.saturating_add(copied);

        let stored_rel = format!("{}/{}/{}", ws.as_str(), task.as_str(), file_name);
        out.attachments.push(AttachmentRef {
            id,
            label: valid.label,
            kind,
            mode: AttachmentMode::Staged,
            stored_path: Some(stored_rel),
            source_path: source_display,
            bytes: Some(copied),
            sha256_12: Some(sha256_12),
            reason: None,
            machine: None,
        });
    }

    for raw in attach_ref {
        let id = format!("{}-a{idx}", task.as_str());
        idx += 1;
        if let Some(machine) = remote_machine {
            let raw_str = raw.to_string_lossy().into_owned();
            ensure!(
                !raw_str.is_empty() && !raw_str.contains('\0') && !raw_str.starts_with("file://"),
                "invalid remote attachment path"
            );
            let label = raw
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "attachment".into());
            let kind = classify_kind(&label);
            out.attachments.push(AttachmentRef {
                id,
                label,
                kind,
                mode: AttachmentMode::Ref,
                stored_path: None,
                source_path: raw_str,
                bytes: None,
                sha256_12: None,
                reason: Some("remote_host".into()),
                machine: Some(machine.to_owned()),
            });
            continue;
        }
        let valid = validate_local_regular_file(raw, paths)?;
        let kind = classify_kind(&valid.label);
        out.attachments.push(AttachmentRef {
            id,
            label: valid.label,
            kind,
            mode: AttachmentMode::Ref,
            stored_path: None,
            source_path: valid.canonical_file.to_string_lossy().into_owned(),
            bytes: Some(valid.bytes),
            sha256_12: None,
            reason: Some("explicit_ref".into()),
            machine: None,
        });
    }

    Ok(out)
}

#[derive(Clone, Debug)]
struct ResolvedAttachment<'a> {
    ws: &'a WsId,
    task: &'a TaskId,
    task_title: &'a str,
    rec: &'a AttachmentRec,
}

fn find_attachment<'a>(state: &'a State, id: &str) -> Result<ResolvedAttachment<'a>> {
    validate_attachment_id(id)?;
    for task in state.tasks.values() {
        if let Some(rec) = task.attachments.iter().find(|a| a.id == id) {
            return Ok(ResolvedAttachment {
                ws: &task.ws,
                task: &task.id,
                task_title: &task.title,
                rec,
            });
        }
    }
    Err(error::refused(
        "artifact_not_found",
        format!("unknown attachment: {id}"),
    ))
}

pub fn validate_attachment_id(id: &str) -> Result<()> {
    let Some((task_part, suffix)) = id.rsplit_once("-a") else {
        return Err(error::refused(
            "invalid_artifact_id",
            format!("invalid attachment id: {id}"),
        ));
    };
    TaskId::parse(task_part).map_err(|_| {
        error::refused(
            "invalid_artifact_id",
            format!("invalid attachment id: {id}"),
        )
    })?;
    if suffix.is_empty()
        || suffix.starts_with('0')
        || !suffix.bytes().all(|b| b.is_ascii_digit())
        || suffix.parse::<u32>().is_err()
    {
        return Err(error::refused(
            "invalid_artifact_id",
            format!("invalid attachment id: {id}"),
        ));
    }
    Ok(())
}

fn resolve_stored_path(paths: &CoordPaths, rel: &str) -> Result<PathBuf> {
    let p = Path::new(rel);
    ensure!(!rel.is_empty() && !p.is_absolute(), "invalid stored path");
    let mut parts = 0usize;
    for comp in p.components() {
        match comp {
            Component::Normal(_) => parts += 1,
            _ => bail!("unsafe stored path component"),
        }
    }
    ensure!(parts == 3, "invalid stored path structure");
    let full = paths.artifacts.join(p);
    no_links(&full, SymlinkMessage::WithPath)?;
    Ok(full)
}

#[derive(Clone, Debug)]
enum Availability {
    Available(PathBuf, u64),
    Missing(Option<PathBuf>),
    Pruned,
    Remote,
    Unsafe(String),
}

impl Availability {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Available(_, _) => "available",
            Self::Missing(_) => "missing",
            Self::Pruned => "pruned",
            Self::Remote => "remote",
            Self::Unsafe(_) => "unsafe",
        }
    }
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Available(p, _) => Some(p.as_path()),
            Self::Missing(p) => p.as_deref(),
            _ => None,
        }
    }
}

fn check_availability(paths: &CoordPaths, rec: &AttachmentRec) -> Availability {
    if rec.pruned {
        return Availability::Pruned;
    }
    if rec.machine.is_some() {
        return Availability::Remote;
    }
    match rec.mode {
        AttachmentMode::Staged => {
            let Some(rel) = rec.stored_path.as_deref() else {
                return Availability::Missing(None);
            };
            let full = match resolve_stored_path(paths, rel) {
                Ok(p) => p,
                Err(e) => return Availability::Unsafe(e.to_string()),
            };
            match fs::symlink_metadata(&full) {
                Ok(m) if m.file_type().is_symlink() => {
                    Availability::Unsafe("staged file is a symlink".into())
                }
                Ok(m) if m.file_type().is_file() => Availability::Available(full, m.len()),
                Ok(_) => Availability::Unsafe("staged path is not a regular file".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Availability::Missing(Some(full))
                }
                Err(e) => Availability::Unsafe(e.to_string()),
            }
        }
        AttachmentMode::Ref => {
            let raw = Path::new(&rec.source_path);
            if !raw.is_absolute()
                || rec.source_path.starts_with("file://")
                || rec.source_path.contains('\0')
            {
                return Availability::Unsafe("invalid reference path".into());
            }
            if is_sensitive_path(raw, paths) {
                return Availability::Unsafe("sensitive reference path refused".into());
            }
            match fs::symlink_metadata(raw) {
                Ok(m) if m.file_type().is_symlink() => {
                    Availability::Unsafe("reference path is a symlink".into())
                }
                Ok(m) if m.file_type().is_file() => {
                    let Some(parent) = raw.parent() else {
                        return Availability::Unsafe("invalid reference parent".into());
                    };
                    let Some(name) = raw.file_name() else {
                        return Availability::Unsafe("invalid reference file name".into());
                    };
                    match fs::canonicalize(parent) {
                        Ok(canon_parent) => {
                            let canon = canon_parent.join(name);
                            if is_sensitive_path(&canon, paths) {
                                Availability::Unsafe("sensitive reference path refused".into())
                            } else {
                                Availability::Available(raw.to_path_buf(), m.len())
                            }
                        }
                        Err(e) => Availability::Unsafe(e.to_string()),
                    }
                }
                Ok(_) => Availability::Unsafe("reference path is not a regular file".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Availability::Missing(Some(raw.to_path_buf()))
                }
                Err(e) => Availability::Unsafe(e.to_string()),
            }
        }
    }
}

fn require_available_file(paths: &CoordPaths, rec: &AttachmentRec) -> Result<(PathBuf, u64)> {
    match check_availability(paths, rec) {
        Availability::Available(p, len) => Ok((p, len)),
        Availability::Missing(_) => Err(error::refused(
            "artifact_missing",
            format!("attachment is missing on disk: {}", rec.label),
        )),
        Availability::Pruned => Err(error::refused(
            "artifact_pruned",
            format!("attachment was pruned: {}", rec.label),
        )),
        Availability::Remote => Err(error::refused(
            "artifact_remote",
            format!("attachment lives on a remote machine: {}", rec.label),
        )),
        Availability::Unsafe(msg) => Err(error::refused("unsafe_attachment_path", msg)),
    }
}

pub fn list(ctx: &Ctx, ws_filter: Option<&str>, task_filter: Option<&str>) -> Result<Value> {
    let task_id = task_filter.map(TaskId::parse).transpose()?;
    let ws_id = ws_filter
        .map(WsId::parse)
        .transpose()?
        .or_else(|| task_id.as_ref().map(TaskId::ws));
    if let (Some(w), Some(t)) = (&ws_id, &task_id) {
        if *w != t.ws() {
            return Err(error::refused(
                "invalid_scope",
                "task does not belong to workstream",
            ));
        }
    }
    let state = ctx.journal()?.read_state()?;
    let mut items = Vec::new();
    for task in state.tasks.values() {
        if let Some(ref w) = ws_id {
            if &task.ws != w {
                continue;
            }
        }
        if let Some(ref t) = task_id {
            if &task.id != t {
                continue;
            }
        }
        let ws_state = state
            .workstreams
            .get(&task.ws)
            .map(|w| w.state)
            .unwrap_or(WsState::Open);
        for rec in &task.attachments {
            let avail = check_availability(&ctx.paths, rec);
            let live_bytes = match avail {
                Availability::Available(_, len) => Some(len),
                _ => rec.bytes,
            };
            let resolved = avail
                .path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| rec.source_path.clone());
            let unsafe_reason = match &avail {
                Availability::Unsafe(msg) => Some(msg.as_str()),
                _ => None,
            };
            items.push(json!({
                "id": rec.id,
                "ws": task.ws,
                "ws_state": ws_state,
                "task": task.id,
                "task_title": task.title,
                "role": task.role,
                "label": rec.label,
                "kind": rec.kind,
                "mode": rec.mode,
                "availability": avail.as_str(),
                "stored_path": rec.stored_path,
                "source_path": rec.source_path,
                "resolved_path": resolved,
                "bytes": live_bytes,
                "sha256_12": rec.sha256_12,
                "reason": rec.reason,
                "machine": rec.machine,
                "report_seq": rec.report_seq,
                "report_kind": rec.report_kind,
                "ts_ms": rec.ts_ms,
                "pruned": rec.pruned,
                "pruned_reason": rec.pruned_reason,
                "unsafe_reason": unsafe_reason,
            }));
        }
    }
    items.sort_by_key(|item| {
        (
            item["ts_ms"].as_u64().unwrap_or(0),
            item["report_seq"].as_u64().unwrap_or(0),
            item["id"].as_str().unwrap_or("").to_owned(),
        )
    });
    Ok(json!({
        "schema_version": 1,
        "generated_at_ms": ctx.now(),
        "ws": ws_id,
        "task": task_id,
        "attachments": items,
    }))
}

pub fn sanitize_plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    // CSI sequence: parameter bytes 0x30-0x3f, intermediate 0x20-0x2f, final 0x40-0x7e.
                    while let Some(c) = chars.next() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC sequence: terminated by BEL (\x07) or ST (\x1b\\).
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        match ch {
            '\n' => out.push('\n'),
            '\r' => {}
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

pub fn read_text(ctx: &Ctx, id: &str, max_bytes: usize) -> Result<Value> {
    let state = ctx.journal()?.read_state()?;
    let found = find_attachment(&state, id)?;
    if !matches!(
        found.rec.kind,
        AttachmentKind::Log
            | AttachmentKind::Text
            | AttachmentKind::Markdown
            | AttachmentKind::Html
    ) {
        return Err(error::refused(
            "not_text_attachment",
            format!("{} cannot be viewed as plain text", found.rec.label),
        ));
    }
    let (path, total_bytes) = require_available_file(&ctx.paths, found.rec)?;
    let cap = max_bytes.clamp(1, MAX_READ_TEXT_BYTES);
    let mut buf = Vec::with_capacity(cap.min(total_bytes as usize) + 1);
    File::open(&path)?
        .take((cap + 1) as u64)
        .read_to_end(&mut buf)?;
    let truncated = buf.len() > cap || total_bytes > cap as u64;
    if buf.len() > cap {
        buf.truncate(cap);
    }
    if buf.contains(&0) {
        return Err(error::refused(
            "binary_content",
            format!("{} appears to contain binary data", found.rec.label),
        ));
    }
    let decoded = String::from_utf8_lossy(&buf);
    let text = sanitize_plain_text(&decoded);
    Ok(json!({
        "schema_version": 1,
        "id": found.rec.id,
        "ws": found.ws,
        "task": found.task,
        "task_title": found.task_title,
        "label": found.rec.label,
        "kind": found.rec.kind,
        "mode": found.rec.mode,
        "report_kind": found.rec.report_kind,
        "ts_ms": found.rec.ts_ms,
        "resolved_path": path.to_string_lossy(),
        "total_bytes": total_bytes,
        "shown_bytes": buf.len(),
        "cap_bytes": cap,
        "truncated": truncated,
        "text": text,
    }))
}

pub fn detect_raster_dimensions(bytes: &[u8]) -> Option<RasterInfo> {
    // PNG: 8-byte signature + IHDR chunk
    if bytes.len() >= 24 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" && &bytes[12..16] == b"IHDR" {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return (w > 0 && h > 0).then_some(RasterInfo {
            format: RasterFormat::Png,
            width: w,
            height: h,
        });
    }
    // GIF87a / GIF89a
    if bytes.len() >= 10 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        let w = u32::from(u16::from_le_bytes([bytes[6], bytes[7]]));
        let h = u32::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        return (w > 0 && h > 0).then_some(RasterInfo {
            format: RasterFormat::Gif,
            width: w,
            height: h,
        });
    }
    // WebP: RIFF <size> WEBP <chunk>
    if bytes.len() >= 30 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let chunk = &bytes[12..16];
        if chunk == b"VP8X" && bytes.len() >= 30 {
            let w = 1
                + (u32::from(bytes[24])
                    | (u32::from(bytes[25]) << 8)
                    | (u32::from(bytes[26]) << 16));
            let h = 1
                + (u32::from(bytes[27])
                    | (u32::from(bytes[28]) << 8)
                    | (u32::from(bytes[29]) << 16));
            return (w > 0 && h > 0).then_some(RasterInfo {
                format: RasterFormat::Webp,
                width: w,
                height: h,
            });
        }
        if chunk == b"VP8 "
            && bytes.len() >= 30
            && bytes[23] == 0x9d
            && bytes[24] == 0x01
            && bytes[25] == 0x2a
        {
            let w = u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3fff);
            let h = u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3fff);
            return (w > 0 && h > 0).then_some(RasterInfo {
                format: RasterFormat::Webp,
                width: w,
                height: h,
            });
        }
        if chunk == b"VP8L" && bytes.len() >= 25 && bytes[20] == 0x2f {
            let b0 = u32::from(bytes[21]);
            let b1 = u32::from(bytes[22]);
            let b2 = u32::from(bytes[23]);
            let b3 = u32::from(bytes[24]);
            let bits = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
            let w = (bits & 0x3fff) + 1;
            let h = ((bits >> 14) & 0x3fff) + 1;
            return (w > 0 && h > 0).then_some(RasterInfo {
                format: RasterFormat::Webp,
                width: w,
                height: h,
            });
        }
        return None;
    }
    // JPEG: SOI (FF D8) + SOF0..SOF3, SOF5..SOF7, SOF9..SOF11, SOF13..SOF15
    if bytes.len() >= 4 && bytes[0] == 0xff && bytes[1] == 0xd8 {
        let mut i = 2usize;
        while i + 3 < bytes.len() {
            if bytes[i] != 0xff {
                return None;
            }
            while i < bytes.len() && bytes[i] == 0xff {
                i += 1;
            }
            if i >= bytes.len() {
                return None;
            }
            let marker = bytes[i];
            i += 1;
            if marker == 0xd9 || marker == 0xda {
                break; // EOI or SOS
            }
            if (0xd0..=0xd7).contains(&marker) || marker == 0x01 {
                continue; // standalone markers
            }
            if i + 1 >= bytes.len() {
                return None;
            }
            let seg_len = usize::from(u16::from_be_bytes([bytes[i], bytes[i + 1]]));
            if seg_len < 2 || i + seg_len > bytes.len() {
                return None;
            }
            let is_sof = matches!(
                marker,
                0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf
            );
            if is_sof {
                if seg_len < 7 {
                    return None;
                }
                let h = u32::from(u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]));
                let w = u32::from(u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]));
                return (w > 0 && h > 0).then_some(RasterInfo {
                    format: RasterFormat::Jpeg,
                    width: w,
                    height: h,
                });
            }
            i += seg_len;
        }
    }
    None
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
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

pub fn preview_image(ctx: &Ctx, id: &str, inline_bytes: bool) -> Result<Value> {
    let state = ctx.journal()?.read_state()?;
    let found = find_attachment(&state, id)?;
    if found.rec.kind != AttachmentKind::Image {
        return Err(error::refused(
            "unsupported_image_format",
            format!("{} is not a supported raster image", found.rec.label),
        ));
    }
    let (path, total_bytes) = require_available_file(&ctx.paths, found.rec)?;
    if total_bytes == 0 {
        return Err(error::refused(
            "unsupported_image_format",
            format!("{} is empty", found.rec.label),
        ));
    }
    if total_bytes > MAX_PREVIEW_BYTES {
        return Err(error::refused(
            "preview_too_large",
            format!(
                "{} exceeds 4 MiB inline preview limit ({total_bytes} bytes); use Open",
                found.rec.label
            ),
        ));
    }
    let mut file = File::open(&path)?;
    let meta = file.metadata()?;
    ensure!(meta.file_type().is_file(), "not a regular file");
    let read_cap = if inline_bytes {
        (MAX_PREVIEW_BYTES + 1) as usize
    } else {
        // Read header window (64 KiB covers PNG/GIF/WebP and JPEG EXIF+SOF).
        64 * 1024
    };
    let mut buf = Vec::with_capacity(read_cap.min(total_bytes as usize));
    Read::by_ref(&mut file)
        .take(read_cap as u64)
        .read_to_end(&mut buf)?;
    if inline_bytes && buf.len() as u64 > MAX_PREVIEW_BYTES {
        return Err(error::refused(
            "preview_too_large",
            format!(
                "{} exceeds 4 MiB inline preview limit; use Open",
                found.rec.label
            ),
        ));
    }
    let Some(info) = detect_raster_dimensions(&buf) else {
        return Err(error::refused(
            "unsupported_image_format",
            format!(
                "{} is not a valid PNG, JPEG, WebP, or GIF image",
                found.rec.label
            ),
        ));
    };
    let pixels = u64::from(info.width).saturating_mul(u64::from(info.height));
    if info.width > MAX_PREVIEW_DIM || info.height > MAX_PREVIEW_DIM || pixels > MAX_PREVIEW_PIXELS
    {
        return Err(error::refused(
            "preview_dimensions_exceeded",
            format!(
                "{} ({}x{}) exceeds inline preview bounds ({}px / {} MP); use Open",
                found.rec.label,
                info.width,
                info.height,
                MAX_PREVIEW_DIM,
                MAX_PREVIEW_PIXELS / 1_000_000
            ),
        ));
    }
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let cache_key = if let Some(sha) = &found.rec.sha256_12 {
        format!("{}-{sha}", found.rec.id)
    } else {
        format!("{}-{total_bytes}-{mtime_ms}", found.rec.id)
    };
    let mut payload = json!({
        "schema_version": 1,
        "id": found.rec.id,
        "label": found.rec.label,
        "format": info.format.as_str(),
        "ext": info.format.ext(),
        "width": info.width,
        "height": info.height,
        "bytes": total_bytes,
        "cache_key": cache_key,
        "resolved_path": path.to_string_lossy(),
    });
    if inline_bytes {
        payload["data_base64"] = json!(encode_base64(&buf));
    }
    Ok(payload)
}

fn is_safe_open_text_ext(label: &str) -> bool {
    let ext = Path::new(label)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            "log"
                | "txt"
                | "md"
                | "markdown"
                | "json"
                | "toml"
                | "yaml"
                | "yml"
                | "csv"
                | "diff"
                | "patch"
                | "jsonl"
        )
    )
}

pub fn validate_open_target(
    paths: &CoordPaths,
    rec: &AttachmentRec,
    allow_html: bool,
) -> Result<PathBuf> {
    let (path, _) = require_available_file(paths, rec)?;
    match rec.kind {
        AttachmentKind::Image => {
            let mut head = Vec::with_capacity(64 * 1024);
            File::open(&path)?.take(64 * 1024).read_to_end(&mut head)?;
            if detect_raster_dimensions(&head).is_none() {
                return Err(error::refused(
                    "open_refused_unsafe_kind",
                    format!(
                        "refusing to open {}: not a verified raster image",
                        rec.label
                    ),
                ));
            }
            Ok(path)
        }
        AttachmentKind::Html => {
            if !allow_html {
                return Err(error::refused(
                    "html_confirmation_required",
                    "opening local HTML in a browser requires --allow-html",
                ));
            }
            Ok(path)
        }
        AttachmentKind::Log | AttachmentKind::Markdown | AttachmentKind::Text => {
            if !is_safe_open_text_ext(&rec.label) {
                return Err(error::refused(
                    "open_refused_unsafe_kind",
                    format!(
                        "refusing direct open for {}; use View or Reveal instead",
                        rec.label
                    ),
                ));
            }
            Ok(path)
        }
        AttachmentKind::File => Err(error::refused(
            "open_refused_unsafe_kind",
            format!("refusing direct open for {}; use Reveal instead", rec.label),
        )),
    }
}

fn spawn_detached(program: &str, args: &[&std::ffi::OsStr]) -> Result<()> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to launch {program}"))?;
    Ok(())
}

pub fn open_external(ctx: &Ctx, id: &str, allow_html: bool) -> Result<Value> {
    let state = ctx.journal()?.read_state()?;
    let found = find_attachment(&state, id)?;
    let path = validate_open_target(&ctx.paths, found.rec, allow_html)?;
    #[cfg(target_os = "macos")]
    {
        spawn_detached("open", &[path.as_os_str()])?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        spawn_detached("xdg-open", &[path.as_os_str()])?;
    }
    Ok(json!({
        "action": "opened",
        "id": found.rec.id,
        "path": path.to_string_lossy(),
    }))
}

pub fn reveal_external(ctx: &Ctx, id: &str) -> Result<Value> {
    let state = ctx.journal()?.read_state()?;
    let found = find_attachment(&state, id)?;
    let (path, _) = require_available_file(&ctx.paths, found.rec)?;
    #[cfg(target_os = "macos")]
    {
        spawn_detached("open", &[std::ffi::OsStr::new("-R"), path.as_os_str()])?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let parent = path.parent().unwrap_or(&path);
        // Prefer parent directory via xdg-open (reliable across Wayland/niri/minimal desktops).
        spawn_detached("xdg-open", &[parent.as_os_str()])?;
    }
    Ok(json!({
        "action": "revealed",
        "id": found.rec.id,
        "path": path.to_string_lossy(),
    }))
}

fn remove_staged_file_and_empty_parents(paths: &CoordPaths, rel: &str) -> Result<u64> {
    let full = resolve_stored_path(paths, rel)?;
    let bytes = match fs::symlink_metadata(&full) {
        Ok(m) if m.file_type().is_file() => {
            let len = m.len();
            fs::remove_file(&full)?;
            len
        }
        Ok(m) if m.file_type().is_symlink() => bail!("refusing to delete symlink"),
        Ok(_) => bail!("refusing to delete non-file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e.into()),
    };
    // Best-effort cleanup of empty <task> and <ws> directories under artifacts/.
    if let Some(task_dir) = full.parent() {
        if task_dir.starts_with(&paths.artifacts) && task_dir != paths.artifacts {
            let _ = fs::remove_dir(task_dir);
            if let Some(ws_dir) = task_dir.parent() {
                if ws_dir.starts_with(&paths.artifacts) && ws_dir != paths.artifacts {
                    let _ = fs::remove_dir(ws_dir);
                }
            }
        }
    }
    Ok(bytes)
}

pub fn delete(ctx: &Ctx, caller: &Caller, target: &str) -> Result<Value> {
    let j = ctx.journal()?;
    let mut lock = j.lock()?;
    let mut to_delete: Vec<(WsId, Option<TaskId>, String, Option<String>)> = Vec::new();
    if let Ok(ws) = WsId::parse(target) {
        if !lock.state().workstreams.contains_key(&ws) {
            return Err(error::refused(
                "unknown_ws",
                format!("unknown workstream: {target}"),
            ));
        }
        for task in lock.state().tasks.values().filter(|t| t.ws == ws) {
            for rec in &task.attachments {
                if !rec.pruned {
                    to_delete.push((
                        task.ws.clone(),
                        Some(task.id.clone()),
                        rec.id.clone(),
                        (rec.mode == AttachmentMode::Staged)
                            .then(|| rec.stored_path.clone())
                            .flatten(),
                    ));
                }
            }
        }
    } else {
        let found = find_attachment(lock.state(), target)?;
        if !found.rec.pruned {
            to_delete.push((
                found.ws.clone(),
                Some(found.task.clone()),
                found.rec.id.clone(),
                (found.rec.mode == AttachmentMode::Staged)
                    .then(|| found.rec.stored_path.clone())
                    .flatten(),
            ));
        }
    }
    let mut freed_bytes = 0u64;
    let mut ids = Vec::new();
    let mut ws_scope = None;
    let mut task_scope = None;
    for (ws, task, id, rel) in to_delete {
        if let Some(rel) = rel {
            freed_bytes =
                freed_bytes.saturating_add(remove_staged_file_and_empty_parents(&ctx.paths, &rel)?);
        }
        ws_scope = Some(ws);
        task_scope = task;
        ids.push(id);
    }
    let seq = if !ids.is_empty() {
        let single_task = if ids.len() == 1 { task_scope } else { None };
        let entries = lock.append(
            &caller.actor(),
            ctx.now(),
            vec![(
                ws_scope,
                single_task,
                Kind::ArtifactPruned {
                    ids: ids.clone(),
                    reason: "user_deleted".into(),
                },
            )],
        )?;
        Some(entries[0].seq)
    } else {
        None
    };
    Ok(json!({
        "action": "deleted",
        "target": target,
        "pruned_ids": ids,
        "freed_bytes": freed_bytes,
        "seq": seq,
    }))
}

pub fn prune(ctx: &Ctx, caller: &Caller, older_than: &str, dry_run: bool) -> Result<Value> {
    let now = ctx.now();
    let cutoff = parse_since(older_than, now)?;
    let j = ctx.journal()?;
    let mut lock = j.lock()?;
    let mut candidates: Vec<(String, String, u64)> = Vec::new();
    for task in lock.state().tasks.values() {
        let ws_closed = lock
            .state()
            .workstreams
            .get(&task.ws)
            .is_some_and(|w| matches!(w.state, WsState::Closed | WsState::Cleaned));
        if !ws_closed {
            continue;
        }
        for rec in &task.attachments {
            if rec.pruned || rec.mode != AttachmentMode::Staged || rec.ts_ms >= cutoff {
                continue;
            }
            if let Some(rel) = &rec.stored_path {
                candidates.push((rec.id.clone(), rel.clone(), rec.bytes.unwrap_or(0)));
            }
        }
    }
    let mut freed_bytes = 0u64;
    let mut ids = Vec::new();
    if dry_run {
        for (id, _rel, bytes) in &candidates {
            ids.push(id.clone());
            freed_bytes = freed_bytes.saturating_add(*bytes);
        }
        return Ok(json!({
            "action": "prune",
            "dry_run": true,
            "older_than": older_than,
            "cutoff_ms": cutoff,
            "pruned_ids": ids,
            "freed_bytes": freed_bytes,
        }));
    }
    for (id, rel, _) in candidates {
        freed_bytes =
            freed_bytes.saturating_add(remove_staged_file_and_empty_parents(&ctx.paths, &rel)?);
        ids.push(id);
    }
    let seq = if !ids.is_empty() {
        let entries = lock.append(
            &caller.actor(),
            now,
            vec![(
                None,
                None,
                Kind::ArtifactPruned {
                    ids: ids.clone(),
                    reason: "pruned".into(),
                },
            )],
        )?;
        Some(entries[0].seq)
    } else {
        None
    };
    Ok(json!({
        "action": "prune",
        "dry_run": false,
        "older_than": older_than,
        "cutoff_ms": cutoff,
        "pruned_ids": ids,
        "freed_bytes": freed_bytes,
        "seq": seq,
    }))
}

pub fn mark_pruned(state: &mut State, ids: &[String], reason: &str) {
    let set: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    for task in state.tasks.values_mut() {
        for rec in &mut task.attachments {
            if set.contains(rec.id.as_str()) {
                rec.pruned = true;
                rec.pruned_reason = Some(reason.to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_png(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&w.to_be_bytes());
        bytes.extend_from_slice(&h.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    fn minimal_jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut bytes = vec![
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x0b, 0x08,
        ];
        bytes.extend_from_slice(&h.to_be_bytes());
        bytes.extend_from_slice(&w.to_be_bytes());
        bytes.extend_from_slice(&[0x03, 0x01, 0x11, 0x00, 0xff, 0xd9]);
        bytes
    }

    #[test]
    fn raster_header_detection_supports_png_jpeg_gif_webp_and_rejects_svg() {
        let png = detect_raster_dimensions(&minimal_png(320, 200)).unwrap();
        assert_eq!(
            (png.format, png.width, png.height),
            (RasterFormat::Png, 320, 200)
        );
        let jpg = detect_raster_dimensions(&minimal_jpeg(640, 480)).unwrap();
        assert_eq!(
            (jpg.format, jpg.width, jpg.height),
            (RasterFormat::Jpeg, 640, 480)
        );
        let gif = detect_raster_dimensions(b"GIF89a\x20\x00\x10\x00\x00").unwrap();
        assert_eq!(
            (gif.format, gif.width, gif.height),
            (RasterFormat::Gif, 32, 16)
        );
        assert!(
            detect_raster_dimensions(b"<svg xmlns='http://www.w3.org/2000/svg'></svg>").is_none()
        );
        assert!(detect_raster_dimensions(b"<html><script>alert(1)</script></html>").is_none());
    }

    #[test]
    fn plain_text_sanitizer_strips_ansi_and_control_chars() {
        let raw = "\x1b[31mERROR\x1b[0m\tline1\r\n\x1b]0;title\x07ok\x07\n";
        assert_eq!(sanitize_plain_text(raw), "ERROR    line1\nok\n");
    }

    #[test]
    fn sanitize_filename_and_classify_kind() {
        assert_eq!(
            sanitize_filename("../weird name (1).png"),
            "weird_name_1_.png"
        );
        assert_eq!(sanitize_filename("..."), "attachment");
        assert_eq!(classify_kind("shot.PNG"), AttachmentKind::Image);
        assert_eq!(classify_kind("mock.html"), AttachmentKind::Html);
        assert_eq!(classify_kind("check.log"), AttachmentKind::Log);
        assert_eq!(classify_kind("notes.md"), AttachmentKind::Markdown);
        assert_eq!(classify_kind("script.sh"), AttachmentKind::Text);
        assert_eq!(classify_kind("archive.tar.gz"), AttachmentKind::File);
    }

    #[test]
    fn staging_validates_symlinks_sensitive_paths_and_limits() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CoordPaths::new(&temp.path().join("data"));
        let ws = WsId::parse("ws-abc234").unwrap();
        let task = TaskId::new(&ws, 1);

        let src_dir = temp.path().join("work");
        fs::create_dir_all(&src_dir).unwrap();
        let img = src_dir.join("shot.png");
        fs::write(&img, minimal_png(120, 80)).unwrap();

        let staged = stage_for_report(&paths, &ws, &task, 1, &[img.clone()], &[], None).unwrap();
        assert_eq!(staged.attachments.len(), 1);
        assert!(staged.warnings.is_empty());
        assert_eq!(staged.attachments[0].mode, AttachmentMode::Staged);
        assert_eq!(staged.attachments[0].id, "ws-abc234-t1-a1");
        let stored = resolve_stored_path(
            &paths,
            staged.attachments[0].stored_path.as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(
            fs::metadata(&stored).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // Symlink refused
        let link = src_dir.join("link.png");
        std::os::unix::fs::symlink(&img, &link).unwrap();
        assert!(stage_for_report(&paths, &ws, &task, 2, &[link], &[], None).is_err());

        // Sensitive filename refused
        let env_file = src_dir.join(".env.local");
        fs::write(&env_file, "SECRET=1").unwrap();
        assert!(stage_for_report(&paths, &ws, &task, 2, &[env_file], &[], None).is_err());

        // Directory refused
        assert!(stage_for_report(&paths, &ws, &task, 2, &[src_dir.clone()], &[], None).is_err());

        // Remote worker records ref without local file check
        let remote = stage_for_report(
            &paths,
            &ws,
            &task,
            2,
            &[PathBuf::from("/remote/path/shot.png")],
            &[],
            Some("gpu-box"),
        )
        .unwrap();
        assert_eq!(remote.attachments[0].mode, AttachmentMode::Ref);
        assert_eq!(remote.attachments[0].machine.as_deref(), Some("gpu-box"));
        assert_eq!(remote.attachments[0].reason.as_deref(), Some("remote_host"));
    }

    #[test]
    fn base64_encoder_matches_rfc4648() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn end_to_end_report_attachments_survive_worktree_cleanup_and_enforce_safety() {
        use super::super::{
            adapter::fake::{FakeApp, FakeHc},
            cli::ReportKind,
            kinds::{Actor, ResType, ResourceRef, TaskOutcome, WsOutcome},
            place, release, views,
        };

        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("data");
        let worktree = temp.path().join("wt");
        fs::create_dir_all(&worktree).unwrap();

        let png_path = worktree.join("card-dark.png");
        let jpg_path = worktree.join("card-light.jpg");
        let log_path = worktree.join("check.log");
        let html_path = worktree.join("mock.html");
        let script_path = worktree.join("run.sh");
        let ref_path = worktree.join("ephemeral-ref.log");
        let svg_fake_png = worktree.join("fake.png");
        let result_path = temp.path().join("result.md");

        fs::write(&png_path, minimal_png(320, 200)).unwrap();
        fs::write(&jpg_path, minimal_jpeg(640, 480)).unwrap();
        fs::write(&log_path, "\x1b[31mFAIL\x1b[0m line1\nline2\n").unwrap();
        fs::write(
            &html_path,
            "<html><body><script>alert(1)</script></body></html>",
        )
        .unwrap();
        fs::write(&script_path, "#!/bin/sh\necho hi\n").unwrap();
        fs::write(&ref_path, "ref log inside worktree\n").unwrap();
        fs::write(
            &svg_fake_png,
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        )
        .unwrap();
        fs::write(&result_path, "# Final Result\n").unwrap();

        let mut ctx = Ctx::new(&data_root).unwrap();
        ctx.hc = Box::new(FakeHc::default());
        ctx.app = Box::new(FakeApp::default());
        ctx.clock = Box::new(|| 1_795_300_000_000);

        let j = ctx.journal().unwrap();
        let ws = WsId::parse("ws-abc234").unwrap();
        let task = TaskId::new(&ws, 1);
        {
            let mut lock = j.lock().unwrap();
            lock.append(
                &Actor::Coordinator,
                1_795_300_000_000,
                vec![
                    (
                        Some(ws.clone()),
                        None,
                        Kind::WsOpened {
                            title: "Polish UI".into(),
                            repo: None,
                            delivery_target: Some("main".into()),
                            delivery_branch: None,
                            purpose: None,
                            request: None,
                        },
                    ),
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
                        Kind::TaskPlaced {
                            rung: 4,
                            reason: "isolated worktree".into(),
                            rejected: vec![],
                            role: "coord-default".into(),
                            harness: "omp".into(),
                            model: None,
                            effort: None,
                            title: "Implement attachments".into(),
                            brief: temp.path().join("brief.md"),
                            session_of: None,
                            session: None,
                            request: None,
                            follows: None,
                            role_hash: String::new(),
                            brief_bytes: 10,
                        },
                    ),
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
                        Kind::TaskStarted {
                            terminal: "t1".into(),
                            tab: None,
                            tab_name: "task".into(),
                            space: "sp1".into(),
                            cwd: worktree.clone(),
                            pid: None,
                            created_ms: 1,
                            session: None,
                            resumed: false,
                        },
                    ),
                    (
                        Some(ws.clone()),
                        None,
                        Kind::ResCreated(ResourceRef::new(
                            ResType::Worktree,
                            worktree.to_string_lossy(),
                        )),
                    ),
                ],
            )
            .unwrap();
        }

        let delegate = Caller::Delegate {
            terminal: "t1".into(),
            task: task.clone(),
            ws: ws.clone(),
        };
        let person = Caller::Person { terminal: None };

        // Progress report with image attachment: stays non-waking.
        let prog = place::report(
            &ctx,
            &delegate,
            ReportKind::Progress,
            "Captured dark mode",
            None,
            &[png_path.clone()],
            &[],
        )
        .unwrap();
        assert_eq!(prog["attachments"][0]["id"], "ws-abc234-t1-a1");
        assert!(j.read_state().unwrap().waking.is_empty());

        // Terminal report with JPEG, log, HTML, script, fake-PNG, and ref + --result: waking!
        let done = place::report(
            &ctx,
            &delegate,
            ReportKind::Done,
            "All checks done",
            Some(&result_path),
            &[
                jpg_path.clone(),
                log_path.clone(),
                html_path.clone(),
                script_path.clone(),
                svg_fake_png.clone(),
            ],
            &[ref_path.clone()],
        )
        .unwrap();
        assert_eq!(done["attachments"].as_array().unwrap().len(), 6);
        assert_eq!(j.read_state().unwrap().waking.len(), 1);

        // Close task & workstream, remove worktree directory, mark cleaned.
        {
            let mut lock = j.lock().unwrap();
            lock.append(
                &Actor::Coordinator,
                1_795_300_010_000,
                vec![
                    (
                        Some(ws.clone()),
                        Some(task.clone()),
                        Kind::TaskClosed {
                            reason: "accepted".into(),
                            outcome: Some(TaskOutcome::Accepted),
                        },
                    ),
                    (
                        Some(ws.clone()),
                        None,
                        Kind::WsClosed {
                            summary: "Delivered".into(),
                            outcome: Some(WsOutcome::Delivered),
                        },
                    ),
                    (
                        Some(ws.clone()),
                        None,
                        Kind::ResReleased {
                            r#type: ResType::Worktree,
                            name: worktree.to_string_lossy().into_owned(),
                            note: None,
                        },
                    ),
                ],
            )
            .unwrap();
        }
        fs::remove_dir_all(&worktree).unwrap();
        let rel_out =
            release::release_ws(&ctx, &j, &Actor::Coordinator, &ws, release::Mode::Command)
                .unwrap();
        assert!(rel_out.cleaned);

        // Staged attachments survive worktree deletion; ref inside deleted worktree is missing.
        let listed = list(&ctx, Some(ws.as_str()), None).unwrap();
        let items = listed["attachments"].as_array().unwrap();
        assert_eq!(items.len(), 7);
        assert_eq!(items[0]["id"], "ws-abc234-t1-a1");
        assert_eq!(items[0]["availability"], "available");
        assert_eq!(items[6]["id"], "ws-abc234-t1-a7");
        assert_eq!(items[6]["availability"], "missing");

        // Board includes metadata roll-up.
        let board = views::board(&j.read_state().unwrap(), None, 1_795_300_020_000, true);
        assert_eq!(
            board["workstreams"][0]["tasks"][0]["attachments"]
                .as_array()
                .unwrap()
                .len(),
            7
        );

        // Preview validates real PNG & JPEG and rejects SVG disguised as .png.
        let p_png = preview_image(&ctx, "ws-abc234-t1-a1", true).unwrap();
        assert_eq!(
            (
                p_png["format"].as_str(),
                p_png["width"].as_u64(),
                p_png["height"].as_u64()
            ),
            (Some("png"), Some(320), Some(200))
        );
        assert!(p_png["data_base64"].as_str().is_some_and(|s| !s.is_empty()));
        let p_jpg = preview_image(&ctx, "ws-abc234-t1-a2", false).unwrap();
        assert_eq!(
            (
                p_jpg["format"].as_str(),
                p_jpg["width"].as_u64(),
                p_jpg["height"].as_u64()
            ),
            (Some("jpeg"), Some(640), Some(480))
        );
        assert!(preview_image(&ctx, "ws-abc234-t1-a6", false).is_err());

        // Inert plain-text read strips ANSI & reads HTML as source.
        let log_read = read_text(&ctx, "ws-abc234-t1-a3", 65536).unwrap();
        assert_eq!(log_read["text"], "FAIL line1\nline2\n");
        let html_read = read_text(&ctx, "ws-abc234-t1-a4", 65536).unwrap();
        assert!(
            html_read["text"]
                .as_str()
                .unwrap()
                .contains("<script>alert(1)</script>")
        );
        // Binary image cannot be read as text.
        assert!(read_text(&ctx, "ws-abc234-t1-a1", 65536).is_err());

        // Safe open policy: HTML needs --allow-html; .sh and fake-PNG refused; PNG and .log allowed.
        let state = j.read_state().unwrap();
        assert!(
            validate_open_target(
                &ctx.paths,
                &find_attachment(&state, "ws-abc234-t1-a1").unwrap().rec,
                false
            )
            .is_ok()
        );
        assert!(
            validate_open_target(
                &ctx.paths,
                &find_attachment(&state, "ws-abc234-t1-a3").unwrap().rec,
                false
            )
            .is_ok()
        );
        assert!(
            validate_open_target(
                &ctx.paths,
                &find_attachment(&state, "ws-abc234-t1-a4").unwrap().rec,
                false
            )
            .is_err()
        );
        assert!(
            validate_open_target(
                &ctx.paths,
                &find_attachment(&state, "ws-abc234-t1-a4").unwrap().rec,
                true
            )
            .is_ok()
        );
        assert!(
            validate_open_target(
                &ctx.paths,
                &find_attachment(&state, "ws-abc234-t1-a5").unwrap().rec,
                false
            )
            .is_err()
        );
        assert!(
            validate_open_target(
                &ctx.paths,
                &find_attachment(&state, "ws-abc234-t1-a6").unwrap().rec,
                false
            )
            .is_err()
        );

        // Explicit delete marks pruned and removes staged file.
        let del = delete(&ctx, &person, "ws-abc234-t1-a1").unwrap();
        assert_eq!(del["pruned_ids"], json!(["ws-abc234-t1-a1"]));
        let listed_after_del = list(&ctx, Some(ws.as_str()), None).unwrap();
        assert_eq!(listed_after_del["attachments"][0]["availability"], "pruned");

        // Prune dry-run & real prune on closed/cleaned ws older than 1s.
        ctx.clock = Box::new(|| 1_795_300_100_000);
        let dry = prune(&ctx, &person, "10s", true).unwrap();
        assert_eq!(dry["pruned_ids"].as_array().unwrap().len(), 5);
        let pruned = prune(&ctx, &person, "10s", false).unwrap();
        assert_eq!(pruned["pruned_ids"].as_array().unwrap().len(), 5);
        assert!(pruned["freed_bytes"].as_u64().unwrap() > 0);
        // Pruned metadata remains discoverable.
        let listed_after_prune = list(&ctx, Some(ws.as_str()), None).unwrap();
        assert_eq!(
            listed_after_prune["attachments"].as_array().unwrap().len(),
            7
        );
        assert_eq!(
            listed_after_prune["attachments"][1]["availability"],
            "pruned"
        );
    }
}
