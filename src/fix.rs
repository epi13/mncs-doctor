//! Automated repair: applicability model, fix providers, convergence loop.
//!
//! Safety model (initial):
//!
//! - [`Applicability::Safe`] — formatting normalization, unambiguous syntax
//!   modernization where semantics are unchanged, metadata normalization,
//!   deterministic canonicalization.
//! - [`Applicability::SemanticallyProven`] — type/semantic-directed
//!   transformations with established equivalence (no built-in provider
//!   claims this yet; the level exists so language-backed providers can).
//! - [`Applicability::Review`] — highly likely migration with possible
//!   semantic implications; never applied without explicit opt-in.
//! - [`Applicability::Manual`] — explainable but not safely transformable.
//!
//! Default automatic repair applies `Safe` only (`SemanticallyProven` with
//! `--proven`, never `Review`/`Manual`). One repair may expose another, so
//! [`repair_to_fixpoint`] iterates diagnose → repair → re-diagnose until no
//! eligible fix remains, an oscillation is detected, or the iteration budget
//! is exhausted.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{Diagnostic, LanguageBackend};
use crate::discovery::{fingerprint, SourceFile};
use crate::edits::{EditSet, TextEdit};

/// Repair safety classification. Ordered by increasing risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Applicability {
    Safe,
    SemanticallyProven,
    Review,
    Manual,
}

impl std::fmt::Display for Applicability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Applicability::Safe => write!(f, "safe"),
            Applicability::SemanticallyProven => write!(f, "semantically_proven"),
            Applicability::Review => write!(f, "review"),
            Applicability::Manual => write!(f, "manual"),
        }
    }
}

/// A proposed repair: the diagnostics it addresses plus the edit set that
/// implements it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fix {
    /// Stable provider id (e.g. `hygiene.trailing-whitespace`).
    pub provider: String,
    pub title: String,
    pub applicability: Applicability,
    /// Codes of diagnostics this fix addresses (may be empty for pure
    /// normalization providers that scan text directly).
    pub addresses: Vec<String>,
    pub edits: EditSet,
}

/// A provider of fixes for one file's current text.
pub trait FixProvider {
    fn provider_id(&self) -> &str;
    fn fixes_for(&self, relative: &str, text: &str, diagnostics: &[Diagnostic]) -> Vec<Fix>;
}

fn fp(text: &str) -> String {
    fingerprint(text.as_bytes())
}

/// Removes trailing horizontal whitespace at line ends (SAFE).
pub struct TrailingWhitespace;
impl FixProvider for TrailingWhitespace {
    fn provider_id(&self) -> &str {
        "hygiene.trailing-whitespace"
    }

    fn fixes_for(&self, relative: &str, text: &str, _diagnostics: &[Diagnostic]) -> Vec<Fix> {
        // Note: `split_inclusive` already yields the unterminated final line,
        // so no separate last-line pass is needed (a second pass would emit
        // a duplicate span and trip conflict detection).
        let mut set = EditSet::new(relative, fp(text));
        for line in text.split_inclusive('\n') {
            let stripped = line.trim_end_matches(['\r', '\n']);
            let trimmed = stripped.trim_end_matches([' ', '\t']);
            if trimmed.len() != stripped.len() {
                let start = line.as_ptr() as usize - text.as_ptr() as usize + trimmed.len();
                let end = start + (stripped.len() - trimmed.len());
                set.push(TextEdit::new(
                    start,
                    end,
                    "",
                    "remove trailing whitespace",
                    Applicability::Safe,
                ));
            }
        }
        if set.is_empty() {
            return Vec::new();
        }
        vec![Fix {
            provider: self.provider_id().to_owned(),
            title: "Remove trailing whitespace".to_owned(),
            applicability: Applicability::Safe,
            addresses: Vec::new(),
            edits: set,
        }]
    }
}

/// Ensures the file ends with exactly one `\n` (SAFE).
pub struct FinalNewline;
impl FixProvider for FinalNewline {
    fn provider_id(&self) -> &str {
        "hygiene.final-newline"
    }

    fn fixes_for(&self, relative: &str, text: &str, _diagnostics: &[Diagnostic]) -> Vec<Fix> {
        if text.is_empty() || text.ends_with('\n') {
            return Vec::new();
        }
        let mut set = EditSet::new(relative, fp(text));
        set.push(TextEdit::new(
            text.len(),
            text.len(),
            "\n",
            "append final newline",
            Applicability::Safe,
        ));
        vec![Fix {
            provider: self.provider_id().to_owned(),
            title: "Append missing final newline".to_owned(),
            applicability: Applicability::Safe,
            addresses: Vec::new(),
            edits: set,
        }]
    }
}

/// Normalizes CRLF line endings to LF (SAFE). Mixed-ending files are
/// normalized only when every CRLF is a clean `\r\n` pair; lone `\r`
/// characters block the fix (conservative: possible semantic content).
pub struct CrlfToLf;
impl FixProvider for CrlfToLf {
    fn provider_id(&self) -> &str {
        "hygiene.crlf-to-lf"
    }

    fn fixes_for(&self, relative: &str, text: &str, _diagnostics: &[Diagnostic]) -> Vec<Fix> {
        if !text.contains("\r\n") || text.contains('\r') && has_lone_cr(text) {
            return Vec::new();
        }
        let mut set = EditSet::new(relative, fp(text));
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                set.push(TextEdit::new(
                    i,
                    i + 1,
                    "",
                    "normalize CRLF to LF",
                    Applicability::Safe,
                ));
                i += 2;
            } else {
                i += 1;
            }
        }
        if set.is_empty() {
            return Vec::new();
        }
        vec![Fix {
            provider: self.provider_id().to_owned(),
            title: "Normalize CRLF line endings to LF".to_owned(),
            applicability: Applicability::Safe,
            addresses: vec!["DOC107".to_owned()],
            edits: set,
        }]
    }
}

fn has_lone_cr(text: &str) -> bool {
    let bytes = text.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'\r' && (i + 1 >= bytes.len() || bytes[i + 1] != b'\n') {
            return true;
        }
    }
    false
}

/// Removes a leading UTF-8 BOM (SAFE; addresses DOC107).
pub struct RemoveBom;
impl FixProvider for RemoveBom {
    fn provider_id(&self) -> &str {
        "hygiene.remove-bom"
    }

    fn fixes_for(&self, relative: &str, text: &str, _diagnostics: &[Diagnostic]) -> Vec<Fix> {
        if !text.starts_with('\u{FEFF}') {
            return Vec::new();
        }
        let mut set = EditSet::new(relative, fp(text));
        set.push(TextEdit::new(
            0,
            '\u{FEFF}'.len_utf8(),
            "",
            "remove UTF-8 BOM",
            Applicability::Safe,
        ));
        vec![Fix {
            provider: self.provider_id().to_owned(),
            title: "Remove UTF-8 BOM".to_owned(),
            applicability: Applicability::Safe,
            addresses: vec!["DOC107".to_owned()],
            edits: set,
        }]
    }
}

/// The built-in provider set (all SAFE hygiene normalization).
pub fn default_providers() -> Vec<Box<dyn FixProvider>> {
    vec![
        Box::new(TrailingWhitespace),
        Box::new(FinalNewline),
        Box::new(CrlfToLf),
        Box::new(RemoveBom),
    ]
}

/// Which applicability levels an operation may apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Eligibility {
    pub allow_safe: bool,
    pub allow_proven: bool,
}

impl Eligibility {
    /// Default automatic repair: `Safe` only.
    pub fn safe_only() -> Self {
        Self {
            allow_safe: true,
            allow_proven: false,
        }
    }

    pub fn with_proven() -> Self {
        Self {
            allow_safe: true,
            allow_proven: true,
        }
    }

    pub fn allows(&self, level: Applicability) -> bool {
        match level {
            Applicability::Safe => self.allow_safe,
            Applicability::SemanticallyProven => self.allow_proven,
            Applicability::Review | Applicability::Manual => false,
        }
    }
}

/// Outcome of planning a whole workspace in deterministic file order.
#[derive(Debug)]
pub struct WorkspacePlan {
    pub plans: Vec<(SourceFile, FilePlan)>,
    pub blocked_review: usize,
    pub blocked_manual: usize,
    pub conflict_notes: Vec<String>,
}

/// Plan repairs for every file with text, in deterministic (sorted) order.
/// Single pass; convergence happens at apply/verify time.
pub fn plan_workspace(
    sources: &[SourceFile],
    diagnostics: &std::collections::BTreeMap<String, Vec<Diagnostic>>,
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
) -> WorkspacePlan {
    let empty = Vec::new();
    let mut ordered: Vec<&SourceFile> = sources.iter().collect();
    ordered.sort_by(|a, b| a.relative.cmp(&b.relative));
    let mut plans = Vec::new();
    let mut blocked_review = 0usize;
    let mut blocked_manual = 0usize;
    let mut conflict_notes = Vec::new();
    for file in ordered {
        let Some(text) = file.text.as_deref() else {
            continue;
        };
        let mut round_review = Vec::new();
        let mut round_manual = Vec::new();
        let mut round_conflicts = Vec::new();
        let file_diags = diagnostics.get(&file.relative).unwrap_or(&empty);
        if let Some(plan) = plan_file(
            &file.relative,
            text,
            file_diags,
            providers,
            eligibility,
            &mut round_review,
            &mut round_manual,
            &mut round_conflicts,
        ) {
            plans.push(((*file).clone(), plan));
        }
        blocked_review += round_review.len();
        blocked_manual += round_manual.len();
        if !round_conflicts.is_empty() {
            conflict_notes.push(format!(
                "{}: {} conflicting fix(es) skipped",
                file.relative,
                round_conflicts.len()
            ));
        }
    }
    WorkspacePlan {
        plans,
        blocked_review,
        blocked_manual,
        conflict_notes,
    }
}

/// One planned file repair: ordered fixes plus the resulting content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePlan {
    pub relative: String,
    pub base_fingerprint: String,
    pub fixes: Vec<Fix>,
    pub result_fingerprint: String,
    /// Number of changed lines (added + removed) in the plan.
    pub changed_lines: usize,
}

/// Outcome of the convergence loop for one file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Convergence {
    pub relative: String,
    #[serde(default)]
    pub iterations: u32,
    #[serde(default)]
    pub applied: Vec<String>,
    #[serde(default)]
    pub blocked_review: Vec<String>,
    #[serde(default)]
    pub blocked_manual: Vec<String>,
    /// Fixes skipped for conflicting with an earlier fix in the same round.
    #[serde(default)]
    pub skipped_conflicts: Vec<String>,
    pub stopped: StopReason,
}

/// Why the loop stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// No eligible fixes remain.
    Fixpoint,
    /// Iteration budget exhausted with fixes still eligible.
    BudgetExhausted,
    /// A previously applied fix became applicable again (non-idempotent
    /// provider or oscillating pair); the repeat was not applied.
    Oscillation,
    /// An edit set failed validation (conflict or stale base).
    EditConflict,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::Fixpoint => write!(f, "fixpoint"),
            StopReason::BudgetExhausted => write!(f, "budget-exhausted"),
            StopReason::Oscillation => write!(f, "oscillation"),
            StopReason::EditConflict => write!(f, "edit-conflict"),
        }
    }
}

/// Maximum repair iterations per file before giving up.
pub const MAX_ITERATIONS: u32 = 16;

/// Plan repairs for one file's text: collect fixes from all providers and
/// keep eligible ones. Fixes are merged incrementally in provider order: a
/// fix whose edits conflict with already-kept edits is skipped (recorded in
/// `skipped_conflicts`) rather than dropping the whole file plan. Skipping
/// is fail-closed per fix — the surviving plan still validates as a union.
#[allow(clippy::too_many_arguments)] // one out-vec per outcome class; call sites pass locals
pub fn plan_file(
    relative: &str,
    text: &str,
    diagnostics: &[Diagnostic],
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
    blocked_review: &mut Vec<String>,
    blocked_manual: &mut Vec<String>,
    skipped_conflicts: &mut Vec<String>,
) -> Option<FilePlan> {
    let mut union = EditSet::new(relative, fp(text));
    let mut kept: Vec<Fix> = Vec::new();
    for provider in providers {
        for fix in provider.fixes_for(relative, text, diagnostics) {
            if !eligibility.allows(fix.applicability) {
                match fix.applicability {
                    Applicability::Review => blocked_review.push(fix.title.clone()),
                    Applicability::Manual => blocked_manual.push(fix.title.clone()),
                    _ => {}
                }
                continue;
            }
            let before = union.edits.len();
            for edit in &fix.edits.edits {
                union.push(edit.clone());
            }
            if union.validate(text.len()).is_err() {
                union.edits.truncate(before);
                skipped_conflicts.push(format!(
                    "{}: conflicts with an earlier fix; skipped",
                    fix.title
                ));
                continue;
            }
            kept.push(fix);
        }
    }
    if kept.is_empty() {
        return None;
    }
    let result = union.apply(text).ok()?;
    let changed_lines = count_changed_lines(text, &result);
    Some(FilePlan {
        relative: relative.to_owned(),
        base_fingerprint: fp(text),
        fixes: kept,
        result_fingerprint: fp(&result),
        changed_lines,
    })
}

#[cfg(test)]
mod workspace_tests {
    use super::*;
    use crate::diagnostics::ScannerBackend;
    use std::collections::BTreeMap;

    fn workspace_src(relative: &str, text: &str) -> SourceFile {
        let bytes = text.as_bytes().to_vec();
        SourceFile {
            path: std::path::PathBuf::from(relative),
            relative: relative.to_owned(),
            sha256: fp(text),
            len: bytes.len() as u64,
            newline: crate::discovery::detect_newline(&bytes),
            has_bom: false,
            mode: None,
            is_symlink: false,
            text: Some(text.to_owned()),
            bytes,
        }
    }

    #[test]
    fn workspace_plan_is_deterministic_and_sorted() {
        let files = vec![
            workspace_src("b.mncs", "mncs 0.16;\nmodule b;  \n"),
            workspace_src("a.mncs", "mncs 0.16;\nmodule a;\n"),
        ];
        let backend = ScannerBackend;
        let mut diags = BTreeMap::new();
        for file in &files {
            diags.insert(file.relative.clone(), backend.diagnose(file));
        }
        let first = plan_workspace(
            &files,
            &diags,
            &default_providers(),
            Eligibility::safe_only(),
        );
        let second = plan_workspace(
            &files,
            &diags,
            &default_providers(),
            Eligibility::safe_only(),
        );
        assert_eq!(first.plans.len(), 1);
        assert_eq!(first.plans[0].0.relative, "b.mncs");
        assert_eq!(
            first.plans[0].1.result_fingerprint,
            second.plans[0].1.result_fingerprint
        );
    }
}

fn count_changed_lines(before: &str, after: &str) -> usize {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let prefix = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let suffix = suffix
        .min(a.len().saturating_sub(prefix))
        .min(b.len().saturating_sub(prefix));
    (a.len() - prefix - suffix) + (b.len() - prefix - suffix)
}

/// Iteratively repair one file to a fixpoint.
///
/// Returns the final text and a [`Convergence`] record. `diagnose` is
/// re-run after every applied plan so newly exposed issues are picked up.
/// Applied provider ids are tracked per file: if a provider fires again
/// after having fired before *without the file reaching a clean state*,
/// the loop stops with [`StopReason::Oscillation`] instead of looping.
pub fn repair_to_fixpoint(
    file: &SourceFile,
    backend: &dyn LanguageBackend,
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
) -> (String, Convergence) {
    let mut text = file.text.clone().unwrap_or_default();
    let mut applied: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut blocked_review: Vec<String> = Vec::new();
    let mut blocked_manual: Vec<String> = Vec::new();
    let mut skipped_conflicts: Vec<String> = Vec::new();
    let mut iterations = 0u32;

    // Hygiene providers scan text directly; the backend re-diagnoses so
    // language findings stay fresh for the report.
    let mut current = file.clone();
    loop {
        if iterations >= MAX_ITERATIONS {
            return (
                text,
                Convergence {
                    relative: file.relative.clone(),
                    iterations,
                    applied,
                    blocked_review,
                    blocked_manual,
                    skipped_conflicts: skipped_conflicts.clone(),
                    stopped: StopReason::BudgetExhausted,
                },
            );
        }
        current.text = Some(text.clone());
        current.bytes = text.as_bytes().to_vec();
        let diagnostics = backend.diagnose(&current);
        let mut round_review = Vec::new();
        let mut round_manual = Vec::new();
        let mut round_conflicts = Vec::new();
        let plan = plan_file(
            &file.relative,
            &text,
            &diagnostics,
            providers,
            eligibility,
            &mut round_review,
            &mut round_manual,
            &mut round_conflicts,
        );
        blocked_review.extend(round_review);
        blocked_manual.extend(round_manual);
        skipped_conflicts.extend(round_conflicts);
        let Some(plan) = plan else {
            return (
                text,
                Convergence {
                    relative: file.relative.clone(),
                    iterations,
                    applied,
                    blocked_review,
                    blocked_manual,
                    skipped_conflicts: skipped_conflicts.clone(),
                    stopped: StopReason::Fixpoint,
                },
            );
        };
        iterations += 1;
        // Oscillation guard: the same provider firing twice means its fix
        // is not idempotent (or two providers fight). Apply once, then stop.
        let ids: Vec<String> = plan.fixes.iter().map(|f| f.provider.clone()).collect();
        if ids.iter().any(|id| seen.contains(id)) {
            return (
                text,
                Convergence {
                    relative: file.relative.clone(),
                    iterations,
                    applied,
                    blocked_review,
                    blocked_manual,
                    skipped_conflicts: skipped_conflicts.clone(),
                    stopped: StopReason::Oscillation,
                },
            );
        }
        for id in &ids {
            seen.insert(id.clone());
            applied.push(id.clone());
        }
        match union_apply(&plan, &text) {
            Some(next) => text = next,
            None => {
                return (
                    text,
                    Convergence {
                        relative: file.relative.clone(),
                        iterations,
                        applied,
                        blocked_review,
                        blocked_manual,
                        skipped_conflicts: skipped_conflicts.clone(),
                        stopped: StopReason::EditConflict,
                    },
                )
            }
        }
    }
}

/// Apply a plan's union of edits to `base`.
pub fn union_apply(plan: &FilePlan, base: &str) -> Option<String> {
    let mut union = EditSet::new(&plan.relative, plan.base_fingerprint.clone());
    for fix in &plan.fixes {
        if fix.edits.base_fingerprint != plan.base_fingerprint {
            return None;
        }
        for edit in &fix.edits.edits {
            union.push(edit.clone());
        }
    }
    union.apply(base).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::ScannerBackend;

    fn providers() -> Vec<Box<dyn FixProvider>> {
        default_providers()
    }

    fn src(relative: &str, text: &str) -> SourceFile {
        let bytes = text.as_bytes().to_vec();
        SourceFile {
            path: std::path::PathBuf::from(relative),
            relative: relative.to_owned(),
            sha256: fp(text),
            len: bytes.len() as u64,
            newline: crate::discovery::detect_newline(&bytes),
            has_bom: text.starts_with('\u{FEFF}'),
            mode: None,
            is_symlink: false,
            text: Some(text.to_owned()),
            bytes,
        }
    }

    #[test]
    fn safe_fixes_converge_to_clean_text() {
        let file = src("a.mncs", "mncs 0.16;\nmodule a;  \nfn f() {}");
        let backend = ScannerBackend;
        let (text, conv) =
            repair_to_fixpoint(&file, &backend, &providers(), Eligibility::safe_only());
        assert_eq!(text, "mncs 0.16;\nmodule a;\nfn f() {}\n");
        assert_eq!(conv.stopped, StopReason::Fixpoint);
        assert!(!conv.applied.is_empty());
    }

    #[test]
    fn clean_file_is_immediate_fixpoint() {
        let file = src("a.mncs", "mncs 0.16;\nmodule a;\n");
        let backend = ScannerBackend;
        let (text, conv) =
            repair_to_fixpoint(&file, &backend, &providers(), Eligibility::safe_only());
        assert_eq!(text, "mncs 0.16;\nmodule a;\n");
        assert_eq!(conv.stopped, StopReason::Fixpoint);
        assert!(conv.applied.is_empty());
    }

    #[test]
    fn crlf_and_bom_normalize() {
        let file = src("a.mncs", "\u{FEFF}mncs 0.16;\r\nmodule a;\r\n");
        let backend = ScannerBackend;
        let (text, conv) =
            repair_to_fixpoint(&file, &backend, &providers(), Eligibility::safe_only());
        assert_eq!(text, "mncs 0.16;\nmodule a;\n");
        assert_eq!(conv.stopped, StopReason::Fixpoint);
    }

    #[test]
    fn oscillation_is_detected_not_looped() {
        struct FlipFlop;
        impl FixProvider for FlipFlop {
            fn provider_id(&self) -> &str {
                "test.flipflop"
            }
            fn fixes_for(&self, relative: &str, text: &str, _d: &[Diagnostic]) -> Vec<Fix> {
                // Non-idempotent provider: always appends; fires every round.
                let mut set = EditSet::new(relative, fp(text));
                set.push(TextEdit::new(
                    text.len(),
                    text.len(),
                    "x",
                    "append x",
                    Applicability::Safe,
                ));
                vec![Fix {
                    provider: self.provider_id().to_owned(),
                    title: "Append x".to_owned(),
                    applicability: Applicability::Safe,
                    addresses: Vec::new(),
                    edits: set,
                }]
            }
        }
        let file = src("a.mncs", "mncs 0.16;\nmodule a;\n");
        let backend = ScannerBackend;
        let providers: Vec<Box<dyn FixProvider>> = vec![Box::new(FlipFlop)];
        let (text, conv) =
            repair_to_fixpoint(&file, &backend, &providers, Eligibility::safe_only());
        assert_eq!(conv.stopped, StopReason::Oscillation);
        assert_eq!(text, "mncs 0.16;\nmodule a;\nx");
    }

    #[test]
    fn conflicting_fix_is_skipped_not_fatal() {
        struct OverlapA;
        struct OverlapB;
        impl FixProvider for OverlapA {
            fn provider_id(&self) -> &str {
                "test.overlap-a"
            }
            fn fixes_for(&self, relative: &str, text: &str, _d: &[Diagnostic]) -> Vec<Fix> {
                let mut set = EditSet::new(relative, fp(text));
                set.push(TextEdit::new(0, 3, "AAA", "a", Applicability::Safe));
                vec![Fix {
                    provider: self.provider_id().to_owned(),
                    title: "A".to_owned(),
                    applicability: Applicability::Safe,
                    addresses: Vec::new(),
                    edits: set,
                }]
            }
        }
        impl FixProvider for OverlapB {
            fn provider_id(&self) -> &str {
                "test.overlap-b"
            }
            fn fixes_for(&self, relative: &str, text: &str, _d: &[Diagnostic]) -> Vec<Fix> {
                let mut set = EditSet::new(relative, fp(text));
                set.push(TextEdit::new(1, 4, "BBB", "b", Applicability::Safe));
                vec![Fix {
                    provider: self.provider_id().to_owned(),
                    title: "B".to_owned(),
                    applicability: Applicability::Safe,
                    addresses: Vec::new(),
                    edits: set,
                }]
            }
        }
        let providers: Vec<Box<dyn FixProvider>> = vec![Box::new(OverlapA), Box::new(OverlapB)];
        let mut review = Vec::new();
        let mut manual = Vec::new();
        let mut conflicts = Vec::new();
        let plan = plan_file(
            "t",
            "abcdef",
            &[],
            &providers,
            Eligibility::safe_only(),
            &mut review,
            &mut manual,
            &mut conflicts,
        )
        .expect("one fix survives");
        assert_eq!(plan.fixes.len(), 1);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(union_apply(&plan, "abcdef").unwrap(), "AAAdef");
    }

    #[test]
    fn review_fixes_are_never_auto_applied() {
        struct NeedsReview;
        impl FixProvider for NeedsReview {
            fn provider_id(&self) -> &str {
                "test.review"
            }
            fn fixes_for(&self, relative: &str, text: &str, _d: &[Diagnostic]) -> Vec<Fix> {
                let mut set = EditSet::new(relative, fp(text));
                set.push(TextEdit::new(
                    0,
                    0,
                    "REVIEWED",
                    "review",
                    Applicability::Review,
                ));
                vec![Fix {
                    provider: self.provider_id().to_owned(),
                    title: "Review me".to_owned(),
                    applicability: Applicability::Review,
                    addresses: Vec::new(),
                    edits: set,
                }]
            }
        }
        let file = src("a.mncs", "mncs 0.16;\nmodule a;\n");
        let backend = ScannerBackend;
        let providers: Vec<Box<dyn FixProvider>> = vec![Box::new(NeedsReview)];
        let (text, conv) =
            repair_to_fixpoint(&file, &backend, &providers, Eligibility::safe_only());
        assert_eq!(text, "mncs 0.16;\nmodule a;\n");
        assert_eq!(conv.blocked_review, vec!["Review me".to_owned()]);
    }
}
