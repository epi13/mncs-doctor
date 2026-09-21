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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{Diagnostic, LanguageBackend};
use crate::discovery::{fingerprint, SourceFile};
use crate::edits::{EditSet, TextEdit};
use crate::language_knowledge::{load_migrations, CanonicalModuleRewrite};

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

/// Applies language-owned canonical module identity rewrites. The provider
/// deliberately recognizes only `module` and `use` declarations and ignores
/// comments, so historical evidence and prose are not rewritten as if they
/// were active source. Semantic ownership remains in the migration manifest
/// published by `mncs-language`.
pub struct CanonicalModuleIdentities {
    rewrites: Vec<CanonicalModuleRewrite>,
}

impl CanonicalModuleIdentities {
    pub fn new(rewrites: Vec<CanonicalModuleRewrite>) -> Self {
        Self { rewrites }
    }
}

impl FixProvider for CanonicalModuleIdentities {
    fn provider_id(&self) -> &str {
        "language.canonical-module-identities"
    }

    fn fixes_for(&self, relative: &str, text: &str, _diagnostics: &[Diagnostic]) -> Vec<Fix> {
        let mut fixes = Vec::new();
        for rewrite in &self.rewrites {
            let spans = module_identity_spans(text, &rewrite.obsolete);
            if spans.is_empty() {
                continue;
            }
            let applicability = if rewrite.mechanically_safe {
                Applicability::Safe
            } else {
                Applicability::Review
            };
            let mut edits = EditSet::new(relative, fp(text));
            for (start, end) in spans {
                edits.push(TextEdit::new(
                    start,
                    end,
                    rewrite.canonical.clone(),
                    format!(
                        "{}: {} -> {}",
                        rewrite.id, rewrite.obsolete, rewrite.canonical
                    ),
                    applicability,
                ));
            }
            fixes.push(Fix {
                provider: self.provider_id().to_owned(),
                title: format!(
                    "Canonicalize MNCS module identity {} -> {}",
                    rewrite.obsolete, rewrite.canonical
                ),
                applicability,
                addresses: vec![format!("MNCS-MIGRATION-{}", rewrite.id)],
                edits,
            });
        }
        fixes
    }
}

fn is_module_identity_byte(value: u8) -> bool {
    value.is_ascii_alphanumeric() || value == b'_' || value == b'.'
}

fn module_identity_spans(text: &str, obsolete: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        // MNCS source currently has no string literal form. Restricting the
        // scan to declaration text still keeps comments and historical prose
        // outside the migration boundary.
        let code = &line[..line.find("//").unwrap_or(line.len())];
        let trimmed = code.trim_start();
        if !(trimmed.starts_with("module ") || trimmed.starts_with("use ")) {
            offset += line.len();
            continue;
        }
        for (position, _) in code.match_indices(obsolete) {
            let before = position
                .checked_sub(1)
                .and_then(|index| code.as_bytes().get(index).copied());
            let after = code.as_bytes().get(position + obsolete.len()).copied();
            if before.is_some_and(is_module_identity_byte)
                || after.is_some_and(is_module_identity_byte)
            {
                continue;
            }
            spans.push((offset + position, offset + position + obsolete.len()));
        }
        offset += line.len();
    }
    spans
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

/// Build the default provider set plus language-owned canonicalization rules.
/// A missing manifest is normal; a malformed present manifest fails closed.
pub fn providers_for_root(root: &std::path::Path) -> Result<Vec<Box<dyn FixProvider>>, String> {
    let mut providers = default_providers();
    if let Some((_path, manifest)) = load_migrations(Some(root))? {
        providers.insert(
            0,
            Box::new(CanonicalModuleIdentities::new(manifest.rewrites)) as Box<dyn FixProvider>,
        );
    }
    Ok(providers)
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
    plan_workspace_with_policies(
        sources,
        diagnostics,
        providers,
        eligibility,
        &rust_validate_edits,
        &rust_apply_edits,
    )
    .expect("reference edit policy is infallible")
}

/// Plan repairs with an explicit edit-policy boundary. The production
/// command supplies MNCS validation/application callbacks; the public
/// [`plan_workspace`] wrapper supplies the independent Rust oracle used by
/// unit and differential tests.
pub fn plan_workspace_with_mncs_policy(
    sources: &[SourceFile],
    diagnostics: &std::collections::BTreeMap<String, Vec<Diagnostic>>,
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
) -> Result<WorkspacePlan, String> {
    plan_workspace_with_policies(
        sources,
        diagnostics,
        providers,
        eligibility,
        &mncs_validate_edits,
        &mncs_apply_edits,
    )
}

fn plan_workspace_with_policies(
    sources: &[SourceFile],
    diagnostics: &std::collections::BTreeMap<String, Vec<Diagnostic>>,
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
    validate_edits: &dyn Fn(&EditSet, usize) -> Result<bool, String>,
    apply_edits: &dyn Fn(&EditSet, &str) -> Result<String, String>,
) -> Result<WorkspacePlan, String> {
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
        if let Some(plan) = plan_file_with_policies(
            &file.relative,
            text,
            file_diags,
            providers,
            eligibility,
            &mut round_review,
            &mut round_manual,
            &mut round_conflicts,
            validate_edits,
            apply_edits,
        )? {
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
    Ok(WorkspacePlan {
        plans,
        blocked_review,
        blocked_manual,
        conflict_notes,
    })
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

type StopRule = dyn Fn(bool, bool, u32, u32) -> Result<Option<StopReason>, String>;
type SeenBefore = dyn Fn(&[u64], u64) -> Result<bool, String>;
pub type Diagnose<'a> = dyn Fn(&SourceFile) -> Result<Vec<Diagnostic>, String> + 'a;
type PlanBuilder = dyn Fn(
    &str,
    &str,
    &[Diagnostic],
    &[Box<dyn FixProvider>],
    Eligibility,
    &mut Vec<String>,
    &mut Vec<String>,
    &mut Vec<String>,
) -> Result<Option<FilePlan>, String>;
type PlanApply = dyn Fn(&FilePlan, &str) -> Result<String, String>;

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
    plan_file_with_policies(
        relative,
        text,
        diagnostics,
        providers,
        eligibility,
        blocked_review,
        blocked_manual,
        skipped_conflicts,
        &rust_validate_edits,
        &rust_apply_edits,
    )
    .expect("reference edit policy is infallible")
}

#[allow(clippy::too_many_arguments)]
fn plan_file_with_policies(
    relative: &str,
    text: &str,
    diagnostics: &[Diagnostic],
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
    blocked_review: &mut Vec<String>,
    blocked_manual: &mut Vec<String>,
    skipped_conflicts: &mut Vec<String>,
    validate_edits: &dyn Fn(&EditSet, usize) -> Result<bool, String>,
    apply_edits: &dyn Fn(&EditSet, &str) -> Result<String, String>,
) -> Result<Option<FilePlan>, String> {
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
            if !validate_edits(&union, text.len())? {
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
        return Ok(None);
    }
    let result = apply_edits(&union, text)?;
    let changed_lines = count_changed_lines(text, &result);
    Ok(Some(FilePlan {
        relative: relative.to_owned(),
        base_fingerprint: fp(text),
        fixes: kept,
        result_fingerprint: fp(&result),
        changed_lines,
    }))
}

fn rust_validate_edits(union: &EditSet, base_len: usize) -> Result<bool, String> {
    Ok(union.validate(base_len).is_ok())
}

fn mncs_validate_edits(union: &EditSet, base_len: usize) -> Result<bool, String> {
    match union.validate_with_mncs(base_len) {
        Ok(()) => Ok(true),
        Err(crate::edits::EditError::Policy(error)) => Err(error.to_string()),
        Err(_) => Ok(false),
    }
}

fn rust_apply_edits(union: &EditSet, base: &str) -> Result<String, String> {
    union.apply(base).map_err(|error| error.to_string())
}

fn mncs_apply_edits(union: &EditSet, base: &str) -> Result<String, String> {
    union.apply_with_mncs(base).map_err(|error| match error {
        crate::edits::EditError::Policy(policy) => policy.to_string(),
        other => other.to_string(),
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
    let stop_rule = |planned_empty: bool,
                     fired_before: bool,
                     iterations: u32,
                     budget: u32|
     -> Result<Option<StopReason>, String> {
        if planned_empty {
            return Ok(Some(StopReason::Fixpoint));
        }
        if fired_before {
            return Ok(Some(StopReason::Oscillation));
        }
        if iterations >= budget {
            return Ok(Some(StopReason::BudgetExhausted));
        }
        Ok(None)
    };
    let seen_before = |fired: &[u64], id: u64| -> Result<bool, String> { Ok(fired.contains(&id)) };
    let diagnose = |current: &SourceFile| Ok(backend.diagnose(current));
    let plan_builder = |relative: &str,
                        text: &str,
                        diagnostics: &[Diagnostic],
                        providers: &[Box<dyn FixProvider>],
                        eligibility: Eligibility,
                        blocked_review: &mut Vec<String>,
                        blocked_manual: &mut Vec<String>,
                        skipped_conflicts: &mut Vec<String>| {
        Ok(self::plan_file(
            relative,
            text,
            diagnostics,
            providers,
            eligibility,
            blocked_review,
            blocked_manual,
            skipped_conflicts,
        ))
    };
    let apply = |plan: &FilePlan, text: &str| union_apply_reference(plan, text);
    repair_to_fixpoint_core(
        file,
        providers,
        eligibility,
        &stop_rule,
        &seen_before,
        &diagnose,
        &plan_builder,
        &apply,
    )
    .expect("reference fix policy is infallible")
}

/// Iteratively repair one file using explicit MNCS-backed loop policy.
///
/// Providers still acquire text and construct edits in the host. The loop's
/// state transitions (budget, oscillation, and repeated-provider detection)
/// are delegated through the supplied policy callbacks so production can use
/// `doctor.fix.v1`, while the wrapper above remains an independent Rust
/// reference for differential tests.
pub fn repair_to_fixpoint_with_policy(
    file: &SourceFile,
    backend: &dyn LanguageBackend,
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
    stop_rule: &StopRule,
    seen_before: &SeenBefore,
) -> Result<(String, Convergence), String> {
    let diagnose = |current: &SourceFile| Ok(backend.diagnose(current));
    repair_to_fixpoint_with_diagnose(
        file,
        providers,
        eligibility,
        stop_rule,
        seen_before,
        &diagnose,
    )
}

/// Iteratively repair one file using MNCS-backed loop policy and an injected
/// diagnostic producer. Production supplies a scanner/version producer here
/// so re-diagnosis after each applied round does not silently return to the
/// Rust scanner oracle.
pub fn repair_to_fixpoint_with_diagnose(
    file: &SourceFile,
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
    stop_rule: &StopRule,
    seen_before: &SeenBefore,
    diagnose: &Diagnose<'_>,
) -> Result<(String, Convergence), String> {
    let plan_builder = |relative: &str,
                        text: &str,
                        diagnostics: &[Diagnostic],
                        providers: &[Box<dyn FixProvider>],
                        eligibility: Eligibility,
                        blocked_review: &mut Vec<String>,
                        blocked_manual: &mut Vec<String>,
                        skipped_conflicts: &mut Vec<String>| {
        plan_file_with_policies(
            relative,
            text,
            diagnostics,
            providers,
            eligibility,
            blocked_review,
            blocked_manual,
            skipped_conflicts,
            &mncs_validate_edits,
            &mncs_apply_edits,
        )
    };
    let apply = |plan: &FilePlan, text: &str| union_apply_with_mncs(plan, text);
    repair_to_fixpoint_core(
        file,
        providers,
        eligibility,
        stop_rule,
        seen_before,
        diagnose,
        &plan_builder,
        &apply,
    )
}

#[allow(clippy::too_many_arguments)]
fn repair_to_fixpoint_core(
    file: &SourceFile,
    providers: &[Box<dyn FixProvider>],
    eligibility: Eligibility,
    stop_rule: &StopRule,
    seen_before: &SeenBefore,
    diagnose: &Diagnose<'_>,
    plan_builder: &PlanBuilder,
    apply: &PlanApply,
) -> Result<(String, Convergence), String> {
    let mut text = file.text.clone().unwrap_or_default();
    let mut applied: Vec<String> = Vec::new();
    let mut fired: Vec<u64> = Vec::new();
    let mut provider_codes: BTreeMap<u64, String> = BTreeMap::new();
    let mut blocked_review: Vec<String> = Vec::new();
    let mut blocked_manual: Vec<String> = Vec::new();
    let mut skipped_conflicts: Vec<String> = Vec::new();
    let mut iterations = 0u32;

    // Hygiene providers scan text directly; the injected diagnostic producer
    // re-diagnoses so language findings stay fresh for the report.
    let mut current = file.clone();
    loop {
        if let Some(reason) = stop_rule(false, false, iterations, MAX_ITERATIONS)? {
            if reason != StopReason::BudgetExhausted {
                return Err(format!("unexpected pre-plan stop reason: {reason}"));
            }
            return Ok((
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
            ));
        }
        current.text = Some(text.clone());
        current.bytes = text.as_bytes().to_vec();
        let diagnostics = diagnose(&current)?;
        let mut round_review = Vec::new();
        let mut round_manual = Vec::new();
        let mut round_conflicts = Vec::new();
        let plan = plan_builder(
            &file.relative,
            &text,
            &diagnostics,
            providers,
            eligibility,
            &mut round_review,
            &mut round_manual,
            &mut round_conflicts,
        )?;
        blocked_review.extend(round_review);
        blocked_manual.extend(round_manual);
        skipped_conflicts.extend(round_conflicts);
        let Some(plan) = plan else {
            let reason = stop_rule(true, false, iterations, MAX_ITERATIONS)?
                .ok_or_else(|| "fix policy continued after an empty plan".to_owned())?;
            if reason != StopReason::Fixpoint {
                return Err(format!("unexpected empty-plan stop reason: {reason}"));
            }
            return Ok((
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
            ));
        };
        // Oscillation guard: the same provider firing twice means its fix
        // is not idempotent (or two providers fight). Apply once, then stop.
        let ids: Vec<String> = plan.fixes.iter().map(|f| f.provider.clone()).collect();
        let mut fired_before = false;
        let mut round_codes = Vec::with_capacity(ids.len());
        for id in &ids {
            let code = provider_code(id);
            if let Some(previous) = provider_codes.get(&code) {
                if previous != id {
                    return Err(format!(
                        "provider identity collision between {previous:?} and {id:?}"
                    ));
                }
            } else {
                provider_codes.insert(code, id.clone());
            }
            if seen_before(&fired, code)? {
                fired_before = true;
            }
            round_codes.push(code);
        }
        if let Some(reason) = stop_rule(false, fired_before, iterations, MAX_ITERATIONS)? {
            if reason != StopReason::Oscillation {
                return Err(format!("unexpected planned stop reason: {reason}"));
            }
            return Ok((
                text,
                Convergence {
                    relative: file.relative.clone(),
                    iterations: iterations + 1,
                    applied,
                    blocked_review,
                    blocked_manual,
                    skipped_conflicts: skipped_conflicts.clone(),
                    stopped: StopReason::Oscillation,
                },
            ));
        }
        iterations += 1;
        for (id, code) in ids.iter().zip(round_codes) {
            fired.push(code);
            applied.push(id.clone());
        }
        text = apply(&plan, &text)
            .map_err(|error| format!("edit application failed (fail-closed): {error}"))?;
    }
}

fn provider_code(id: &str) -> u64 {
    // FNV-1a is deterministic across processes and platforms. A collision
    // is rejected above instead of allowing two providers to alias in the
    // bounded MNCS convergence state.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Apply a plan's union of edits to `base`.
pub fn union_apply(plan: &FilePlan, base: &str) -> Option<String> {
    union_apply_reference(plan, base).ok()
}

fn union_apply_reference(plan: &FilePlan, base: &str) -> Result<String, String> {
    let mut union = EditSet::new(&plan.relative, plan.base_fingerprint.clone());
    for fix in &plan.fixes {
        if fix.edits.base_fingerprint != plan.base_fingerprint {
            return Err("fix base fingerprint differs from file plan".to_owned());
        }
        for edit in &fix.edits.edits {
            union.push(edit.clone());
        }
    }
    union.apply(base).map_err(|error| error.to_string())
}

/// Apply a file plan after MNCS has validated its edit policy. Fingerprint
/// checking and text replacement remain host mechanisms; a policy/runtime
/// error is returned so production cannot silently downgrade to a Rust path.
pub fn union_apply_with_mncs(plan: &FilePlan, base: &str) -> Result<String, String> {
    let mut union = EditSet::new(&plan.relative, plan.base_fingerprint.clone());
    for fix in &plan.fixes {
        if fix.edits.base_fingerprint != plan.base_fingerprint {
            return Err("fix base fingerprint differs from file plan".to_owned());
        }
        for edit in &fix.edits.edits {
            union.push(edit.clone());
        }
    }
    union
        .apply_with_mncs(base)
        .map_err(|error| error.to_string())
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
    fn canonical_module_fix_rewrites_declarations_but_not_history() {
        let rewrite = CanonicalModuleRewrite {
            id: "test.digest-v2".to_owned(),
            kind: "module_import".to_owned(),
            obsolete: "mncs.index.digest.v2".to_owned(),
            canonical: "mncs.index.digest".to_owned(),
            mechanically_safe: true,
            semantic_caveats: "same implementation".to_owned(),
            minimum_profile: "0.10".to_owned(),
            source_transformation: "declaration only".to_owned(),
            verification: crate::language_knowledge::MigrationVerification {
                command: "pytest".to_owned(),
                obligation: "kernel tests".to_owned(),
            },
        };
        let provider = CanonicalModuleIdentities::new(vec![rewrite]);
        let text = "mncs 0.10;\n// module mncs.index.digest.v2 is historical\nmodule mncs.index.digest.v2;\nuse mncs.index.digest.v2 as digest;\n";
        let fixes = provider.fixes_for("src/digest.mncs", text, &[]);
        assert_eq!(fixes.len(), 1);
        let result = fixes[0].edits.apply(text).unwrap();
        assert!(result.contains("// module mncs.index.digest.v2 is historical"));
        assert!(result.contains("module mncs.index.digest;"));
        assert!(result.contains("use mncs.index.digest as digest;"));
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
