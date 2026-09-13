//! Human-readable and machine-readable reporting.
//!
//! Reports carry enough provenance to reproduce or audit a run: tool and
//! schema versions, workspace root, content fingerprints, transitions, and
//! per-file outcomes. See `docs/EXIT-CODES.md` for status semantics.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{Diagnostic, Severity};
use crate::discovery::InventorySummary;
use crate::fix::Convergence;
use crate::health::{overall_status, CheckResult, Status};
use crate::migration::MigrationRecord;
use crate::mncs_runtime::PolicyProvenance;
use crate::toolchain::ToolchainStatus;
use crate::transaction::DiffSummary;
use crate::verify::VerificationOutcome;
use crate::{DOCTOR_VERSION, REPORT_SCHEMA_VERSION};

/// Process exit semantics (see `docs/EXIT-CODES.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Healthy = 0,
    Findings = 1,
    ReviewRequired = 2,
    VerificationFailed = 3,
    ToolFailure = 4,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// Derive the exit code from a finished run.
pub fn exit_for(
    checks: &[CheckResult],
    review_blocked: bool,
    verification: Option<&VerificationOutcome>,
) -> ExitCode {
    if let Some(v) = verification {
        if !v.passed {
            return ExitCode::VerificationFailed;
        }
    }
    match overall_status(checks) {
        Status::Pass => ExitCode::Healthy,
        Status::Skipped => ExitCode::Healthy,
        Status::Warning if review_blocked => ExitCode::ReviewRequired,
        Status::Warning => ExitCode::Findings,
        Status::Fail if review_blocked => ExitCode::ReviewRequired,
        Status::Fail => ExitCode::Findings,
    }
}

/// Full machine-readable report envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: String,
    pub doctor_version: String,
    pub command: String,
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory: Option<InventorySummary>,
    #[serde(default)]
    pub checks: Vec<CheckResult>,
    #[serde(default)]
    pub file_diagnostics: BTreeMap<String, Vec<Diagnostic>>,
    #[serde(default)]
    pub planned_diffs: Vec<DiffSummary>,
    #[serde(default)]
    pub convergence: Vec<Convergence>,
    #[serde(default)]
    pub migrations: Vec<MigrationRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<ToolchainStatus>,
    /// Which policy engine and exact embedded artifacts produced decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyProvenance>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub exit_code: i32,
    pub exit_meaning: String,
}

impl Report {
    pub fn new(command: impl Into<String>, root: impl Into<String>) -> Self {
        Self {
            schema_version: REPORT_SCHEMA_VERSION.to_owned(),
            doctor_version: DOCTOR_VERSION.to_owned(),
            command: command.into(),
            root: root.into(),
            inventory: None,
            checks: Vec::new(),
            file_diagnostics: BTreeMap::new(),
            planned_diffs: Vec::new(),
            convergence: Vec::new(),
            migrations: Vec::new(),
            verification: None,
            toolchain: None,
            policy: None,
            notes: Vec::new(),
            exit_code: 0,
            exit_meaning: String::new(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }
}

/// Render the concise terminal summary plus optional explain details.
pub fn render_human(report: &Report, explain: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "MNCS repository health");
    let _ = writeln!(out);
    if let Some(inv) = &report.inventory {
        let _ = writeln!(out, "Source:");
        let _ = writeln!(out, "  {} files checked", inv.files_checked);
    }
    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut infos = 0usize;
    for diags in report.file_diagnostics.values() {
        for diag in diags {
            match diag.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                Severity::Info => infos += 1,
            }
        }
    }
    let _ = writeln!(out, "  {errors} errors");
    let _ = writeln!(out, "  {warnings} warnings");
    let _ = writeln!(out, "  {infos} informational (sealed profiles)");
    let _ = writeln!(out);
    let _ = writeln!(out, "Checks:");
    for check in &report.checks {
        let _ = writeln!(out, "  {:<24} {}", check.id, check.status);
    }
    if let Some(tc) = &report.toolchain {
        let _ = writeln!(out);
        let _ = writeln!(out, "Toolchain:");
        match &tc.rust_cli {
            Some(t) => {
                let _ = writeln!(out, "  language CLI: {} ({})", t.path, t.version);
            }
            None => {
                let _ = writeln!(out, "  language CLI: not found");
            }
        }
        match &tc.family_cli {
            Some(t) => {
                let _ = writeln!(out, "  family CLI:   {} ({})", t.path, t.version);
            }
            None => {
                let _ = writeln!(out, "  family CLI:   not found");
            }
        }
    }
    if !report.planned_diffs.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Planned changes:");
        for diff in &report.planned_diffs {
            let _ = writeln!(
                out,
                "  {} (+{} -{})",
                diff.relative, diff.additions, diff.deletions
            );
            if explain {
                for hunk in &diff.hunks {
                    let _ = writeln!(out, "    {hunk}");
                }
            }
        }
    }
    if !report.migrations.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Migrations:");
        for record in &report.migrations {
            let _ = writeln!(
                out,
                "  {}: {} -> {} ({} steps)",
                record.relative,
                record.from,
                record.to,
                record.steps.len()
            );
            if explain {
                for step in &record.steps {
                    let _ = writeln!(
                        out,
                        "    {} [{}] {}",
                        step.transition, step.kind, step.provenance
                    );
                }
            }
        }
    }
    if let Some(v) = &report.verification {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Verification: {} ({} files rechecked, {} -> {} errors)",
            if v.passed { "pass" } else { "FAIL" },
            v.files_rechecked,
            v.errors_before,
            v.errors_after
        );
        for note in &v.notes {
            let _ = writeln!(out, "  note: {note}");
        }
    }
    if explain {
        let _ = writeln!(out);
        let _ = writeln!(out, "Findings:");
        let mut paths: Vec<&String> = report.file_diagnostics.keys().collect();
        paths.sort();
        for path in paths {
            for diag in &report.file_diagnostics[path] {
                let _ = writeln!(
                    out,
                    "  {}:{}:{} [{}|{}] {}",
                    path,
                    diag.span.start_line,
                    diag.span.start_col,
                    diag.code,
                    diag.severity,
                    diag.message
                );
                if !diag.explanation.is_empty() {
                    for line in diag.explanation.lines() {
                        let _ = writeln!(out, "      {line}");
                    }
                }
                if !diag.suggested_action.is_empty() {
                    let _ = writeln!(out, "      action: {}", diag.suggested_action);
                }
            }
        }
        if !report.notes.is_empty() {
            let _ = writeln!(out);
            for note in &report.notes {
                let _ = writeln!(out, "note: {note}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_semantics() {
        let pass = vec![CheckResult {
            id: "x".to_owned(),
            title: "x".to_owned(),
            status: Status::Pass,
            findings: Vec::new(),
        }];
        assert_eq!(exit_for(&pass, false, None), ExitCode::Healthy);
        let warn = vec![CheckResult {
            id: "x".to_owned(),
            title: "x".to_owned(),
            status: Status::Warning,
            findings: Vec::new(),
        }];
        assert_eq!(exit_for(&warn, false, None), ExitCode::Findings);
        assert_eq!(exit_for(&warn, true, None), ExitCode::ReviewRequired);
        let v = VerificationOutcome {
            files_rechecked: 1,
            errors_before: 0,
            errors_after: 1,
            warnings_after: 0,
            idempotent: None,
            external: Vec::new(),
            passed: false,
            notes: Vec::new(),
        };
        assert_eq!(
            exit_for(&pass, false, Some(&v)),
            ExitCode::VerificationFailed
        );
    }

    #[test]
    fn human_render_is_deterministic() {
        let mut report = Report::new("doctor", "/repo");
        report.inventory = Some(InventorySummary {
            files_checked: 2,
            manifests: 0,
            skipped_dirs: 0,
            total_bytes: 10,
        });
        let a = render_human(&report, false);
        let b = render_human(&report, false);
        assert_eq!(a, b);
        assert!(a.contains("2 files checked"));
        let json = report.to_json();
        assert!(json.contains(REPORT_SCHEMA_VERSION));
    }
}
