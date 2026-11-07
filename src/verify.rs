//! Post-repair verification: re-diagnose, idempotence, external checks.
//!
//! After any mutation doctor re-runs what it can locally (re-scan,
//! re-diagnose, migration idempotence) and optionally submits project
//! verification commands to the native bounded process effect. Verification
//! never mutates.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{LanguageBackend, Severity};
use crate::discovery::SourceFile;
use crate::mncs_runtime::{doctor_version, DoctorMncsRuntime};

const PROCESS_ARGUMENT_LIMIT: usize = 256;
const PROCESS_ARGUMENT_BYTES: usize = 1024;
const PROCESS_PROGRAM_BYTES: usize = 1024;
const PROCESS_DIRECTORY_BYTES: usize = 1024;
const PROCESS_ENVIRONMENT_ENTRIES: usize = 256;
const PROCESS_ENVIRONMENT_KEY_BYTES: usize = 64;
const PROCESS_ENVIRONMENT_VALUE_BYTES: usize = 4096;
const PROCESS_ENVIRONMENT_TOTAL_BYTES: usize = 64 * 1024;
const PROCESS_OUTPUT_WINDOW_BYTES: usize = 1024;

/// One external verification command outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCheck {
    pub command: String,
    #[serde(default)]
    pub verdict: ExternalCheckVerdict,
    pub success: bool,
    pub status_code: Option<i32>,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub stdout_tail: String,
    #[serde(default)]
    pub stderr_tail: String,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
}

/// Outcome classification produced by Doctor's native process policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCheckVerdict {
    Pass,
    Fail,
    TimedOut,
    Unknown,
}

impl Default for ExternalCheckVerdict {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Run an external verification command through Doctor's native process
/// policy, retaining only the bounded output windows admitted by Stdlib.
pub fn run_external(argv: &[String], cwd: &Path) -> ExternalCheck {
    let command = argv.join(" ");
    if argv.is_empty() {
        return failed_external_check(command, "empty command");
    }

    let input = match process_input(argv, cwd) {
        Ok(input) => input,
        Err(error) => return failed_external_check(command, &error),
    };
    let runtime = match DoctorMncsRuntime::production() {
        Ok(runtime) => runtime,
        Err(error) => return unknown_external_check(command, &error.to_string()),
    };
    let outcome = match runtime.execute_verification_process(input) {
        Ok(outcome) => outcome,
        Err(error) => return unknown_external_check(command, &error.to_string()),
    };
    let status_code = outcome
        .has_exit_code
        .then(|| i32::try_from(outcome.exit_code).ok())
        .flatten();
    let verdict = match outcome.verdict {
        doctor_version::ProcessVerdict::Pass => ExternalCheckVerdict::Pass,
        doctor_version::ProcessVerdict::Fail => ExternalCheckVerdict::Fail,
        doctor_version::ProcessVerdict::TimedOut => ExternalCheckVerdict::TimedOut,
        doctor_version::ProcessVerdict::Unknown => ExternalCheckVerdict::Unknown,
    };
    let stdout = String::from_utf8_lossy(&outcome.stdout);
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    let mut stdout_tail = tail(&stdout, 20);
    let mut stderr_tail = tail(&stderr, 20);
    if outcome.stdout_truncated {
        stdout_tail = truncated_window_note(stdout_tail);
    }
    if outcome.stderr_truncated {
        stderr_tail = truncated_window_note(stderr_tail);
    }
    ExternalCheck {
        command,
        success: verdict == ExternalCheckVerdict::Pass,
        verdict,
        status_code,
        timed_out: outcome.timed_out,
        duration_ms: outcome.duration_ms,
        stdout_tail,
        stderr_tail,
        stdout_truncated: outcome.stdout_truncated,
        stderr_truncated: outcome.stderr_truncated,
    }
}

fn failed_external_check(command: String, error: &str) -> ExternalCheck {
    ExternalCheck {
        command,
        verdict: ExternalCheckVerdict::Fail,
        success: false,
        status_code: None,
        timed_out: false,
        duration_ms: 0,
        stdout_tail: String::new(),
        stderr_tail: error.to_owned(),
        stdout_truncated: false,
        stderr_truncated: false,
    }
}

fn unknown_external_check(command: String, error: &str) -> ExternalCheck {
    ExternalCheck {
        command,
        verdict: ExternalCheckVerdict::Unknown,
        success: false,
        status_code: None,
        timed_out: false,
        duration_ms: 0,
        stdout_tail: String::new(),
        stderr_tail: error.to_owned(),
        stdout_truncated: false,
        stderr_truncated: false,
    }
}

fn process_input(argv: &[String], cwd: &Path) -> Result<doctor_version::ProcessInput, String> {
    let requested_program = argv.first().ok_or_else(|| "empty command".to_owned())?;
    let program = resolve_executable(requested_program, cwd)?;
    let program = program.as_bytes();
    if program.is_empty() || program.len() > PROCESS_PROGRAM_BYTES {
        return Err(format!(
            "process executable must contain 1 to {PROCESS_PROGRAM_BYTES} UTF-8 bytes"
        ));
    }
    let arguments = argv.get(1..).unwrap_or_default();
    if arguments.len() > PROCESS_ARGUMENT_LIMIT {
        return Err(format!(
            "verification command has {} arguments after its executable; limit is {PROCESS_ARGUMENT_LIMIT}",
            arguments.len()
        ));
    }
    for argument in arguments {
        if argument.len() > PROCESS_ARGUMENT_BYTES {
            return Err(format!(
                "verification argument exceeds the {PROCESS_ARGUMENT_BYTES}-byte MNCS process limit"
            ));
        }
    }
    let directory = cwd
        .to_str()
        .ok_or_else(|| "verification working directory is not valid UTF-8".to_owned())?
        .as_bytes();
    if directory.len() > PROCESS_DIRECTORY_BYTES {
        return Err(format!(
            "verification working directory exceeds the {PROCESS_DIRECTORY_BYTES}-byte MNCS process limit"
        ));
    }

    let environment = std::env::vars_os()
        .map(|(key, value)| process_environment_entry(key, value))
        .collect::<Result<Vec<_>, _>>()?;
    if environment.len() > PROCESS_ENVIRONMENT_ENTRIES {
        return Err(format!(
            "process environment has {} entries; limit is {PROCESS_ENVIRONMENT_ENTRIES}",
            environment.len()
        ));
    }
    let total_environment_bytes = environment
        .iter()
        .map(|entry| entry.key.len() + entry.value.len())
        .sum::<usize>();
    if total_environment_bytes > PROCESS_ENVIRONMENT_TOTAL_BYTES {
        return Err(format!(
            "process environment exceeds the {PROCESS_ENVIRONMENT_TOTAL_BYTES}-byte MNCS process limit"
        ));
    }

    Ok(doctor_version::ProcessInput {
        program: program.to_vec(),
        arguments: arguments
            .iter()
            .map(|argument| argument.as_bytes().to_vec())
            .collect(),
        argument_count: arguments.len() as u64,
        current_dir: directory.to_vec(),
        environment_count: environment.len() as u64,
        environment,
    })
}

fn resolve_executable(program: &str, cwd: &Path) -> Result<String, String> {
    let requested = Path::new(program);
    if requested.is_absolute() || program.contains(std::path::MAIN_SEPARATOR) {
        let path = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            cwd.join(requested)
        };
        return executable_path(path);
    }

    let search_path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&search_path) {
        let directory = if directory.is_absolute() {
            directory
        } else {
            cwd.join(directory)
        };
        let candidate = directory.join(program);
        if is_executable_file(&candidate) {
            return executable_path(candidate);
        }
    }
    Err(format!(
        "verification executable {program:?} was not found on PATH"
    ))
}

fn executable_path(path: std::path::PathBuf) -> Result<String, String> {
    if !is_executable_file(&path) {
        return Err(format!(
            "verification executable {} is not an executable file",
            path.display()
        ));
    }
    let path = path.canonicalize().unwrap_or(path);
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "verification executable path is not valid UTF-8".to_owned())
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn process_environment_entry(
    key: OsString,
    value: OsString,
) -> Result<doctor_version::EnvironmentEntry, String> {
    let key = key
        .into_string()
        .map_err(|_| "process environment contains a non-UTF-8 key".to_owned())?;
    let value = value
        .into_string()
        .map_err(|_| "process environment contains a non-UTF-8 value".to_owned())?;
    if key.is_empty() || key.as_bytes().len() > PROCESS_ENVIRONMENT_KEY_BYTES {
        return Err(format!(
            "process environment key must contain 1 to {PROCESS_ENVIRONMENT_KEY_BYTES} UTF-8 bytes"
        ));
    }
    if value.as_bytes().len() > PROCESS_ENVIRONMENT_VALUE_BYTES {
        return Err(format!(
            "process environment value exceeds the {PROCESS_ENVIRONMENT_VALUE_BYTES}-byte MNCS process limit"
        ));
    }
    Ok(doctor_version::EnvironmentEntry {
        key: key.into_bytes(),
        value: value.into_bytes(),
    })
}

fn truncated_window_note(output: String) -> String {
    let note = format!(
        "[MNCS process output window is truncated to {PROCESS_OUTPUT_WINDOW_BYTES} bytes]\n"
    );
    if output.is_empty() {
        note
    } else {
        format!("{note}{output}")
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
    verify_after_with_diagnostics(files.len(), before, &after, idempotent, external)
}

/// Verify using diagnostics that were already produced by a selected policy
/// backend. This keeps production MNCS runs from re-diagnosing through the
/// Rust scanner merely because the verification envelope is host-owned.
pub fn verify_after_with_diagnostics(
    files_rechecked: usize,
    before: &BTreeMap<String, Vec<crate::diagnostics::Diagnostic>>,
    after: &BTreeMap<String, Vec<crate::diagnostics::Diagnostic>>,
    idempotent: Option<bool>,
    external: Vec<ExternalCheck>,
) -> VerificationOutcome {
    let errors_before = count_errors(before);
    let errors_after = count_errors(after);
    let warnings_after = count_warnings(after);
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
        files_rechecked,
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
            native_scan: None,
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
        let check = run_external(
            &["printf".to_owned(), "%s".to_owned(), "hello".to_owned()],
            &std::env::temp_dir(),
        );
        assert!(matches!(
            check.verdict,
            ExternalCheckVerdict::Pass | ExternalCheckVerdict::Unknown
        ));
        assert_eq!(
            check.success,
            check.verdict == ExternalCheckVerdict::Pass,
            "success must agree with the native verdict"
        );
        assert_eq!(check.status_code, Some(0), "process result: {check:?}");
        assert_eq!(check.stdout_tail, "hello");
        assert!(!check.timed_out);
        assert!(check.duration_ms > 0);

        let check = run_external(
            &["sleep".to_owned(), "0.1".to_owned()],
            &std::env::temp_dir(),
        );
        assert_eq!(
            check.verdict,
            ExternalCheckVerdict::Pass,
            "MNCS process returned {check:?}"
        );
        assert!(check.success);
        assert_eq!(check.status_code, Some(0));
        assert!(!check.timed_out);

        let check = run_external(&["false".to_owned()], &std::env::temp_dir());
        assert_eq!(check.verdict, ExternalCheckVerdict::Fail);
        assert!(!check.success);
        assert_eq!(check.status_code, Some(1));

        let check = run_external(
            &["printf".to_owned(), "%1500s".to_owned(), "x".to_owned()],
            &std::env::temp_dir(),
        );
        assert!(matches!(
            check.verdict,
            ExternalCheckVerdict::Pass | ExternalCheckVerdict::Unknown
        ));
        assert_eq!(check.success, check.verdict == ExternalCheckVerdict::Pass);
        assert!(check.stdout_truncated);
        assert!(check.stdout_tail.starts_with("[MNCS process output window"));

        let check = run_external(
            &["mncs-doctor-no-such-binary-xyz".to_owned()],
            &std::env::temp_dir(),
        );
        assert!(!check.success);
        assert_eq!(check.verdict, ExternalCheckVerdict::Fail);
        assert!(check.status_code.is_none());
        assert!(!check.stderr_tail.is_empty());
    }

    #[test]
    fn process_adapter_keeps_argv_explicit_and_enforces_native_bounds() {
        let input = process_input(
            &[
                "printf".to_owned(),
                "%s".to_owned(),
                "one argument with spaces".to_owned(),
            ],
            &std::env::temp_dir(),
        )
        .expect("bounded request");
        assert!(Path::new(std::str::from_utf8(&input.program).unwrap()).is_absolute());
        assert_eq!(
            input.arguments,
            [b"%s".to_vec(), b"one argument with spaces".to_vec()]
        );
        assert_eq!(input.argument_count, 2);

        let oversized = "x".repeat(PROCESS_ARGUMENT_BYTES + 1);
        let error = process_input(&["printf".to_owned(), oversized], &std::env::temp_dir())
            .expect_err("argument above the MNCS ABI bound must be rejected");
        assert!(error.contains("MNCS process limit"));
    }
}
