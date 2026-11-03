//! Production MNCS policy runtime.
//!
//! This is the one boundary between Doctor's trusted host mechanisms and its
//! executable MNCS policy. The host owns discovery, byte acquisition,
//! subprocesses, and rendering. This runtime owns loading verified policy
//! artifacts, retaining sessions, validating scalar transport, and exposing
//! policy decisions to command orchestration.
//!
//! The eleven policy sources are frozen into one checked-in artifact generated
//! by the upstream compiler's import-resolution path. The shipped crate opens
//! those bytes directly, so production startup does not invoke a compiler and
//! all imported Doctor modules share one artifact/session identity.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::diagnostics::{DelimiterScanResult, DelimiterScanStatus, Severity};
use crate::discovery::{DirectoryDecision, DirectoryFacts, FileClass, FileFacts, NewlineStyle};
use crate::fix::Applicability;
use crate::fix::StopReason;
use crate::health::{CheckResult, Status};
use crate::migration::{MigrationVerdict, TransitionKind};
use crate::remediate::RemediationClass;
use crate::report::ExitCode;
use crate::verify::VerificationOutcome;
use crate::version::{LanguageVersion, VersionClass};

#[allow(non_camel_case_types)]
#[path = "generated/doctor_version.rs"]
pub(crate) mod doctor_version;

/// Exact language revision used by the embedded policy runtime.
pub const MNCS_LANGUAGE_REV: &str = "2317e629a768b6cac11fbd6e61dc16a5805873e0";
/// Profile declared by the Doctor family root. The remediation adapter imports
/// the Commons policy at its independently declared 0.18 profile.
pub const POLICY_PROFILE: &str = "0.16";
/// Backend whose value-level execution is currently proven for Doctor.
pub const POLICY_BACKEND: &str = "mncs-research-bytecode";

const POLICY_STEP_BUDGET: u64 = 32_768;
const DELIMITER_STATE_WIDTH: usize = 1_024;
const DELIMITER_WINDOW_WIDTH: usize = 64;
const FAMILY_SOURCE: &str = include_str!("../mncs/doctor_family.mncs");
const FAMILY_ARTIFACT: &[u8] = include_bytes!("../mncs/doctor/family.backend.json");
const FAMILY_ARTIFACT_SHA256: &str =
    "56873d51bdaddf0e79c5c65d70e8f5b2d6856d5031e49958943a1935845d9fdc";

struct ModuleSpec {
    key: &'static str,
    source: &'static str,
}

const MODULES: &[ModuleSpec] = &[
    ModuleSpec {
        key: "version",
        source: include_str!("../mncs/doctor/version.mncs"),
    },
    ModuleSpec {
        key: "health",
        source: include_str!("../mncs/doctor/health.mncs"),
    },
    ModuleSpec {
        key: "migration",
        source: include_str!("../mncs/doctor/migration.mncs"),
    },
    ModuleSpec {
        key: "edits",
        source: include_str!("../mncs/doctor/edits.mncs"),
    },
    ModuleSpec {
        key: "fix",
        source: include_str!("../mncs/doctor/fix.mncs"),
    },
    ModuleSpec {
        key: "report",
        source: include_str!("../mncs/doctor/report.mncs"),
    },
    ModuleSpec {
        key: "verify",
        source: include_str!("../mncs/doctor/verify.mncs"),
    },
    ModuleSpec {
        key: "transaction",
        source: include_str!("../mncs/doctor/transaction.mncs"),
    },
    ModuleSpec {
        key: "discovery",
        source: include_str!("../mncs/doctor/discovery.mncs"),
    },
    ModuleSpec {
        key: "scanner",
        source: include_str!("../mncs/doctor/scanner.mncs"),
    },
    ModuleSpec {
        key: "remediate",
        source: include_str!("../mncs/doctor/remediate.mncs"),
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

/// Residual diagnostic policy returned by the shared Commons law.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidualClassification {
    pub escalate: bool,
    pub degraded: bool,
}

/// Convergence-stop policy returned by the shared Commons law.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopClassification {
    pub escalate: bool,
    pub severity: Severity,
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

    /// Classify one repair applicability through the shared Commons policy.
    /// The host projects the closed applicability enum to its stable code;
    /// Commons owns the class decision.
    pub fn classify_repair(
        &self,
        applicability: Applicability,
        granted: bool,
    ) -> Result<RemediationClass, RuntimeError> {
        let code = match applicability {
            Applicability::Safe => 0,
            Applicability::SemanticallyProven => 1,
            Applicability::Review => 2,
            Applicability::Manual => 3,
        };
        self.classify_repair_code(code, granted)
    }

    /// Classify an explicitly projected applicability code. Exposed for
    /// independent tests of the Commons fail-closed behavior on unknowns.
    pub fn classify_repair_code(
        &self,
        code: i64,
        granted: bool,
    ) -> Result<RemediationClass, RuntimeError> {
        let class = doctor_version::classify_repair(
            &self.session,
            code,
            granted,
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.remediate.v1", "classify_repair")?;
        Ok(match class {
            doctor_version::RemediationClass::safe_automatic => RemediationClass::SafeAutomatic,
            doctor_version::RemediationClass::bounded_reconciliation => {
                RemediationClass::BoundedReconciliation
            }
            doctor_version::RemediationClass::escalation => RemediationClass::Escalation,
        })
    }

    /// Classify a convergence stop. `StopReason` is projected to Commons'
    /// stable closed integer vocabulary at this boundary.
    pub fn classify_stop(
        &self,
        stopped: StopReason,
        validated: bool,
    ) -> Result<StopClassification, RuntimeError> {
        let code = match stopped {
            StopReason::Fixpoint => 0,
            StopReason::BudgetExhausted => 1,
            StopReason::Oscillation => 2,
            StopReason::EditConflict => 3,
        };
        self.classify_stop_code(code, validated)
    }

    /// Classify a projected stop code, including unknown codes for
    /// independent fail-closed verification.
    pub fn classify_stop_code(
        &self,
        code: i64,
        validated: bool,
    ) -> Result<StopClassification, RuntimeError> {
        let disposition = doctor_version::classify_stop(
            &self.session,
            code,
            validated,
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.remediate.v1", "classify_stop")?;
        Ok(StopClassification {
            escalate: disposition.escalate,
            severity: host_remediation_severity(disposition.severity),
        })
    }

    /// Classify residual severity through Commons. Unknown severity codes
    /// remain available to independent tests and fail closed in native code.
    pub fn classify_residual_code(
        &self,
        code: i64,
    ) -> Result<ResidualClassification, RuntimeError> {
        let disposition = doctor_version::classify_residual(
            &self.session,
            code,
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.remediate.v1", "classify_residual")?;
        Ok(ResidualClassification {
            escalate: disposition.escalate,
            degraded: disposition.degraded,
        })
    }

    /// Classify a closed Doctor severity value through Commons.
    pub fn classify_residual(
        &self,
        severity: Severity,
    ) -> Result<ResidualClassification, RuntimeError> {
        self.classify_residual_code(severity_code(severity))
    }

    /// Count files with native-classified residual warnings.
    pub fn degraded_files(
        &self,
        diagnostics: &BTreeMap<String, Vec<crate::diagnostics::Diagnostic>>,
    ) -> Result<usize, RuntimeError> {
        let mut degraded = 0;
        for file_diagnostics in diagnostics.values() {
            let mut file_is_degraded = false;
            for diagnostic in file_diagnostics {
                if self
                    .classify_residual_code(severity_code(diagnostic.severity))?
                    .degraded
                {
                    file_is_degraded = true;
                    break;
                }
            }
            degraded += usize::from(file_is_degraded);
        }
        Ok(degraded)
    }

    /// Count blocker escalations from the Commons residual classification.
    pub fn blocker_count(
        &self,
        escalations: &[crate::remediate::Escalation],
    ) -> Result<usize, RuntimeError> {
        let mut blockers = 0;
        for escalation in escalations {
            if self.classify_residual(escalation.severity)?.escalate {
                blockers += 1;
            }
        }
        Ok(blockers)
    }

    /// Classify one finding-count pair through the health policy session.
    pub fn check_status(
        &self,
        error_count: u64,
        warning_count: u64,
    ) -> Result<Status, RuntimeError> {
        let status = doctor_version::check_status(
            &self.session,
            doctor_version::CheckStatusInput {
                error_count,
                warning_count,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.health.v1", "check_status")?;
        Ok(host_status(status))
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
        let mut statuses = Vec::with_capacity(checks.len());
        for check in &mut *checks {
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
            let check_kind = native_health_check_kind(&check.id)?;
            let status = doctor_version::check_status_with_skip(
                &self.session,
                doctor_version::CheckStatusWithSkipInput {
                    check: check_kind,
                    error_count: errors,
                    warning_count: warnings,
                    info_count: infos,
                    skipped: check.status == Status::Skipped,
                },
                mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
            )
            .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
            self.record_entrypoint("doctor.health.v1", "check_status_with_skip")?;
            check.status = host_status(status);
            statuses.push(status);
        }
        let mut packed = statuses;
        packed.resize(8, doctor_version::Status::Pass);
        let overall = doctor_version::overall(
            &self.session,
            doctor_version::OverallInput {
                statuses: packed,
                count: checks.len() as u64,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.health.v1", "overall")?;
        Ok(host_status(overall))
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
        let mut statuses: Vec<doctor_version::Status> = checks
            .iter()
            .map(|check| native_status(check.status))
            .collect();
        statuses.resize(8, doctor_version::Status::Pass);
        let worst = doctor_version::overall(
            &self.session,
            doctor_version::OverallInput {
                statuses,
                count: checks.len() as u64,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.health.v1", "overall")?;
        let verified = verification.is_some();
        let verify_passed = verification.is_none_or(|outcome| outcome.passed);
        let decision = doctor_version::exit_for(
            &self.session,
            doctor_version::ExitInput {
                worst,
                review_blocked,
                verified,
                verify_passed,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.report.v1", "exit_for")?;
        Ok(match decision {
            doctor_version::ExitDecision::Healthy => ExitCode::Healthy,
            doctor_version::ExitDecision::Findings => ExitCode::Findings,
            doctor_version::ExitDecision::ReviewRequired => ExitCode::ReviewRequired,
            doctor_version::ExitDecision::VerificationFailed => ExitCode::VerificationFailed,
        })
    }

    /// Run version classification through the production policy session.
    /// Registry data remains host-owned and is passed as the current profile.
    pub fn classify_version(&self, version: LanguageVersion) -> Result<VersionClass, RuntimeError> {
        let current = crate::version::current_version();
        let class = doctor_version::classify(
            &self.session,
            doctor_version::VersionInput {
                major: version.major as i64,
                minor: version.minor as i64,
                current_major: current.major as i64,
                current_minor: current.minor as i64,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.version.v1", "classify")?;
        match class {
            doctor_version::VersionClass::Current => Ok(VersionClass::Current),
            doctor_version::VersionClass::Sealed => Ok(VersionClass::Sealed),
            doctor_version::VersionClass::Unsupported => Ok(VersionClass::Unsupported),
            doctor_version::VersionClass::Unknown => Ok(VersionClass::Unknown),
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
        let verdict = doctor_version::validate_target(
            &self.session,
            doctor_version::TransactionTargetInput {
                expected_present,
                actual_present,
                is_symlink,
                is_file,
                fingerprint_matches,
                identical,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.transaction.v1", "validate_target")?;
        match verdict {
            doctor_version::TransactionTargetVerdict::Allow => Ok(TransactionTargetVerdict::Allow),
            doctor_version::TransactionTargetVerdict::Identical => {
                Ok(TransactionTargetVerdict::Identical)
            }
            doctor_version::TransactionTargetVerdict::Symlink => {
                Ok(TransactionTargetVerdict::Symlink)
            }
            doctor_version::TransactionTargetVerdict::Stale => Ok(TransactionTargetVerdict::Stale),
            doctor_version::TransactionTargetVerdict::NonFile => {
                Ok(TransactionTargetVerdict::NonFile)
            }
        }
    }

    /// Decide directory traversal policy from host-acquired facts.
    pub fn discovery_directory_decision(
        &self,
        facts: DirectoryFacts,
    ) -> Result<DirectoryDecision, RuntimeError> {
        let decision = doctor_version::directory_decision(
            &self.session,
            doctor_version::DirectoryDecisionInput {
                name: facts.name,
                extra_excluded: facts.extra_excluded,
                is_symlink: facts.is_symlink,
                follow_symlink: facts.follow_symlink,
                cycle: facts.cycle,
                depth: facts.depth,
                max_depth: facts.max_depth,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.discovery.v1", "directory_decision")?;
        match decision {
            doctor_version::DirectoryDecision::Descend => Ok(DirectoryDecision::Descend),
            doctor_version::DirectoryDecision::SkipDepth => Ok(DirectoryDecision::SkipDepth),
            doctor_version::DirectoryDecision::SkipExcluded => Ok(DirectoryDecision::SkipExcluded),
            doctor_version::DirectoryDecision::SkipCycle => Ok(DirectoryDecision::SkipCycle),
            doctor_version::DirectoryDecision::SkipSymlink => Ok(DirectoryDecision::SkipSymlink),
        }
    }

    /// Classify a host-observed file name/extension pair.
    pub fn discovery_file_class(&self, facts: FileFacts) -> Result<FileClass, RuntimeError> {
        let class = doctor_version::file_class(
            &self.session,
            doctor_version::FileClassInput {
                name: facts.name,
                extension: facts.extension,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.discovery.v1", "file_class")?;
        match class {
            doctor_version::FileClass::Ignore => Ok(FileClass::Ignore),
            doctor_version::FileClass::Source => Ok(FileClass::Source),
            doctor_version::FileClass::ForgeManifest => Ok(FileClass::ForgeManifest),
            doctor_version::FileClass::WorkspaceManifest => Ok(FileClass::WorkspaceManifest),
            doctor_version::FileClass::ManifestJson => Ok(FileClass::ManifestJson),
            doctor_version::FileClass::Cargo => Ok(FileClass::Cargo),
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

    /// Scan delimiter balance through the MNCS streaming state machine.
    /// The host supplies bounded byte windows and carries the fixed state
    /// between calls; lexical mode, delimiter matching, mismatch detection,
    /// and the final unclosed-opener decision stay in MNCS.
    pub fn scan_delimiters(&self, bytes: &[u8]) -> Result<DelimiterScanResult, RuntimeError> {
        let mut state = vec![0_u64; DELIMITER_STATE_WIDTH];
        for chunk in bytes.chunks(DELIMITER_WINDOW_WIDTH) {
            state = doctor_version::delimiter_feed(
                &self.session,
                chunk.to_vec(),
                state,
                mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
            )
            .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
            self.record_entrypoint("doctor.scanner.v1", "delimiter_feed")?;
            if state.len() != DELIMITER_STATE_WIDTH {
                return Err(RuntimeError::new(
                    "mncs_value_contract",
                    format!(
                        "expected delimiter scanner state width {DELIMITER_STATE_WIDTH}, returned {}",
                        state.len()
                    ),
                ));
            }
            if state[5] != 0 {
                break;
            }
        }

        let result = doctor_version::delimiter_finish(
            &self.session,
            state,
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.scanner.v1", "delimiter_finish")?;
        let offset = usize::try_from(result.offset).map_err(|_| {
            RuntimeError::new(
                "mncs_value_contract",
                "delimiter scanner offset does not fit the host span domain",
            )
        })?;
        let status = match result.status {
            doctor_version::DelimiterStatus::Balanced => DelimiterScanStatus::Balanced,
            doctor_version::DelimiterStatus::Unbalanced => DelimiterScanStatus::Unbalanced,
            doctor_version::DelimiterStatus::CapacityExceeded => {
                DelimiterScanStatus::CapacityExceeded
            }
        };
        Ok(DelimiterScanResult { status, offset })
    }

    fn scan_feed(&self, bytes: &[u8], state: [u64; 6]) -> Result<[u64; 6], RuntimeError> {
        let values = doctor_version::feed(
            &self.session,
            bytes.to_vec(),
            state.to_vec(),
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.scanner.v1", "feed")?;
        state_array(values)
    }

    fn scan_finish(&self, state: [u64; 6]) -> Result<[u64; 6], RuntimeError> {
        let values = doctor_version::finish(
            &self.session,
            state.to_vec(),
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.scanner.v1", "finish")?;
        state_array(values)
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
        let decision = doctor_version::stop_rule(
            &self.session,
            doctor_version::StopRuleInput {
                planned_empty,
                fired_before,
                iterations: u64::from(iterations),
                budget: u64::from(budget),
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.fix.v1", "stop_rule")?;
        match decision {
            doctor_version::StopDecision::Continue => Ok(None),
            doctor_version::StopDecision::Fixpoint => Ok(Some(StopReason::Fixpoint)),
            doctor_version::StopDecision::BudgetExhausted => Ok(Some(StopReason::BudgetExhausted)),
            doctor_version::StopDecision::Oscillation => Ok(Some(StopReason::Oscillation)),
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
        let mut packed = fired.to_vec();
        packed.resize(8, 0);
        let verdict = doctor_version::seen_before(
            &self.session,
            doctor_version::SeenBeforeInput {
                fired: packed,
                count: fired.len() as u64,
                id,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.fix.v1", "seen_before")?;
        Ok(matches!(verdict, doctor_version::SeenVerdict::Seen))
    }

    /// Decide whether two edit spans conflict through the live edit policy.
    pub fn edits_pair_conflict(
        &self,
        a_start: u64,
        a_end: u64,
        b_start: u64,
        b_end: u64,
    ) -> Result<bool, RuntimeError> {
        let verdict = doctor_version::pair_conflict(
            &self.session,
            doctor_version::PairConflictInput {
                a_start,
                a_end,
                b_start,
                b_end,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.edits.v1", "pair_conflict")?;
        Ok(matches!(verdict, doctor_version::ConflictVerdict::Conflict))
    }

    /// Validate the structural verdict of a host-loaded migration plan.
    /// Transition knowledge and provenance remain host data; MNCS decides
    /// whether the resulting path is actionable or blocked by an unknown
    /// edge.
    pub fn migration_plan_verdict(
        &self,
        from: LanguageVersion,
        to: LanguageVersion,
        kinds: &[TransitionKind],
    ) -> Result<MigrationVerdict, RuntimeError> {
        if kinds.len() > 16 {
            return Err(RuntimeError::new(
                "mncs_value_contract",
                format!(
                    "migration policy accepts at most 16 edges, got {}",
                    kinds.len()
                ),
            ));
        }
        let mut packed: Vec<doctor_version::TransitionKind> =
            kinds.iter().copied().map(native_transition_kind).collect();
        packed.resize(16, doctor_version::TransitionKind::Noop);
        let verdict = doctor_version::plan_verdict(
            &self.session,
            doctor_version::PlanInput {
                from_minor: version_coordinate(from),
                to_minor: version_coordinate(to),
                kinds: packed,
                count: kinds.len() as u64,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.migration.v1", "plan_verdict")?;
        Ok(match verdict {
            doctor_version::MigrationVerdict::Planned => MigrationVerdict::Planned,
            doctor_version::MigrationVerdict::Noop => MigrationVerdict::Noop,
            doctor_version::MigrationVerdict::DowngradeRefused => {
                MigrationVerdict::DowngradeRefused
            }
            doctor_version::MigrationVerdict::OfflineRefused => MigrationVerdict::OfflineRefused,
            doctor_version::MigrationVerdict::Blocked => MigrationVerdict::Blocked,
        })
    }

    /// Compose verification evidence through the live verification policy.
    pub fn verify_compose(
        &self,
        errors_before: usize,
        errors_after: usize,
        idempotent: bool,
        external_pass: bool,
    ) -> Result<bool, RuntimeError> {
        let delta = doctor_version::delta_ok(
            &self.session,
            doctor_version::DeltaInput {
                errors_before: errors_before as u64,
                errors_after: errors_after as u64,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.verify.v1", "delta_ok")?;
        let verdict = doctor_version::compose(
            &self.session,
            doctor_version::ComposeInput {
                delta,
                idempotent,
                external_pass,
            },
            mncs_embed::CallOptions::budgeted(POLICY_STEP_BUDGET),
        )
        .map_err(|error| RuntimeError::new("mncs_value_contract", error.to_string()))?;
        self.record_entrypoint("doctor.verify.v1", "compose")?;
        Ok(matches!(verdict, doctor_version::VerificationVerdict::Pass))
    }

    fn record_entrypoint(&self, module: &str, function: &str) -> Result<(), RuntimeError> {
        let entrypoint = format!("{module}::{function}");
        let mut trace = self
            .entrypoints
            .lock()
            .map_err(|_| RuntimeError::new("mncs_runtime_trace", "entrypoint trace poisoned"))?;
        if !trace.iter().any(|seen| seen == &entrypoint) {
            trace.push(entrypoint);
        }
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Policy identity that is computable without opening the policy runtime.
///
/// This names the exact discovery/health policy a full run would execute:
/// the policy coordinates (profile, backend, language revision), the
/// pinned frozen-artifact digest, and the actual embedded family and
/// per-module source bytes. Health-epoch validation compares this value
/// before deciding that a cached verdict is still valid, so an epoch hit
/// never needs the ~6s runtime startup it is trying to avoid.
///
/// The multi-megabyte frozen artifact contributes its pinned digest
/// rather than re-hashed bytes: re-hashing it on every hit would cost
/// ~1s in debug builds for no new information, because runtime startup
/// already proves bytes match the pin (fail-closed) on every full path.
/// Any policy source change (including an unrotated freeze, whose module
/// bytes still differ) changes this identity and forces the full path.
/// Post-build artifact corruption without a const change would serve
/// still-true cached verdicts until the epoch TTL expires, then fail
/// closed on the next full path; that deferred failure is the documented
/// price of a millisecond hit.
pub fn static_policy_identity() -> String {
    let mut hasher = Sha256::new();
    hasher.update(POLICY_PROFILE.as_bytes());
    hasher.update([0]);
    hasher.update(POLICY_BACKEND.as_bytes());
    hasher.update([0]);
    hasher.update(MNCS_LANGUAGE_REV.as_bytes());
    hasher.update([0]);
    hasher.update(FAMILY_ARTIFACT_SHA256.as_bytes());
    hasher.update([0]);
    hasher.update(FAMILY_SOURCE.as_bytes());
    hasher.update([0]);
    for spec in MODULES {
        hasher.update(spec.key.as_bytes());
        hasher.update([0]);
        hasher.update(spec.source.as_bytes());
        hasher.update([0]);
    }
    use std::fmt::Write as _;
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn version_coordinate(version: LanguageVersion) -> i64 {
    i64::from(version.major) * 1_000 + i64::from(version.minor)
}

fn state_array<const N: usize>(values: Vec<u64>) -> Result<[u64; N], RuntimeError> {
    if values.len() != N {
        return Err(RuntimeError::new(
            "mncs_value_contract",
            format!(
                "expected scanner state width {N}, returned {}",
                values.len()
            ),
        ));
    }
    let mut output = [0_u64; N];
    output.copy_from_slice(&values);
    Ok(output)
}

fn native_status(status: Status) -> doctor_version::Status {
    match status {
        Status::Pass => doctor_version::Status::Pass,
        Status::Warning => doctor_version::Status::Warning,
        Status::Fail => doctor_version::Status::Fail,
        Status::Skipped => doctor_version::Status::Skipped,
    }
}

fn severity_code(severity: Severity) -> i64 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    }
}

fn host_remediation_severity(severity: doctor_version::RemediationSeverity) -> Severity {
    match severity {
        doctor_version::RemediationSeverity::error => Severity::Error,
        doctor_version::RemediationSeverity::warning => Severity::Warning,
        doctor_version::RemediationSeverity::info => Severity::Info,
    }
}

fn native_health_check_kind(id: &str) -> Result<doctor_version::HealthCheckKind, RuntimeError> {
    use doctor_version::HealthCheckKind as Kind;
    match id {
        "source-parse-health" => Ok(Kind::SourceParseHealth),
        "header-health" => Ok(Kind::HeaderHealth),
        "version-drift" => Ok(Kind::VersionDrift),
        "newline-encoding" => Ok(Kind::NewlineEncoding),
        "manifest-health" => Ok(Kind::ManifestHealth),
        "toolchain-health" => Ok(Kind::ToolchainHealth),
        "language-knowledge" => Ok(Kind::LanguageKnowledge),
        "migration-availability" => Ok(Kind::MigrationAvailability),
        _ => Err(RuntimeError::new(
            "mncs_value_contract",
            format!("unknown native health-check identity: {id}"),
        )),
    }
}

fn host_status(status: doctor_version::Status) -> Status {
    match status {
        doctor_version::Status::Pass => Status::Pass,
        doctor_version::Status::Warning => Status::Warning,
        doctor_version::Status::Fail => Status::Fail,
        doctor_version::Status::Skipped => Status::Skipped,
    }
}

fn native_transition_kind(kind: TransitionKind) -> doctor_version::TransitionKind {
    match kind {
        TransitionKind::Noop => doctor_version::TransitionKind::Noop,
        TransitionKind::Metadata => doctor_version::TransitionKind::Metadata,
        TransitionKind::Source => doctor_version::TransitionKind::Source,
        TransitionKind::Unknown => doctor_version::TransitionKind::Unknown,
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
                .classify_version(LanguageVersion::new(0, 18))
                .unwrap(),
            VersionClass::Current
        );
        assert_eq!(
            runtime
                .classify_version(LanguageVersion::new(0, 17))
                .unwrap(),
            VersionClass::Sealed
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
    fn production_health_check_kind_controls_info_promotion() {
        let runtime = DoctorMncsRuntime::new().expect("production policy runtime");
        let cases = [
            ("source-parse-health", Status::Pass),
            ("header-health", Status::Pass),
            ("version-drift", Status::Warning),
            ("newline-encoding", Status::Pass),
            ("manifest-health", Status::Pass),
            ("toolchain-health", Status::Pass),
            ("language-knowledge", Status::Pass),
            ("migration-availability", Status::Pass),
        ];
        for (id, expected) in cases {
            let mut checks = [CheckResult {
                id: id.to_owned(),
                title: id.to_owned(),
                status: Status::Pass,
                findings: vec![crate::health::Finding {
                    severity: Severity::Info,
                    message: "independent expected advisory".to_owned(),
                    path: None,
                    explanation: String::new(),
                    suggested_action: String::new(),
                }],
            }];
            runtime
                .apply_health_policy(&mut checks)
                .expect("native health policy");
            assert_eq!(checks[0].status, expected, "{id}");
        }
    }

    #[test]
    fn production_health_policy_rejects_unknown_check_identity() {
        let runtime = DoctorMncsRuntime::new().expect("production policy runtime");
        let mut checks = [CheckResult {
            id: "unknown-health-check".to_owned(),
            title: "unknown".to_owned(),
            status: Status::Pass,
            findings: Vec::new(),
        }];
        let error = runtime
            .apply_health_policy(&mut checks)
            .expect_err("unknown health checks must fail closed");
        assert!(error
            .to_string()
            .contains("unknown native health-check identity"));
    }

    #[test]
    fn static_policy_identity_is_deterministic_and_content_bound() {
        let first = static_policy_identity();
        let second = static_policy_identity();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        // The identity covers more than the bare artifact digest: the
        // profile, backend, revision, and module sources all participate.
        assert_ne!(first, sha256_hex(FAMILY_ARTIFACT));
        assert_ne!(first, FAMILY_ARTIFACT_SHA256);
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
