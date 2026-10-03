//! Validated health epochs: cheap no-change verification.
//!
//! A full `doctor`/`fix`/`remediate` run is expensive (~8s: ~6s of MNCS
//! policy-runtime startup parsing the frozen family artifact, ~0.7s of
//! toolchain subprocesses, plus discovery, per-file diagnosis, and
//! knowledge probes). Almost all of that work is a pure function of
//! slowly-changing inputs. A health epoch records the validated outcome
//! together with fingerprints of every input that produced it; the next
//! invocation re-fingerprints those inputs with plain host I/O (directory
//! listings, file stats, small-file hashes, file-only knowledge probes)
//! and, when everything matches, re-emits the validated outcome without
//! starting the policy runtime or spawning a subprocess.
//!
//! Soundness rules (any doubt runs the full path):
//!
//! - Every input the full path reads is fingerprinted: canonical root,
//!   discovery options, static policy identity, visited directory
//!   listings, per-source stat metadata, manifest bytes, the family
//!   project manifest, toolchain presence facts plus binary content
//!   identities, file-knowledge probe outputs (re-probed and compared),
//!   and the language migration manifest.
//! - Validation needs no policy runtime: directory listings replay the
//!   recorded visited set, and identical listings plus unchanged
//!   policy/options identities imply an identical projection.
//! - Subprocess-derived facts (tool `--version` strings, debugger
//!   capabilities) are validated by binary stat identity (ctime proves
//!   untouched bytes), not by re-execution, and are additionally bounded
//!   by [`HEALTH_EPOCH_MAX_AGE_SECS`].
//! - Epochs never cover `--changed-path` (scoped state is not full
//!   state), `--with-language-backend` (per-file subprocess diagnosis),
//!   or `--verify-cmd` (unvalidated external commands) for remediation
//!   reuse. Only fixpoint-proven remediation states are reusable, so a
//!   run that would repair or continue repairing never fast-paths.
//! - The epoch digest covers inputs *and* the validated state, so any
//!   hand edit invalidates. The file is ignored derived state
//!   (`.mncs/doctor/`, excluded from topology); deleting or corrupting
//!   it only costs one full pass.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diagnostics::Diagnostic;
use crate::discovery::{fingerprint, Inventory, InventorySummary, ManifestHit};
use crate::fix::Convergence;
use crate::health::CheckResult;
use crate::mncs_runtime::{static_policy_identity, PolicyProvenance};
use crate::remediate::{
    degraded_files, Escalation, RemediationEvidence, RemediationReport, RepairRecord,
    REMEDIATION_EVIDENCE_SCHEMA_VERSION, REMEDIATION_SCHEMA_VERSION,
};
use crate::report::Report;
use crate::toolchain::{presence_facts, ToolchainPresence, ToolchainStatus};
use crate::transaction::DiffSummary;
use crate::verify::VerificationOutcome;
use crate::{DOCTOR_VERSION, REPORT_SCHEMA_VERSION};

/// Schema of the persisted health epoch.
pub const HEALTH_EPOCH_SCHEMA: &str = "mncs.doctor.health-epoch/1";

/// Maximum age of a reusable epoch. Validation re-checks every checkable
/// input exactly; the TTL additionally bounds staleness of facts that are
/// validated by stat identity or pinned digest rather than re-execution
/// (tool version strings, debugger capability documents) or re-hashing
/// (the frozen policy artifact bytes).
pub const HEALTH_EPOCH_MAX_AGE_SECS: u64 = 3600;

/// Epoch file name inside the ignored `.mncs/doctor/` directory.
pub const EPOCH_FILE_NAME: &str = "health-epoch.json";

/// Marker for a manifest path that exists in the projection but cannot be
/// read. The full path treats unreadable manifests with default bytes, so
/// a stable unreadable marker validates exactly like stable bytes.
pub const UNREADABLE_DIGEST: &str = "unreadable";

/// Marker for an absent optional manifest (`.mncs/project.json`, language
/// migration manifest).
pub const ABSENT_DIGEST: &str = "absent";

/// Path of the family repository manifest whose bytes feed manifest health.
pub const PROJECT_MANIFEST_RELATIVE: &str = ".mncs/project.json";

/// Where the health epoch lives. Inside `.mncs/doctor/`, which discovery
/// excludes from topology, so recording never invalidates the epoch.
pub fn epoch_path(root: &Path) -> PathBuf {
    root.join(".mncs/doctor").join(EPOCH_FILE_NAME)
}

/// Current Unix timestamp, or `None` when the clock is unusable (which
/// forces the full path: doubt never reuses).
pub fn now_unix() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

// -- source stat metadata ------------------------------------------------

/// Stat fingerprint for one inventoried source. Identical bytes with an
/// unchanged device/inode/size/ctime/mtime triple validate; anything else
/// re-runs the full path. This is the same trust bar as the incremental
/// inventory cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMetadata {
    pub len: u64,
    pub modified_seconds: Option<u64>,
    pub modified_nanos: Option<u32>,
    #[serde(default)]
    pub device: Option<u64>,
    #[serde(default)]
    pub inode: Option<u64>,
    #[serde(default)]
    pub change_seconds: Option<i64>,
    #[serde(default)]
    pub change_nanos: Option<i64>,
}

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Stat one source path (symlinks followed, matching byte acquisition).
pub fn source_metadata(path: &Path) -> Result<SourceMetadata, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot stat source {}: {error}", path.display()))?;
    let modified = metadata.modified().ok().and_then(|value| {
        value
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| (duration.as_secs(), duration.subsec_nanos()))
    });
    Ok(SourceMetadata {
        len: metadata.len(),
        modified_seconds: modified.map(|value| value.0),
        modified_nanos: modified.map(|value| value.1),
        #[cfg(unix)]
        device: Some(metadata.dev()),
        #[cfg(not(unix))]
        device: None,
        #[cfg(unix)]
        inode: Some(metadata.ino()),
        #[cfg(not(unix))]
        inode: None,
        #[cfg(unix)]
        change_seconds: Some(metadata.ctime()),
        #[cfg(not(unix))]
        change_seconds: None,
        #[cfg(unix)]
        change_nanos: Some(metadata.ctime_nsec()),
        #[cfg(not(unix))]
        change_nanos: None,
    })
}

/// Stat every inventoried source, keyed by slash-separated relative path.
pub fn inventory_source_metadata(
    inventory: &Inventory,
) -> Result<BTreeMap<String, SourceMetadata>, String> {
    inventory
        .sources
        .iter()
        .map(|source| Ok((source.relative.clone(), source_metadata(&source.path)?)))
        .collect()
}

// -- manifest digests ----------------------------------------------------

/// SHA-256 hex of file bytes, or [`UNREADABLE_DIGEST`] when the path
/// cannot be read.
pub fn bytes_digest(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| fingerprint(&bytes))
        .unwrap_or_else(|_| UNREADABLE_DIGEST.to_owned())
}

/// Digest every manifest in the projection. Manifest health reads manifest
/// bytes (JSON validity, `version` field presence), so topology paths
/// alone cannot validate it.
pub fn manifest_digests(manifests: &[ManifestHit]) -> BTreeMap<String, String> {
    manifests
        .iter()
        .map(|manifest| (manifest.relative.clone(), bytes_digest(&manifest.path)))
        .collect()
}

/// Digest the family repository manifest: [`ABSENT_DIGEST`] when missing,
/// [`UNREADABLE_DIGEST`] when unreadable, else content SHA-256.
pub fn project_manifest_digest(root: &Path) -> String {
    let path = root.join(PROJECT_MANIFEST_RELATIVE);
    if !path.is_file() {
        return ABSENT_DIGEST.to_owned();
    }
    bytes_digest(&path)
}

// -- toolchain digest ----------------------------------------------------

/// Subprocess-derived toolchain facts, validated without re-execution.
/// `presence` re-resolves exactly what the probe resolves (overrides,
/// `PATH`, resolved paths); `binary_identities` pins the stat identity
/// behind every `--version`/`--help`/`capabilities` output the epoch
/// reuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainDigest {
    pub presence: ToolchainPresence,
    pub binary_identities: BTreeMap<String, String>,
}

/// Stat identity for one probed binary: device, inode, length, and
/// nanosecond mtime/ctime. Content hashing is deliberately avoided: a
/// 25MB toolchain binary would cost ~1s per hit in debug builds, while
/// ctime already proves bytes are untouched (the kernel bumps it on any
/// write and it cannot be forged back) and inode proves the path was not
/// replaced. A toolchain upgrade always changes these and forces the
/// full path, which re-probes.
fn binary_identity(path: Option<&str>) -> String {
    let Some(raw) = path else {
        return ABSENT_DIGEST.to_owned();
    };
    let candidate = PathBuf::from(raw);
    let metadata = match std::fs::metadata(&candidate) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return "missing".to_owned(),
        Err(_) => return "missing".to_owned(),
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| format!("{}:{}", duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or_else(|| "unknown".to_owned());
    #[cfg(unix)]
    {
        format!(
            "stat:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            modified,
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    }
    #[cfg(not(unix))]
    {
        format!("stat:{}:{}", metadata.len(), modified)
    }
}

/// Collect the toolchain digest without spawning any subprocess.
pub fn collect_toolchain_digest() -> ToolchainDigest {
    let presence = presence_facts();
    let mut binary_identities = BTreeMap::new();
    binary_identities.insert(
        "rust_cli".to_owned(),
        binary_identity(presence.rust_cli_path.as_deref()),
    );
    binary_identities.insert(
        "family_cli".to_owned(),
        binary_identity(presence.family_cli_path.as_deref()),
    );
    binary_identities.insert(
        "cargo".to_owned(),
        binary_identity(presence.cargo_path.as_deref()),
    );
    binary_identities.insert(
        "forge".to_owned(),
        binary_identity(presence.forge_path.as_deref()),
    );
    binary_identities.insert(
        "ravel".to_owned(),
        binary_identity(presence.ravel_path.as_deref()),
    );
    binary_identities.insert(
        "test_provider".to_owned(),
        binary_identity(presence.test_provider_path.as_deref()),
    );
    binary_identities.insert(
        "debug_provider".to_owned(),
        binary_identity(presence.debug_provider_path.as_deref()),
    );
    ToolchainDigest {
        presence,
        binary_identities,
    }
}

// -- knowledge snapshot --------------------------------------------------

/// File-knowledge probe outputs, re-probed and compared exactly on the
/// validation path. The probes are pure file I/O (no subprocesses, no
/// policy runtime), so re-running them *is* the validation: whatever
/// environment or file change would alter the outcome fails the compare
/// and forces the full path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeSnapshot {
    pub language: serde_json::Value,
    pub architecture: serde_json::Value,
}

/// Re-run the file-only knowledge probes for `root`. Must mirror the
/// probe context recording used ([`probe_toolchain_at`] with `Some(root)`).
pub fn collect_knowledge(root: &Path) -> KnowledgeSnapshot {
    let language = crate::language_knowledge::probe(Some(root));
    let architecture = crate::architecture_knowledge::probe(Some(root), Some(&language));
    KnowledgeSnapshot {
        language: serde_json::to_value(&language).unwrap_or(serde_json::Value::Null),
        architecture: serde_json::to_value(&architecture).unwrap_or(serde_json::Value::Null),
    }
}

/// Snapshot the recorded toolchain's knowledge outputs for comparison.
pub fn knowledge_of(toolchain: &ToolchainStatus) -> KnowledgeSnapshot {
    KnowledgeSnapshot {
        language: toolchain
            .language_knowledge
            .as_ref()
            .and_then(|status| serde_json::to_value(status).ok())
            .unwrap_or(serde_json::Value::Null),
        architecture: toolchain
            .architecture_knowledge
            .as_ref()
            .and_then(|status| serde_json::to_value(status).ok())
            .unwrap_or(serde_json::Value::Null),
    }
}

// -- migration manifest digest -------------------------------------------

/// Language migration manifest identity. The manifest selects fix
/// providers, so remediation reuse must prove it is unchanged. Digested
/// over bytes: a manifest that becomes malformed changes bytes (or
/// readability) and forces the full path, which then fails closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationDigest {
    pub path: Option<String>,
    pub digest: String,
}

/// Discover and digest the migration manifest without parsing it.
pub fn collect_migration_digest(root: &Path) -> MigrationDigest {
    match crate::language_knowledge::discover_migrations(Some(root)) {
        None => MigrationDigest {
            path: None,
            digest: ABSENT_DIGEST.to_owned(),
        },
        Some(path) => {
            let digest = bytes_digest(&path);
            MigrationDigest {
                path: Some(path.to_string_lossy().into_owned()),
                digest,
            }
        }
    }
}

// -- validated state -----------------------------------------------------

/// The validated outcome for one exact world state. A `doctor` run fills
/// the diagnostic half; `fix`/`remediate` additionally prove the repair
/// fixpoint (convergence, escalations, post-state verification) that
/// makes remediation reuse sound. Fields a command cannot prove stay at
/// their inert defaults and gate reuse off.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedState {
    /// Health checks over this state (policy-classified statuses).
    pub checks: Vec<CheckResult>,
    /// Per-file diagnostics keyed by relative path.
    pub file_diagnostics: BTreeMap<String, Vec<Diagnostic>>,
    /// Full recorded toolchain (reused as last-validated facts).
    pub toolchain: ToolchainStatus,
    /// Recorded acquisition metrics. Re-emitted as-is so human output
    /// stays stable across the fast path; only the epoch marker differs.
    pub inventory_summary: InventorySummary,
    /// Policy provenance of the validating run. Entrypoints describe the
    /// validating process and are cleared on re-emit.
    pub policy: PolicyProvenance,
    /// Exit a `doctor` run over this state reports.
    pub doctor_exit_code: i32,
    pub doctor_exit_meaning: String,
    pub review_blocked_doctor: bool,
    pub scope_note: String,
    /// True only when a Safe-only validation loop proved no eligible fix
    /// remains, with no budget exhaustion and no external verify command.
    /// Remediation reuse requires this (or a matching dry run).
    #[serde(default)]
    pub clean_fixpoint: bool,
    /// True when this state was validated by a dry run (nothing was
    /// mutated, so pre-state and post-state coincide). Reuse requires
    /// the current run to be a dry run as well.
    #[serde(default)]
    pub dry_run: bool,
    /// Post-state fixpoint validation records (empty for doctor-only).
    #[serde(default)]
    pub convergence: Vec<Convergence>,
    /// Residual escalations over this state (empty for doctor-only).
    #[serde(default)]
    pub escalations: Vec<Escalation>,
    /// Post-state verification with `errors_before == errors_after`
    /// (what a no-change rerun would compute), or `None` when the
    /// validating run could not prove it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_post: Option<VerificationOutcome>,
    /// Exit a clean `remediate` run over this state reports.
    #[serde(default)]
    pub remediation_exit_code: i32,
    #[serde(default)]
    pub remediation_exit_meaning: String,
    /// Hypothetical repairs/diffs/notes, populated only for dry runs so
    /// a dry-run hit re-emits exactly what a dry-run full pass would.
    #[serde(default)]
    pub dry_repairs: Vec<RepairRecord>,
    #[serde(default)]
    pub dry_diffs: Vec<DiffSummary>,
    #[serde(default)]
    pub dry_notes: Vec<String>,
}

/// Every input the full path reads, as recorded by the validating run.
/// Validation re-collects each of these with plain host I/O and compares
/// exactly; the digest additionally binds inputs to state so hand edits
/// invalidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthEpoch {
    pub schema_version: String,
    pub doctor_version: String,
    pub report_schema: String,
    pub remediation_schema: String,
    pub remediation_evidence_schema: String,
    pub recorded_at_unix: u64,
    pub digest: String,
    pub root: PathBuf,
    pub options_identity: String,
    pub policy_identity: String,
    pub inventory_identity: String,
    pub topology_identity: String,
    pub visited_directories: Vec<crate::discovery::DirectoryTopology>,
    pub source_metadata: BTreeMap<String, SourceMetadata>,
    pub manifest_digests: BTreeMap<String, String>,
    pub project_manifest_digest: String,
    pub toolchain: ToolchainDigest,
    pub knowledge: KnowledgeSnapshot,
    pub migration_manifest: MigrationDigest,
    pub with_language_backend: bool,
    pub state: ValidatedState,
}

impl HealthEpoch {
    /// Short stable prefix of the digest for notes and evidence.
    pub fn short_digest(&self) -> String {
        self.digest.chars().take(12).collect()
    }
}

/// Inputs bundle: the recomputable validation inputs plus recorded
/// evidence identities, over which the epoch digest is computed.
#[derive(Debug, Clone, Serialize)]
struct EpochInputs<'a> {
    root: &'a Path,
    options_identity: &'a str,
    policy_identity: &'a str,
    inventory_identity: &'a str,
    topology_identity: &'a str,
    visited_directories: &'a [crate::discovery::DirectoryTopology],
    source_metadata: &'a BTreeMap<String, SourceMetadata>,
    manifest_digests: &'a BTreeMap<String, String>,
    project_manifest_digest: &'a str,
    toolchain: &'a ToolchainDigest,
    knowledge: &'a KnowledgeSnapshot,
    migration_manifest: &'a MigrationDigest,
    with_language_backend: bool,
}

fn epoch_digest_for(inputs: &EpochInputs<'_>, state: &ValidatedState) -> String {
    let payload = serde_json::json!({"inputs": inputs, "state": state});
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    fingerprint(&bytes)
}

/// Record a new epoch over a just-validated full run. All identity and
/// digest inputs describe the state the run leaves behind (post-mutation
/// for `fix`/`remediate`, current for `doctor`).
#[allow(clippy::too_many_arguments)]
pub fn record_epoch(
    root: &Path,
    options_identity: String,
    inventory_identity: String,
    topology_identity: String,
    visited_directories: Vec<crate::discovery::DirectoryTopology>,
    source_metadata: BTreeMap<String, SourceMetadata>,
    manifest_digests: BTreeMap<String, String>,
    project_manifest_digest: String,
    toolchain: ToolchainDigest,
    knowledge: KnowledgeSnapshot,
    migration_manifest: MigrationDigest,
    state: ValidatedState,
) -> Result<HealthEpoch, String> {
    let recorded_at_unix = now_unix().ok_or_else(|| "clock is unusable".to_owned())?;
    let inputs = EpochInputs {
        root,
        options_identity: &options_identity,
        policy_identity: &static_policy_identity(),
        inventory_identity: &inventory_identity,
        topology_identity: &topology_identity,
        visited_directories: &visited_directories,
        source_metadata: &source_metadata,
        manifest_digests: &manifest_digests,
        project_manifest_digest: &project_manifest_digest,
        toolchain: &toolchain,
        knowledge: &knowledge,
        migration_manifest: &migration_manifest,
        with_language_backend: false,
    };
    let digest = epoch_digest_for(&inputs, &state);
    Ok(HealthEpoch {
        schema_version: HEALTH_EPOCH_SCHEMA.to_owned(),
        doctor_version: DOCTOR_VERSION.to_owned(),
        report_schema: REPORT_SCHEMA_VERSION.to_owned(),
        remediation_schema: REMEDIATION_SCHEMA_VERSION.to_owned(),
        remediation_evidence_schema: REMEDIATION_EVIDENCE_SCHEMA_VERSION.to_owned(),
        recorded_at_unix,
        digest,
        root: root.to_path_buf(),
        options_identity,
        policy_identity: static_policy_identity(),
        inventory_identity,
        topology_identity,
        visited_directories,
        source_metadata,
        manifest_digests,
        project_manifest_digest,
        toolchain,
        knowledge,
        migration_manifest,
        with_language_backend: false,
        state,
    })
}

/// Persist the epoch atomically (temp + rename). A stale temp file from
/// a crashed run is removed first; failure to record never fails the
/// validating command (callers degrade to a note).
pub fn store_epoch(root: &Path, epoch: &HealthEpoch) -> Result<(), String> {
    let path = epoch_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create epoch directory: {error}"))?;
    }
    let bytes = serde_json::to_vec_pretty(epoch)
        .map_err(|error| format!("cannot encode health epoch: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&temporary);
    std::fs::write(&temporary, &bytes)
        .map_err(|error| format!("cannot write health epoch: {error}"))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot install health epoch: {error}"))?;
    Ok(())
}

/// Load the persisted epoch. Any failure (missing, unreadable,
/// unparsable, wrong schema) is an invalidation reason, never an error:
/// the caller runs the full path.
pub fn load_epoch(root: &Path) -> Result<HealthEpoch, String> {
    let path = epoch_path(root);
    let bytes = std::fs::read(&path).map_err(|error| format!("no epoch: {error}"))?;
    let epoch: HealthEpoch =
        serde_json::from_slice(&bytes).map_err(|error| format!("epoch-unparsable: {error}"))?;
    if epoch.schema_version != HEALTH_EPOCH_SCHEMA {
        return Err("epoch-schema".to_owned());
    }
    Ok(epoch)
}

/// Validate a loaded epoch against the live world. Returns `Ok(())` when
/// every input matches, else the first stable invalidation reason.
/// Performs no policy calls and spawns no subprocesses.
pub fn validate_epoch(
    epoch: &HealthEpoch,
    root: &Path,
    options_identity: &str,
    now_unix: u64,
) -> Result<(), String> {
    if epoch.schema_version != HEALTH_EPOCH_SCHEMA {
        return Err("epoch-schema".to_owned());
    }
    if epoch.doctor_version != DOCTOR_VERSION {
        return Err("doctor-version".to_owned());
    }
    if epoch.report_schema != REPORT_SCHEMA_VERSION
        || epoch.remediation_schema != REMEDIATION_SCHEMA_VERSION
        || epoch.remediation_evidence_schema != REMEDIATION_EVIDENCE_SCHEMA_VERSION
    {
        return Err("report-schema".to_owned());
    }
    if epoch.root != root {
        return Err("root".to_owned());
    }
    if epoch.options_identity != options_identity {
        return Err("options".to_owned());
    }
    if epoch.policy_identity != static_policy_identity() {
        return Err("policy".to_owned());
    }
    if epoch.with_language_backend {
        return Err("backend".to_owned());
    }
    if epoch.recorded_at_unix > now_unix {
        return Err("epoch-clock".to_owned());
    }
    if now_unix - epoch.recorded_at_unix > HEALTH_EPOCH_MAX_AGE_SECS {
        return Err("expired".to_owned());
    }
    // Directory listings replay the recorded visited set exactly. A new,
    // removed, renamed, retyped, or symlink-retargeted entry inside any
    // visited directory changes that directory's identity; entries can
    // only appear inside visited directories, so nothing hides. Together
    // with unchanged policy/options identities this implies the recorded
    // projection (sources, manifests, skipped sets, extension counts).
    for recorded in &epoch.visited_directories {
        let dir = if recorded.relative.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&recorded.relative)
        };
        match crate::discovery::directory_entry_identity(root, &dir) {
            Ok(identity) if identity == recorded.identity => {}
            _ => {
                let name = if recorded.relative.is_empty() {
                    "<root>"
                } else {
                    recorded.relative.as_str()
                };
                return Err(format!("dir:{name}"));
            }
        }
    }
    for (relative, expected) in &epoch.source_metadata {
        match source_metadata(&root.join(relative)) {
            Ok(current) if &current == expected => {}
            _ => return Err(format!("source:{relative}")),
        }
    }
    for (relative, expected) in &epoch.manifest_digests {
        if &bytes_digest(&root.join(relative)) != expected {
            return Err(format!("manifest:{relative}"));
        }
    }
    if project_manifest_digest(root) != epoch.project_manifest_digest {
        return Err("project-manifest".to_owned());
    }
    if collect_toolchain_digest() != epoch.toolchain {
        return Err("toolchain".to_owned());
    }
    if collect_knowledge(root) != epoch.knowledge {
        return Err("knowledge".to_owned());
    }
    if collect_migration_digest(root) != epoch.migration_manifest {
        return Err("migrations".to_owned());
    }
    // Integrity: the digest binds recorded inputs to recorded state. The
    // live inputs above already match, so a digest mismatch means the
    // epoch file itself was tampered with or spliced.
    let inputs = EpochInputs {
        root,
        options_identity: &epoch.options_identity,
        policy_identity: &epoch.policy_identity,
        inventory_identity: &epoch.inventory_identity,
        topology_identity: &epoch.topology_identity,
        visited_directories: &epoch.visited_directories,
        source_metadata: &epoch.source_metadata,
        manifest_digests: &epoch.manifest_digests,
        project_manifest_digest: &epoch.project_manifest_digest,
        toolchain: &epoch.toolchain,
        knowledge: &epoch.knowledge,
        migration_manifest: &epoch.migration_manifest,
        with_language_backend: epoch.with_language_backend,
    };
    if epoch_digest_for(&inputs, &epoch.state) != epoch.digest {
        return Err("digest".to_owned());
    }
    Ok(())
}

// -- re-emit -------------------------------------------------------------

/// Provenance for a re-emitted outcome: the validating engine and
/// artifacts still describe the decisions, but no entrypoint ran in
/// this process, so the trace is cleared rather than replayed.
fn reused_policy(policy: &PolicyProvenance) -> PolicyProvenance {
    let mut reused = policy.clone();
    reused.entrypoints.clear();
    reused
}

fn reuse_note(epoch: &HealthEpoch, now_unix: u64) -> String {
    let age = now_unix.saturating_sub(epoch.recorded_at_unix);
    format!(
        "epoch reused: {} (validated {age}s ago; no rescan, no policy startup)",
        epoch.short_digest()
    )
}

/// Rebuild a `doctor` report from a validated epoch. Current output
/// shaping flags (`explain`, `verbose`) apply to the stored data; the
/// stored scope note and acquisition metrics are kept so human output
/// stays stable across the fast path.
pub fn doctor_report_from_epoch(
    epoch: &HealthEpoch,
    explain: bool,
    verbose: bool,
    now_unix: u64,
) -> Report {
    let mut summary = epoch.state.inventory_summary.clone();
    summary.epoch_reused = true;
    summary.epoch_digest = Some(epoch.digest.clone());
    let mut notes = vec![epoch.state.scope_note.clone()];
    if explain {
        notes.push("explain".to_owned());
    }
    notes.push(reuse_note(epoch, now_unix));
    if verbose {
        notes.push(
            "epoch validated without policy-runtime startup, subprocesses, or byte rescans"
                .to_owned(),
        );
    }
    let mut report = Report::new("doctor", epoch.root.to_string_lossy());
    report.inventory = Some(summary);
    report.checks = epoch.state.checks.clone();
    report.file_diagnostics = epoch.state.file_diagnostics.clone();
    report.toolchain = Some(epoch.state.toolchain.clone());
    report.policy = Some(reused_policy(&epoch.state.policy));
    report.notes = notes;
    report.exit_code = epoch.state.doctor_exit_code;
    report.exit_meaning = epoch.state.doctor_exit_meaning.clone();
    report
}

/// A remediation outcome synthesized from a validated epoch. The caller
/// sets the evidence path after persisting the evidence artifact.
pub struct RemediationHit {
    pub report: RemediationReport,
    pub evidence: RemediationEvidence,
}

/// Rebuild a `remediate` outcome from a validated epoch. A non-dry hit
/// requires a fixpoint-proven state (`clean_fixpoint` with no dry run:
/// a full pass would repair nothing); a dry hit requires a dry-run
/// epoch (the hypothetical plan re-emits exactly). Anything else is a
/// caller bug and refuses rather than guessing. The synthesized
/// envelope equals what a full pass over the unchanged state would
/// emit, plus the reuse note.
pub fn remediation_from_epoch(
    epoch: &HealthEpoch,
    dry_run: bool,
    budget_files: usize,
    now_unix: u64,
) -> Result<RemediationHit, String> {
    let state = &epoch.state;
    if dry_run {
        if !state.dry_run {
            return Err("epoch run mode does not match".to_owned());
        }
    } else if !state.clean_fixpoint || state.dry_run {
        return Err("epoch does not prove a repair fixpoint".to_owned());
    }
    let verification = state
        .verification_post
        .clone()
        .ok_or_else(|| "epoch lacks post-state verification".to_owned())?;
    let mut notes = vec![state.scope_note.clone()];
    if dry_run {
        notes.extend(state.dry_notes.clone());
    }
    notes.push(reuse_note(epoch, now_unix));

    let mut report = RemediationReport::new(epoch.root.to_string_lossy());
    report.dry_run = dry_run;
    report.budget.max_items = budget_files;
    if dry_run {
        // A dry run plans hypothetically without mutating; the recorded
        // hypothetical repairs are budget-independent and re-emit as-is.
        report.repairs = state.dry_repairs.clone();
        report.budget.items_done = state.dry_repairs.len();
    }
    let degraded = degraded_files(&state.file_diagnostics);
    report.refresh_summary(&state.escalations, degraded);
    report.validation = crate::remediate::RemediationValidation {
        passed: verification.passed,
        errors_before: verification.errors_before,
        errors_after: verification.errors_after,
        idempotent_known: verification.idempotent.is_some(),
        idempotent: verification.idempotent.unwrap_or(false),
    };
    report.notes = notes.clone();
    report.exit_code = state.remediation_exit_code;
    report.exit_meaning = state.remediation_exit_meaning.clone();

    let evidence = RemediationEvidence {
        schema_version: REMEDIATION_EVIDENCE_SCHEMA_VERSION.to_owned(),
        report_schema_version: REPORT_SCHEMA_VERSION.to_owned(),
        doctor_version: DOCTOR_VERSION.to_owned(),
        root: epoch.root.to_string_lossy().into_owned(),
        dry_run,
        repairs: if dry_run {
            state.dry_repairs.clone()
        } else {
            Vec::new()
        },
        reconciliations: Vec::new(),
        escalations: state.escalations.clone(),
        diffs: if dry_run {
            state.dry_diffs.clone()
        } else {
            Vec::new()
        },
        convergence: state.convergence.clone(),
        file_diagnostics: state.file_diagnostics.clone(),
        verification: Some(verification),
        policy: Some(reused_policy(&state.policy)),
        notes,
    };
    Ok(RemediationHit { report, evidence })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch_root() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "mncs-doctor-epoch-unit-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("mncs-forge.toml"), "version = 1\n").unwrap();
        std::fs::write(root.join("src/a.mncs"), "mncs 0.17;\nmodule a;\n").unwrap();
        root
    }

    fn test_state() -> ValidatedState {
        ValidatedState {
            checks: Vec::new(),
            file_diagnostics: BTreeMap::new(),
            toolchain: ToolchainStatus::default(),
            inventory_summary: InventorySummary {
                files_checked: 1,
                manifests: 1,
                skipped_dirs: 0,
                total_bytes: 24,
                files_reused: 0,
                files_rescanned: 1,
                directories_revalidated: 2,
                topology_reused: false,
                topology_invalidated: false,
                cache_identity: None,
                topology_identity: None,
                invalidation_reason: Some("repository_scan_requested".to_owned()),
                epoch_reused: false,
                epoch_digest: None,
            },
            policy: PolicyProvenance {
                engine: "mncs".to_owned(),
                profile: "0.16".to_owned(),
                backend: "mncs-research-bytecode".to_owned(),
                language_revision: "rev".to_owned(),
                family_source_sha256: "abc".to_owned(),
                modules: BTreeMap::new(),
                entrypoints: vec!["doctor.health.v1::overall".to_owned()],
            },
            doctor_exit_code: 0,
            doctor_exit_meaning: "healthy".to_owned(),
            review_blocked_doctor: false,
            scope_note: "scope: repository (1 source file(s))".to_owned(),
            clean_fixpoint: false,
            dry_run: false,
            convergence: Vec::new(),
            escalations: Vec::new(),
            verification_post: None,
            remediation_exit_code: 0,
            remediation_exit_meaning: String::new(),
            dry_repairs: Vec::new(),
            dry_diffs: Vec::new(),
            dry_notes: Vec::new(),
        }
    }

    fn test_epoch(root: &Path) -> HealthEpoch {
        let visited = vec![
            crate::discovery::DirectoryTopology {
                relative: String::new(),
                identity: crate::discovery::directory_entry_identity(root, root).unwrap(),
            },
            crate::discovery::DirectoryTopology {
                relative: "src".to_owned(),
                identity: crate::discovery::directory_entry_identity(root, &root.join("src"))
                    .unwrap(),
            },
        ];
        let mut sources = BTreeMap::new();
        sources.insert(
            "src/a.mncs".to_owned(),
            source_metadata(&root.join("src/a.mncs")).unwrap(),
        );
        let mut manifests = BTreeMap::new();
        manifests.insert(
            "mncs-forge.toml".to_owned(),
            bytes_digest(&root.join("mncs-forge.toml")),
        );
        record_epoch(
            root,
            "options".to_owned(),
            "inventory".to_owned(),
            "topology".to_owned(),
            visited,
            sources,
            manifests,
            project_manifest_digest(root),
            collect_toolchain_digest(),
            collect_knowledge(root),
            collect_migration_digest(root),
            test_state(),
        )
        .unwrap()
    }

    #[test]
    fn record_then_validate_hits_on_unchanged_tree() {
        let root = scratch_root();
        let epoch = test_epoch(&root);
        let now = now_unix().unwrap();
        assert_eq!(validate_epoch(&epoch, &root, "options", now), Ok(()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn epoch_digest_is_deterministic_over_inputs_and_state() {
        let root = scratch_root();
        let first = test_epoch(&root);
        let second = test_epoch(&root);
        assert_eq!(first.digest, second.digest);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn each_input_class_invalidates_with_a_stable_reason() {
        let root = scratch_root();
        let epoch = test_epoch(&root);
        let now = now_unix().unwrap();
        // Altered options identity.
        assert_eq!(
            validate_epoch(&epoch, &root, "other-options", now),
            Err("options".to_owned())
        );
        // Altered source bytes (length change: deterministic even under
        // coarse timestamp granularity).
        std::fs::write(root.join("src/a.mncs"), "mncs 0.17;\nmodule b;\n// note\n").unwrap();
        assert_eq!(
            validate_epoch(&epoch, &root, "options", now),
            Err("source:src/a.mncs".to_owned())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn new_listing_entries_invalidate_the_parent_directory() {
        let root = scratch_root();
        let epoch = test_epoch(&root);
        std::fs::write(root.join("src/new.mncs"), "mncs 0.17;\nmodule new;\n").unwrap();
        let now = now_unix().unwrap();
        assert_eq!(
            validate_epoch(&epoch, &root, "options", now),
            Err("dir:src".to_owned())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn manifest_and_project_manifest_bytes_are_validated() {
        let root = scratch_root();
        let epoch = test_epoch(&root);
        let now = now_unix().unwrap();
        std::fs::write(root.join("mncs-forge.toml"), "version = 2\n").unwrap();
        assert_eq!(
            validate_epoch(&epoch, &root, "options", now),
            Err("manifest:mncs-forge.toml".to_owned())
        );
        let _ = std::fs::remove_dir_all(&root);

        let root = scratch_root();
        let epoch = test_epoch(&root);
        std::fs::create_dir_all(root.join(".mncs")).unwrap();
        std::fs::write(root.join(".mncs/project.json"), "{}").unwrap();
        let now = now_unix().unwrap();
        assert_eq!(
            validate_epoch(&epoch, &root, "options", now),
            Err("project-manifest".to_owned())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn expired_and_future_epochs_never_validate() {
        let root = scratch_root();
        let mut epoch = test_epoch(&root);
        let now = now_unix().unwrap();
        epoch.recorded_at_unix = now.saturating_sub(HEALTH_EPOCH_MAX_AGE_SECS + 1);
        assert_eq!(
            validate_epoch(&epoch, &root, "options", now),
            Err("expired".to_owned())
        );
        epoch.recorded_at_unix = now + 60;
        assert_eq!(
            validate_epoch(&epoch, &root, "options", now),
            Err("epoch-clock".to_owned())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tampered_state_fails_the_digest_check() {
        let root = scratch_root();
        let mut epoch = test_epoch(&root);
        epoch.state.doctor_exit_code = 3;
        let now = now_unix().unwrap();
        assert_eq!(
            validate_epoch(&epoch, &root, "options", now),
            Err("digest".to_owned())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn doctor_reemit_marks_reuse_and_clears_entrypoints() {
        let root = scratch_root();
        let epoch = test_epoch(&root);
        let now = now_unix().unwrap();
        let report = doctor_report_from_epoch(&epoch, false, false, now);
        let summary = report.inventory.unwrap();
        assert!(summary.epoch_reused);
        assert_eq!(summary.epoch_digest.as_deref(), Some(epoch.digest.as_str()));
        // Recorded acquisition metrics are kept for stable human output.
        assert_eq!(summary.files_rescanned, 1);
        assert_eq!(summary.files_reused, 0);
        let policy = report.policy.unwrap();
        assert!(policy.entrypoints.is_empty());
        assert_eq!(policy.engine, "mncs");
        assert!(report
            .notes
            .iter()
            .any(|note| note.starts_with("epoch reused: ")));
        assert_eq!(report.exit_code, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remediation_reemit_refuses_unproven_states() {
        let root = scratch_root();
        let epoch = test_epoch(&root);
        let now = now_unix().unwrap();
        // Doctor-only epochs prove no repair fixpoint.
        assert!(remediation_from_epoch(&epoch, false, 256, now).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn toolchain_digest_covers_resolution_and_stat_identity() {
        let digest = collect_toolchain_digest();
        // Every resolved binary carries an identity marker.
        assert_eq!(digest.binary_identities.len(), 7);
        for identity in digest.binary_identities.values() {
            assert!(!identity.is_empty());
        }
        // Presence round-trips through the epoch encoding.
        let bytes = serde_json::to_vec(&digest).unwrap();
        let back: ToolchainDigest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, digest);
    }
}
