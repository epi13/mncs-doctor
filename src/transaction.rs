//! Repository transaction model: dry-run planning, staged atomic commit,
//! backup, and rollback.
//!
//! Mutation safety rules enforced here:
//!
//! - Plans are computed against recorded fingerprints; commit re-validates
//!   every file before touching anything (fail-closed on external change).
//! - Commit order is deterministic (sorted by relative path).
//! - Each file is written to a temporary sibling and renamed over the
//!   original, so a crash never leaves a half-written file in place.
//! - Unix permission bits are preserved (or explicitly set for creates).
//! - On partial failure, completed writes roll back from in-memory
//!   snapshots and the error reports exactly what happened.
//! - Symlinks are never followed on write: a planned op whose target is
//!   (or becomes) a symlink is refused.
//! - Newlines/encoding pass through untouched except for bytes the fix or
//!   migration engine explicitly produced.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::discovery::fingerprint;

#[derive(Debug, Error)]
pub enum TransactionError {
    #[error("refusing to write {relative}: expected fingerprint {expected}, found {found}")]
    StaleBase {
        relative: String,
        expected: String,
        found: String,
    },
    #[error("refusing to write {0}: target is a symlink")]
    SymlinkTarget(String),
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("partial failure after {completed} of {total} writes (failed at {failed_at}): {cause}; rollback {rollback}")]
    PartialFailure {
        completed: usize,
        total: usize,
        failed_at: String,
        cause: String,
        rollback: String,
    },
}

fn io_err(path: &Path, source: std::io::Error) -> TransactionError {
    TransactionError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// One staged file mutation.
#[derive(Debug, Clone)]
pub struct FileOp {
    /// Slash-separated path relative to the workspace root.
    pub relative: String,
    /// Expected fingerprint of the current content, or `None` to create.
    pub old_fingerprint: Option<String>,
    /// New content.
    pub new_bytes: Vec<u8>,
    /// Permission bits to set. `None` preserves existing bits on overwrite.
    pub mode: Option<u32>,
}

/// A staged, validated, deterministically ordered set of file mutations.
#[derive(Debug, Clone, Default)]
pub struct Transaction {
    ops: Vec<FileOp>,
    /// Optional directory (relative to root) receiving pre-write copies.
    backup_dir: Option<PathBuf>,
}

impl Transaction {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_backup_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.backup_dir = Some(dir.into());
        self
    }

    pub fn push(&mut self, op: FileOp) {
        self.ops.push(op);
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Deterministic commit order.
    fn ordered(&self) -> Vec<&FileOp> {
        let mut ops: Vec<&FileOp> = self.ops.iter().collect();
        ops.sort_by(|a, b| a.relative.cmp(&b.relative));
        ops
    }

    /// Validate without mutating: every overwrite target must exist,
    /// must not be a symlink, and must match its recorded fingerprint;
    /// every create target must be absent.
    pub fn validate(&self, root: &Path) -> Result<(), TransactionError> {
        for op in self.ordered() {
            let path = root.join(&op.relative);
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => Some(meta),
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound && op.old_fingerprint.is_none() =>
                {
                    // Create op for an absent path: allowed; commit creates
                    // parent directories.
                    None
                }
                Err(e) => return Err(io_err(&path, e)),
            };
            let Some(meta) = meta else {
                continue;
            };
            if meta.file_type().is_symlink() {
                return Err(TransactionError::SymlinkTarget(op.relative.clone()));
            }
            match &op.old_fingerprint {
                Some(expected) => {
                    if !meta.is_file() {
                        return Err(TransactionError::SymlinkTarget(op.relative.clone()));
                    }
                    let current = fs::read(&path).map_err(|e| io_err(&path, e))?;
                    let found = fingerprint(&current);
                    if &found != expected {
                        return Err(TransactionError::StaleBase {
                            relative: op.relative.clone(),
                            expected: expected.clone(),
                            found,
                        });
                    }
                    if current == op.new_bytes {
                        continue; // identical: commit will skip
                    }
                }
                None => {
                    if meta.is_file() || meta.is_dir() {
                        return Err(TransactionError::StaleBase {
                            relative: op.relative.clone(),
                            expected: "<absent>".to_owned(),
                            found: "<present>".to_owned(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Commit all ops atomically-ish: validate everything first, then write
    /// each file via temp-file + rename. Rolls back completed writes on
    /// failure. Returns per-file outcomes.
    pub fn commit(&self, root: &Path) -> Result<CommitReport, TransactionError> {
        // Phase 1: validate all before touching anything.
        self.validate(root).map_err(|e| match e {
            TransactionError::StaleBase { .. } | TransactionError::SymlinkTarget(_) => e,
            other => other,
        })?;

        // Phase 2: snapshot originals for rollback.
        let mut snapshots: Vec<Snapshot> = Vec::new();
        for op in self.ordered() {
            let path = root.join(&op.relative);
            let original = fs::read(&path).ok();
            let mode = file_mode(&path);
            snapshots.push(Snapshot {
                relative: op.relative.clone(),
                path,
                original,
                mode,
            });
        }

        // Optional backup copies.
        let backup_root = match &self.backup_dir {
            Some(dir) => {
                let full = root.join(dir);
                fs::create_dir_all(&full).map_err(|e| io_err(&full, e))?;
                Some(full)
            }
            None => None,
        };
        if let Some(backup_root) = &backup_root {
            for snap in &snapshots {
                if let Some(bytes) = &snap.original {
                    let dest = backup_root.join(&snap.relative);
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent).map_err(|e| io_err(&dest, e))?;
                    }
                    fs::write(&dest, bytes).map_err(|e| io_err(&dest, e))?;
                }
            }
        }

        // Phase 3: write in deterministic order.
        let total = snapshots.len();
        let mut report = CommitReport {
            written: Vec::new(),
            skipped_identical: Vec::new(),
            bytes_written: 0,
            backup_dir: self.backup_dir.clone(),
        };
        let mut completed = 0usize;
        for (snap, op) in snapshots.iter().zip(self.ordered()) {
            let outcome = write_one(root, snap, op);
            match outcome {
                Ok(WriteOutcome::Written(n)) => {
                    report.written.push(op.relative.clone());
                    report.bytes_written += n;
                    completed += 1;
                }
                Ok(WriteOutcome::SkippedIdentical) => {
                    report.skipped_identical.push(op.relative.clone());
                    completed += 1;
                }
                Err(cause) => {
                    let rollback = rollback(root, &snapshots[..completed]);
                    return Err(TransactionError::PartialFailure {
                        completed,
                        total,
                        failed_at: op.relative.clone(),
                        cause: cause.to_string(),
                        rollback,
                    });
                }
            }
        }
        Ok(report)
    }
}

struct Snapshot {
    relative: String,
    path: PathBuf,
    original: Option<Vec<u8>>,
    #[allow(dead_code)]
    mode: Option<u32>,
}

enum WriteOutcome {
    Written(u64),
    SkippedIdentical,
}

fn write_one(root: &Path, snap: &Snapshot, op: &FileOp) -> Result<WriteOutcome, TransactionError> {
    let path = root.join(&op.relative);
    if let Some(original) = &snap.original {
        if *original == op.new_bytes {
            return Ok(WriteOutcome::SkippedIdentical);
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(&path, e))?;
    }
    // Refuse symlink targets at write time too (TOCTOU guard).
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            return Err(TransactionError::SymlinkTarget(op.relative.clone()));
        }
    }
    let tmp = sibling_tmp(&path);
    fs::write(&tmp, &op.new_bytes).map_err(|e| io_err(&tmp, e))?;
    // Preserve or set permission bits before the rename.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bits = match op.mode {
            Some(m) => Some(m),
            None => snap.mode,
        };
        if let Some(bits) = bits {
            fs::set_permissions(&tmp, fs::Permissions::from_mode(bits))
                .map_err(|e| io_err(&tmp, e))?;
        }
    }
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io_err(&path, e)
    })?;
    Ok(WriteOutcome::Written(op.new_bytes.len() as u64))
}

fn rollback(_root: &Path, completed: &[Snapshot]) -> String {
    let mut restored = 0usize;
    let mut failures = Vec::new();
    for snap in completed.iter().rev() {
        let result = match &snap.original {
            Some(bytes) => {
                let tmp = sibling_tmp(&snap.path);
                fs::write(&tmp, bytes)
                    .and_then(|()| {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Some(bits) = snap.mode {
                                fs::set_permissions(&tmp, fs::Permissions::from_mode(bits))?;
                            }
                        }
                        fs::rename(&tmp, &snap.path)
                    })
                    .map_err(|e| e.to_string())
            }
            None => fs::remove_file(&snap.path).map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => restored += 1,
            Err(e) => failures.push(format!("{}: {e}", snap.relative)),
        }
    }
    if failures.is_empty() {
        format!("ok ({restored} restored)")
    } else {
        format!(
            "INCOMPLETE (restored {restored}, failed: {})",
            failures.join("; ")
        )
    }
}

fn sibling_tmp(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".doctor-tmp-{}", std::process::id()));
    path.with_file_name(name)
}

fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Outcome of a committed transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitReport {
    #[serde(default)]
    pub written: Vec<String>,
    #[serde(default)]
    pub skipped_identical: Vec<String>,
    #[serde(default)]
    pub bytes_written: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_dir: Option<PathBuf>,
}

/// A compact human-readable diff summary for one file (dry-run output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffSummary {
    pub relative: String,
    pub additions: usize,
    pub deletions: usize,
    /// Up to a few rendered hunk lines (` ` context, `-`/`+` changes,
    /// `...` truncation markers).
    #[serde(default)]
    pub hunks: Vec<String>,
    /// Old and new fingerprints for audit.
    pub old_fingerprint: String,
    pub new_fingerprint: String,
}

/// Summarize `old -> new` as prefix/suffix-trimmed changed regions with
/// context lines. Pure function of content: dry-run output always matches
/// what apply would write.
pub fn summarize_diff(relative: &str, old: &str, new: &str) -> DiffSummary {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let prefix = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let max_suffix = (a.len() - prefix).min(b.len() - prefix);
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let a_mid = &a[prefix..a.len() - suffix];
    let b_mid = &b[prefix..b.len() - suffix];
    const CONTEXT: usize = 3;
    let mut hunks = Vec::new();
    let ctx_start = prefix.saturating_sub(CONTEXT);
    if prefix > CONTEXT {
        hunks.push("...".to_owned());
    }
    for line in &a[ctx_start..prefix] {
        hunks.push(format!(" {line}"));
    }
    for line in a_mid {
        hunks.push(format!("-{line}"));
        if hunks.len() > 40 {
            break;
        }
    }
    for line in b_mid {
        hunks.push(format!("+{line}"));
        if hunks.len() > 80 {
            break;
        }
    }
    let ctx_end = (b.len() - suffix + CONTEXT).min(b.len());
    for line in &b[b.len() - suffix..ctx_end] {
        hunks.push(format!(" {line}"));
    }
    if b.len() - suffix > ctx_end {
        hunks.push("...".to_owned());
    }
    DiffSummary {
        relative: relative.to_owned(),
        additions: b_mid.len(),
        deletions: a_mid.len(),
        hunks,
        old_fingerprint: fingerprint(old.as_bytes()),
        new_fingerprint: fingerprint(new.as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mncs-doctor-tx-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn op(relative: &str, old: Option<&str>, new: &str) -> FileOp {
        FileOp {
            relative: relative.to_owned(),
            old_fingerprint: old.map(|o| fingerprint(o.as_bytes())),
            new_bytes: new.as_bytes().to_vec(),
            mode: None,
        }
    }

    #[test]
    fn commit_writes_atomically_and_skips_identical() {
        let root = scratch("basic");
        fs::write(root.join("a.mncs"), "old\n").unwrap();
        let mut tx = Transaction::new();
        tx.push(op("a.mncs", Some("old\n"), "new\n"));
        tx.push(op("b.mncs", Some("same\n"), "same\n"));
        fs::write(root.join("b.mncs"), "same\n").unwrap();
        let report = tx.commit(&root).unwrap();
        assert_eq!(report.written, vec!["a.mncs".to_owned()]);
        assert_eq!(report.skipped_identical, vec!["b.mncs".to_owned()]);
        assert_eq!(fs::read_to_string(root.join("a.mncs")).unwrap(), "new\n");
        assert!(root.read_dir().unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".doctor-tmp-")
        }));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_base_refuses_before_any_write() {
        let root = scratch("stale");
        fs::write(root.join("a.mncs"), "changed\n").unwrap();
        fs::write(root.join("b.mncs"), "b\n").unwrap();
        let mut tx = Transaction::new();
        tx.push(op("b.mncs", Some("b\n"), "b2\n"));
        tx.push(op("a.mncs", Some("old\n"), "new\n"));
        assert!(matches!(
            tx.commit(&root),
            Err(TransactionError::StaleBase { .. })
        ));
        assert_eq!(fs::read_to_string(root.join("b.mncs")).unwrap(), "b\n");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn preserves_permissions_and_refuses_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = scratch("perms");
        let p = root.join("x.mncs");
        fs::write(&p, "x\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        let mut tx = Transaction::new();
        tx.push(op("x.mncs", Some("x\n"), "y\n"));
        tx.commit(&root).unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);

        symlink(&p, root.join("link.mncs")).unwrap();
        let mut tx2 = Transaction::new();
        tx2.push(op("link.mncs", Some("y\n"), "z\n"));
        assert!(matches!(
            tx2.commit(&root),
            Err(TransactionError::SymlinkTarget(_))
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn creates_parent_dirs_and_backs_up() {
        let root = scratch("backup");
        fs::write(root.join("a.mncs"), "a\n").unwrap();
        let mut tx = Transaction::new().with_backup_dir("bk");
        tx.push(op("a.mncs", Some("a\n"), "a2\n"));
        tx.push(op("sub/n.mncs", None, "new\n"));
        let report = tx.commit(&root).unwrap();
        assert_eq!(report.written.len(), 2);
        assert_eq!(fs::read_to_string(root.join("bk/a.mncs")).unwrap(), "a\n");
        assert_eq!(
            fs::read_to_string(root.join("sub/n.mncs")).unwrap(),
            "new\n"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn diff_summary_is_stable_and_informative() {
        let d = summarize_diff("a.mncs", "l1\nl2\nl3\nl4\nl5\n", "l1\nl2X\nl3\nl4\nl5\n");
        assert_eq!((d.deletions, d.additions), (1, 1));
        assert!(d.hunks.iter().any(|h| h == "-l2"));
        assert!(d.hunks.iter().any(|h| h == "+l2X"));
        assert_ne!(d.old_fingerprint, d.new_fingerprint);
    }
}
