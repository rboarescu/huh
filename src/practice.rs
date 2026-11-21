//! Safe filesystem sandbox setup, teardown, and verification for `huh learn` practice labs.
//!
//! Every operation stays strictly inside the canonicalized practice root
//! (`$XDG_DATA_HOME/huh/practice` or `$HOME/.local/share/huh/practice`), refusing `..`,
//! absolute paths, and symlink escapes. Nothing here ever spawns a subprocess.

use crate::session;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const MAX_PRACTICE_ID_LEN: usize = 40;
const MAX_SYMLINK_DEPTH: usize = 40;
const MAX_CHECK_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Practice {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub hints: Vec<String>,
    #[serde(default)]
    pub setup: Setup,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    #[serde(default)]
    pub dirs: Vec<String>,
    #[serde(default)]
    pub files: Vec<SetupFile>,
    #[serde(default)]
    pub symlinks: Vec<SetupSymlink>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupFile {
    pub path: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub mode: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupSymlink {
    pub link: String,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    DirExists {
        path: String,
        label: String,
    },
    FileExists {
        path: String,
        label: String,
    },
    NotExists {
        path: String,
        label: String,
    },
    FileContains {
        path: String,
        text: String,
        label: String,
    },
    FileEquals {
        path: String,
        text: String,
        label: String,
    },
    FileModeHas {
        path: String,
        bits: u32,
        label: String,
    },
    FileModeEq {
        path: String,
        mode: u32,
        label: String,
    },
    IsSymlink {
        path: String,
        #[serde(default)]
        target: Option<String>,
        label: String,
    },
    RecordedOutput {
        program: String,
        #[serde(default)]
        command_contains: Option<String>,
        output_contains: String,
        fallback_prompt: String,
        fallback_answer: String,
        label: String,
    },
}

impl Check {
    pub fn label(&self) -> &str {
        match self {
            Check::DirExists { label, .. }
            | Check::FileExists { label, .. }
            | Check::NotExists { label, .. }
            | Check::FileContains { label, .. }
            | Check::FileEquals { label, .. }
            | Check::FileModeHas { label, .. }
            | Check::FileModeEq { label, .. }
            | Check::IsSymlink { label, .. }
            | Check::RecordedOutput { label, .. } => label,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckOutcome {
    pub label: String,
    pub passed: bool,
    pub detail: String,
    pub needs_fallback: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub outcomes: Vec<CheckOutcome>,
}

impl Report {
    pub fn all_passed(&self) -> bool {
        !self.outcomes.is_empty() && self.outcomes.iter().all(|o| o.passed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedCommand {
    pub command: String,
    pub exit: Option<i32>,
    pub output: String,
}

/// `$XDG_DATA_HOME/huh/practice`, or `$HOME/.local/share/huh/practice`.
pub fn practice_root(xdg_data: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_data.filter(|v| !v.is_empty()) {
        Some(dir) => Some(Path::new(dir).join("huh/practice")),
        None => Some(Path::new(home.filter(|v| !v.is_empty())?).join(".local/share/huh/practice")),
    }
}

pub fn default_practice_root() -> Option<PathBuf> {
    practice_root(std::env::var("XDG_DATA_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

/// Validates and normalizes a relative path inside a practice sandbox.
///
/// Rejects empty/blank strings, NUL bytes, absolute paths, Windows prefixes, `..` components,
/// and `.` root-only paths.
pub fn safe_rel_path(rel: &str) -> Result<PathBuf> {
    if rel.trim().is_empty() {
        bail!("practice path cannot be empty");
    }
    if rel.contains('\0') {
        bail!("practice path cannot contain NUL bytes");
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        bail!("practice path must be relative: {rel:?}");
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => bail!("practice path must be relative: {rel:?}"),
            Component::ParentDir => bail!("practice path cannot contain `..`: {rel:?}"),
            Component::CurDir => {}
            Component::Normal(part) => clean.push(part),
        }
    }
    if clean.as_os_str().is_empty() {
        bail!("practice path cannot refer to the sandbox root itself: {rel:?}");
    }
    Ok(clean)
}

/// Validates a practice lab definition before it is prepared or loaded.
pub fn validate_practice(practice: &Practice) -> Result<()> {
    if practice.id.is_empty() || practice.id.len() > MAX_PRACTICE_ID_LEN {
        bail!("practice id {:?} must be 1..={MAX_PRACTICE_ID_LEN} characters", practice.id);
    }
    if !practice.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') {
        bail!("practice id {:?} may contain only ASCII [a-z0-9_-]", practice.id);
    }
    if practice.title.trim().is_empty() {
        bail!("practice {:?} has an empty title", practice.id);
    }
    if practice.goal.trim().is_empty() {
        bail!("practice {:?} has an empty goal", practice.id);
    }
    if practice.hints.is_empty() || practice.hints.iter().any(|h| h.trim().is_empty()) {
        bail!("practice {:?} must have at least one non-empty hint", practice.id);
    }
    if practice.checks.is_empty() {
        bail!("practice {:?} must have at least one check", practice.id);
    }

    for dir in &practice.setup.dirs {
        safe_rel_path(dir).with_context(|| format!("practice {:?} setup dir", practice.id))?;
    }
    for file in &practice.setup.files {
        safe_rel_path(&file.path).with_context(|| format!("practice {:?} setup file", practice.id))?;
    }
    for sym in &practice.setup.symlinks {
        safe_rel_path(&sym.link).with_context(|| format!("practice {:?} setup symlink link", practice.id))?;
        safe_rel_path(&sym.target).with_context(|| format!("practice {:?} setup symlink target", practice.id))?;
    }

    for (idx, check) in practice.checks.iter().enumerate() {
        if check.label().trim().is_empty() {
            bail!("practice {:?} check #{idx} has an empty label", practice.id);
        }
        match check {
            Check::DirExists { path, .. }
            | Check::FileExists { path, .. }
            | Check::NotExists { path, .. }
            | Check::FileContains { path, .. }
            | Check::FileEquals { path, .. }
            | Check::FileModeHas { path, .. }
            | Check::FileModeEq { path, .. } => {
                safe_rel_path(path).with_context(|| format!("practice {:?} check #{idx}", practice.id))?;
            }
            Check::IsSymlink { path, target, .. } => {
                safe_rel_path(path).with_context(|| format!("practice {:?} check #{idx}", practice.id))?;
                if let Some(target) = target {
                    safe_rel_path(target).with_context(|| format!("practice {:?} check #{idx} target", practice.id))?;
                }
            }
            Check::RecordedOutput {
                program,
                command_contains,
                output_contains,
                fallback_prompt,
                fallback_answer,
                ..
            } => {
                if program.trim().is_empty() {
                    bail!("practice {:?} check #{idx} has an empty program", practice.id);
                }
                if command_contains.as_deref().is_some_and(|s| s.trim().is_empty()) {
                    bail!("practice {:?} check #{idx} has an empty command_contains", practice.id);
                }
                if output_contains.trim().is_empty() {
                    bail!("practice {:?} check #{idx} has an empty output_contains", practice.id);
                }
                if fallback_prompt.trim().is_empty() {
                    bail!("practice {:?} check #{idx} has an empty fallback_prompt", practice.id);
                }
                if fallback_answer.trim().is_empty() {
                    bail!("practice {:?} check #{idx} has an empty fallback_answer", practice.id);
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Sandbox preparation & cleanup
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Sandbox {
    root: PathBuf,
    dir: PathBuf,
}

impl Sandbox {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Creates a fresh sandbox directory for `practice` strictly inside `root` and populates `setup`.
    pub fn prepare(root: &Path, practice: &Practice) -> Result<Sandbox> {
        validate_practice(practice)?;

        std::fs::create_dir_all(root).with_context(|| format!("cannot create practice root {}", root.display()))?;
        let canonical_root = std::fs::canonicalize(root)
            .with_context(|| format!("cannot canonicalize practice root {}", root.display()))?;
        if !canonical_root.is_dir() {
            bail!("practice root is not a directory: {}", canonical_root.display());
        }

        let candidate = canonical_root.join(&practice.id);
        match std::fs::symlink_metadata(&candidate) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    bail!("refusing to use symlink as practice sandbox: {}", candidate.display());
                }
                if !meta.is_dir() {
                    bail!("refusing to replace non-directory at practice sandbox: {}", candidate.display());
                }
                safe_remove_dir_all(&canonical_root, &candidate)?;
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("cannot inspect {}", candidate.display())),
        }

        std::fs::create_dir(&candidate).with_context(|| format!("cannot create sandbox {}", candidate.display()))?;
        let meta = std::fs::symlink_metadata(&candidate)
            .with_context(|| format!("cannot inspect sandbox {}", candidate.display()))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            bail!("sandbox path is not a real directory: {}", candidate.display());
        }
        let canonical_dir = std::fs::canonicalize(&candidate)
            .with_context(|| format!("cannot canonicalize sandbox {}", candidate.display()))?;
        if !canonical_dir.starts_with(&canonical_root) || canonical_dir == canonical_root {
            bail!("sandbox escaped practice root: {}", canonical_dir.display());
        }

        for dir in &practice.setup.dirs {
            let rel = safe_rel_path(dir)?;
            ensure_dir_inside(&canonical_dir, &rel)?;
        }

        for file in &practice.setup.files {
            let rel = safe_rel_path(&file.path)?;
            if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
                ensure_dir_inside(&canonical_dir, parent)?;
            }
            let full = canonical_dir.join(&rel);
            let parent = full.parent().context("setup file has no parent directory")?;
            let canon_parent =
                std::fs::canonicalize(parent).with_context(|| format!("cannot canonicalize {}", parent.display()))?;
            if !canon_parent.starts_with(&canonical_dir) {
                bail!("setup file parent escaped sandbox: {}", full.display());
            }
            if let Ok(existing) = std::fs::symlink_metadata(&full)
                && existing.file_type().is_symlink()
            {
                bail!("refusing to write through symlink inside sandbox: {}", full.display());
            }
            std::fs::write(&full, &file.content).with_context(|| format!("cannot write {}", full.display()))?;
            #[cfg(unix)]
            if let Some(mode) = file.mode {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&full, std::fs::Permissions::from_mode(mode & 0o777))
                    .with_context(|| format!("cannot set permissions on {}", full.display()))?;
            }
        }

        for sym in &practice.setup.symlinks {
            let link_rel = safe_rel_path(&sym.link)?;
            let target_rel = safe_rel_path(&sym.target)?;
            if let Some(parent) = link_rel.parent().filter(|p| !p.as_os_str().is_empty()) {
                ensure_dir_inside(&canonical_dir, parent)?;
            }
            let link_path = canonical_dir.join(&link_rel);
            let parent = link_path.parent().context("setup symlink has no parent directory")?;
            let canon_parent =
                std::fs::canonicalize(parent).with_context(|| format!("cannot canonicalize {}", parent.display()))?;
            if !canon_parent.starts_with(&canonical_dir) {
                bail!("setup symlink parent escaped sandbox: {}", link_path.display());
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&target_rel, &link_path)
                    .with_context(|| format!("cannot create symlink {}", link_path.display()))?;
            }
            #[cfg(not(unix))]
            {
                let _ = (&target_rel, &link_path);
                bail!("symlinks in practice setup require Unix");
            }
        }

        Ok(Sandbox { root: canonical_root, dir: canonical_dir })
    }

    /// Removes this sandbox directory after verifying it stays strictly inside `self.root` and is not a symlink.
    pub fn cleanup(&self) -> Result<()> {
        if !self.dir.starts_with(&self.root) || self.dir == self.root {
            bail!("refusing to clean up path outside practice root: {}", self.dir.display());
        }
        match std::fs::symlink_metadata(&self.root) {
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err).with_context(|| format!("cannot inspect {}", self.root.display())),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                bail!("refusing to clean up under non-directory or symlink root: {}", self.root.display());
            }
            Ok(_) => {}
        }
        let canonical_root = std::fs::canonicalize(&self.root)
            .with_context(|| format!("cannot canonicalize practice root {}", self.root.display()))?;
        if canonical_root != self.root {
            bail!("practice root moved or became a symlink: {}", self.root.display());
        }
        match std::fs::symlink_metadata(&self.dir) {
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err).with_context(|| format!("cannot inspect {}", self.dir.display())),
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    bail!("refusing to remove symlink sandbox: {}", self.dir.display());
                }
                if !meta.is_dir() {
                    bail!("refusing to remove non-directory sandbox: {}", self.dir.display());
                }
            }
        }
        let canonical_dir = std::fs::canonicalize(&self.dir)
            .with_context(|| format!("cannot canonicalize sandbox {}", self.dir.display()))?;
        if canonical_dir != self.dir || !canonical_dir.starts_with(&canonical_root) || canonical_dir == canonical_root {
            bail!("sandbox path changed or escaped practice root: {}", self.dir.display());
        }
        safe_remove_dir_all(&canonical_root, &canonical_dir)
    }
}

/// Creates a nested directory under `canonical_sandbox` one normal component at a time,
/// refusing to traverse symlinks or leave `canonical_sandbox`.
fn ensure_dir_inside(canonical_sandbox: &Path, rel: &Path) -> Result<PathBuf> {
    let mut cur = canonical_sandbox.to_path_buf();
    for component in rel.components() {
        let Component::Normal(part) = component else {
            bail!("unsafe directory component in {}", rel.display());
        };
        cur.push(part);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    bail!("refusing to traverse symlink inside sandbox: {}", cur.display());
                }
                if !meta.is_dir() {
                    bail!("expected directory inside sandbox: {}", cur.display());
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                std::fs::create_dir(&cur).with_context(|| format!("cannot create {}", cur.display()))?;
            }
            Err(err) => return Err(err).with_context(|| format!("cannot inspect {}", cur.display())),
        }
    }
    let canon = std::fs::canonicalize(&cur).with_context(|| format!("cannot canonicalize {}", cur.display()))?;
    if !canon.starts_with(canonical_sandbox) || canon == canonical_sandbox {
        bail!("directory escaped sandbox: {}", cur.display());
    }
    Ok(canon)
}

/// Recursively removes `dir` strictly inside `canonical_root` without ever following symlinks.
fn safe_remove_dir_all(canonical_root: &Path, dir: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(dir).with_context(|| format!("cannot inspect {}", dir.display()))?;
    if meta.file_type().is_symlink() {
        bail!("refusing to follow symlink during cleanup: {}", dir.display());
    }
    if !meta.is_dir() {
        bail!("refusing to remove non-directory during cleanup: {}", dir.display());
    }
    let canon = std::fs::canonicalize(dir).with_context(|| format!("cannot canonicalize {}", dir.display()))?;
    if !canon.starts_with(canonical_root) || canon == canonical_root {
        bail!("refusing to remove path outside practice root: {}", dir.display());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if (mode & 0o700) != 0o700 {
            std::fs::set_permissions(&canon, std::fs::Permissions::from_mode((mode & 0o777) | 0o700))
                .with_context(|| format!("cannot make {} writable for cleanup", canon.display()))?;
        }
    }

    for entry in std::fs::read_dir(&canon).with_context(|| format!("cannot read {}", canon.display()))? {
        let entry = entry?;
        let path = entry.path();
        let child_meta =
            std::fs::symlink_metadata(&path).with_context(|| format!("cannot inspect {}", path.display()))?;
        if child_meta.file_type().is_symlink() || !child_meta.is_dir() {
            std::fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
        } else {
            safe_remove_dir_all(canonical_root, &path)?;
        }
    }
    std::fs::remove_dir(&canon).with_context(|| format!("cannot remove {}", canon.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Recorded session reading
// ---------------------------------------------------------------------------

fn default_recording_dir() -> Option<PathBuf> {
    if let Some(own) = std::env::var_os("HUH_SESSION_LOG").filter(|v| !v.is_empty())
        && let Some(dir) = Path::new(&own).parent()
        && dir.is_dir()
    {
        return Some(dir.to_path_buf());
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| Path::new(&h).join(".cache")))?;
    Some(base.join("huh"))
}

fn recorded_from_env() -> Option<(Option<SystemTime>, RecordedCommand)> {
    let status_path = std::env::var_os("HUH_SESSION_STATUS").filter(|v| !v.is_empty())?;
    let _log_path = std::env::var_os("HUH_SESSION_LOG").filter(|v| !v.is_empty())?;
    let last = session::last().ok()?;
    if last.command.trim().is_empty() {
        return None;
    }
    let mtime = std::fs::metadata(&status_path).ok().and_then(|m| m.modified().ok());
    Some((mtime, RecordedCommand { command: last.command, exit: last.exit, output: last.output }))
}

fn latest_recorded_in_dir(dir: &Path) -> Option<(Option<SystemTime>, RecordedCommand)> {
    let entries = std::fs::read_dir(dir).ok()?;
    let check_proc = Path::new("/proc").is_dir();
    let mut candidates: Vec<(Option<SystemTime>, PathBuf, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let status_path = entry.path();
        if status_path.extension().is_none_or(|ext| ext != "status") {
            continue;
        }
        let Some(pid) = status_path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if check_proc && !Path::new("/proc").join(pid).is_dir() {
            continue;
        }
        let log_path = status_path.with_extension("log");
        if !log_path.is_file() {
            continue;
        }
        let mtime = std::fs::metadata(&status_path).ok().and_then(|m| m.modified().ok());
        candidates.push((mtime, status_path, log_path));
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

    for (mtime, status_path, log_path) in candidates {
        let Ok(status_text) = std::fs::read_to_string(&status_path) else {
            continue;
        };
        let (exit, command) = session::parse_status(&status_text);
        if command.trim().is_empty() {
            continue;
        }
        let Ok(log_bytes) = std::fs::read(&log_path) else {
            continue;
        };
        let output = session::tail(&session::clean(session::last_region(&String::from_utf8_lossy(&log_bytes))));
        return Some((mtime, RecordedCommand { command, exit, output }));
    }
    None
}

/// Reads the newest recorded command, either from an explicit `recording_dir`, or from
/// `HUH_SESSION_LOG`/`HUH_SESSION_STATUS` and active sibling recordings in the default directory.
pub fn latest_recorded_from_env_or_dir(recording_dir: Option<&Path>) -> Option<RecordedCommand> {
    if let Some(dir) = recording_dir {
        return latest_recorded_in_dir(dir).map(|(_, rec)| rec);
    }
    let from_env = recorded_from_env();
    let from_dir = default_recording_dir().and_then(|dir| latest_recorded_in_dir(&dir));
    match (from_env, from_dir) {
        (Some((env_time, env_rec)), Some((dir_time, dir_rec))) => {
            if dir_time > env_time {
                Some(dir_rec)
            } else {
                Some(env_rec)
            }
        }
        (Some((_, rec)), None) | (None, Some((_, rec))) => Some(rec),
        (None, None) => None,
    }
}

// ---------------------------------------------------------------------------
// Sandbox evaluation
// ---------------------------------------------------------------------------

enum ResolvedPath {
    Existing(PathBuf),
    Missing,
    Escaped,
    Loop,
    Denied(String),
}

/// Resolves `rel` inside `canonical_sandbox` one component at a time without ever following
/// a symlink outside `canonical_sandbox`. When `allow_root` is false, the final resolved path
/// must be strictly deeper than `canonical_sandbox`.
fn resolve_inside(canonical_sandbox: &Path, rel: &Path, allow_root: bool) -> Result<ResolvedPath> {
    let mut cur = canonical_sandbox.to_path_buf();
    let mut remaining: VecDeque<OsString> = VecDeque::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => remaining.push_back(part.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Ok(ResolvedPath::Escaped);
            }
        }
    }

    let mut depth = 0usize;
    while let Some(part) = remaining.pop_front() {
        if part == "." {
            continue;
        }
        if part == ".." {
            if cur == canonical_sandbox {
                return Ok(ResolvedPath::Escaped);
            }
            cur.pop();
            continue;
        }

        let next = cur.join(&part);
        match std::fs::symlink_metadata(&next) {
            Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok(ResolvedPath::Missing);
            }
            Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                return Ok(ResolvedPath::Denied(err.to_string()));
            }
            Err(err) => return Err(err).with_context(|| format!("cannot inspect {}", next.display())),
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    depth += 1;
                    if depth > MAX_SYMLINK_DEPTH {
                        return Ok(ResolvedPath::Loop);
                    }
                    let target =
                        std::fs::read_link(&next).with_context(|| format!("cannot read link {}", next.display()))?;
                    let mut prefix: Vec<OsString> = Vec::new();
                    if target.is_absolute() {
                        let Ok(stripped) = target.strip_prefix(canonical_sandbox) else {
                            return Ok(ResolvedPath::Escaped);
                        };
                        cur = canonical_sandbox.to_path_buf();
                        for comp in stripped.components() {
                            match comp {
                                Component::Normal(p) => prefix.push(p.to_os_string()),
                                Component::CurDir => {}
                                Component::ParentDir => prefix.push(OsString::from("..")),
                                Component::RootDir | Component::Prefix(_) => return Ok(ResolvedPath::Escaped),
                            }
                        }
                    } else {
                        for comp in target.components() {
                            match comp {
                                Component::Normal(p) => prefix.push(p.to_os_string()),
                                Component::CurDir => {}
                                Component::ParentDir => prefix.push(OsString::from("..")),
                                Component::RootDir | Component::Prefix(_) => return Ok(ResolvedPath::Escaped),
                            }
                        }
                    }
                    for item in prefix.into_iter().rev() {
                        remaining.push_front(item);
                    }
                } else {
                    cur = next;
                    if !remaining.is_empty() && !meta.is_dir() {
                        return Ok(ResolvedPath::Missing);
                    }
                }
            }
        }
    }

    if !cur.starts_with(canonical_sandbox) || (!allow_root && cur == canonical_sandbox) {
        return Ok(ResolvedPath::Escaped);
    }
    Ok(ResolvedPath::Existing(cur))
}

/// Resolves the parent directory of `rel` inside `canonical_sandbox` (without touching the final entry).
fn resolve_parent_inside(canonical_sandbox: &Path, rel: &Path) -> Result<ResolvedPath> {
    match rel.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => resolve_inside(canonical_sandbox, parent, true),
        _ => Ok(ResolvedPath::Existing(canonical_sandbox.to_path_buf())),
    }
}

fn outcome(label: &str, passed: bool, detail: impl Into<String>, needs_fallback: bool) -> CheckOutcome {
    CheckOutcome { label: label.to_string(), passed, detail: detail.into(), needs_fallback }
}

fn read_text_inside(path: &Path, display_rel: &str) -> Result<std::result::Result<String, String>> {
    let meta = std::fs::metadata(path).with_context(|| format!("cannot inspect {}", path.display()))?;
    if !meta.is_file() {
        return Ok(Err(format!("{display_rel} is not a regular file")));
    }
    if meta.len() > MAX_CHECK_FILE_BYTES {
        return Ok(Err(format!("{display_rel} is too large to check")));
    }
    match std::fs::read(path) {
        Ok(bytes) => Ok(Ok(String::from_utf8_lossy(&bytes).into_owned())),
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            Ok(Err(format!("cannot read {display_rel}: permission denied")))
        }
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn stage_program(stage: &str) -> Option<&str> {
    let mut after_wrapper = false;
    for raw in stage.split_whitespace() {
        let word = raw.trim_start_matches(['(', '\\']);
        if word.is_empty() || word.contains('=') {
            continue;
        }
        if matches!(word, "sudo" | "env" | "time" | "command" | "builtin") {
            after_wrapper = true;
            continue;
        }
        if after_wrapper && word.starts_with('-') {
            continue;
        }
        return Some(word.rsplit('/').next().unwrap_or(word));
    }
    None
}

fn command_mentions_program(command: &str, program: &str) -> bool {
    command.split(['|', ';', '&']).filter_map(stage_program).any(|prog| prog == program)
}

fn evaluate_check(
    canonical_sandbox: &Path,
    check: &Check,
    recorded: Option<&RecordedCommand>,
    fallback_answers: &[String],
) -> Result<CheckOutcome> {
    match check {
        Check::DirExists { path, label } => {
            let rel = safe_rel_path(path)?;
            match resolve_inside(canonical_sandbox, &rel, false)? {
                ResolvedPath::Existing(resolved) => {
                    if resolved.is_dir() {
                        Ok(outcome(label, true, format!("directory {path} exists"), false))
                    } else {
                        Ok(outcome(label, false, format!("{path} exists but is not a directory"), false))
                    }
                }
                ResolvedPath::Missing => Ok(outcome(label, false, format!("directory {path} does not exist"), false)),
                ResolvedPath::Escaped => {
                    Ok(outcome(label, false, format!("{path} escapes the practice sandbox"), false))
                }
                ResolvedPath::Loop => Ok(outcome(label, false, format!("{path} hits a symlink loop"), false)),
                ResolvedPath::Denied(msg) => Ok(outcome(label, false, format!("cannot inspect {path}: {msg}"), false)),
            }
        }
        Check::FileExists { path, label } => {
            let rel = safe_rel_path(path)?;
            match resolve_inside(canonical_sandbox, &rel, false)? {
                ResolvedPath::Existing(resolved) => {
                    if resolved.is_file() {
                        Ok(outcome(label, true, format!("file {path} exists"), false))
                    } else {
                        Ok(outcome(label, false, format!("{path} exists but is not a regular file"), false))
                    }
                }
                ResolvedPath::Missing => Ok(outcome(label, false, format!("file {path} does not exist"), false)),
                ResolvedPath::Escaped => {
                    Ok(outcome(label, false, format!("{path} escapes the practice sandbox"), false))
                }
                ResolvedPath::Loop => Ok(outcome(label, false, format!("{path} hits a symlink loop"), false)),
                ResolvedPath::Denied(msg) => Ok(outcome(label, false, format!("cannot inspect {path}: {msg}"), false)),
            }
        }
        Check::NotExists { path, label } => {
            let rel = safe_rel_path(path)?;
            let name = rel.file_name().context("validated path has no file name")?;
            match resolve_parent_inside(canonical_sandbox, &rel)? {
                ResolvedPath::Missing => Ok(outcome(label, true, format!("{path} does not exist"), false)),
                ResolvedPath::Escaped => {
                    Ok(outcome(label, false, format!("{path} escapes the practice sandbox"), false))
                }
                ResolvedPath::Loop => Ok(outcome(label, false, format!("{path} hits a symlink loop"), false)),
                ResolvedPath::Denied(msg) => Ok(outcome(label, false, format!("cannot inspect {path}: {msg}"), false)),
                ResolvedPath::Existing(parent_dir) => {
                    if !parent_dir.is_dir() {
                        return Ok(outcome(label, true, format!("{path} does not exist"), false));
                    }
                    match std::fs::symlink_metadata(parent_dir.join(name)) {
                        Err(err) if err.kind() == ErrorKind::NotFound => {
                            Ok(outcome(label, true, format!("{path} does not exist"), false))
                        }
                        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                            Ok(outcome(label, false, format!("cannot inspect {path}: {err}"), false))
                        }
                        Err(err) => Err(err).with_context(|| format!("cannot inspect {path}")),
                        Ok(_) => Ok(outcome(label, false, format!("{path} still exists"), false)),
                    }
                }
            }
        }
        Check::FileContains { path, text, label } => {
            let rel = safe_rel_path(path)?;
            match resolve_inside(canonical_sandbox, &rel, false)? {
                ResolvedPath::Existing(resolved) => match read_text_inside(&resolved, path)? {
                    Ok(content) if content.contains(text.as_str()) => {
                        Ok(outcome(label, true, format!("{path} contains expected text"), false))
                    }
                    Ok(_) => Ok(outcome(label, false, format!("{path} does not contain expected text"), false)),
                    Err(msg) => Ok(outcome(label, false, msg, false)),
                },
                ResolvedPath::Missing => Ok(outcome(label, false, format!("file {path} does not exist"), false)),
                ResolvedPath::Escaped => {
                    Ok(outcome(label, false, format!("{path} escapes the practice sandbox"), false))
                }
                ResolvedPath::Loop => Ok(outcome(label, false, format!("{path} hits a symlink loop"), false)),
                ResolvedPath::Denied(msg) => Ok(outcome(label, false, format!("cannot inspect {path}: {msg}"), false)),
            }
        }
        Check::FileEquals { path, text, label } => {
            let rel = safe_rel_path(path)?;
            match resolve_inside(canonical_sandbox, &rel, false)? {
                ResolvedPath::Existing(resolved) => match read_text_inside(&resolved, path)? {
                    Ok(content) if content.trim() == text.trim() => {
                        Ok(outcome(label, true, format!("{path} matches expected contents"), false))
                    }
                    Ok(_) => Ok(outcome(label, false, format!("{path} contents do not match"), false)),
                    Err(msg) => Ok(outcome(label, false, msg, false)),
                },
                ResolvedPath::Missing => Ok(outcome(label, false, format!("file {path} does not exist"), false)),
                ResolvedPath::Escaped => {
                    Ok(outcome(label, false, format!("{path} escapes the practice sandbox"), false))
                }
                ResolvedPath::Loop => Ok(outcome(label, false, format!("{path} hits a symlink loop"), false)),
                ResolvedPath::Denied(msg) => Ok(outcome(label, false, format!("cannot inspect {path}: {msg}"), false)),
            }
        }
        Check::FileModeHas { path, bits, label } => {
            let rel = safe_rel_path(path)?;
            match resolve_inside(canonical_sandbox, &rel, false)? {
                ResolvedPath::Existing(resolved) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let meta = std::fs::metadata(&resolved)
                            .with_context(|| format!("cannot inspect {}", resolved.display()))?;
                        let mode = meta.permissions().mode() & 0o7777;
                        if (mode & *bits) == *bits {
                            Ok(outcome(label, true, format!("{path} has mode {:03o}", mode & 0o777), false))
                        } else {
                            Ok(outcome(
                                label,
                                false,
                                format!("{path} has mode {:03o} (missing bits {:03o})", mode & 0o777, bits & 0o777),
                                false,
                            ))
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = (resolved, bits);
                        Ok(outcome(label, false, "permission mode checks require Unix", false))
                    }
                }
                ResolvedPath::Missing => Ok(outcome(label, false, format!("{path} does not exist"), false)),
                ResolvedPath::Escaped => {
                    Ok(outcome(label, false, format!("{path} escapes the practice sandbox"), false))
                }
                ResolvedPath::Loop => Ok(outcome(label, false, format!("{path} hits a symlink loop"), false)),
                ResolvedPath::Denied(msg) => Ok(outcome(label, false, format!("cannot inspect {path}: {msg}"), false)),
            }
        }
        Check::FileModeEq { path, mode: expected, label } => {
            let rel = safe_rel_path(path)?;
            match resolve_inside(canonical_sandbox, &rel, false)? {
                ResolvedPath::Existing(resolved) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let meta = std::fs::metadata(&resolved)
                            .with_context(|| format!("cannot inspect {}", resolved.display()))?;
                        let actual = meta.permissions().mode() & 0o777;
                        let want = *expected & 0o777;
                        if actual == want {
                            Ok(outcome(label, true, format!("{path} has mode {actual:03o}"), false))
                        } else {
                            Ok(outcome(
                                label,
                                false,
                                format!("{path} has mode {actual:03o}, expected {want:03o}"),
                                false,
                            ))
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = (resolved, expected);
                        Ok(outcome(label, false, "permission mode checks require Unix", false))
                    }
                }
                ResolvedPath::Missing => Ok(outcome(label, false, format!("{path} does not exist"), false)),
                ResolvedPath::Escaped => {
                    Ok(outcome(label, false, format!("{path} escapes the practice sandbox"), false))
                }
                ResolvedPath::Loop => Ok(outcome(label, false, format!("{path} hits a symlink loop"), false)),
                ResolvedPath::Denied(msg) => Ok(outcome(label, false, format!("cannot inspect {path}: {msg}"), false)),
            }
        }
        Check::IsSymlink { path, target, label } => {
            let rel = safe_rel_path(path)?;
            if let Some(expected_target) = target {
                safe_rel_path(expected_target)?;
            }
            let name = rel.file_name().context("validated path has no file name")?;
            match resolve_parent_inside(canonical_sandbox, &rel)? {
                ResolvedPath::Missing => Ok(outcome(label, false, format!("{path} does not exist"), false)),
                ResolvedPath::Escaped => {
                    Ok(outcome(label, false, format!("{path} escapes the practice sandbox"), false))
                }
                ResolvedPath::Loop => Ok(outcome(label, false, format!("{path} hits a symlink loop"), false)),
                ResolvedPath::Denied(msg) => Ok(outcome(label, false, format!("cannot inspect {path}: {msg}"), false)),
                ResolvedPath::Existing(parent_dir) => {
                    if !parent_dir.is_dir() {
                        return Ok(outcome(label, false, format!("{path} parent is not a directory"), false));
                    }
                    let link_path = parent_dir.join(name);
                    let meta = match std::fs::symlink_metadata(&link_path) {
                        Ok(meta) => meta,
                        Err(err) if err.kind() == ErrorKind::NotFound => {
                            return Ok(outcome(label, false, format!("{path} does not exist"), false));
                        }
                        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                            return Ok(outcome(label, false, format!("cannot inspect {path}: {err}"), false));
                        }
                        Err(err) => return Err(err).with_context(|| format!("cannot inspect {path}")),
                    };
                    if !meta.file_type().is_symlink() {
                        return Ok(outcome(label, false, format!("{path} is not a symbolic link"), false));
                    }
                    if let Some(expected) = target {
                        let actual =
                            std::fs::read_link(&link_path).with_context(|| format!("cannot read link {path}"))?;
                        let actual_str = actual.to_string_lossy();
                        if actual_str == expected.as_str() {
                            Ok(outcome(label, true, format!("{path} -> {actual_str}"), false))
                        } else {
                            Ok(outcome(
                                label,
                                false,
                                format!("{path} points to {actual_str}, expected {expected}"),
                                false,
                            ))
                        }
                    } else {
                        Ok(outcome(label, true, format!("{path} is a symbolic link"), false))
                    }
                }
            }
        }
        Check::RecordedOutput { program, command_contains, output_contains, fallback_answer, label, .. } => {
            let mut detail = String::from("no recorded terminal session found");
            if let Some(rec) = recorded {
                let cmd_ok = command_mentions_program(&rec.command, program);
                let contains_ok = command_contains.as_deref().is_none_or(|needle| rec.command.contains(needle));
                let exit_ok = rec.exit == Some(0);
                let out_ok = rec.output.contains(output_contains.as_str());
                if cmd_ok && contains_ok && exit_ok && out_ok {
                    return Ok(outcome(label, true, format!("verified from recorded `{}`", rec.command), false));
                }
                detail = if rec.command.trim().is_empty() {
                    "no recorded command yet".to_string()
                } else if !cmd_ok {
                    format!("last recorded command `{}` did not run `{program}`", rec.command)
                } else if !exit_ok {
                    match rec.exit {
                        Some(code) => format!("last recorded `{}` exited with code {code}", rec.command),
                        None => format!("last recorded `{}` has no exit code yet", rec.command),
                    }
                } else if !contains_ok {
                    format!(
                        "last recorded `{}` did not include `{}`",
                        rec.command,
                        command_contains.as_deref().unwrap_or("")
                    )
                } else {
                    format!("output of `{}` did not include expected text", rec.command)
                };
            }

            let expected = fallback_answer.trim();
            if fallback_answers.iter().any(|ans| ans.trim().eq_ignore_ascii_case(expected)) {
                return Ok(outcome(label, true, "verified via check question", false));
            }

            Ok(outcome(label, false, detail, true))
        }
    }
}

/// Evaluates every check in `practice` against `sandbox_dir`, plus optional recorded command
/// output and fallback answers.
pub fn evaluate(
    sandbox_dir: &Path,
    practice: &Practice,
    recorded: Option<&RecordedCommand>,
    fallback_answers: &[String],
) -> Result<Report> {
    let canonical_sandbox = std::fs::canonicalize(sandbox_dir)
        .with_context(|| format!("cannot access practice sandbox {}", sandbox_dir.display()))?;
    if !canonical_sandbox.is_dir() {
        bail!("practice sandbox is not a directory: {}", canonical_sandbox.display());
    }

    let mut outcomes = Vec::with_capacity(practice.checks.len());
    for check in &practice.checks {
        outcomes.push(evaluate_check(&canonical_sandbox, check, recorded, fallback_answers)?);
    }
    Ok(Report { outcomes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("huh-practice-{tag}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample_practice(id: &str, setup: Setup, checks: Vec<Check>) -> Practice {
        Practice {
            id: id.into(),
            title: "Test Lab".into(),
            goal: "Complete the task".into(),
            hints: vec!["Try checking the files".into()],
            setup,
            checks,
        }
    }

    #[test]
    fn practice_root_uses_xdg_or_home() {
        assert_eq!(practice_root(Some("/data/xdg"), Some("/home/kid")), Some(PathBuf::from("/data/xdg/huh/practice")));
        assert_eq!(
            practice_root(Some(""), Some("/home/kid")),
            Some(PathBuf::from("/home/kid/.local/share/huh/practice"))
        );
        assert_eq!(practice_root(None, Some("/home/kid")), Some(PathBuf::from("/home/kid/.local/share/huh/practice")));
        assert_eq!(practice_root(None, None), None);
        assert_eq!(practice_root(Some(""), Some("")), None);
    }

    #[test]
    fn safe_rel_path_rejects_traversal_and_invalid_inputs() {
        assert_eq!(safe_rel_path("notes/todo.txt").unwrap(), PathBuf::from("notes/todo.txt"));
        assert_eq!(safe_rel_path("./notes/todo.txt").unwrap(), PathBuf::from("notes/todo.txt"));
        assert_eq!(safe_rel_path(".secret_clue").unwrap(), PathBuf::from(".secret_clue"));

        for bad in ["", "   ", ".", "./", "./.", "/", "/etc/passwd", "..", "../escape", "a/../b", "a/b/../../c", "a\0b"]
        {
            assert!(safe_rel_path(bad).is_err(), "expected safe_rel_path to reject {bad:?}");
        }
    }

    #[test]
    fn validate_practice_checks_id_fields_and_paths() {
        let valid = sample_practice(
            "u2-tidy_desk",
            Setup {
                dirs: vec!["inbox".into()],
                files: vec![SetupFile { path: "draft.txt".into(), content: "hi".into(), mode: Some(0o644) }],
                symlinks: vec![SetupSymlink { link: "cur.txt".into(), target: "draft.txt".into() }],
            },
            vec![Check::FileExists { path: "draft.txt".into(), label: "draft exists".into() }],
        );
        validate_practice(&valid).unwrap();

        for bad_id in ["", "BadCaps", "has space", "bad/slash", "bad.dot", &"a".repeat(41)] {
            let mut p = valid.clone();
            p.id = bad_id.into();
            assert!(validate_practice(&p).is_err(), "expected invalid id {bad_id:?}");
        }

        let mut no_hints = valid.clone();
        no_hints.hints.clear();
        assert!(validate_practice(&no_hints).is_err());

        let mut no_checks = valid.clone();
        no_checks.checks.clear();
        assert!(validate_practice(&no_checks).is_err());

        let mut bad_setup = valid.clone();
        bad_setup.setup.files.push(SetupFile { path: "../escape".into(), content: String::new(), mode: None });
        assert!(validate_practice(&bad_setup).is_err());

        let mut bad_sym = valid.clone();
        bad_sym.setup.symlinks.push(SetupSymlink { link: "x".into(), target: "/etc/passwd".into() });
        assert!(validate_practice(&bad_sym).is_err());

        let mut bad_recorded = valid.clone();
        bad_recorded.checks = vec![Check::RecordedOutput {
            program: String::new(),
            command_contains: None,
            output_contains: "ok".into(),
            fallback_prompt: "q".into(),
            fallback_answer: "a".into(),
            label: "run".into(),
        }];
        assert!(validate_practice(&bad_recorded).is_err());
    }

    #[test]
    fn prepare_populates_and_is_idempotent_with_cleanup() {
        let root = TempDir::new("prepare");
        let practice = sample_practice(
            "u4-lock-and-run",
            Setup {
                dirs: vec!["sub/nested".into()],
                files: vec![
                    SetupFile { path: "deploy.sh".into(), content: "#!/bin/sh\necho hi\n".into(), mode: Some(0o644) },
                    SetupFile { path: "sub/secret.key".into(), content: "top-secret".into(), mode: Some(0o600) },
                ],
                symlinks: vec![SetupSymlink { link: "latest.sh".into(), target: "deploy.sh".into() }],
            },
            vec![Check::FileExists { path: "deploy.sh".into(), label: "deploy exists".into() }],
        );

        let sb = Sandbox::prepare(root.path(), &practice).unwrap();
        assert!(sb.dir().join("sub/nested").is_dir());
        assert_eq!(std::fs::read_to_string(sb.dir().join("deploy.sh")).unwrap(), "#!/bin/sh\necho hi\n");
        assert_eq!(std::fs::read_link(sb.dir().join("latest.sh")).unwrap(), PathBuf::from("deploy.sh"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(sb.dir().join("deploy.sh")).unwrap().permissions().mode() & 0o777, 0o644);
            assert_eq!(std::fs::metadata(sb.dir().join("sub/secret.key")).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // Add stray files, nested read-only subdirs, and a symlink pointing outside the sandbox.
        let outside = TempDir::new("outside-safe");
        let precious = outside.path().join("precious.txt");
        std::fs::write(&precious, "keep me").unwrap();

        std::fs::write(sb.dir().join("stray.txt"), "junk").unwrap();
        let locked_dir = sb.dir().join("locked");
        let nested_locked = locked_dir.join("deeper");
        std::fs::create_dir_all(&nested_locked).unwrap();
        std::fs::write(nested_locked.join("inside.txt"), "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::os::unix::fs::symlink(outside.path(), sb.dir().join("escape_dir")).unwrap();
            std::fs::set_permissions(&nested_locked, std::fs::Permissions::from_mode(0o000)).unwrap();
            std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        }

        // Re-preparing resets the sandbox without touching `outside`.
        let sb2 = Sandbox::prepare(root.path(), &practice).unwrap();
        assert!(!sb2.dir().join("stray.txt").exists());
        assert!(!sb2.dir().join("locked").exists());
        assert!(sb2.dir().join("deploy.sh").is_file());
        assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");

        // Cleanup is idempotent and leaves outside untouched.
        sb2.cleanup().unwrap();
        assert!(!sb2.dir().exists());
        sb2.cleanup().unwrap();
        assert_eq!(std::fs::read_to_string(&precious).unwrap(), "keep me");
    }

    #[cfg(unix)]
    #[test]
    fn prepare_and_cleanup_refuse_symlink_sandbox_dir() {
        let root = TempDir::new("sym-root");
        let outside = TempDir::new("sym-outside");
        let victim = outside.path().join("keep.txt");
        std::fs::write(&victim, "untouched").unwrap();

        let practice = sample_practice(
            "u2-tidy-desk",
            Setup::default(),
            vec![Check::FileExists { path: "a.txt".into(), label: "a".into() }],
        );

        std::os::unix::fs::symlink(outside.path(), root.path().join("u2-tidy-desk")).unwrap();
        assert!(Sandbox::prepare(root.path(), &practice).is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");

        let fake_sb = Sandbox {
            root: std::fs::canonicalize(root.path()).unwrap(),
            dir: std::fs::canonicalize(root.path()).unwrap().join("u2-tidy-desk"),
        };
        assert!(fake_sb.cleanup().is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    }

    #[test]
    fn evaluate_filesystem_and_permission_and_symlink_checks() {
        let root = TempDir::new("eval-fs");
        let practice = sample_practice(
            "u-all-fs",
            Setup {
                dirs: vec![],
                files: vec![
                    SetupFile { path: "draft.txt".into(), content: "hello\n".into(), mode: Some(0o644) },
                    SetupFile { path: "deploy.sh".into(), content: "#!/bin/sh\n".into(), mode: Some(0o644) },
                    SetupFile { path: "secret.key".into(), content: "k".into(), mode: Some(0o644) },
                    SetupFile { path: "notes_v2.txt".into(), content: "v2\n".into(), mode: Some(0o644) },
                    SetupFile { path: "old.tmp".into(), content: "bye".into(), mode: Some(0o644) },
                ],
                symlinks: vec![],
            },
            vec![
                Check::DirExists { path: "notes".into(), label: "dir".into() },
                Check::FileExists { path: "notes/final.txt".into(), label: "file".into() },
                Check::NotExists { path: "old.tmp".into(), label: "removed".into() },
                Check::FileContains {
                    path: "notes/final.txt".into(),
                    text: "learn linux".into(),
                    label: "contains".into(),
                },
                Check::FileEquals { path: "notes/exact.txt".into(), text: "exact line".into(), label: "equals".into() },
                Check::FileModeHas { path: "deploy.sh".into(), bits: 0o100, label: "exec".into() },
                Check::FileModeEq { path: "secret.key".into(), mode: 0o600, label: "mode600".into() },
                Check::IsSymlink {
                    path: "latest.txt".into(),
                    target: Some("notes_v2.txt".into()),
                    label: "symlink".into(),
                },
            ],
        );

        let sb = Sandbox::prepare(root.path(), &practice).unwrap();
        let before = evaluate(sb.dir(), &practice, None, &[]).unwrap();
        assert!(!before.all_passed());
        assert!(before.outcomes.iter().all(|o| !o.passed));

        // Perform all required changes inside the sandbox.
        std::fs::create_dir(sb.dir().join("notes")).unwrap();
        std::fs::write(sb.dir().join("notes/final.txt"), "time to learn linux!\n").unwrap();
        std::fs::write(sb.dir().join("notes/exact.txt"), "  exact line \n").unwrap();
        std::fs::remove_file(sb.dir().join("old.tmp")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(sb.dir().join("deploy.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::set_permissions(sb.dir().join("secret.key"), std::fs::Permissions::from_mode(0o600)).unwrap();
            std::os::unix::fs::symlink("notes_v2.txt", sb.dir().join("latest.txt")).unwrap();
        }

        let after = evaluate(sb.dir(), &practice, None, &[]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(after.all_passed(), "expected all checks to pass: {:#?}", after.outcomes);

            // An unreadable file (chmod 000) reports a clean failure rather than erroring out.
            std::fs::set_permissions(sb.dir().join("notes/final.txt"), std::fs::Permissions::from_mode(0o000)).unwrap();
            let unreadable = sample_practice(
                "unreadable",
                Setup::default(),
                vec![Check::FileContains {
                    path: "notes/final.txt".into(),
                    text: "learn linux".into(),
                    label: "unreadable".into(),
                }],
            );
            let r_unreadable = evaluate(sb.dir(), &unreadable, None, &[]).unwrap();
            assert!(!r_unreadable.all_passed());
            assert!(r_unreadable.outcomes[0].detail.contains("permission denied"));
        }
        #[cfg(not(unix))]
        let _ = after;
    }

    #[cfg(unix)]
    #[test]
    fn evaluate_rejects_symlink_escapes_and_dotdot() {
        use std::os::unix::fs::PermissionsExt;

        let root = TempDir::new("escape-root");
        let outside = TempDir::new("escape-outside");
        let secret_file = outside.path().join("secret.txt");
        std::fs::write(&secret_file, "learn linux\n").unwrap();
        std::fs::set_permissions(&secret_file, std::fs::Permissions::from_mode(0o700)).unwrap();
        let outside_dir = outside.path().join("dir");
        std::fs::create_dir(&outside_dir).unwrap();
        std::os::unix::fs::symlink("secret.txt", outside_dir.join("sym")).unwrap();

        let base = sample_practice(
            "escape-lab",
            Setup::default(),
            vec![
                Check::FileExists { path: "f_abs".into(), label: "file abs".into() },
                Check::FileContains { path: "f_abs".into(), text: "learn linux".into(), label: "contains abs".into() },
                Check::FileEquals { path: "f_rel".into(), text: "learn linux".into(), label: "equals rel".into() },
                Check::FileModeHas { path: "f_abs".into(), bits: 0o100, label: "mode has".into() },
                Check::FileModeEq { path: "f_abs".into(), mode: 0o700, label: "mode eq".into() },
                Check::DirExists { path: "d_abs".into(), label: "dir abs".into() },
                Check::NotExists { path: "d_abs/nonexistent".into(), label: "not exists outside".into() },
                Check::IsSymlink { path: "d_abs/sym".into(), target: None, label: "symlink outside".into() },
            ],
        );

        let sb = Sandbox::prepare(root.path(), &base).unwrap();
        std::os::unix::fs::symlink(&secret_file, sb.dir().join("f_abs")).unwrap();
        std::os::unix::fs::symlink("../../nonexistent_or_outside", sb.dir().join("f_rel")).unwrap();
        std::os::unix::fs::symlink(&outside_dir, sb.dir().join("d_abs")).unwrap();

        let report = evaluate(sb.dir(), &base, None, &[]).unwrap();
        assert!(!report.all_passed());
        for outcome in &report.outcomes {
            assert!(!outcome.passed, "check `{}` should not pass on symlink escape", outcome.label);
            assert!(
                outcome.detail.contains("escapes"),
                "unexpected detail for `{}`: {}",
                outcome.label,
                outcome.detail
            );
        }

        // Absolute symlink whose prefix matches canonical_sandbox before escaping with `..`.
        let tricky = sb.dir().join("..").join("escape_via_abs_dotdot");
        std::os::unix::fs::symlink(&tricky, sb.dir().join("f_abs_dotdot")).unwrap();
        let abs_dotdot = sample_practice(
            "abs-dotdot",
            Setup::default(),
            vec![Check::FileExists { path: "f_abs_dotdot".into(), label: "abs dotdot".into() }],
        );
        let r_dotdot = evaluate(sb.dir(), &abs_dotdot, None, &[]).unwrap();
        assert!(!r_dotdot.all_passed());
        assert!(r_dotdot.outcomes[0].detail.contains("escapes"));

        // Symlink loop fails cleanly without hanging.
        std::os::unix::fs::symlink("loop_b", sb.dir().join("loop_a")).unwrap();
        std::os::unix::fs::symlink("loop_a", sb.dir().join("loop_b")).unwrap();
        let loop_lab = sample_practice(
            "loop-lab",
            Setup::default(),
            vec![Check::FileExists { path: "loop_a".into(), label: "loop".into() }],
        );
        let r_loop = evaluate(sb.dir(), &loop_lab, None, &[]).unwrap();
        assert!(!r_loop.all_passed());
        assert!(r_loop.outcomes[0].detail.contains("loop"));

        // Dangling symlink still counts as existing for NotExists, and still matches IsSymlink.
        std::os::unix::fs::symlink("missing_target", sb.dir().join("dangling")).unwrap();
        let dangling_lab = sample_practice(
            "dangling-lab",
            Setup::default(),
            vec![
                Check::NotExists { path: "dangling".into(), label: "not exists dangling".into() },
                Check::IsSymlink { path: "dangling".into(), target: None, label: "any symlink".into() },
                Check::IsSymlink {
                    path: "dangling".into(),
                    target: Some("other".into()),
                    label: "wrong target".into(),
                },
            ],
        );
        let r_dangling = evaluate(sb.dir(), &dangling_lab, None, &[]).unwrap();
        assert!(!r_dangling.outcomes[0].passed);
        assert!(r_dangling.outcomes[1].passed);
        assert!(!r_dangling.outcomes[2].passed);

        // Direct `..` in Check path returns an error.
        let bad = sample_practice(
            "bad-check",
            Setup::default(),
            vec![Check::FileExists { path: "../outside".into(), label: "bad".into() }],
        );
        assert!(evaluate(sb.dir(), &bad, None, &[]).is_err());
    }

    #[test]
    fn evaluate_recorded_output_and_fallback() {
        let root = TempDir::new("recorded");
        let practice = sample_practice(
            "u1-hidden-clue",
            Setup::default(),
            vec![Check::RecordedOutput {
                program: "ls".into(),
                command_contains: Some("-a".into()),
                output_contains: ".secret_clue".into(),
                fallback_prompt: "What secret phrase is inside .secret_clue?".into(),
                fallback_answer: "penguin-tracks".into(),
                label: "List hidden files".into(),
            }],
        );
        let sb = Sandbox::prepare(root.path(), &practice).unwrap();

        // Neither recorded nor fallback -> fails and requests fallback.
        let r0 = evaluate(sb.dir(), &practice, None, &[]).unwrap();
        assert!(!r0.all_passed());
        assert!(r0.outcomes[0].needs_fallback);

        // Wrong program / nonzero exit / missing flag / missing output all fail.
        let wrong_prog = RecordedCommand { command: "echo ls -a".into(), exit: Some(0), output: ".secret_clue".into() };
        assert!(!evaluate(sb.dir(), &practice, Some(&wrong_prog), &[]).unwrap().all_passed());

        let nonzero = RecordedCommand { command: "ls -a".into(), exit: Some(2), output: ".secret_clue".into() };
        assert!(!evaluate(sb.dir(), &practice, Some(&nonzero), &[]).unwrap().all_passed());

        let no_flag = RecordedCommand { command: "ls".into(), exit: Some(0), output: ".secret_clue".into() };
        assert!(!evaluate(sb.dir(), &practice, Some(&no_flag), &[]).unwrap().all_passed());

        // Matching recorded command passes (including env/sudo/pipeline forms).
        let ok_rec = RecordedCommand {
            command: "LC_ALL=C sudo ls -a | cat".into(),
            exit: Some(0),
            output: ".secret_clue\nreadme.txt".into(),
        };
        let r1 = evaluate(sb.dir(), &practice, Some(&ok_rec), &[]).unwrap();
        assert!(r1.all_passed());
        assert!(!r1.outcomes[0].needs_fallback);

        // Without command_contains, `cd ... && /bin/ls -la` matches program = "ls".
        let any_ls = sample_practice(
            "u1-any-ls",
            Setup::default(),
            vec![Check::RecordedOutput {
                program: "ls".into(),
                command_contains: None,
                output_contains: ".secret_clue".into(),
                fallback_prompt: "q".into(),
                fallback_answer: "penguin-tracks".into(),
                label: "List hidden files".into(),
            }],
        );
        let chained =
            RecordedCommand { command: "cd /tmp && /bin/ls -la".into(), exit: Some(0), output: ".secret_clue".into() };
        assert!(evaluate(sb.dir(), &any_ls, Some(&chained), &[]).unwrap().all_passed());

        // Fallback answer passes case-insensitively and trimmed even without a recording.
        let r2 = evaluate(sb.dir(), &practice, None, &["  PENGUIN-TRACKS \n".into()]).unwrap();
        assert!(r2.all_passed());
        assert!(!r2.outcomes[0].needs_fallback);

        // Empty Report never passes.
        assert!(!Report { outcomes: vec![] }.all_passed());
    }

    #[test]
    fn latest_recorded_scans_active_recordings_in_dir() {
        let rec_dir = TempDir::new("rec-scan");
        let live_pid = std::process::id();

        // Dead PID should be ignored on Linux where /proc exists.
        std::fs::write(rec_dir.path().join("999999999.status"), "0\ndead-cmd\n").unwrap();
        std::fs::write(rec_dir.path().join("999999999.log"), "\x1b]7770;huh;start\x07dead\x1b]7770;huh;end\x07")
            .unwrap();

        // Live PID recording should be parsed cleanly.
        std::fs::write(rec_dir.path().join(format!("{live_pid}.status")), "0\nls -la\n").unwrap();
        std::fs::write(
            rec_dir.path().join(format!("{live_pid}.log")),
            "old\x1b]7770;huh;start\x07\x1b[32m.secret_clue\x1b[0m\r\nreadme.txt\r\n\x1b]7770;huh;end\x07",
        )
        .unwrap();

        let rec = latest_recorded_from_env_or_dir(Some(rec_dir.path())).unwrap();
        assert_eq!(rec.command, "ls -la");
        assert_eq!(rec.exit, Some(0));
        assert_eq!(rec.output, ".secret_clue\nreadme.txt");
    }

    #[test]
    fn toml_deserializes_all_check_variants() {
        let raw = r#"
id = "u7-all-checks"
title = "All Checks"
goal = "Exercise every variant"
hints = ["Hint 1"]

[setup]
dirs = ["sub"]

[[setup.files]]
path = "a.txt"
content = "hello"
mode = 0o644

[[setup.symlinks]]
link = "link.txt"
target = "a.txt"

[[checks]]
kind = "dir_exists"
path = "sub"
label = "sub dir"

[[checks]]
kind = "file_exists"
path = "a.txt"
label = "a file"

[[checks]]
kind = "not_exists"
path = "gone.txt"
label = "gone"

[[checks]]
kind = "file_contains"
path = "a.txt"
text = "hel"
label = "contains"

[[checks]]
kind = "file_equals"
path = "a.txt"
text = "hello"
label = "equals"

[[checks]]
kind = "file_mode_has"
path = "a.txt"
bits = 0o400
label = "readable"

[[checks]]
kind = "file_mode_eq"
path = "a.txt"
mode = 0o644
label = "644"

[[checks]]
kind = "is_symlink"
path = "link.txt"
target = "a.txt"
label = "symlink"

[[checks]]
kind = "recorded_output"
program = "ls"
output_contains = "a.txt"
fallback_prompt = "What file?"
fallback_answer = "a.txt"
label = "recorded"
"#;
        let parsed: Practice = toml::from_str(raw).unwrap();
        validate_practice(&parsed).unwrap();
        assert_eq!(parsed.checks.len(), 9);
        let labels: Vec<&str> = parsed.checks.iter().map(Check::label).collect();
        assert_eq!(
            labels,
            vec!["sub dir", "a file", "gone", "contains", "equals", "readable", "644", "symlink", "recorded"]
        );
    }
}
