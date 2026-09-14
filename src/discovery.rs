//! Repository discovery and source-tree inventory.
//!
//! Doctor owns project-level discovery: locating the workspace root,
//! enumerating MNCS sources and manifests, applying exclusion policy, and
//! producing a deterministic inventory. It never interprets file contents
//! beyond the byte-level facts recorded here (newline style, BOM, size,
//! content fingerprint); header semantics live in [`crate::diagnostics`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Marker files that identify an MNCS workspace root (first hit wins, in
/// walk-up order from the start directory).
const ROOT_MARKERS: &[&str] = &["mncs-forge.toml", "mncs-workspace.toml", ".mncs-forge"];

/// File extensions inventoried as MNCS sources.
const SOURCE_EXTENSIONS: &[&str] = &["mncs"];

/// Facts acquired by the host before MNCS decides whether a directory should
/// be visited. Names are compact identity codes because arbitrary strings do
/// not yet cross the bounded value boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryFacts {
    pub name_code: u64,
    pub extra_excluded: bool,
    pub is_symlink: bool,
    pub follow_symlink: bool,
    pub cycle: bool,
    pub depth: u64,
    pub max_depth: u64,
}

/// MNCS directory policy verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryDecision {
    Descend,
    SkipDepth,
    SkipExcluded,
    SkipCycle,
    SkipSymlink,
}

impl DirectoryDecision {
    fn reason(self) -> &'static str {
        match self {
            Self::Descend => "",
            Self::SkipDepth => "max depth exceeded",
            Self::SkipExcluded => "excluded directory",
            Self::SkipCycle => "symlink cycle",
            Self::SkipSymlink => "symlinked directory (policy=skip)",
        }
    }
}

/// Facts used to classify one regular file or a followed file symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFacts {
    pub name_code: u64,
    pub extension_code: u64,
}

/// MNCS file classification. The host retains path and byte acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    Ignore,
    Source,
    ForgeManifest,
    WorkspaceManifest,
    ManifestJson,
    Cargo,
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("workspace root has no file name: {0}")]
    BadRoot(PathBuf),
    #[error("MNCS discovery policy failed: {0}")]
    Policy(String),
}

fn io_err(path: &Path, source: std::io::Error) -> DiscoveryError {
    DiscoveryError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// How directory symlinks are treated during traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymlinkPolicy {
    /// Do not descend into symlinked directories; record them as skipped.
    #[default]
    Skip,
    /// Descend into symlinked directories (cycle-guarded by visited set).
    Follow,
}

/// Options controlling discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryOptions {
    /// Extra directory names to exclude (exact match, any depth).
    #[serde(default)]
    pub extra_excluded_dirs: Vec<String>,
    /// Maximum traversal depth relative to the root (0 = root only).
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    /// Symlink traversal policy.
    #[serde(default)]
    pub symlink_policy: SymlinkPolicy,
    /// Follow file symlinks and read their targets (default true; the link
    /// itself is still recorded as a symlink).
    #[serde(default = "default_true")]
    pub follow_file_symlinks: bool,
}

fn default_max_depth() -> usize {
    64
}

fn default_true() -> bool {
    true
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            extra_excluded_dirs: Vec::new(),
            max_depth: default_max_depth(),
            symlink_policy: SymlinkPolicy::Skip,
            follow_file_symlinks: true,
        }
    }
}

/// Newline convention observed in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NewlineStyle {
    Lf,
    Crlf,
    Mixed,
    None,
}

impl fmt::Display for NewlineStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NewlineStyle::Lf => write!(f, "lf"),
            NewlineStyle::Crlf => write!(f, "crlf"),
            NewlineStyle::Mixed => write!(f, "mixed"),
            NewlineStyle::None => write!(f, "none"),
        }
    }
}

/// Detect newline style from raw bytes.
pub fn detect_newline(bytes: &[u8]) -> NewlineStyle {
    let mut crlf = 0usize;
    let mut lf = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            crlf += 1;
            i += 2;
            continue;
        }
        if bytes[i] == b'\n' {
            lf += 1;
        }
        i += 1;
    }
    match (crlf, lf) {
        (0, 0) => NewlineStyle::None,
        (_, 0) => NewlineStyle::Crlf,
        (0, _) => NewlineStyle::Lf,
        _ => NewlineStyle::Mixed,
    }
}

/// SHA-256 hex fingerprint of content.
pub fn fingerprint(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
    }
    out
}

/// A single inventoried source file with byte-level facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFile {
    /// Absolute path.
    pub path: PathBuf,
    /// Path relative to the workspace root, slash-separated.
    pub relative: String,
    /// Raw content.
    #[serde(skip)]
    pub bytes: Vec<u8>,
    /// UTF-8 text when the file is valid UTF-8.
    #[serde(skip)]
    pub text: Option<String>,
    /// SHA-256 of `bytes`.
    pub sha256: String,
    /// Byte length.
    pub len: u64,
    /// Observed newline convention.
    pub newline: NewlineStyle,
    /// Whether a UTF-8 BOM is present.
    pub has_bom: bool,
    /// Unix permission bits (preserved across transactions).
    pub mode: Option<u32>,
    /// True when `path` itself is a symlink.
    pub is_symlink: bool,
}

/// A manifest sidecar found during discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestHit {
    pub path: PathBuf,
    pub relative: String,
    pub kind: ManifestKind,
}

/// Recognised manifest kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestKind {
    Forge,
    Workspace,
    ManifestJson,
    Cargo,
}

/// A directory skipped during traversal, with the reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedDir {
    pub relative: String,
    pub reason: String,
}

/// Deterministic inventory of a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub root: PathBuf,
    /// Sources sorted by `relative`.
    pub sources: Vec<SourceFile>,
    /// Manifests sorted by `relative`.
    pub manifests: Vec<ManifestHit>,
    /// Skipped directories sorted by `relative`.
    pub skipped: Vec<SkippedDir>,
    /// Per-extension file counts (includes non-source files seen).
    pub extension_counts: BTreeMap<String, u64>,
    /// Runtime-only accounting for incremental inventory reuse.
    #[serde(skip)]
    pub metrics: InventoryMetrics,
}

/// Evidence accounting for one inventory acquisition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventoryMetrics {
    pub files_reused: usize,
    pub files_rescanned: usize,
    pub cache_identity: Option<String>,
    pub invalidation_reason: Option<String>,
}

impl Inventory {
    /// Summary counts for reports.
    pub fn summary(&self) -> InventorySummary {
        InventorySummary {
            files_checked: self.sources.len(),
            manifests: self.manifests.len(),
            skipped_dirs: self.skipped.len(),
            total_bytes: self.sources.iter().map(|s| s.len).sum(),
            files_reused: self.metrics.files_reused,
            files_rescanned: self.metrics.files_rescanned,
            cache_identity: self.metrics.cache_identity.clone(),
            invalidation_reason: self.metrics.invalidation_reason.clone(),
        }
    }
}

/// Aggregate inventory counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventorySummary {
    pub files_checked: usize,
    pub manifests: usize,
    pub skipped_dirs: usize,
    pub total_bytes: u64,
    pub files_reused: usize,
    pub files_rescanned: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalidation_reason: Option<String>,
}

/// Locate the workspace root by walking up from `start` looking for marker
/// files. Falls back to `start` itself (canonicalized) when no marker is
/// found; the fallback is reported by callers, not hidden.
pub fn find_root(start: &Path) -> Result<PathBuf, DiscoveryError> {
    let mut dir = canonicalize(start)?;
    loop {
        for marker in ROOT_MARKERS {
            if dir.join(marker).exists() {
                return Ok(dir);
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return canonicalize(start),
        }
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, DiscoveryError> {
    std::fs::canonicalize(path).map_err(|e| io_err(path, e))
}

/// Discover sources and manifests under `root`.
///
/// Traversal is deterministic: directory entries are visited in sorted order
/// and all output vectors are sorted by slash-separated relative path.
pub fn discover(root: &Path, options: &DiscoveryOptions) -> Result<Inventory, DiscoveryError> {
    let directory_policy = |facts| Ok(rust_directory_decision(facts));
    let file_policy = |facts| Ok(rust_file_class(facts));
    discover_with_policy(root, options, &directory_policy, &file_policy)
}

/// Discover using explicit host-facts → policy callbacks. Production Doctor
/// supplies MNCS callbacks; `discover` above keeps an independent Rust
/// reference policy for differential tests and non-production callers.
pub fn discover_with_policy(
    root: &Path,
    options: &DiscoveryOptions,
    directory_policy: &dyn Fn(DirectoryFacts) -> Result<DirectoryDecision, String>,
    file_policy: &dyn Fn(FileFacts) -> Result<FileClass, String>,
) -> Result<Inventory, DiscoveryError> {
    let mut sources = Vec::new();
    let mut manifests = Vec::new();
    let mut skipped = Vec::new();
    let mut extension_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut visited: Vec<(u64, u64)> = Vec::new();
    visit_dir(
        root,
        root,
        0,
        options,
        &mut sources,
        &mut manifests,
        &mut skipped,
        &mut extension_counts,
        &mut visited,
        directory_policy,
        file_policy,
    )?;
    sources.sort_by(|a: &SourceFile, b: &SourceFile| a.relative.cmp(&b.relative));
    manifests.sort_by(|a: &ManifestHit, b: &ManifestHit| a.relative.cmp(&b.relative));
    skipped.sort_by(|a: &SkippedDir, b: &SkippedDir| a.relative.cmp(&b.relative));
    let files_rescanned = sources.len();
    Ok(Inventory {
        root: root.to_path_buf(),
        sources,
        manifests,
        skipped,
        extension_counts,
        metrics: InventoryMetrics {
            files_rescanned,
            ..InventoryMetrics::default()
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn visit_dir(
    root: &Path,
    dir: &Path,
    depth: usize,
    options: &DiscoveryOptions,
    sources: &mut Vec<SourceFile>,
    manifests: &mut Vec<ManifestHit>,
    skipped: &mut Vec<SkippedDir>,
    extension_counts: &mut BTreeMap<String, u64>,
    visited: &mut Vec<(u64, u64)>,
    directory_policy: &dyn Fn(DirectoryFacts) -> Result<DirectoryDecision, String>,
    file_policy: &dyn Fn(FileFacts) -> Result<FileClass, String>,
) -> Result<(), DiscoveryError> {
    let boundary = directory_policy(DirectoryFacts {
        name_code: 0,
        extra_excluded: false,
        is_symlink: false,
        follow_symlink: false,
        cycle: false,
        depth: depth as u64,
        max_depth: options.max_depth as u64,
    })
    .map_err(DiscoveryError::Policy)?;
    if boundary != DirectoryDecision::Descend {
        skipped.push(SkippedDir {
            relative: rel_string(root, dir),
            reason: boundary.reason().to_owned(),
        });
        return Ok(());
    }
    let read_dir = fs::read_dir(dir).map_err(|e| io_err(dir, e))?;
    let mut entries: Vec<_> = read_dir
        .collect::<Result<_, _>>()
        .map_err(|e| io_err(dir, e))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| io_err(&path, e))?;
        if file_type.is_symlink() {
            handle_symlink(
                root,
                &path,
                depth,
                options,
                sources,
                manifests,
                skipped,
                extension_counts,
                visited,
                directory_policy,
                file_policy,
            )?;
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let decision = directory_policy(DirectoryFacts {
                name_code: directory_name_code(&name),
                extra_excluded: options
                    .extra_excluded_dirs
                    .iter()
                    .any(|excluded| excluded == &name),
                is_symlink: false,
                follow_symlink: false,
                cycle: false,
                depth: (depth + 1) as u64,
                max_depth: options.max_depth as u64,
            })
            .map_err(DiscoveryError::Policy)?;
            if decision != DirectoryDecision::Descend {
                skipped.push(SkippedDir {
                    relative: rel_string(root, &path),
                    reason: decision.reason().to_owned(),
                });
                continue;
            }
            visit_dir(
                root,
                &path,
                depth + 1,
                options,
                sources,
                manifests,
                skipped,
                extension_counts,
                visited,
                directory_policy,
                file_policy,
            )?;
            continue;
        }
        if file_type.is_file() {
            record_file(
                root,
                &path,
                false,
                sources,
                manifests,
                extension_counts,
                file_policy,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_symlink(
    root: &Path,
    path: &Path,
    depth: usize,
    options: &DiscoveryOptions,
    sources: &mut Vec<SourceFile>,
    manifests: &mut Vec<ManifestHit>,
    skipped: &mut Vec<SkippedDir>,
    extension_counts: &mut BTreeMap<String, u64>,
    visited: &mut Vec<(u64, u64)>,
    directory_policy: &dyn Fn(DirectoryFacts) -> Result<DirectoryDecision, String>,
    file_policy: &dyn Fn(FileFacts) -> Result<FileClass, String>,
) -> Result<(), DiscoveryError> {
    let meta = fs::metadata(path);
    match meta {
        Ok(md) if md.is_dir() => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let id = dir_id(&md);
            let decision = directory_policy(DirectoryFacts {
                name_code: directory_name_code(&name),
                extra_excluded: false,
                is_symlink: true,
                follow_symlink: options.symlink_policy == SymlinkPolicy::Follow,
                cycle: visited.contains(&id),
                depth: (depth + 1) as u64,
                max_depth: options.max_depth as u64,
            })
            .map_err(DiscoveryError::Policy)?;
            if decision == DirectoryDecision::Descend {
                visited.push(id);
                visit_dir(
                    root,
                    path,
                    depth + 1,
                    options,
                    sources,
                    manifests,
                    skipped,
                    extension_counts,
                    visited,
                    directory_policy,
                    file_policy,
                )?;
            } else {
                skipped.push(SkippedDir {
                    relative: rel_string(root, path),
                    reason: decision.reason().to_owned(),
                });
            }
            Ok(())
        }
        Ok(md) if md.is_file() => {
            if options.follow_file_symlinks {
                record_file(
                    root,
                    path,
                    true,
                    sources,
                    manifests,
                    extension_counts,
                    file_policy,
                )?;
            } else {
                skipped.push(SkippedDir {
                    relative: rel_string(root, path),
                    reason: "symlinked file (policy=skip)".to_owned(),
                });
            }
            Ok(())
        }
        _ => {
            skipped.push(SkippedDir {
                relative: rel_string(root, path),
                reason: "unresolvable symlink".to_owned(),
            });
            Ok(())
        }
    }
}

#[cfg(unix)]
fn dir_id(md: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (md.dev(), md.ino())
}

#[cfg(not(unix))]
fn dir_id(_md: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

fn rust_directory_decision(facts: DirectoryFacts) -> DirectoryDecision {
    if facts.depth > facts.max_depth {
        return DirectoryDecision::SkipDepth;
    }
    if !facts.is_symlink && (facts.extra_excluded || is_excluded_name_code(facts.name_code)) {
        return DirectoryDecision::SkipExcluded;
    }
    if facts.is_symlink && facts.cycle {
        return DirectoryDecision::SkipCycle;
    }
    if facts.is_symlink && !facts.follow_symlink {
        return DirectoryDecision::SkipSymlink;
    }
    DirectoryDecision::Descend
}

fn rust_file_class(facts: FileFacts) -> FileClass {
    match facts.name_code {
        1 => FileClass::ForgeManifest,
        2 => FileClass::WorkspaceManifest,
        3 => FileClass::Cargo,
        4 => FileClass::ManifestJson,
        _ if facts.extension_code == 1 => FileClass::Source,
        _ => FileClass::Ignore,
    }
}

fn directory_name_code(name: &str) -> u64 {
    match name {
        ".git" => 1,
        ".hg" => 2,
        ".svn" => 3,
        "target" => 4,
        "node_modules" => 5,
        "venv" => 6,
        "__pycache__" => 7,
        ".pytest_cache" => 8,
        ".ruff_cache" => 9,
        ".mypy_cache" => 10,
        "dist" => 11,
        ".worktrees" => 12,
        ".tox" => 13,
        ".idea" => 14,
        ".vscode" => 15,
        ".atlas-joern-baseline" => 16,
        ".venv" => 17,
        _ => 0,
    }
}

fn is_excluded_name_code(code: u64) -> bool {
    (1..=17).contains(&code)
}

fn file_name_code(name: &str) -> u64 {
    match name {
        "mncs-forge.toml" => 1,
        "mncs-workspace.toml" => 2,
        "Cargo.toml" => 3,
        _ if name.ends_with(".mncs.json") => 4,
        _ => 0,
    }
}

fn record_file(
    root: &Path,
    path: &Path,
    is_symlink: bool,
    sources: &mut Vec<SourceFile>,
    manifests: &mut Vec<ManifestHit>,
    extension_counts: &mut BTreeMap<String, u64>,
    file_policy: &dyn Fn(FileFacts) -> Result<FileClass, String>,
) -> Result<(), DiscoveryError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    *extension_counts.entry(ext.clone()).or_insert(0) += 1;

    let class = file_policy(FileFacts {
        name_code: file_name_code(&name),
        extension_code: u64::from(SOURCE_EXTENSIONS.contains(&ext.as_str())),
    })
    .map_err(DiscoveryError::Policy)?;
    match class {
        FileClass::ForgeManifest
        | FileClass::WorkspaceManifest
        | FileClass::ManifestJson
        | FileClass::Cargo => {
            let kind = match class {
                FileClass::ForgeManifest => ManifestKind::Forge,
                FileClass::WorkspaceManifest => ManifestKind::Workspace,
                FileClass::ManifestJson => ManifestKind::ManifestJson,
                FileClass::Cargo => ManifestKind::Cargo,
                _ => unreachable!(),
            };
            manifests.push(ManifestHit {
                path: path.to_path_buf(),
                relative: rel_string(root, path),
                kind,
            });
        }
        FileClass::Source => {
            sources.push(read_source_file(root, path, is_symlink)?);
        }
        FileClass::Ignore => {}
    }
    Ok(())
}

/// Read one already-identified source without traversing its repository.
pub fn read_source_file(
    root: &Path,
    path: &Path,
    is_symlink: bool,
) -> Result<SourceFile, DiscoveryError> {
    let bytes = fs::read(path).map_err(|e| io_err(path, e))?;
    let mode = file_mode(path);
    let text = String::from_utf8(bytes.clone()).ok();
    Ok(SourceFile {
        relative: rel_string(root, path),
        path: path.to_path_buf(),
        sha256: fingerprint(&bytes),
        len: bytes.len() as u64,
        newline: detect_newline(&bytes),
        has_bom: bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
        mode,
        is_symlink,
        bytes,
        text,
    })
}

fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        fs::metadata(path)
            .ok()
            .map(|md| md.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn rel_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    pub fn scratch_root(name: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "mncs-doctor-test-{}-{}-{}",
            name,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn newline_detection_covers_styles() {
        assert_eq!(detect_newline(b"a\nb\n"), NewlineStyle::Lf);
        assert_eq!(detect_newline(b"a\r\nb\r\n"), NewlineStyle::Crlf);
        assert_eq!(detect_newline(b"a\r\nb\n"), NewlineStyle::Mixed);
        assert_eq!(detect_newline(b"no newline"), NewlineStyle::None);
        assert_eq!(detect_newline(b""), NewlineStyle::None);
    }

    #[test]
    fn discovers_sources_with_deterministic_order() {
        let root = scratch_root("discover");
        fs::write(root.join("b.mncs"), "mncs 0.16;\nmodule b;\n").unwrap();
        fs::write(root.join("a.mncs"), "mncs 0.16;\nmodule a;\n").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/c.mncs"), "mncs 0.16;\nmodule c;\n").unwrap();
        let inv = discover(&root, &DiscoveryOptions::default()).unwrap();
        let rels: Vec<_> = inv.sources.iter().map(|s| s.relative.as_str()).collect();
        assert_eq!(rels, vec!["a.mncs", "b.mncs", "sub/c.mncs"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn excludes_tool_dirs_and_records_them() {
        let root = scratch_root("excluded");
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target/x.mncs"), "mncs 0.16;\n").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/y.mncs"), "mncs 0.16;\n").unwrap();
        fs::write(root.join("keep.mncs"), "mncs 0.16;\n").unwrap();
        let inv = discover(&root, &DiscoveryOptions::default()).unwrap();
        assert_eq!(inv.sources.len(), 1);
        assert!(inv.skipped.iter().any(|s| s.relative == "target"));
        assert!(inv.skipped.iter().any(|s| s.relative == ".git"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_root_via_forge_marker() {
        let root = scratch_root("rootmark");
        fs::write(root.join("mncs-forge.toml"), "version = 1\n").unwrap();
        let sub = root.join("a/b");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_root(&sub).unwrap(), root);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_manifest_kinds() {
        let root = scratch_root("manifests");
        fs::write(root.join("mncs-forge.toml"), "version = 1\n").unwrap();
        fs::write(root.join("prog.mncs.json"), "{}\n").unwrap();
        fs::write(root.join("x.mncs"), "mncs 0.16;\n").unwrap();
        let inv = discover(&root, &DiscoveryOptions::default()).unwrap();
        assert_eq!(inv.sources.len(), 1);
        assert_eq!(inv.manifests.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn skips_symlinked_dirs_by_default() {
        use std::os::unix::fs::symlink;
        let root = scratch_root("symlink");
        fs::create_dir_all(root.join("real")).unwrap();
        fs::write(root.join("real/a.mncs"), "mncs 0.16;\n").unwrap();
        symlink(root.join("real"), root.join("link")).unwrap();
        let inv = discover(&root, &DiscoveryOptions::default()).unwrap();
        assert_eq!(inv.sources.len(), 1);
        assert!(inv.skipped.iter().any(|s| s.relative == "link"));
        let _ = fs::remove_dir_all(&root);
    }
}
