//! Post-repair verification: re-diagnose, idempotence, external checks.
//!
//! After any mutation doctor re-runs what it can locally (re-scan,
//! re-diagnose, migration idempotence) and optionally shells out to project
//! verification commands (build/tests). Verification never mutates.

use std::collections::BTreeMap;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{LanguageBackend, Severity};
use crate::discovery::SourceFile;

/// One external verification command outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCheck {
    pub command: String,
    pub success: bool,
    pub status_code: Option<i32>,
    #[serde(default)]
    pub stdout_tail: String,
    #[serde(default)]
    pub stderr_tail: String,
}

/// Run an external verification command, capturing output tails.
pub fn run_external(argv: &[String], cwd: &std::path::Path) -> ExternalCheck {
    let command = argv.join(" ");
    if argv.is_empty() {
        return ExternalCheck {
            command,
            success: false,
            status_code: None,
            stdout_tail: String::new(),
            stderr_tail: "empty command".to_owned(),
        };
    }
    match Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .output()
    {
        Ok(output) => ExternalCheck {
            command,
            success: output.status.success(),
            status_code: output.status.code(),
            stdout_tail: tail(&String::from_utf8_lossy(&output.stdout), 20),
            stderr_tail: tail(&String::from_utf8_lossy(&output.stderr), 20),
        },
        Err(e) => ExternalCheck {
            command,
            success: false,
            status_code: None,
            stdout_tail: String::new(),
            stderr_tail: format!("failed to spawn: {e}"),
        },
    }
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// Outcome of post-mutation verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationOutcome {
    pub files_rechecked: usize,
    pub errors_before: usize,
    pub errors_after: usize,
    pub warnings_after: usize,
    /// Whether a second identical pass produces no further changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(default)]
    pub external: Vec<ExternalCheck>,
    pub passed: bool,
    #[serde(default)]
    pub notes: Vec<String>,
}

fn count_errors(diags: &BTreeMap<String, Vec<crate::diagnostics::Diagnostic>>) -> usize {
    diags
        .values()
        .flatten()
        .filter(|d| d.severity == Severity::Error)
        .count()
}

fn count_warnings(diags: &BTreeMap<String, Vec<crate::diagnostics::Diagnostic>>) -> usize {
    diags
        .values()
        .flatten()
        .filter(|d| d.severity == Severity::Warning)
        .count()
}

/// Re-diagnose `files` and compare against the pre-mutation diagnostic map.
/// Passes when no *new* errors appear. Idempotence must be established by
/// the caller (fix engine or migration applier) and passed in.
pub fn verify_after(
    files: &[SourceFile],
    backend: &dyn LanguageBackend,
    before: &BTreeMap<String, Vec<crate::diagnostics::Diagnostic>>,
    idempotent: Option<bool>,
    external: Vec<ExternalCheck>,
) -> VerificationOutcome {
    let mut after: BTreeMap<String, Vec<crate::diagnostics::Diagnostic>> = BTreeMap::new();
    for file in files {
        after.insert(file.relative.clone(), backend.diagnose(file));
    }
    let errors_before = count_errors(before);
    let errors_after = count_errors(&after);
    let warnings_after = count_warnings(&after);
    let mut notes = Vec::new();
    if errors_after > errors_before {
        notes.push(format!(
            "diagnostic errors increased from {errors_before} to {errors_after}"
        ));
    }
    if idempotent == Some(false) {
        notes.push("second pass produced further changes (not idempotent)".to_owned());
    }
    for check in &external {
        if !check.success {
            notes.push(format!("external check failed: {}", check.command));
        }
    }
    let passed = errors_after <= errors_before
        && idempotent != Some(false)
        && external.iter().all(|c| c.success);
    VerificationOutcome {
        files_rechecked: files.len(),
        errors_before,
        errors_after,
        warnings_after,
        idempotent,
        external,
        passed,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::ScannerBackend;
    use crate::discovery::{detect_newline, fingerprint};

    fn src(relative: &str, text: &str) -> SourceFile {
        let bytes = text.as_bytes().to_vec();
        SourceFile {
            path: std::path::PathBuf::from(relative),
            relative: relative.to_owned(),
            sha256: fingerprint(&bytes),
            len: bytes.len() as u64,
            newline: detect_newline(&bytes),
            has_bom: false,
            mode: None,
            is_symlink: false,
            text: Some(text.to_owned()),
            bytes,
        }
    }

    #[test]
    fn clean_reverification_passes() {
        let backend = ScannerBackend;
        let files = vec![src("a.mncs", "mncs 0.16;\nmodule a;\n")];
        let mut before = BTreeMap::new();
        before.insert("a.mncs".to_owned(), backend.diagnose(&files[0]));
        let outcome = verify_after(&files, &backend, &before, Some(true), Vec::new());
        assert!(outcome.passed);
        assert_eq!(outcome.errors_after, 0);
    }

    #[test]
    fn new_errors_fail_verification() {
        let backend = ScannerBackend;
        let good = src("a.mncs", "mncs 0.16;\nmodule a;\n");
        let mut before = BTreeMap::new();
        before.insert("a.mncs".to_owned(), backend.diagnose(&good));
        let bad = vec![src("a.mncs", "mncs 0.16;\nmodule a;\nfn f() {\n")];
        let outcome = verify_after(&bad, &backend, &before, None, Vec::new());
        assert!(!outcome.passed);
        assert!(outcome.errors_after > outcome.errors_before);
    }

    #[test]
    fn external_command_outcome_is_captured() {
        let check = run_external(&["true".to_owned()], &std::env::temp_dir());
        assert!(check.success);
        let check = run_external(&["false".to_owned()], &std::env::temp_dir());
        assert!(!check.success);
        let check = run_external(
            &["mncs-doctor-no-such-binary-xyz".to_owned()],
            &std::env::temp_dir(),
        );
        assert!(!check.success);
    }
}
