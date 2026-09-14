//! Source diagnostics integration.
//!
//! Boundary statement: `mncs-language` owns parsing, semantic interpretation,
//! and authoritative diagnostics. Doctor must not reimplement the parser.
//! This module therefore provides:
//!
//! 1. [`Diagnostic`] — a structured diagnostic envelope shaped so that an LSP
//!    code action and `mncs-doctor fix` can consume the *same* underlying
//!    repair semantics (code, severity, span, message, explanation,
//!    applicability, edit set, version metadata, migration transition).
//! 2. [`LanguageBackend`] — the trait through which real language analysis
//!    plugs in (a subprocess adapter over the Rust `mncs` CLI exists in
//!    [`crate::toolchain`]; a future in-process adapter over `mncs-syntax`
//!    types would implement this same trait).
//! 3. [`ScannerBackend`] — a deliberately shallow, byte-level repository
//!    hygiene scan (header presence, declared-version classification, module
//!    declaration presence, delimiter balance, and detailed diagnostic
//!    construction). Production BOM/newline facts are first obtained through
//!    the stateful `doctor.scanner.v1` ingress. It mirrors the *documented*
//!    header rule of upstream
//!    `infer_source_profile` (first non-blank/non-comment `mncs X.Y;`) and
//!    performs no semantic interpretation. Anything deeper belongs upstream
//!    (see `pressure/DOC-P-001.md`).

use serde::{Deserialize, Serialize};

use crate::discovery::SourceFile;
use crate::fix::Applicability;
use crate::version::{classify, LanguageVersion, VersionClass};

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

/// Where a diagnostic originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSource {
    /// Shallow repository-hygiene scan (this module). Never semantic.
    Hygiene,
    /// A real language backend (parser/elaborator via [`LanguageBackend`]).
    Language,
    /// Toolchain/manifest/project-level inference.
    Toolchain,
}

/// A byte-offset span with line/column projections (1-based lines,
/// 1-based columns counted in Unicode scalar values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

impl Span {
    /// Whole-file span for diagnostics without a precise location.
    pub fn whole_file(len: usize, lines: u32) -> Self {
        Self {
            start: 0,
            end: len,
            start_line: 1,
            start_col: 1,
            end_line: lines.max(1),
            end_col: 1,
        }
    }
}

/// A structured diagnostic in the proposed shared contract shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable code (`DOC1xx` hygiene, `MNE...`/`MNP...` language).
    pub code: String,
    pub severity: Severity,
    pub source: DiagnosticSource,
    pub span: Span,
    pub message: String,
    /// Longer explanation for `--explain` output.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub explanation: String,
    /// Repair safety classification.
    pub applicability: Applicability,
    /// Suggested action for the operator.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suggested_action: String,
    /// Declared language version this diagnostic is relative to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_version: Option<LanguageVersion>,
    /// Migration transition this diagnostic belongs to, if any (`"0.15 -> 0.16"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_transition: Option<String>,
    /// Name of the backend that produced this diagnostic.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub backend: String,
}

/// A language analysis backend: parse + diagnose one file, preserving spans.
pub trait LanguageBackend {
    fn name(&self) -> &str;
    fn diagnose(&self, file: &SourceFile) -> Vec<Diagnostic>;
}

/// Result of the shallow header scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderFacts {
    /// Byte span of the header statement, if present.
    pub span: Option<(usize, usize)>,
    /// Declared version text as written.
    pub declared_text: Option<String>,
    /// Declared version when it parses as `X.Y`.
    pub declared: Option<LanguageVersion>,
    /// Whether a `module ...;` declaration is present.
    pub has_module_decl: bool,
}

/// Extract header facts using the documented upstream rule: the first
/// non-blank, non-comment statement must be `mncs X.Y;`.
pub fn scan_header(text: &str) -> HeaderFacts {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut declared_text = None;
    let mut span = None;
    // Skip leading trivia (whitespace and comments), then inspect the first
    // real statement: the header must come first (matches upstream).
    skip_trivia(bytes, &mut i);
    if i < bytes.len() {
        // Candidate statement: read one `...;`-terminated statement.
        let stmt_start = i;
        while i < bytes.len() && bytes[i] != b';' && bytes[i] != b'\n' {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b';' {
            let stmt = text.get(stmt_start..=i).unwrap_or("");
            let mut parts = stmt.trim().trim_end_matches(';').split_whitespace();
            if parts.next() == Some("mncs") {
                if let Some(ver) = parts.next() {
                    declared_text = Some(ver.to_owned());
                    span = Some((stmt_start, i + 1));
                }
            }
            // Otherwise the first real statement is not a header: no header.
        }
    }
    let declared = declared_text
        .as_deref()
        .and_then(|t| t.parse::<LanguageVersion>().ok());
    HeaderFacts {
        span,
        declared_text,
        declared,
        has_module_decl: module_decl_present(text),
    }
}

fn skip_trivia(bytes: &[u8], i: &mut usize) {
    loop {
        while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
            *i += 1;
        }
        if *i + 1 < bytes.len() && bytes[*i] == b'/' && bytes[*i + 1] == b'/' {
            while *i < bytes.len() && bytes[*i] != b'\n' {
                *i += 1;
            }
            continue;
        }
        if *i + 1 < bytes.len() && bytes[*i] == b'/' && bytes[*i + 1] == b'*' {
            *i += 2;
            while *i + 1 < bytes.len() && !(bytes[*i] == b'*' && bytes[*i + 1] == b'/') {
                *i += 1;
            }
            *i = (*i + 2).min(bytes.len());
            continue;
        }
        break;
    }
}

fn module_decl_present(text: &str) -> bool {
    // `trim` makes detection tolerant of trailing horizontal whitespace (a
    // fixable hygiene issue, not a missing module).
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("module ") && t.ends_with(';') {
            return true;
        }
    }
    false
}

/// Convert a byte offset to (line, col), both 1-based; col counts Unicode
/// scalar values from the line start.
pub fn offset_to_line_col(text: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(text.len());
    let mut line = 1u32;
    let mut line_start = 0usize;
    for (idx, ch) in text.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = idx + 1;
        }
    }
    let col = text
        .get(line_start..offset)
        .map(|s| s.chars().count() as u32 + 1)
        .unwrap_or(1);
    (line, col)
}

fn span_of(text: &str, start: usize, end: usize) -> Span {
    let (sl, sc) = offset_to_line_col(text, start);
    let (el, ec) = offset_to_line_col(text, end);
    Span {
        start,
        end,
        start_line: sl,
        start_col: sc,
        end_line: el,
        end_col: ec,
    }
}

fn line_count(text: &str) -> u32 {
    text.lines().count() as u32
}

/// Shallow hygiene backend. See module docs for the non-goals.
pub struct ScannerBackend;

impl ScannerBackend {
    pub fn name(&self) -> &str {
        "scanner"
    }

    /// Run the hygiene scan with an injected version-policy classifier.
    ///
    /// The scanner still owns byte-level facts and diagnostic construction,
    /// but callers can route the version decision through the production
    /// MNCS policy runtime instead of silently using the Rust oracle.
    pub fn diagnose_with_version_classifier<F>(
        &self,
        file: &SourceFile,
        classifier: F,
    ) -> Result<Vec<Diagnostic>, String>
    where
        F: Fn(LanguageVersion) -> Result<VersionClass, String>,
    {
        let mut out = Vec::new();
        let Some(text) = file.text.as_deref() else {
            return Ok(vec![Diagnostic {
                code: "DOC108".to_owned(),
                severity: Severity::Error,
                source: DiagnosticSource::Hygiene,
                span: Span::whole_file(file.bytes.len(), 1),
                message: "file is not valid UTF-8".to_owned(),
                explanation: "MNCS sources are UTF-8 text. This file cannot be \
                    decoded, so no header or module facts can be established. \
                    Re-encode as UTF-8 (preserving content) and re-run."
                    .to_owned(),
                applicability: Applicability::Manual,
                suggested_action: "Re-encode the file as UTF-8.".to_owned(),
                language_version: None,
                migration_transition: None,
                backend: self.name().to_owned(),
            }]);
        };
        if file.has_bom {
            out.push(Diagnostic {
                code: "DOC107".to_owned(),
                severity: Severity::Warning,
                source: DiagnosticSource::Hygiene,
                span: span_of(text, 0, 0),
                message: "UTF-8 BOM present".to_owned(),
                explanation: "A byte-order mark precedes the file content. Upstream \
                    header inference inspects leading bytes; a BOM risks confusing \
                    header recognition and diff tooling."
                    .to_owned(),
                applicability: Applicability::Safe,
                suggested_action: "Remove the BOM (safe: formatting normalization).".to_owned(),
                language_version: None,
                migration_transition: None,
                backend: self.name().to_owned(),
            });
        }
        let facts = scan_header(text);
        match (&facts.declared_text, facts.declared) {
            (None, _) => out.push(Diagnostic {
                code: "DOC101".to_owned(),
                severity: Severity::Warning,
                source: DiagnosticSource::Hygiene,
                span: Span::whole_file(text.len(), line_count(text)),
                message: "missing source header".to_owned(),
                explanation: "No leading `mncs X.Y;` header was found. Upstream \
                    inference defaults headerless sources to 0.1, which silently \
                    pins the file to the oldest semantics. Declare the intended \
                    version explicitly."
                    .to_owned(),
                applicability: Applicability::Manual,
                suggested_action: "Add an explicit `mncs <version>;` header. Doctor \
                    does not guess the intended version."
                    .to_owned(),
                language_version: None,
                migration_transition: None,
                backend: self.name().to_owned(),
            }),
            (Some(_), None) => out.push(Diagnostic {
                code: "DOC102".to_owned(),
                severity: Severity::Error,
                source: DiagnosticSource::Hygiene,
                span: facts
                    .span
                    .map(|(s, e)| span_of(text, s, e))
                    .unwrap_or_else(|| Span::whole_file(text.len(), line_count(text))),
                message: "unparseable source header version".to_owned(),
                explanation: "A header-like statement was found but its version does \
                    not parse as `X.Y` with numeric components. The toolchain will \
                    reject this file at the profile gate."
                    .to_owned(),
                applicability: Applicability::Manual,
                suggested_action: "Correct the header to a known profile (e.g. \
                    `mncs 0.17;`)."
                    .to_owned(),
                language_version: None,
                migration_transition: None,
                backend: self.name().to_owned(),
            }),
            (Some(_), Some(version)) => match classifier(version)? {
                VersionClass::Current => {}
                VersionClass::Sealed => out.push(Diagnostic {
                    code: "DOC103".to_owned(),
                    severity: Severity::Info,
                    source: DiagnosticSource::Hygiene,
                    span: facts
                        .span
                        .map(|(s, e)| span_of(text, s, e))
                        .unwrap_or_else(|| Span::whole_file(text.len(), line_count(text))),
                    message: format!("sealed profile {version}: migration may apply"),
                    explanation: format!(
                        "This file declares sealed profile {version}. The current \
                        toolchain profile is {}. A migration path may carry this \
                        file forward; run `migrate --plan` to inspect it.",
                        crate::version::current_version()
                    ),
                    applicability: Applicability::Review,
                    suggested_action: "Plan a migration to the current profile.".to_owned(),
                    language_version: Some(version),
                    migration_transition: None,
                    backend: self.name().to_owned(),
                }),
                VersionClass::Unsupported => out.push(Diagnostic {
                    code: "DOC104".to_owned(),
                    severity: Severity::Error,
                    source: DiagnosticSource::Hygiene,
                    span: facts
                        .span
                        .map(|(s, e)| span_of(text, s, e))
                        .unwrap_or_else(|| Span::whole_file(text.len(), line_count(text))),
                    message: format!("unsupported profile {version}"),
                    explanation: "This profile is known but rejected by the toolchain \
                        (reserved/unsupported). The file cannot be elaborated as-is."
                        .to_owned(),
                    applicability: Applicability::Manual,
                    suggested_action: "Retarget to a supported profile.".to_owned(),
                    language_version: Some(version),
                    migration_transition: None,
                    backend: self.name().to_owned(),
                }),
                VersionClass::Unknown => out.push(Diagnostic {
                    code: "DOC102".to_owned(),
                    severity: Severity::Error,
                    source: DiagnosticSource::Hygiene,
                    span: facts
                        .span
                        .map(|(s, e)| span_of(text, s, e))
                        .unwrap_or_else(|| Span::whole_file(text.len(), line_count(text))),
                    message: format!("unknown profile {version}"),
                    explanation: "The declared version is absent from the known profile \
                        registry (too old, too new, or mistyped). The toolchain \
                        profile gate will reject it."
                        .to_owned(),
                    applicability: Applicability::Manual,
                    suggested_action: "Correct the header to a known profile.".to_owned(),
                    language_version: Some(version),
                    migration_transition: None,
                    backend: self.name().to_owned(),
                }),
            },
        }
        if !facts.has_module_decl {
            out.push(Diagnostic {
                code: "DOC105".to_owned(),
                severity: Severity::Warning,
                source: DiagnosticSource::Hygiene,
                span: Span::whole_file(text.len(), line_count(text)),
                message: "missing module declaration".to_owned(),
                explanation: "No `module dotted.path;` declaration was found. One \
                    module declaration per file is the project convention; without \
                    it, module resolution cannot place this file."
                    .to_owned(),
                applicability: Applicability::Manual,
                suggested_action: "Add the owning `module` declaration. Doctor does \
                    not invent module names."
                    .to_owned(),
                language_version: facts.declared,
                migration_transition: None,
                backend: self.name().to_owned(),
            });
        }
        if let Some(span) = unbalanced_delimiter(text) {
            out.push(Diagnostic {
                code: "DOC106".to_owned(),
                severity: Severity::Error,
                source: DiagnosticSource::Hygiene,
                span,
                message: "unbalanced delimiter".to_owned(),
                explanation: "A coarse byte-level scan (strings and comments \
                    excluded) found a closing delimiter without its opener, or \
                    input ended with delimiters still open. This is a hygiene \
                    heuristic, not a parse: the authoritative error comes from \
                    the language backend."
                    .to_owned(),
                applicability: Applicability::Manual,
                suggested_action: "Fix the delimiter imbalance; then re-run with a \
                    language backend for the authoritative diagnostic."
                    .to_owned(),
                language_version: facts.declared,
                migration_transition: None,
                backend: self.name().to_owned(),
            });
        }
        Ok(out)
    }
}

impl LanguageBackend for ScannerBackend {
    fn name(&self) -> &str {
        self.name()
    }

    fn diagnose(&self, file: &SourceFile) -> Vec<Diagnostic> {
        self.diagnose_with_version_classifier(file, |version| Ok(classify(Some(version))))
            .expect("the Rust reference version classifier is infallible")
    }
}

/// Coarse delimiter-balance scan skipping strings and comments. Returns the
/// span of the first offending delimiter, if any.
fn unbalanced_delimiter(text: &str) -> Option<Span> {
    let bytes = text.as_bytes();
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut line_start = 0usize;
    let pos_of = |idx: usize, ln: u32, ls: usize| Span {
        start: idx,
        end: idx + 1,
        start_line: ln,
        start_col: text
            .get(ls..idx)
            .map(|s| s.chars().count() as u32 + 1)
            .unwrap_or(1),
        end_line: ln,
        end_col: text
            .get(ls..idx)
            .map(|s| s.chars().count() as u32 + 2)
            .unwrap_or(2),
    };
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' {
            line += 1;
            line_start = i + 1;
            i += 1;
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if bytes[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        if b == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            continue;
        }
        match b {
            b'{' | b'(' | b'[' => stack.push((b, i)),
            b'}' | b')' | b']' => {
                let expect = match b {
                    b'}' => b'{',
                    b')' => b'(',
                    _ => b'[',
                };
                match stack.pop() {
                    Some((o, _)) if o == expect => {}
                    _ => return Some(pos_of(i, line, line_start)),
                }
            }
            _ => {}
        }
        i += 1;
    }
    if let Some((_, idx)) = stack.pop() {
        let (sl, _) = offset_to_line_col(text, idx);
        let ls = text[..idx].rfind('\n').map(|p| p + 1).unwrap_or(0);
        return Some(pos_of(idx, sl, ls));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{detect_newline, fingerprint};

    fn file_with(text: &str) -> SourceFile {
        let bytes = text.as_bytes().to_vec();
        SourceFile {
            path: std::path::PathBuf::from("t.mncs"),
            relative: "t.mncs".to_owned(),
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

    fn codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().map(|d| d.code.as_str()).collect()
    }

    #[test]
    fn healthy_current_file_is_quiet() {
        let diags = ScannerBackend.diagnose(&file_with("mncs 0.17;\nmodule a;\nfn f() {}\n"));
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn missing_header_and_module_reported() {
        let diags = ScannerBackend.diagnose(&file_with("fn f() {}\n"));
        assert!(codes(&diags).contains(&"DOC101"));
        assert!(codes(&diags).contains(&"DOC105"));
    }

    #[test]
    fn sealed_profile_is_info_with_version() {
        let diags = ScannerBackend.diagnose(&file_with("mncs 0.8;\nmodule a;\n"));
        let d = diags.iter().find(|d| d.code == "DOC103").expect("DOC103");
        assert_eq!(d.severity, Severity::Info);
        assert_eq!(d.language_version, Some(LanguageVersion::new(0, 8)));
    }

    #[test]
    fn unsupported_and_unknown_profiles_error() {
        let diags = ScannerBackend.diagnose(&file_with("mncs 1.0;\nmodule a;\n"));
        assert!(codes(&diags).contains(&"DOC104"));
        let diags = ScannerBackend.diagnose(&file_with("mncs 0.99;\nmodule a;\n"));
        assert!(codes(&diags).contains(&"DOC102"));
        let diags = ScannerBackend.diagnose(&file_with("mncs latest;\nmodule a;\n"));
        assert!(codes(&diags).contains(&"DOC102"));
    }

    #[test]
    fn header_after_comments_is_found() {
        let diags = ScannerBackend.diagnose(&file_with(
            "// c\n/* multi\nline */\nmncs 0.16;\nmodule a;\n",
        ));
        assert!(!codes(&diags).contains(&"DOC101"), "{diags:?}");
    }

    #[test]
    fn trailing_whitespace_does_not_hide_module_decl() {
        // Regression: hygiene whitespace must not present as a missing
        // module (the fix engine strips it as SAFE).
        let diags = ScannerBackend.diagnose(&file_with("mncs 0.16;\nmodule a;  \nfn f() {}\n"));
        assert!(!codes(&diags).contains(&"DOC105"), "{diags:?}");
    }

    #[test]
    fn unbalanced_delimiters_reported_with_span() {
        let text = "mncs 0.16;\nmodule a;\nfn f() {\n";
        let diags = ScannerBackend.diagnose(&file_with(text));
        let d = diags.iter().find(|d| d.code == "DOC106").expect("DOC106");
        assert_eq!(d.severity, Severity::Error);
        assert!(d.span.start < text.len());
        // Delimiters inside strings/comments do not count.
        let ok = "mncs 0.16;\nmodule a;\nfn f() {} // }\n/* { */\nlet s = \"{\";\n";
        let diags = ScannerBackend.diagnose(&file_with(ok));
        assert!(!codes(&diags).contains(&"DOC106"), "{diags:?}");
    }

    #[test]
    fn spans_preserve_byte_offsets() {
        let text = "mncs 0.99;\nmodule a;\n";
        let diags = ScannerBackend.diagnose(&file_with(text));
        let d = diags.iter().find(|d| d.code == "DOC102").expect("DOC102");
        assert_eq!(&text[d.span.start..d.span.end], "mncs 0.99;");
        assert_eq!((d.span.start_line, d.span.start_col), (1, 1));
    }
}
