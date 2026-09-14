//! Production MNCS policy runtime.
//!
//! This is the one boundary between Doctor's trusted host mechanisms and its
//! executable MNCS policy. The host owns discovery, byte acquisition,
//! subprocesses, and rendering. This runtime owns loading verified policy
//! artifacts, retaining sessions, validating scalar transport, and exposing
//! policy decisions to command orchestration.
//!
//! The ten policy sources are frozen into one checked-in artifact generated
//! by the upstream compiler's import-resolution path. The shipped crate opens
//! those bytes directly, so production startup does not invoke a compiler and
//! all imported Doctor modules share one artifact/session identity.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::diagnostics::Severity;
use crate::discovery::{DirectoryDecision, DirectoryFacts, FileClass, FileFacts, NewlineStyle};
use crate::fix::StopReason;
use crate::health::{CheckResult, Status};
use crate::report::ExitCode;
use crate::verify::VerificationOutcome;
use crate::version::{LanguageVersion, VersionClass};

/// Exact language revision used by the embedded policy runtime.
pub const MNCS_LANGUAGE_REV: &str = "a0255f8405484481b203117a659b0c1ca0fa7a5e";
/// Policy source profile used by every embedded Doctor module.
pub const POLICY_PROFILE: &str = "0.16";
/// Backend whose value-level execution is currently proven for Doctor.
pub const POLICY_BACKEND: &str = "mncs-research-bytecode";

const POLICY_STEP_BUDGET: u64 = 32_768;
const FAMILY_SOURCE: &str = include_str!("../mncs/doctor_family.mncs");
const FAMILY_ARTIFACT: &[u8] = include_bytes!("../mncs/doctor/family.backend.json");
const FAMILY_ARTIFACT_SHA256: &str =
    "8aad787b1f4d373d5e2c5a64591eed766ebfdab104d9e47ff4681e439f4012e4";

struct ModuleSpec {
    key: &'static str,
    module: &'static str,
    source: &'static str,
}

const MODULES: &[ModuleSpec] = &[
    ModuleSpec {
        key: "version",
        module: "doctor.version.v1",
        source: include_str!("../mncs/doctor/version.mncs"),
    },
    ModuleSpec {
        key: "health",
        module: "doctor.health.v1",
        source: include_str!("../mncs/doctor/health.mncs"),
    },
    ModuleSpec {
        key: "migration",
        module: "doctor.migration.v1",
        source: include_str!("../mncs/doctor/migration.mncs"),
    },
    ModuleSpec {
        key: "edits",
        module: "doctor.edits.v1",
        source: include_str!("../mncs/doctor/edits.mncs"),
    },
    ModuleSpec {
        key: "fix",
        module: "doctor.fix.v1",
        source: include_str!("../mncs/doctor/fix.mncs"),
    },
    ModuleSpec {
        key: "report",
        module: "doctor.report.v1",
        source: include_str!("../mncs/doctor/report.mncs"),
    },
    ModuleSpec {
        key: "verify",
        module: "doctor.verify.v1",
        source: include_str!("../mncs/doctor/verify.mncs"),
    },
    ModuleSpec {
        key: "transaction",
        module: "doctor.transaction.v1",
        source: include_str!("../mncs/doctor/transaction.mncs"),
    },
    ModuleSpec {
        key: "discovery",
        module: "doctor.discovery.v1",
        source: include_str!("../mncs/doctor/discovery.mncs"),
    },
    ModuleSpec {
        key: "scanner",
        module: "doctor.scanner.v1",
        source: include_str!("../mncs/doctor/scanner.mncs"),
    },
];

/// Provenance for one embedded policy module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyModuleProvenance {
    pub source_sha256: String,
    pub artifact_identity: String,
    pub artifact_sha256: String,
}

/// Machine-readable provenance for the policy runtime that produced a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyProvenance {
    pub engine: String,
    pub profile: String,
    pub backend: String,
    pub language_revision: String,
    pub family_source_sha256: String,
    pub modules: BTreeMap<String, PolicyModuleProvenance>,
    #[serde(default)]
    pub entrypoints: Vec<String>,
}

/// A stable, fail-closed runtime error. `code` is suitable for machine
/// matching; `message` retains enough detail for a human failure report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
}

/// Result of the MNCS transaction target policy. Filesystem observation and
/// byte hashing remain host mechanisms; this enum is the policy verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionTargetVerdict {
    Allow,
    Identical,
    Symlink,
    Stale,
    NonFile,
}

/// File-level evidence returned by the bounded MNCS scanner. The bytes and
/// their chunk boundaries remain host-owned; these fields are the policy
/// result produced after the scanner session has consumed every window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanSummary {
    pub has_bom: bool,
    pub newline: NewlineStyle,
    pub bare_lf: u64,
    pub crlf: u64,
    pub bare_cr: u64,
    pub chunks: usize,
}

impl RuntimeError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// One retained session for the complete Doctor policy family.
pub struct DoctorMncsRuntime {
    session: mncs_embed::Session,
    provenance: PolicyProvenance,
    entrypoints: Mutex<Vec<String>>,
}

impl DoctorMncsRuntime {
    /// Open the verified, frozen Doctor policy family artifact.
    ///
    /// No partially initialized runtime is returned. An invalid artifact,
    /// unsupported backend, invalid artifact identity, or failed session
    /// open aborts the whole runtime so callers cannot mix MNCS and fallback
    /// policy accidentally.
    pub fn new() -> Result<Self, RuntimeError> {
        let raw_artifact_sha256 = sha256_hex(FAMILY_ARTIFACT);
        if raw_artifact_sha256 != FAMILY_ARTIFACT_SHA256 {
            return Err(RuntimeError::new(
                "mncs_runtime_artifact",
                format!(
                    "frozen Doctor family artifact digest mismatch: expected {FAMILY_ARTIFACT_SHA256}, found {raw_artifact_sha256}"
                ),
            ));
        }
        let artifact = mncs_embed::Artifact::from_json(FAMILY_ARTIFACT).map_err(|error| {
            RuntimeError::new(
                "mncs_runtime_artifact",
                format!("frozen Doctor family artifact refused: {error}"),
            )
        })?;
        if artifact.backend_name() != POLICY_BACKEND {
            return Err(RuntimeError::new(
                "mncs_runtime_artifact",
                format!(
                    "frozen Doctor family uses backend {}, expected {POLICY_BACKEND}",
                    artifact.backend_name()
                ),
            ));
        }
        let artifact_identity = artifact.artifact_identity().to_owned();
        let artifact_sha256 = artifact.digest().to_owned();
        let session = mncs_embed::Session::open(artifact).map_err(|error| {
            RuntimeError::new(
                "mncs_runtime_open",
                format!("frozen Doctor family session refused: {error}"),
            )
        })?;
        let mut module_provenance = BTreeMap::new();

        for spec in MODULES {
            module_provenance.insert(
                spec.key.to_owned(),
                PolicyModuleProvenance {
                    source_sha256: sha256_hex(spec.source.as_bytes()),
                    artifact_identity: artifact_identity.clone(),
                    artifact_sha256: artifact_sha256.clone(),
                },
            );
        }

        Ok(Self {
            session,
            provenance: PolicyProvenance {
                engine: "mncs".to_owned(),
                profile: POLICY_PROFILE.to_owned(),
                backend: POLICY_BACKEND.to_owned(),
                language_revision: MNCS_LANGUAGE_REV.to_owned(),
                family_source_sha256: sha256_hex(FAMILY_SOURCE.as_bytes()),
                modules: module_provenance,
                entrypoints: Vec::new(),
            },
            entrypoints: Mutex::new(Vec::new()),
        })
    }

    /// Get the process-wide production runtime. Initialization opens the
    /// frozen family once; all subsequent calls reuse the verified session.
    pub fn production() -> Result<&'static Self, RuntimeError> {
        static RUNTIME: OnceLock<Result<DoctorMncsRuntime, RuntimeError>> = OnceLock::new();
        match RUNTIME.get_or_init(Self::new) {
            Ok(runtime) => Ok(runtime),
            Err(error) => Err(error.clone()),
        }
    }

    /// Snapshot provenance, including entrypoints successfully executed so
    /// far in this process.
    pub fn provenance(&self) -> Result<PolicyProvenance, RuntimeError> {
        let mut provenance = self.provenance.clone();
        provenance.entrypoints = self
            .entrypoints
            .lock()
            .map_err(|_| RuntimeError::new("mncs_runtime_trace", "entrypoint trace poisoned"))?
            .clone();
        Ok(provenance)
    }

    /// Classify one finding-count pair through the health policy session.
    pub fn check_status(
        &self,
        error_count: u64,
        warning_count: u64,
    ) -> Result<Status, RuntimeError> {
        let code = self.call_u64(
            "health",
            "check_status",
            &format!("[{}, {}]", u64_arg(error_count), u64_arg(warning_count)),
        )?;
        status_from_code(code)
    }

    /// Apply MNCS health-status policy to host-collected findings and return
    /// the MNCS overall status. The host still owns finding text and source
    /// collection; status classification and aggregation are live MNCS calls.
    pub fn apply_health_policy(&self, checks: &mut [CheckResult]) -> Result<Status, RuntimeError> {
        if checks.len() > 8 {
            return Err(RuntimeError::new(
                "mncs_value_contract",
                format!(
                    "health policy accepts at most 8 checks, got {}",
                    checks.len()
                ),
            ));
        }
        let mut codes = Vec::with_capacity(checks.len());
        for check in checks {
            let errors = check
                .findings
                .iter()
                .filter(|finding| finding.severity == Severity::Error)
                .count() as u64;
            let warnings = check
                .findings
                .iter()
                .filter(|finding| finding.severity == Severity::Warning)
                .count() as u64;
            let infos = check
                .findings
                .iter()
                .filter(|finding| finding.severity == Severity::Info)
                .count() as u64;
            // The check registry deliberately treats sealed-profile drift as
            // actionable even though its file diagnostics are informational.
            // This is a policy input, not a Rust-computed verdict.
            let promote_info = check.id == "version-drift";
            let code = self.call_u64(
                "health",
                "check_status_with_skip",
                &format!(
                    "[{}, {}, {}, {}, {}]",
                    u64_arg(errors),
                    u64_arg(warnings),
                    u64_arg(infos),
                    bool_arg(promote_info),
                    bool_arg(check.status == Status::Skipped)
                ),
            )?;
            check.status = status_from_code(code)?;
            codes.push(code);
        }
        let overall = self.call_u64(
            "health",
            "overall",
            &format!(
                "[{}, {}]",
                sequence_arg(&codes, 8),
                u64_arg(codes.len() as u64)
            ),
        )?;
        status_from_code(overall)
    }

    /// Apply the MNCS report exit policy to already classified checks.
    pub fn exit_for(
        &self,
        checks: &[CheckResult],
        review_blocked: bool,
        verification: Option<&VerificationOutcome>,
    ) -> Result<ExitCode, RuntimeError> {
        if checks.len() > 8 {
            return Err(RuntimeError::new(
                "mncs_value_contract",
                format!(
                    "report policy accepts at most 8 checks, got {}",
                    checks.len()
                ),
            ));
        }
        let codes: Vec<u64> = checks
            .iter()
            .map(|check| status_code(check.status))
            .collect();
        let worst = self.call_u64(
            "health",
            "overall",
            &format!(
                "[{}, {}]",
                sequence_arg(&codes, 8),
                u64_arg(codes.len() as u64)
            ),
        )?;
        let verified = verification.is_some();
        let verify_passed = verification.is_none_or(|outcome| outcome.passed);
        let code = self.call_u64(
            "report",
            "exit_for",
            &format!(
                "[{}, {}, {}, {}]",
                u64_arg(worst),
                bool_arg(review_blocked),
                bool_arg(verified),
                bool_arg(verify_passed)
            ),
        )?;
        match code {
            0 => Ok(ExitCode::Healthy),
            1 => Ok(ExitCode::Findings),
            2 => Ok(ExitCode::ReviewRequired),
            3 => Ok(ExitCode::VerificationFailed),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("report returned unknown exit code {other}"),
            )),
        }
    }

    /// Run version classification through the production policy session.
    /// Registry data remains host-owned and is passed as the current profile.
    pub fn classify_version(&self, version: LanguageVersion) -> Result<VersionClass, RuntimeError> {
        let code = self.call_i64(
            "version",
            "classify",
            &format!(
                "[{}, {}, {}, {}]",
                i64_arg(version.major as i64),
                i64_arg(version.minor as i64),
                i64_arg(0),
                i64_arg(17)
            ),
        )?;
        match code {
            0 => Ok(VersionClass::Current),
            1 => Ok(VersionClass::Sealed),
            2 => Ok(VersionClass::Unsupported),
            3 => Ok(VersionClass::Unknown),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("version returned unknown classification code {other}"),
            )),
        }
    }

    /// Decide whether one observed target may participate in a transaction.
    /// This is deliberately called during real transaction validation, not
    /// only from parity tests.
    pub fn transaction_target_verdict(
        &self,
        expected_present: bool,
        actual_present: bool,
        is_symlink: bool,
        is_file: bool,
        fingerprint_matches: bool,
        identical: bool,
    ) -> Result<TransactionTargetVerdict, RuntimeError> {
        let code = self.call_u64(
            "transaction",
            "validate_target",
            &format!(
                "[{}, {}, {}, {}, {}, {}]",
                bool_arg(expected_present),
                bool_arg(actual_present),
                bool_arg(is_symlink),
                bool_arg(is_file),
                bool_arg(fingerprint_matches),
                bool_arg(identical)
            ),
        )?;
        match code {
            0 => Ok(TransactionTargetVerdict::Allow),
            1 => Ok(TransactionTargetVerdict::Identical),
            2 => Ok(TransactionTargetVerdict::Symlink),
            3 => Ok(TransactionTargetVerdict::Stale),
            4 => Ok(TransactionTargetVerdict::NonFile),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("transaction returned unknown target verdict {other}"),
            )),
        }
    }

    /// Decide directory traversal policy from host-acquired facts.
    pub fn discovery_directory_decision(
        &self,
        facts: DirectoryFacts,
    ) -> Result<DirectoryDecision, RuntimeError> {
        let code = self.call_u64(
            "discovery",
            "directory_decision",
            &format!(
                "[{}, {}, {}, {}, {}, {}, {}]",
                u64_arg(facts.name_code),
                bool_arg(facts.extra_excluded),
                bool_arg(facts.is_symlink),
                bool_arg(facts.follow_symlink),
                bool_arg(facts.cycle),
                u64_arg(facts.depth),
                u64_arg(facts.max_depth)
            ),
        )?;
        match code {
            0 => Ok(DirectoryDecision::Descend),
            1 => Ok(DirectoryDecision::SkipDepth),
            2 => Ok(DirectoryDecision::SkipExcluded),
            3 => Ok(DirectoryDecision::SkipCycle),
            4 => Ok(DirectoryDecision::SkipSymlink),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("discovery returned unknown directory verdict {other}"),
            )),
        }
    }

    /// Classify a host-observed file name/extension pair.
    pub fn discovery_file_class(&self, facts: FileFacts) -> Result<FileClass, RuntimeError> {
        let code = self.call_u64(
            "discovery",
            "file_class",
            &format!(
                "[{}, {}]",
                u64_arg(facts.name_code),
                u64_arg(facts.extension_code)
            ),
        )?;
        match code {
            0 => Ok(FileClass::Ignore),
            1 => Ok(FileClass::Source),
            2 => Ok(FileClass::ForgeManifest),
            3 => Ok(FileClass::WorkspaceManifest),
            4 => Ok(FileClass::ManifestJson),
            5 => Ok(FileClass::Cargo),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("discovery returned unknown file class {other}"),
            )),
        }
    }

    /// Feed arbitrary source bytes through the bounded MNCS scanner in
    /// 64-byte windows, retaining its fixed scanner state across calls.
    /// This is the production ingress path for BOM/newline facts; callers do
    /// not need to materialize a larger MNCS value than one view.
    pub fn scan_bytes(&self, bytes: &[u8]) -> Result<ScanSummary, RuntimeError> {
        let mut state = [0_u64; 6];
        let mut chunks = 0usize;
        for chunk in bytes.chunks(64) {
            state = self.scan_feed(chunk, state)?;
            chunks += 1;
        }
        state = self.scan_finish(state)?;
        let styles = u64::from(state[2] > 0) + u64::from(state[3] > 0) + u64::from(state[4] > 0);
        let newline = if styles == 0 {
            NewlineStyle::None
        } else if styles > 1 {
            NewlineStyle::Mixed
        } else if state[3] > 0 {
            NewlineStyle::Crlf
        } else if state[2] > 0 {
            NewlineStyle::Lf
        } else {
            // The host newline enum has no native-only style; preserve its
            // historical None result for a file containing only CR bytes.
            NewlineStyle::None
        };
        Ok(ScanSummary {
            has_bom: state[1] == 1,
            newline,
            bare_lf: state[2],
            crlf: state[3],
            bare_cr: state[4],
            chunks,
        })
    }

    fn scan_feed(&self, bytes: &[u8], state: [u64; 6]) -> Result<[u64; 6], RuntimeError> {
        let value = self.call(
            "scanner",
            "feed",
            &format!(
                "[{}, {}]",
                byte_sequence_arg(bytes),
                sequence_arg(&state, 6)
            ),
        )?;
        read_u64_array(&value, 6)
    }

    fn scan_finish(&self, state: [u64; 6]) -> Result<[u64; 6], RuntimeError> {
        let value = self.call(
            "scanner",
            "finish",
            &format!("[{}]", sequence_arg(&state, 6)),
        )?;
        read_u64_array(&value, 6)
    }

    /// Ask MNCS whether a repair loop should stop. `None` means continue;
    /// the concrete stop reason is decoded before it reaches orchestration.
    pub fn fix_stop_rule(
        &self,
        planned_empty: bool,
        fired_before: bool,
        iterations: u32,
        budget: u32,
    ) -> Result<Option<StopReason>, RuntimeError> {
        let code = self.call_u64(
            "fix",
            "stop_rule",
            &format!(
                "[{}, {}, {}, {}]",
                bool_arg(planned_empty),
                bool_arg(fired_before),
                u64_arg(iterations as u64),
                u64_arg(budget as u64)
            ),
        )?;
        match code {
            0 => Ok(None),
            1 => Ok(Some(StopReason::Fixpoint)),
            2 => Ok(Some(StopReason::BudgetExhausted)),
            3 => Ok(Some(StopReason::Oscillation)),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("fix returned unknown stop code {other}"),
            )),
        }
    }

    /// Detect a previously fired provider over the bounded state window.
    pub fn fix_seen_before(&self, fired: &[u64], id: u64) -> Result<bool, RuntimeError> {
        if fired.len() > 8 {
            return Err(RuntimeError::new(
                "mncs_value_contract",
                format!(
                    "fix state accepts at most 8 fired providers, got {}",
                    fired.len()
                ),
            ));
        }
        let code = self.call_u64(
            "fix",
            "seen_before",
            &format!(
                "[{}, {}, {}]",
                sequence_arg(fired, 8),
                u64_arg(fired.len() as u64),
                u64_arg(id)
            ),
        )?;
        match code {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("fix returned unknown seen-provider code {other}"),
            )),
        }
    }

    /// Decide whether two edit spans conflict through the live edit policy.
    pub fn edits_pair_conflict(
        &self,
        a_start: u64,
        a_end: u64,
        b_start: u64,
        b_end: u64,
    ) -> Result<bool, RuntimeError> {
        let code = self.call_u64(
            "edits",
            "pair_conflict",
            &format!(
                "[{}, {}, {}, {}]",
                u64_arg(a_start),
                u64_arg(a_end),
                u64_arg(b_start),
                u64_arg(b_end)
            ),
        )?;
        match code {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("edits returned unknown conflict code {other}"),
            )),
        }
    }

    /// Validate the structural verdict of a host-loaded migration plan.
    /// Transition knowledge and provenance remain host data; MNCS decides
    /// whether the resulting path is actionable or blocked by an unknown
    /// edge.
    pub fn migration_plan_verdict(
        &self,
        from: LanguageVersion,
        to: LanguageVersion,
        kinds: &[i64],
    ) -> Result<i64, RuntimeError> {
        if kinds.len() > 16 {
            return Err(RuntimeError::new(
                "mncs_value_contract",
                format!(
                    "migration policy accepts at most 16 edges, got {}",
                    kinds.len()
                ),
            ));
        }
        self.call_i64(
            "migration",
            "plan_verdict",
            &format!(
                "[{}, {}, {}, {}]",
                i64_arg(version_coordinate(from)),
                i64_arg(version_coordinate(to)),
                sequence_i64_arg(kinds, 16),
                u64_arg(kinds.len() as u64)
            ),
        )
    }

    /// Compose verification evidence through the live verification policy.
    pub fn verify_compose(
        &self,
        errors_before: usize,
        errors_after: usize,
        idempotent: bool,
        external_pass: bool,
    ) -> Result<bool, RuntimeError> {
        let delta = self.call_u64(
            "verify",
            "delta_ok",
            &format!(
                "[{}, {}]",
                u64_arg(errors_before as u64),
                u64_arg(errors_after as u64)
            ),
        )?;
        let verdict = self.call_u64(
            "verify",
            "compose",
            &format!(
                "[{}, {}, {}]",
                u64_arg(delta),
                u64_arg(u64::from(idempotent)),
                u64_arg(u64::from(external_pass))
            ),
        )?;
        match verdict {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(RuntimeError::new(
                "mncs_value_contract",
                format!("verify returned unknown composition code {other}"),
            )),
        }
    }

    fn call_u64(&self, key: &str, function: &str, args: &str) -> Result<u64, RuntimeError> {
        let value = self.call(key, function, args)?;
        read_u64(&value)
    }

    fn call_i64(&self, key: &str, function: &str, args: &str) -> Result<i64, RuntimeError> {
        let value = self.call(key, function, args)?;
        read_i64(&value)
    }

    fn call(
        &self,
        key: &str,
        function: &str,
        args: &str,
    ) -> Result<serde_json::Value, RuntimeError> {
        let spec = MODULES.iter().find(|spec| spec.key == key).ok_or_else(|| {
            RuntimeError::new("mncs_runtime_config", format!("unknown module {key}"))
        })?;
        let output = self
            .session
            .call_json(
                spec.module,
                function,
                args,
                &mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
            )
            .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        if output.status != "returned" {
            return Err(RuntimeError::new(
                "mncs_policy_call",
                format!(
                    "{}::{} returned {}; artifact={} reason={}",
                    spec.module,
                    function,
                    output.status,
                    output.artifact_sha256,
                    output.failure_reason.as_deref().unwrap_or("unspecified")
                ),
            ));
        }
        if output.returned.len() != 1 {
            return Err(RuntimeError::new(
                "mncs_value_contract",
                format!(
                    "{}::{} returned {} values; expected exactly one",
                    spec.module,
                    function,
                    output.returned.len()
                ),
            ));
        }
        let value = serde_json::to_value(&output.returned[0]).map_err(|error| {
            RuntimeError::new(
                "mncs_value_contract",
                format!("return serialization failed: {error}"),
            )
        })?;
        let entrypoint = format!("{}::{}", spec.module, function);
        let mut trace = self
            .entrypoints
            .lock()
            .map_err(|_| RuntimeError::new("mncs_runtime_trace", "entrypoint trace poisoned"))?;
        if !trace.iter().any(|seen| seen == &entrypoint) {
            trace.push(entrypoint);
        }
        Ok(value)
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn version_coordinate(version: LanguageVersion) -> i64 {
    i64::from(version.major) * 1_000 + i64::from(version.minor)
}

fn i64_arg(value: i64) -> String {
    format!("{{\"integer\":{{\"value\":{value},\"type\":{{\"bits\":64,\"signed\":true}}}}}}")
}

fn u64_arg(value: u64) -> String {
    format!("{{\"integer\":{{\"value\":{value},\"type\":{{\"bits\":64,\"signed\":false}}}}}}")
}

fn bool_arg(value: bool) -> String {
    format!("{{\"boolean\":{{\"value\":{value}}}}}")
}

fn sequence_arg(values: &[u64], width: usize) -> String {
    let mut items: Vec<String> = values.iter().copied().map(u64_arg).collect();
    items.resize_with(width, || u64_arg(0));
    format!("{{\"sequence\":{{\"values\":[{}]}}}}", items.join(","))
}

fn byte_sequence_arg(values: &[u8]) -> String {
    let items = values
        .iter()
        .map(|value| format!("{{\"byte\":{{\"value\":{value}}}}}"))
        .collect::<Vec<_>>();
    format!("{{\"sequence\":{{\"values\":[{}]}}}}", items.join(","))
}

fn sequence_i64_arg(values: &[i64], width: usize) -> String {
    let mut items: Vec<String> = values.iter().copied().map(i64_arg).collect();
    items.resize_with(width, || i64_arg(0));
    format!("{{\"sequence\":{{\"values\":[{}]}}}}", items.join(","))
}

fn read_integer(
    value: &serde_json::Value,
    signed: bool,
) -> Result<&serde_json::Value, RuntimeError> {
    let integer = value
        .get("integer")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| RuntimeError::new("mncs_value_contract", "expected integer return"))?;
    let type_info = integer
        .get("type")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| RuntimeError::new("mncs_value_contract", "integer return has no type"))?;
    let bits = type_info.get("bits").and_then(serde_json::Value::as_u64);
    let actual_signed = type_info.get("signed").and_then(serde_json::Value::as_bool);
    if bits != Some(64) || actual_signed != Some(signed) {
        return Err(RuntimeError::new(
            "mncs_value_contract",
            format!("expected i64/u64 return, got bits={bits:?} signed={actual_signed:?}"),
        ));
    }
    integer
        .get("value")
        .ok_or_else(|| RuntimeError::new("mncs_value_contract", "integer return has no value"))
}

fn read_u64(value: &serde_json::Value) -> Result<u64, RuntimeError> {
    read_integer(value, false)?
        .as_u64()
        .ok_or_else(|| RuntimeError::new("mncs_value_contract", "u64 return value is not unsigned"))
}

fn read_i64(value: &serde_json::Value) -> Result<i64, RuntimeError> {
    read_integer(value, true)?
        .as_i64()
        .ok_or_else(|| RuntimeError::new("mncs_value_contract", "i64 return value is not signed"))
}

fn read_u64_array(
    value: &serde_json::Value,
    expected_len: usize,
) -> Result<[u64; 6], RuntimeError> {
    if expected_len != 6 {
        return Err(RuntimeError::new(
            "mncs_runtime_config",
            format!("scanner array reader expected width 6, got {expected_len}"),
        ));
    }
    let values = value
        .get("sequence")
        .and_then(serde_json::Value::as_object)
        .and_then(|sequence| sequence.get("values"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| RuntimeError::new("mncs_value_contract", "expected u64 sequence return"))?;
    if values.len() != expected_len {
        return Err(RuntimeError::new(
            "mncs_value_contract",
            format!(
                "expected sequence width {expected_len}, returned {}",
                values.len()
            ),
        ));
    }
    let mut output = [0_u64; 6];
    for (index, element) in values.iter().enumerate() {
        output[index] = read_u64(element)?;
    }
    Ok(output)
}

fn status_code(status: Status) -> u64 {
    match status {
        Status::Pass => 0,
        Status::Warning => 1,
        Status::Fail => 2,
        Status::Skipped => 3,
    }
}

fn status_from_code(code: u64) -> Result<Status, RuntimeError> {
    match code {
        0 => Ok(Status::Pass),
        1 => Ok(Status::Warning),
        2 => Ok(Status::Fail),
        3 => Ok(Status::Skipped),
        other => Err(RuntimeError::new(
            "mncs_value_contract",
            format!("health returned unknown status code {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_loads_all_modules_and_records_provenance() {
        let runtime = DoctorMncsRuntime::new().expect("production policy runtime");
        let provenance = runtime.provenance().expect("provenance");
        assert_eq!(provenance.engine, "mncs");
        assert_eq!(provenance.profile, POLICY_PROFILE);
        assert_eq!(provenance.backend, POLICY_BACKEND);
        assert_eq!(provenance.language_revision, MNCS_LANGUAGE_REV);
        assert_eq!(
            provenance.family_source_sha256,
            sha256_hex(FAMILY_SOURCE.as_bytes())
        );
        assert_eq!(provenance.modules.len(), MODULES.len());
        assert_eq!(
            provenance
                .modules
                .values()
                .map(|module| module.artifact_identity.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1
        );
        assert!(provenance
            .modules
            .values()
            .all(|module| !module.source_sha256.is_empty()
                && !module.artifact_identity.is_empty()
                && !module.artifact_sha256.is_empty()));
    }

    #[test]
    fn production_policy_calls_are_session_reused_and_typed() {
        let runtime = DoctorMncsRuntime::new().expect("production policy runtime");
        assert_eq!(runtime.check_status(0, 0).unwrap(), Status::Pass);
        assert_eq!(runtime.check_status(0, 2).unwrap(), Status::Warning);
        assert_eq!(runtime.check_status(1, 0).unwrap(), Status::Fail);
        assert_eq!(
            runtime
                .classify_version(LanguageVersion::new(0, 17))
                .unwrap(),
            VersionClass::Current
        );
        assert!(!runtime.fix_seen_before(&[], 17).unwrap());
        assert!(runtime.fix_seen_before(&[17], 17).unwrap());
        let provenance = runtime.provenance().unwrap();
        assert!(provenance
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint == "doctor.health.v1::check_status"));
        assert!(provenance
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint == "doctor.version.v1::classify"));
        assert!(provenance
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint == "doctor.fix.v1::seen_before"));
    }

    #[test]
    fn chunked_scanner_carries_bom_and_newline_state_across_windows() {
        let runtime = DoctorMncsRuntime::new().expect("production policy runtime");
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend(std::iter::repeat_n(b'x', 60));
        bytes.extend_from_slice(b"\r\nline\nlast\r");
        let summary = runtime.scan_bytes(&bytes).expect("scan");
        assert!(summary.has_bom);
        assert_eq!(summary.newline, NewlineStyle::Mixed);
        assert_eq!(summary.crlf, 1);
        assert_eq!(summary.bare_lf, 1);
        assert_eq!(summary.bare_cr, 1);
        assert_eq!(summary.chunks, 2);
        let provenance = runtime.provenance().unwrap();
        assert!(provenance
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint == "doctor.scanner.v1::feed"));
        assert!(provenance
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint == "doctor.scanner.v1::finish"));
    }
}
