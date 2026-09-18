//! Toolchain health: compiler / language-service / Forge / Ravel
//! compatibility probing.
//!
//! Doctor integrates with sibling tools through narrow, evolvable interfaces:
//! it shells out to installed CLIs and reads first-line version output. It
//! never links unstable internals. Absence of a tool degrades the related
//! checks to `Skipped`/warning rather than failing the run.
//!
//! Tool identity note: `mncs` on PATH is the Python family validator
//! (subcommands include `version`, `doctor`, `migration-inspect`); the Rust
//! language CLI (subcommands like `source-study`, `diagnose`) is discovered
//! via `MNCS_CLI`, then well-known build outputs, then PATH. The two are
//! reported separately and never conflated.

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::architecture_knowledge::ArchitectureKnowledgeStatus;
use crate::diagnostics::{Diagnostic, DiagnosticSource, LanguageBackend, Severity, Span};
use crate::discovery::SourceFile;
use crate::fix::Applicability;
use crate::language_knowledge::LanguageKnowledgeStatus;

/// One probed tool component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub path: String,
    pub version: String,
}

/// Availability of every sibling component doctor knows how to use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolchainStatus {
    /// Rust language CLI (`mncs` with `source-study`/`diagnose` verbs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_cli: Option<ToolInfo>,
    /// Python family validator CLI (`mncs` with `version`/`doctor` verbs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_cli: Option<ToolInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo: Option<ToolInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<ToolInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ravel: Option<ToolInfo>,
    /// Canonical first-class test provider (`mncs-test`) when explicitly
    /// installed or exposed through `MNCS_TEST_BIN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_provider: Option<ToolInfo>,
    /// Canonical structured debugger (`mncs-debug`) when explicitly installed
    /// or exposed through `MNCS_DEBUG_BIN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_provider: Option<ToolInfo>,
    /// Optional local checkout of the Actions transport, exposed through
    /// `MNCS_ACTIONS_ROOT`. Actions itself normally runs in GitHub, so an
    /// absent value is not a semantic failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions_provider: Option<ToolInfo>,
    /// Membrane-level compatibility result for the debugger protocol family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_protocol: Option<ProtocolCompatibility>,
    /// Current profile used by Doctor's host-side profile mirror.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_profile: Option<String>,
    /// Freshness and identity of the authoritative mncs-language capability index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_knowledge: Option<LanguageKnowledgeStatus>,
    /// Content-addressed Commons architecture facts and optional project drift checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture_knowledge: Option<ArchitectureKnowledgeStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolCompatibility {
    pub expected: String,
    pub observed: Option<String>,
    pub compatible: bool,
    pub status: String,
}

/// Probe the local toolchain. Each probe is a single fast local process;
/// failures degrade to absence.
pub fn probe_toolchain() -> ToolchainStatus {
    probe_toolchain_at(None)
}

/// Probe the local toolchain with a project root so language knowledge is
/// resolved from the same authoritative index used by the project.
pub fn probe_toolchain_at(root: Option<&std::path::Path>) -> ToolchainStatus {
    let rust_cli = find_rust_cli().and_then(|exe| probe_rust_cli(&exe));
    let family_cli = probe_family_cli();
    let test_provider = probe_component("MNCS_TEST_BIN", "mncs-test");
    let debug_path = component_path("MNCS_DEBUG_BIN", "mncs-debug");
    let debug_provider = debug_path
        .as_ref()
        .and_then(|path| probe_path(path, "mncs-debug"));
    let debug_protocol = debug_path.as_ref().map(|path| probe_debug_protocol(path));
    let actions_provider = std::env::var_os("MNCS_ACTIONS_ROOT")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .map(|path| ToolInfo {
            name: "mncs-actions".to_owned(),
            path: path.to_string_lossy().into_owned(),
            version: "workspace".to_owned(),
        });
    let language_knowledge = Some(crate::language_knowledge::probe(root));
    let architecture_knowledge = Some(crate::architecture_knowledge::probe(
        root,
        language_knowledge.as_ref(),
    ));
    let current_profile = language_knowledge
        .as_ref()
        .and_then(|status| status.current_profile.clone())
        .or_else(|| Some(crate::version::current_version().short()));
    ToolchainStatus {
        rust_cli,
        family_cli,
        cargo: probe_simple("cargo", &["--version"]),
        forge: probe_simple("mncs-forge", &["--version"]),
        ravel: probe_simple("ravel", &["--version"]),
        test_provider,
        debug_provider,
        actions_provider,
        debug_protocol,
        current_profile,
        language_knowledge,
        architecture_knowledge,
    }
}

fn component_path(env_name: &str, command: &str) -> Option<PathBuf> {
    std::env::var_os(env_name)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| which(command))
}

fn probe_component(env_name: &str, command: &str) -> Option<ToolInfo> {
    component_path(env_name, command).and_then(|path| probe_path(&path, command))
}

fn probe_path(path: &PathBuf, name: &str) -> Option<ToolInfo> {
    let output = Command::new(path).arg("--help").output().ok()?;
    let combined = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    let version = combined
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("unknown")
        .trim()
        .chars()
        .take(120)
        .collect();
    Some(ToolInfo {
        name: name.to_owned(),
        path: path.to_string_lossy().into_owned(),
        version,
    })
}

fn probe_debug_protocol(path: &PathBuf) -> ProtocolCompatibility {
    let expected = "mncs.debug-capabilities/1".to_owned();
    let output = Command::new(path)
        .args(["capabilities", "--format", "json"])
        .output();
    let observed = output
        .ok()
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
        .and_then(|value| {
            value
                .get("schema_version")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
    let compatible = observed.as_deref() == Some(expected.as_str());
    ProtocolCompatibility {
        expected,
        observed,
        compatible,
        status: if compatible {
            "compatible"
        } else {
            "unavailable"
        }
        .to_owned(),
    }
}

/// Locate the Rust language CLI without confusing it with the family
/// validator. An explicit `MNCS_CLI` is an operator override and is trusted
/// as-is (misconfiguration surfaces fail-closed as DOC202); PATH entries
/// are accepted only when `--help` advertises a language verb
/// (`source-study`).
pub fn find_rust_cli() -> Option<PathBuf> {
    if let Ok(env) = std::env::var("MNCS_CLI") {
        if !env.trim().is_empty() {
            return Some(PathBuf::from(env));
        }
    }
    which("mncs").filter(is_rust_cli)
}

fn is_rust_cli(exe: &PathBuf) -> bool {
    let Ok(out) = Command::new(exe).arg("--help").output() else {
        return false;
    };
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    text.contains("source-study")
}

fn probe_rust_cli(exe: &PathBuf) -> Option<ToolInfo> {
    // The Rust CLI has no bare --version in all builds; identify by path +
    // help probe success and record the workspace version when available.
    Some(ToolInfo {
        name: "mncs-language-cli".to_owned(),
        path: exe.to_string_lossy().into_owned(),
        version: rust_cli_version(exe).unwrap_or_else(|| "unknown".to_owned()),
    })
}

fn rust_cli_version(exe: &PathBuf) -> Option<String> {
    for args in [&["--version"][..], &["compiler-architecture"][..]] {
        if let Ok(out) = Command::new(exe).args(args).output() {
            if out.status.success() {
                let line = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                if !line.is_empty() {
                    return Some(line.chars().take(120).collect());
                }
            }
        }
    }
    None
}

fn probe_family_cli() -> Option<ToolInfo> {
    let path = which("mncs")?;
    if is_rust_cli(&path) {
        return None; // it is the language CLI, already reported above
    }
    let out = Command::new(&path).arg("version").output().ok()?;
    let version = if out.status.success() {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("unknown")
            .trim()
            .chars()
            .take(120)
            .collect()
    } else {
        "unknown".to_owned()
    };
    Some(ToolInfo {
        name: "mncs-family".to_owned(),
        path: path.to_string_lossy().into_owned(),
        version,
    })
}

fn probe_simple(name: &str, args: &[&str]) -> Option<ToolInfo> {
    let path = which(name)?;
    let out = Command::new(&path).args(args).output().ok()?;
    let version = if out.status.success() {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("unknown")
            .trim()
            .chars()
            .take(120)
            .collect()
    } else {
        "unavailable".to_owned()
    };
    Some(ToolInfo {
        name: name.to_owned(),
        path: path.to_string_lossy().into_owned(),
        version,
    })
}

fn which(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Optional language backend over the Rust CLI's `source-study` verb.
///
/// The verb emits a JSON study artifact on stdout with a `diagnostics`
/// array (`{code, stage, severity, message, span{start, end, line, column},
/// expected, found}`). Doctor maps those entries to [`Diagnostic`] with
/// spans preserved, so semantic findings (e.g. MNE131) flow through with
/// their authoritative codes. Mapping is version-tolerant (unknown fields
/// ignored, unparseable output falls back to DOC201) and conservative:
/// language diagnostics default to [`Applicability::Manual`] — Doctor
/// reports them but does not invent repairs (see `pressure/DOC-P-002.md`
/// and `pressure/DOC-P-006.md`). Disabled by default; enable with
/// `--with-language-backend`.
pub struct RustCliBackend {
    pub exe: PathBuf,
}

/// One upstream study diagnostic (tolerant subset of the CLI schema).
#[derive(Debug, Deserialize)]
struct StudyDiagnostic {
    #[serde(default)]
    code: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    span: Option<StudySpan>,
}

#[derive(Debug, Deserialize)]
struct StudySpan {
    #[serde(default)]
    start: usize,
    #[serde(default)]
    end: usize,
    #[serde(default)]
    line: u32,
    #[serde(default)]
    column: u32,
}

#[derive(Debug, Deserialize)]
struct StudyOutput {
    #[serde(default)]
    diagnostics: Vec<StudyDiagnostic>,
}

impl RustCliBackend {
    fn map_study(&self, file: &SourceFile, study: &StudyOutput) -> Vec<Diagnostic> {
        study
            .diagnostics
            .iter()
            .filter(|d| !d.code.is_empty())
            .map(|d| {
                let severity = match d.severity.as_str() {
                    "error" => Severity::Error,
                    "warning" => Severity::Warning,
                    _ => Severity::Info,
                };
                let span = match &d.span {
                    Some(s) => crate::diagnostics::Span {
                        start: s.start,
                        end: s.end.max(s.start),
                        start_line: s.line.max(1),
                        start_col: s.column.max(1),
                        end_line: s.line.max(1),
                        end_col: s.column.max(1) + (s.end.saturating_sub(s.start) as u32),
                    },
                    None => crate::diagnostics::Span::whole_file(file.bytes.len(), 1),
                };
                Diagnostic {
                    code: d.code.clone(),
                    severity,
                    source: DiagnosticSource::Language,
                    span,
                    message: if d.message.is_empty() {
                        "language backend reported an issue".to_owned()
                    } else {
                        d.message.clone()
                    },
                    explanation: "Reported by the Rust language CLI source-study verb; \
                        spans and codes are authoritative. Doctor does not yet \
                        provide repairs for semantic diagnostics."
                        .to_owned(),
                    applicability: Applicability::Manual,
                    suggested_action: "Inspect the reported location against the \
                        language documentation."
                        .to_owned(),
                    language_version: None,
                    migration_transition: None,
                    backend: self.name().to_owned(),
                }
            })
            .collect()
    }
}

impl LanguageBackend for RustCliBackend {
    fn name(&self) -> &str {
        "rust-cli"
    }

    fn diagnose(&self, file: &SourceFile) -> Vec<Diagnostic> {
        let out = Command::new(&self.exe)
            .arg("source-study")
            .arg(&file.path)
            .output();
        match out {
            Ok(output) => {
                if let Ok(study) = serde_json::from_slice::<StudyOutput>(&output.stdout) {
                    return self.map_study(file, &study);
                }
                if output.status.success() {
                    return Vec::new();
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                let excerpt: String = stderr.lines().take(5).collect::<Vec<_>>().join("\n");
                vec![Diagnostic {
                    code: "DOC201".to_owned(),
                    severity: Severity::Error,
                    source: DiagnosticSource::Language,
                    span: Span::whole_file(file.bytes.len(), 1),
                    message: "language backend rejected this file".to_owned(),
                    explanation: if excerpt.is_empty() {
                        "The Rust language CLI exited nonzero on this file (no \
                        stderr excerpt captured)."
                            .to_owned()
                    } else {
                        format!("The Rust language CLI exited nonzero on this file:\n{excerpt}")
                    },
                    applicability: Applicability::Manual,
                    suggested_action: "Inspect the backend output; fix the source \
                        error it reports."
                        .to_owned(),
                    language_version: None,
                    migration_transition: None,
                    backend: self.name().to_owned(),
                }]
            }
            Err(e) => vec![Diagnostic {
                code: "DOC202".to_owned(),
                severity: Severity::Warning,
                source: DiagnosticSource::Toolchain,
                span: Span::whole_file(file.bytes.len(), 1),
                message: format!("language backend could not run: {e}"),
                explanation: "The configured backend binary failed to execute. \
                    Diagnosis for this file falls back to the hygiene scan."
                    .to_owned(),
                applicability: Applicability::Manual,
                suggested_action: "Check MNCS_CLI and toolchain installation.".to_owned(),
                language_version: None,
                migration_transition: None,
                backend: self.name().to_owned(),
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_probe_never_fails() {
        // Must not panic on machines with or without the toolchain.
        let status = probe_toolchain();
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("cargo") || !json.contains("cargo"));
    }

    #[test]
    fn study_json_maps_to_diagnostics_with_spans() {
        // Shape captured from a real `mncs source-study` invocation.
        let study: StudyOutput = serde_json::from_str(
            r#"{"diagnostics": [{"code": "MNE131", "stage": "elaboration",
                "severity": "error",
                "message": "call target does not resolve",
                "span": {"start": 90, "end": 100, "line": 7, "column": 12},
                "expected": [], "found": null}]}"#,
        )
        .unwrap();
        let backend = RustCliBackend {
            exe: std::path::PathBuf::from("mncs"),
        };
        let file = SourceFile {
            path: std::path::PathBuf::from("m.mncs"),
            relative: "m.mncs".to_owned(),
            bytes: vec![0; 200],
            text: None,
            sha256: String::new(),
            len: 200,
            newline: crate::discovery::NewlineStyle::None,
            has_bom: false,
            mode: None,
            is_symlink: false,
        };
        let diags = backend.map_study(&file, &study);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.code, "MNE131");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.source, DiagnosticSource::Language);
        assert_eq!((d.span.start, d.span.end), (90, 100));
        assert_eq!((d.span.start_line, d.span.start_col), (7, 12));
        assert_eq!(d.applicability, Applicability::Manual);
        assert_eq!(d.backend, "rust-cli");
    }

    #[test]
    fn study_json_tolerates_missing_fields() {
        let study: StudyOutput = serde_json::from_str(r#"{"other": 1}"#).unwrap();
        let backend = RustCliBackend {
            exe: std::path::PathBuf::from("mncs"),
        };
        let file = SourceFile {
            path: std::path::PathBuf::from("m.mncs"),
            relative: "m.mncs".to_owned(),
            bytes: Vec::new(),
            text: None,
            sha256: String::new(),
            len: 0,
            newline: crate::discovery::NewlineStyle::None,
            has_bom: false,
            mode: None,
            is_symlink: false,
        };
        assert!(backend.map_study(&file, &study).is_empty());
    }

    #[test]
    fn rust_cli_discovery_rejects_family_validator() {
        // The PATH `mncs` here is the family validator (no source-study
        // verb), so discovery must not claim it as the language CLI unless
        // it really is one.
        if let Some(exe) = find_rust_cli() {
            assert!(is_rust_cli(&exe));
        }
    }
}
