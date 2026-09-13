//! Structured source edits.
//!
//! The edit representation is the machine-applicable half of the shared
//! diagnostic/fix contract: byte-span replacements with conflict detection,
//! deterministic application order, and source-fingerprint validation so a
//! fix computed against stale content can never silently apply.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::discovery::fingerprint;
use crate::fix::Applicability;
use crate::mncs_runtime::DoctorMncsRuntime;

/// A single span replacement. `start..end` are byte offsets into the base
/// content; `start == end` is an insertion, empty `replacement` a deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
    /// Human-readable label for reports and review.
    #[serde(default)]
    pub label: String,
    pub applicability: Applicability,
}

impl TextEdit {
    pub fn new(
        start: usize,
        end: usize,
        replacement: impl Into<String>,
        label: impl Into<String>,
        applicability: Applicability,
    ) -> Self {
        Self {
            start,
            end,
            replacement: replacement.into(),
            label: label.into(),
            applicability,
        }
    }

    fn validate(&self, base_len: usize) -> Result<(), EditError> {
        if self.start > self.end {
            return Err(EditError::InvertedSpan {
                start: self.start,
                end: self.end,
            });
        }
        if self.end > base_len {
            return Err(EditError::OutOfBounds {
                end: self.end,
                len: base_len,
            });
        }
        Ok(())
    }
}

/// A set of edits against one file, computed against a known base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditSet {
    /// Slash-separated relative path (matches [`crate::discovery`]).
    pub relative: String,
    /// SHA-256 of the base content the spans refer to.
    pub base_fingerprint: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("edit span {start}..{end} is inverted")]
    InvertedSpan { start: usize, end: usize },
    #[error("edit end {end} exceeds content length {len}")]
    OutOfBounds { end: usize, len: usize },
    #[error("edits overlap: [{a_start}..{a_end}] vs [{b_start}..{b_end}]")]
    Overlap {
        a_start: usize,
        a_end: usize,
        b_start: usize,
        b_end: usize,
    },
    #[error("stale base: expected fingerprint {expected}, found {found}")]
    StaleBase { expected: String, found: String },
    #[error("MNCS edit policy failed: {0}")]
    Policy(String),
}

impl EditSet {
    pub fn new(relative: impl Into<String>, base_fingerprint: impl Into<String>) -> Self {
        Self {
            relative: relative.into(),
            base_fingerprint: base_fingerprint.into(),
            edits: Vec::new(),
        }
    }

    pub fn push(&mut self, edit: TextEdit) {
        self.edits.push(edit);
    }

    /// Validate spans (bounds) and detect conflicts (overlaps, including
    /// insertions strictly inside another edit's range). Adjacent edits
    /// (`a.end == b.start`) do not conflict. Deterministic: edits are
    /// checked in sorted order.
    pub fn validate(&self, base_len: usize) -> Result<(), EditError> {
        self.validate_bounds(base_len)?;
        let mut sorted: Vec<&TextEdit> = self.edits.iter().collect();
        sorted.sort_by_key(|e| (e.start, e.end));
        for pair in sorted.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // Conflict unless strictly ordered, or both are insertions at the
            // same point (which still conflict: order would be ambiguous).
            let ordered = a.end < b.start || (a.end == b.start && a.start < b.start);
            let same_point_insert = a.start == a.end && a.start == b.start && b.start == b.end;
            if !ordered || same_point_insert {
                return Err(EditError::Overlap {
                    a_start: a.start,
                    a_end: a.end,
                    b_start: b.start,
                    b_end: b.end,
                });
            }
        }
        Ok(())
    }

    fn validate_bounds(&self, base_len: usize) -> Result<(), EditError> {
        for edit in &self.edits {
            edit.validate(base_len)?;
        }
        Ok(())
    }

    /// Validate using the production MNCS edit policy. The Rust `validate`
    /// method above remains an independent oracle for differential tests.
    pub fn validate_with_mncs(&self, base_len: usize) -> Result<(), EditError> {
        self.validate_bounds(base_len)?;
        let policy = DoctorMncsRuntime::production()
            .map_err(|error| EditError::Policy(error.to_string()))?;
        let mut sorted: Vec<&TextEdit> = self.edits.iter().collect();
        sorted.sort_by_key(|e| (e.start, e.end));
        for pair in sorted.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let conflict = policy
                .edits_pair_conflict(a.start as u64, a.end as u64, b.start as u64, b.end as u64)
                .map_err(|error| EditError::Policy(error.to_string()))?;
            if conflict {
                return Err(EditError::Overlap {
                    a_start: a.start,
                    a_end: a.end,
                    b_start: b.start,
                    b_end: b.end,
                });
            }
        }
        Ok(())
    }

    /// Apply the set to `base` after fingerprint and conflict validation.
    /// Application order is deterministic (descending offset, so earlier
    /// spans are unaffected by later replacements).
    pub fn apply(&self, base: &str) -> Result<String, EditError> {
        if fingerprint(base.as_bytes()) != self.base_fingerprint {
            return Err(EditError::StaleBase {
                expected: self.base_fingerprint.clone(),
                found: fingerprint(base.as_bytes()),
            });
        }
        self.validate(base.len())?;
        let mut sorted: Vec<&TextEdit> = self.edits.iter().collect();
        sorted.sort_by_key(|e| (e.start, e.end));
        let mut out = base.to_owned();
        for edit in sorted.into_iter().rev() {
            out.replace_range(edit.start..edit.end, &edit.replacement);
        }
        Ok(out)
    }

    /// Apply after production MNCS conflict validation. Fingerprint and text
    /// replacement remain host mechanisms.
    pub fn apply_with_mncs(&self, base: &str) -> Result<String, EditError> {
        if fingerprint(base.as_bytes()) != self.base_fingerprint {
            return Err(EditError::StaleBase {
                expected: self.base_fingerprint.clone(),
                found: fingerprint(base.as_bytes()),
            });
        }
        self.validate_with_mncs(base.len())?;
        let mut sorted: Vec<&TextEdit> = self.edits.iter().collect();
        sorted.sort_by_key(|e| (e.start, e.end));
        let mut out = base.to_owned();
        for edit in sorted.into_iter().rev() {
            out.replace_range(edit.start..edit.end, &edit.replacement);
        }
        Ok(out)
    }

    /// True when the set carries no edits.
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn len(&self) -> usize {
        self.edits.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(s: &str) -> String {
        fingerprint(s.as_bytes())
    }

    fn set(base: &str, edits: Vec<TextEdit>) -> EditSet {
        EditSet {
            relative: "t.mncs".to_owned(),
            base_fingerprint: fp(base),
            edits,
        }
    }

    fn safe(start: usize, end: usize, rep: &str) -> TextEdit {
        TextEdit::new(start, end, rep, "test", Applicability::Safe)
    }

    #[test]
    fn applies_multi_edit_in_stable_order() {
        let base = "mncs 0.8;\nmodule a;  \n";
        assert_eq!(base.len(), 22);
        let mut s = set(base, vec![safe(0, 9, "mncs 0.16;")]);
        // trailing whitespace removal ("module a;" ends at 19, spaces 19..21)
        s.push(safe(19, 21, ""));
        assert_eq!(s.apply(base).unwrap(), "mncs 0.16;\nmodule a;\n");
    }

    #[test]
    fn detects_overlapping_edits() {
        let base = "abcdef";
        let s = set(base, vec![safe(1, 4, "X"), safe(3, 6, "Y")]);
        assert!(matches!(
            s.validate(base.len()),
            Err(EditError::Overlap { .. })
        ));
    }

    #[test]
    fn adjacent_edits_do_not_conflict() {
        let base = "abcdef";
        let s = set(base, vec![safe(0, 2, "X"), safe(2, 4, "Y")]);
        assert!(s.validate(base.len()).is_ok());
        assert_eq!(s.apply(base).unwrap(), "XYef");
    }

    #[test]
    fn same_point_insertions_conflict() {
        let base = "ab";
        let s = set(base, vec![safe(1, 1, "X"), safe(1, 1, "Y")]);
        assert!(matches!(
            s.validate(base.len()),
            Err(EditError::Overlap { .. })
        ));
    }

    #[test]
    fn rejects_stale_base() {
        let s = set("abc", vec![safe(0, 1, "X")]);
        assert!(matches!(s.apply("abd"), Err(EditError::StaleBase { .. })));
    }

    #[test]
    fn rejects_out_of_bounds_and_inverted() {
        let base = "abc";
        assert!(matches!(
            set(base, vec![safe(0, 9, "X")]).validate(base.len()),
            Err(EditError::OutOfBounds { .. })
        ));
        assert!(matches!(
            set(base, vec![safe(2, 1, "X")]).validate(base.len()),
            Err(EditError::InvertedSpan { .. })
        ));
    }

    #[test]
    fn insertion_and_deletion_shapes() {
        let base = "ac";
        let s = set(base, vec![safe(1, 1, "b")]);
        assert_eq!(s.apply(base).unwrap(), "abc");
        let s = set("abc", vec![safe(1, 2, "")]);
        assert_eq!(s.apply("abc").unwrap(), "ac");
    }
}
