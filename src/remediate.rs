//! Ambient remediation: detect, classify, repair, validate, record.
//!
//! `remediate` is the machine-native entrypoint over the existing Safe repair
//! machinery. It applies [`Applicability::Safe`][crate::fix::Applicability]
//! repairs automatically, reconciles bounded recoverable state (stale
//! inventory caches, multi-round convergence), validates every mutation,
//! and emits a terse summary. Full evidence is written to an artifact file
//! and referenced by path; it is never forced into the working context.
//!
//! Remediation classes:
//!
//! - [`RemediationClass::SafeAutomatic`] — deterministic problem and repair,
//!   validated after mutation. Applied without involving the caller.
//! - [`RemediationClass::BoundedReconciliation`] — the invariant is known
//!   but recovery takes work (cache regeneration, another converge round).
//!   Bounded, idempotent, validated; failure escalates instead of retrying
//!   forever.
//! - [`RemediationClass::Escalation`] — needs intent, judgment, or work
//!   outside safe repair. Carries the smallest sufficient evidence
//!   (`code:path` plus one action), retrievable in full from the artifact.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diagnostics::{Diagnostic, Severity};
use crate::fix::{Applicability, Convergence, StopReason};
use crate::transaction::DiffSummary;
use crate::verify::VerificationOutcome;
use crate::DOCTOR_VERSION;

/// Schema of the terse remediation envelope on stdout.
///
/// Family-standard contract owned by MNCS-Commons
/// (`mncs.commons.family.remediation.v1`); this binary is the reference
/// repository-domain provider.
pub const REMEDIATION_SCHEMA_VERSION: &str = "mncs.remediation/1";

/// Schema of the full evidence artifact.
pub const REMEDIATION_EVIDENCE_SCHEMA_VERSION: &str = "mncs.remediation-evidence/1";

/// Provider identity carried on every envelope.
pub const REMEDIATION_PROVIDER: &str = "mncs-doctor";

/// Repository-domain scope marker (`mncs.remediation/1` v1 vocabulary).
pub const REMEDIATION_REPOSITORY_DOMAIN: &str = "repository";

/// Maximum escalation ids carried inline on stdout; overflow stays in
/// evidence and flips `remaining_truncated`.
pub const MAX_REMAINING_INLINE: usize = 64;

/// Session artifact directory exported by `mncs-environment` invocation.
/// When present, evidence lands here so the calling session owns the trail.
pub const ARTIFACT_DIR_ENV: &str = "MNCS_ENV_SESSION_ARTIFACT_DIR";

/// Evidence file name inside an artifact directory or `.mncs/doctor/`.
pub const EVIDENCE_FILE_NAME: &str = "remediation-evidence.json";

/// Default cap on files repaired in one run. Excess files escalate as
/// `budget-exhausted` so a huge tree degrades to bounded work, not a
/// surprise marathon.
pub const DEFAULT_BUDGET_FILES: usize = 256;

/// Maximum apply rounds per run. Round one is the normal plan/apply pass;
/// further rounds only run while verification still shows eligible fixes.
pub const MAX_APPLY_ROUNDS: u32 = 3;

/// Remediation class for one record: what the machine did or needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemediationClass {
    SafeAutomatic,
    BoundedReconciliation,
    Escalation,
}

impl std::fmt::Display for RemediationClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemediationClass::SafeAutomatic => write!(f, "safe_automatic"),
            RemediationClass::BoundedReconciliation => write!(f, "bounded_reconciliation"),
            RemediationClass::Escalation => write!(f, "escalation"),
        }
    }
}

/// One validated repair or reconciliation action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairRecord {
    /// Stable record id (`<provider>:<target>` or a reconciliation key).
    pub id: String,
    pub class: RemediationClass,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_fingerprint: Option<String>,
    /// Post-mutation validation state. Dry-run records are never validated.
    pub validated: bool,
}

/// One escalation: the smallest sufficient evidence package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Escalation {
    /// Stable escalation id (`<code>:<target>` or a bare `<code>`).
    pub id: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub severity: Severity,
    pub action: String,
}

/// Terse machine counts. `degraded` counts files with residual warnings;
/// `blockers` counts error-severity escalations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemediationSummary {
    pub repaired: usize,
    pub reconciled: usize,
    pub degraded: usize,
    pub blockers: usize,
}

/// Remediation scope echo: the domain and target this run addressed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationScope {
    pub domain: String,
    pub target: String,
}

/// Post-mutation verification verdict on stdout.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemediationValidation {
    pub passed: bool,
    pub errors_before: usize,
    pub errors_after: usize,
    pub idempotent_known: bool,
    pub idempotent: bool,
}

/// Terse stdout envelope. Bounded by construction: only counts, escalation
/// ids, and one evidence pointer. Field vocabulary follows
/// `mncs.remediation/1`; `exit_code`/`exit_meaning` are tolerated
/// transport extras the orchestrator records in history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationReport {
    pub schema_version: String,
    pub provider: String,
    pub provider_version: String,
    pub scope: RemediationScope,
    #[serde(default)]
    pub dry_run: bool,
    pub summary: RemediationSummary,
    /// Escalation ids only; full records live in the evidence artifact.
    #[serde(default)]
    pub remaining: Vec<String>,
    #[serde(default)]
    pub remaining_truncated: bool,
    #[serde(default)]
    pub repairs: Vec<RepairRecord>,
    #[serde(default)]
    pub reconciliations: Vec<RepairRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default)]
    pub budget: RemediationBudget,
    #[serde(default)]
    pub validation: RemediationValidation,
    #[serde(default)]
    pub notes: Vec<String>,
    pub exit_code: i32,
    pub exit_meaning: String,
}

/// Budget accounting for one run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemediationBudget {
    pub max_items: usize,
    pub items_done: usize,
    pub rounds: u32,
    #[serde(default)]
    pub exhausted: bool,
}

/// Full audit trail. Written to the evidence artifact, never to stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationEvidence {
    pub schema_version: String,
    pub report_schema_version: String,
    pub doctor_version: String,
    pub root: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub repairs: Vec<RepairRecord>,
    #[serde(default)]
    pub reconciliations: Vec<RepairRecord>,
    #[serde(default)]
    pub escalations: Vec<Escalation>,
    #[serde(default)]
    pub diffs: Vec<DiffSummary>,
    #[serde(default)]
    pub convergence: Vec<Convergence>,
    #[serde(default)]
    pub file_diagnostics: BTreeMap<String, Vec<Diagnostic>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<crate::mncs_runtime::PolicyProvenance>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl RemediationReport {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            schema_version: REMEDIATION_SCHEMA_VERSION.to_owned(),
            provider: REMEDIATION_PROVIDER.to_owned(),
            provider_version: DOCTOR_VERSION.to_owned(),
            scope: RemediationScope {
                domain: REMEDIATION_REPOSITORY_DOMAIN.to_owned(),
                target: target.into(),
            },
            dry_run: false,
            summary: RemediationSummary::default(),
            remaining: Vec::new(),
            remaining_truncated: false,
            repairs: Vec::new(),
            reconciliations: Vec::new(),
            evidence: None,
            budget: RemediationBudget::default(),
            validation: RemediationValidation::default(),
            notes: Vec::new(),
            exit_code: 0,
            exit_meaning: String::new(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// Recompute summary counts and the remaining id list from records.
    /// Inline ids cap at [`MAX_REMAINING_INLINE`]; overflow stays in
    /// evidence and flips `remaining_truncated`.
    pub fn refresh_summary(&mut self, escalations: &[Escalation], degraded_files: usize) {
        self.summary.repaired = self.repairs.len();
        self.summary.reconciled = self.reconciliations.len();
        self.summary.degraded = degraded_files;
        self.summary.blockers = escalations
            .iter()
            .filter(|item| item.severity == Severity::Error)
            .count();
        let mut remaining: Vec<String> = escalations.iter().map(|item| item.id.clone()).collect();
        remaining.sort();
        remaining.dedup();
        if remaining.len() > MAX_REMAINING_INLINE {
            remaining.truncate(MAX_REMAINING_INLINE);
            self.remaining_truncated = true;
        } else {
            self.remaining_truncated = false;
        }
        self.remaining = remaining;
    }
}

/// Classify one blocked fix provider outcome as an escalation.
pub fn escalation_for_blocked(
    provider_id: &str,
    relative: &str,
    applicability: Applicability,
) -> Escalation {
    let (code, action) = match applicability {
        Applicability::Review => (
            "review-required",
            "Inspect the planned change and apply it explicitly; automatic repair refuses review-classified edits.",
        ),
        Applicability::Manual => (
            "manual-required",
            "No safe transformation exists; repair by hand or record a fix provider.",
        ),
        _ => (
            "blocked",
            "The fix was unexpectedly ineligible; re-run with diagnostics.",
        ),
    };
    Escalation {
        id: format!("{code}:{relative}"),
        code: code.to_owned(),
        target: Some(relative.to_owned()),
        severity: Severity::Warning,
        action: format!("{provider_id}: {action}"),
    }
}

/// Classify a non-fixpoint convergence stop as an escalation.
pub fn escalation_for_stop(
    relative: &str,
    stopped: StopReason,
    validated: bool,
) -> Option<Escalation> {
    match stopped {
        StopReason::Fixpoint => None,
        StopReason::BudgetExhausted => Some(Escalation {
            id: format!("budget-exhausted:{relative}"),
            code: "budget-exhausted".to_owned(),
            target: Some(relative.to_owned()),
            severity: Severity::Warning,
            action: "The convergence budget expired with eligible fixes left; re-run remediate.".to_owned(),
        }),
        StopReason::Oscillation => Some(Escalation {
            id: format!("oscillation:{relative}"),
            code: "oscillation".to_owned(),
            target: Some(relative.to_owned()),
            severity: if validated { Severity::Warning } else { Severity::Error },
            action: "A fix became applicable again after applying; inspect the provider pair before retrying.".to_owned(),
        }),
        StopReason::EditConflict => Some(Escalation {
            id: format!("edit-conflict:{relative}"),
            code: "edit-conflict".to_owned(),
            target: Some(relative.to_owned()),
            severity: Severity::Error,
            action: "An edit set failed validation against a changed base; re-diagnose before retrying.".to_owned(),
        }),
    }
}

/// Classify one residual error-severity diagnostic as an escalation.
pub fn escalation_for_diagnostic(relative: &str, diagnostic: &Diagnostic) -> Escalation {
    Escalation {
        id: format!("{}:{relative}", diagnostic.code),
        code: diagnostic.code.clone(),
        target: Some(relative.to_owned()),
        severity: diagnostic.severity,
        action: if diagnostic.suggested_action.is_empty() {
            "No safe transformation exists; see the evidence artifact for detail.".to_owned()
        } else {
            diagnostic.suggested_action.clone()
        },
    }
}

/// Build residual escalations over one validated state plus the fixpoint
/// verdict: per-file blocked providers and non-fixpoint stops, then one
/// escalation per residual error diagnostic. The `validated` flag carried
/// into stop classification tracks the running fixpoint verdict in file
/// order (a later break does not rewrite an earlier file's severity),
/// exactly as the remediation validation loop observes it.
/// Returns the escalations and whether every file sits at an immediate
/// Safe fixpoint with nothing applicable.
pub fn escalations_for_post_state(
    convergence: &[Convergence],
    diagnostics: &BTreeMap<String, Vec<Diagnostic>>,
) -> (Vec<Escalation>, bool) {
    let mut escalations = Vec::new();
    let mut idempotent = true;
    for conv in convergence {
        if !conv.applied.is_empty() || conv.stopped != StopReason::Fixpoint {
            idempotent = false;
        }
        for provider_id in &conv.blocked_review {
            escalations.push(escalation_for_blocked(
                provider_id,
                &conv.relative,
                Applicability::Review,
            ));
        }
        for provider_id in &conv.blocked_manual {
            escalations.push(escalation_for_blocked(
                provider_id,
                &conv.relative,
                Applicability::Manual,
            ));
        }
        if let Some(escalation) = escalation_for_stop(&conv.relative, conv.stopped, idempotent) {
            escalations.push(escalation);
        }
    }
    for (relative, file_diags) in diagnostics {
        for diag in file_diags {
            if diag.severity == Severity::Error {
                escalations.push(escalation_for_diagnostic(relative, diag));
            }
        }
    }
    (escalations, idempotent)
}

/// Count files carrying residual warning-severity diagnostics.
pub fn degraded_files(diagnostics: &BTreeMap<String, Vec<Diagnostic>>) -> usize {
    diagnostics
        .values()
        .filter(|diags| diags.iter().any(|diag| diag.severity == Severity::Warning))
        .count()
}

/// Resolve where the evidence artifact goes.
///
/// Explicit `--evidence-path` wins; otherwise the session artifact
/// directory exported by `mncs-environment` keeps the trail with the
/// calling session; otherwise the ignored `.mncs/doctor/` directory in
/// the repaired root holds it.
pub fn resolve_evidence_path(root: &Path, explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(dir) = std::env::var_os(ARTIFACT_DIR_ENV) {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Ok(dir.join(EVIDENCE_FILE_NAME));
        }
        return Err(format!(
            "{ARTIFACT_DIR_ENV} must be absolute, got {}",
            dir.display()
        ));
    }
    Ok(root.join(".mncs/doctor").join(EVIDENCE_FILE_NAME))
}

/// Write the evidence artifact atomically (temp + rename).
pub fn write_evidence(path: &Path, evidence: &RemediationEvidence) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create evidence directory: {error}"))?;
    }
    let bytes = serde_json::to_vec_pretty(evidence)
        .map_err(|error| format!("cannot encode remediation evidence: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, &bytes)
        .map_err(|error| format!("cannot write remediation evidence: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("cannot install remediation evidence: {error}"))?;
    Ok(())
}

/// Render the terse human summary: counts, remaining ids, evidence pointer.
pub fn render_human(report: &RemediationReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "remediation: repaired={} reconciled={} degraded={} blockers={}{}",
        report.summary.repaired,
        report.summary.reconciled,
        report.summary.degraded,
        report.summary.blockers,
        if report.dry_run { " (dry-run)" } else { "" },
    );
    if report.remaining.is_empty() {
        let _ = writeln!(out, "remaining: none");
    } else {
        let _ = writeln!(out, "remaining:");
        for id in &report.remaining {
            let _ = writeln!(out, "  {id}");
        }
    }
    match &report.evidence {
        Some(path) => {
            let _ = writeln!(out, "evidence: {path}");
        }
        None => {
            let _ = writeln!(out, "evidence: not persisted");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_derive_from_records() {
        let mut report = RemediationReport::new("/repo");
        report.repairs.push(RepairRecord {
            id: "hygiene.trailing-whitespace:a.mncs".to_owned(),
            class: RemediationClass::SafeAutomatic,
            provider: "hygiene.trailing-whitespace".to_owned(),
            target: Some("a.mncs".to_owned()),
            detail: "applied".to_owned(),
            before_fingerprint: None,
            after_fingerprint: None,
            validated: true,
        });
        report.reconciliations.push(RepairRecord {
            id: "inventory-cache:regenerated".to_owned(),
            class: RemediationClass::BoundedReconciliation,
            provider: "doctor.inventory-cache".to_owned(),
            target: None,
            detail: "regenerated".to_owned(),
            before_fingerprint: None,
            after_fingerprint: None,
            validated: true,
        });
        let escalations = vec![Escalation {
            id: "DOC108:b.mncs".to_owned(),
            code: "DOC108".to_owned(),
            target: Some("b.mncs".to_owned()),
            severity: Severity::Error,
            action: "fix".to_owned(),
        }];
        report.refresh_summary(&escalations, 1);
        assert_eq!(report.summary.repaired, 1);
        assert_eq!(report.summary.reconciled, 1);
        assert_eq!(report.summary.degraded, 1);
        assert_eq!(report.summary.blockers, 1);
        assert_eq!(report.remaining, vec!["DOC108:b.mncs".to_owned()]);
    }

    #[test]
    fn stop_reasons_classify_deterministically() {
        assert!(escalation_for_stop("a", StopReason::Fixpoint, true).is_none());
        let budget = escalation_for_stop("a", StopReason::BudgetExhausted, true).unwrap();
        assert_eq!(budget.id, "budget-exhausted:a");
        assert_eq!(budget.severity, Severity::Warning);
        let conflict = escalation_for_stop("a", StopReason::EditConflict, false).unwrap();
        assert_eq!(conflict.severity, Severity::Error);
    }

    #[test]
    fn human_render_stays_terse() {
        let mut report = RemediationReport::new("/repo");
        report.summary.repaired = 7;
        report.summary.reconciled = 2;
        report.evidence = Some("/tmp/e.json".to_owned());
        let text = render_human(&report);
        assert!(text.contains("repaired=7 reconciled=2 degraded=0 blockers=0"));
        assert!(text.contains("remaining: none"));
        assert!(text.contains("evidence: /tmp/e.json"));
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn evidence_schema_versions_are_pinned() {
        assert_eq!(REMEDIATION_SCHEMA_VERSION, "mncs.remediation/1");
        assert_eq!(
            REMEDIATION_EVIDENCE_SCHEMA_VERSION,
            "mncs.remediation-evidence/1"
        );
        let report = RemediationReport::new("/repo");
        assert!(report.to_json().contains(REMEDIATION_SCHEMA_VERSION));
    }
}
