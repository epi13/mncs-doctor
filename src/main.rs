//! `mncs-doctor` command surface.
//!
//! Concepts (names follow repository conventions; the underlying ideas match
//! the `doctor` / `fix` / `migrate` / `verify` split):
//!
//! - `doctor` — inspect repository health, explain, emit reports. Never mutates.
//! - `fix` — plan and apply safe repairs (`--dry-run` default-off amongst
//!   flags; dry-run never mutates).
//! - `migrate` — inspect, plan, dry-run, and apply version migrations.
//! - `verify` — re-diagnose and optionally run project verification commands.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode as ProcExit;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use mncs_doctor::diagnostics::{scan_header, Diagnostic, LanguageBackend, ScannerBackend};
use mncs_doctor::discovery::{
    discover_topology_with_policy, discover_with_policy, find_root, read_source_file,
    DiscoveryOptions, Inventory, SourceFile, TopologySnapshot,
};
use mncs_doctor::fix::{
    plan_workspace_with_mncs_policy, providers_for_root, repair_to_fixpoint_with_diagnose,
    union_apply_with_mncs, Eligibility,
};
use mncs_doctor::health::{run_all_checks, CheckResult, HealthContext, Status};
use mncs_doctor::migration::{
    apply_plan, default_registry, plan as plan_migration, resolve_target, MigrationVerdict,
    TransitionKind,
};
use mncs_doctor::mncs_runtime::DoctorMncsRuntime;
use mncs_doctor::report::{render_human, ExitCode, Report};
use mncs_doctor::toolchain::{find_rust_cli, probe_toolchain, probe_toolchain_at, RustCliBackend};
use mncs_doctor::transaction::{summarize_diff, CommitReport, FileOp, Transaction};
use mncs_doctor::verify::{run_external, verify_after_with_diagnostics, ExternalCheck};
use mncs_doctor::{DOCTOR_VERSION, REPORT_SCHEMA_VERSION};

fn main() -> ProcExit {
    match run() {
        Ok(code) => ProcExit::from(code.as_i32() as u8),
        Err(message) => {
            eprintln!("mncs-doctor: error: {message}");
            ProcExit::from(ExitCode::ToolFailure.as_i32() as u8)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(ExitCode::Healthy);
    }
    if args[0] == "--version" || args[0] == "-V" {
        println!("mncs-doctor {DOCTOR_VERSION} (report schema {REPORT_SCHEMA_VERSION})");
        return Ok(ExitCode::Healthy);
    }
    match args[0].as_str() {
        "doctor" => cmd_doctor(&args[1..]),
        "fix" => cmd_fix(&args[1..]),
        "migrate" => cmd_migrate(&args[1..]),
        "verify" => cmd_verify(&args[1..]),
        other => Err(format!(
            "unknown command {other:?}; see `mncs-doctor --help`"
        )),
    }
}

#[derive(Debug, Default)]
struct GlobalFlags {
    root: Option<PathBuf>,
    json: bool,
    explain: bool,
    quiet: bool,
    verbose: bool,
    with_language_backend: bool,
    changed_paths: Vec<PathBuf>,
}

const INVENTORY_CACHE_SCHEMA: &str = "mncs.doctor.inventory-cache/2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceMetadata {
    len: u64,
    modified_seconds: Option<u64>,
    modified_nanos: Option<u32>,
    #[serde(default)]
    device: Option<u64>,
    #[serde(default)]
    inode: Option<u64>,
    #[serde(default)]
    change_seconds: Option<i64>,
    #[serde(default)]
    change_nanos: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InventoryCache {
    schema_version: String,
    root: PathBuf,
    options_identity: String,
    policy_identity: String,
    inventory_identity: String,
    topology_identity: String,
    source_metadata: BTreeMap<String, SourceMetadata>,
    inventory: Inventory,
}

fn parse_globals(
    args: &[String],
    start: usize,
    out: &mut Vec<String>,
) -> Result<GlobalFlags, String> {
    let mut flags = GlobalFlags::default();
    let mut i = start;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                flags.root = Some(PathBuf::from(args.get(i).ok_or("--root requires a value")?));
            }
            "--json" => flags.json = true,
            "--explain" => flags.explain = true,
            "--quiet" | "-q" => flags.quiet = true,
            "--verbose" | "-v" => flags.verbose = true,
            "--with-language-backend" => flags.with_language_backend = true,
            "--changed-path" => {
                i += 1;
                flags.changed_paths.push(PathBuf::from(
                    args.get(i).ok_or("--changed-path requires a value")?,
                ));
            }
            "--no-color" => {}
            _ => out.push(args[i].clone()),
        }
        i += 1;
    }
    Ok(flags)
}

/// Select the smallest Doctor surface named by the caller. Discovery remains
/// the source of truth for identities and bytes; this helper only narrows the
/// deterministic diagnostic/migration/fix work and refuses unknown paths.
fn scoped_sources(
    root: &std::path::Path,
    inventory: &Inventory,
    changed_paths: &[PathBuf],
) -> Result<Vec<SourceFile>, String> {
    if changed_paths.is_empty() {
        return Ok(inventory.sources.clone());
    }
    let mut selected = Vec::new();
    for requested in changed_paths {
        let candidate = if requested.is_absolute() {
            requested.clone()
        } else {
            root.join(requested)
        };
        let canonical = candidate.canonicalize().map_err(|error| {
            format!(
                "changed path {} is unavailable: {error}",
                requested.display()
            )
        })?;
        let relative = canonical
            .strip_prefix(root)
            .map_err(|_| {
                format!(
                    "changed path {} is outside workspace root {}",
                    requested.display(),
                    root.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let source = inventory
            .sources
            .iter()
            .find(|source| source.path == canonical || source.relative == relative)
            .ok_or_else(|| {
                format!(
                    "changed path {} is not an inventoried MNCS source",
                    requested.display()
                )
            })?;
        if !selected
            .iter()
            .any(|item: &SourceFile| item.relative == source.relative)
        {
            selected.push(source.clone());
        }
    }
    selected.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(selected)
}

fn make_scoped_inventory(inventory: &Inventory, sources: &[SourceFile]) -> Inventory {
    let mut scoped = inventory.clone();
    scoped.sources = sources.to_vec();
    scoped
}

fn add_scope_note(
    notes: &mut Vec<String>,
    changed_paths: &[PathBuf],
    selected: usize,
    available: usize,
) {
    if changed_paths.is_empty() {
        notes.push(format!("scope: repository ({available} source file(s))"));
    } else {
        notes.push(format!(
            "scope: changed surface ({selected} of {available} source file(s)); explicit --changed-path selection"
        ));
    }
}

fn workspace_root(flag: &Option<PathBuf>) -> Result<PathBuf, String> {
    let start = flag.clone().unwrap_or_else(|| PathBuf::from("."));
    find_root(&start).map_err(|e| format!("cannot locate workspace root: {e}"))
}

fn discover_with_mncs_policy(
    root: &std::path::Path,
    options: &DiscoveryOptions,
    policy: &DoctorMncsRuntime,
) -> Result<mncs_doctor::discovery::Inventory, String> {
    discover_with_policy(
        root,
        options,
        &|facts| {
            policy
                .discovery_directory_decision(facts)
                .map_err(|error| error.to_string())
        },
        &|facts| {
            policy
                .discovery_file_class(facts)
                .map_err(|error| error.to_string())
        },
    )
    .map_err(|error| error.to_string())
}

fn discover_topology_with_mncs_policy(
    root: &std::path::Path,
    options: &DiscoveryOptions,
    policy: &DoctorMncsRuntime,
) -> Result<TopologySnapshot, String> {
    discover_topology_with_policy(
        root,
        options,
        &|facts| {
            policy
                .discovery_directory_decision(facts)
                .map_err(|error| error.to_string())
        },
        &|facts| {
            policy
                .discovery_file_class(facts)
                .map_err(|error| error.to_string())
        },
    )
    .map_err(|error| error.to_string())
}

fn cache_path(root: &std::path::Path) -> PathBuf {
    root.join(".mncs/doctor/inventory.json")
}

fn policy_identity(policy: &DoctorMncsRuntime) -> Result<String, String> {
    let mut provenance = policy
        .provenance()
        .map_err(|error| format!("MNCS provenance unavailable: {error}"))?;
    // Entrypoints are run-session observations, not discovery-policy
    // semantics.  Excluding them keeps a cache reusable across processes
    // while the report still records the complete dynamic provenance.
    provenance.entrypoints.clear();
    let bytes = serde_json::to_vec(&provenance)
        .map_err(|error| format!("cannot encode policy provenance: {error}"))?;
    Ok(mncs_doctor::discovery::fingerprint(&bytes))
}

fn options_identity(options: &DiscoveryOptions) -> Result<String, String> {
    let bytes = serde_json::to_vec(options)
        .map_err(|error| format!("cannot encode discovery options: {error}"))?;
    Ok(mncs_doctor::discovery::fingerprint(&bytes))
}

fn inventory_identity(inventory: &Inventory) -> Result<String, String> {
    let bytes = serde_json::to_vec(inventory)
        .map_err(|error| format!("cannot encode inventory identity: {error}"))?;
    Ok(mncs_doctor::discovery::fingerprint(&bytes))
}

fn source_metadata(path: &std::path::Path) -> Result<SourceMetadata, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot stat source {}: {error}", path.display()))?;
    let modified = metadata.modified().ok().and_then(|value| {
        value
            .duration_since(UNIX_EPOCH)
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

fn inventory_source_metadata(
    inventory: &Inventory,
) -> Result<BTreeMap<String, SourceMetadata>, String> {
    inventory
        .sources
        .iter()
        .map(|source| Ok((source.relative.clone(), source_metadata(&source.path)?)))
        .collect()
}

fn write_inventory_cache(
    root: &std::path::Path,
    inventory: &Inventory,
    options: &DiscoveryOptions,
    policy: &DoctorMncsRuntime,
) -> Result<String, String> {
    let identity = inventory_identity(inventory)?;
    let topology_identity = inventory.topology.identity();
    let cache = InventoryCache {
        schema_version: INVENTORY_CACHE_SCHEMA.to_owned(),
        root: root.to_path_buf(),
        options_identity: options_identity(options)?,
        policy_identity: policy_identity(policy)?,
        inventory_identity: identity.clone(),
        topology_identity,
        source_metadata: inventory_source_metadata(inventory)?,
        inventory: inventory.clone(),
    };
    let path = cache_path(root);
    let parent = path
        .parent()
        .ok_or_else(|| format!("inventory cache has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create inventory cache directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&cache)
        .map_err(|error| format!("cannot encode inventory cache: {error}"))?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write inventory cache: {error}"))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot install inventory cache: {error}"))?;
    Ok(identity)
}

fn load_inventory_cache(
    root: &std::path::Path,
    options: &DiscoveryOptions,
    policy: &DoctorMncsRuntime,
    changed_paths: &[PathBuf],
) -> Result<(InventoryCache, TopologySnapshot, Vec<String>, bool), String> {
    let path = cache_path(root);
    let bytes = fs::read(&path).map_err(|error| format!("cannot read inventory cache: {error}"))?;
    let cache: InventoryCache = serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot decode inventory cache: {error}"))?;
    if cache.schema_version != INVENTORY_CACHE_SCHEMA {
        return Err("inventory cache schema is unsupported".to_owned());
    }
    if cache.root != root {
        return Err("inventory cache root does not match the workspace".to_owned());
    }
    if cache.options_identity != options_identity(options)? {
        return Err("discovery options changed".to_owned());
    }
    if cache.policy_identity != policy_identity(policy)? {
        return Err("MNCS discovery policy changed".to_owned());
    }
    if cache.inventory_identity != inventory_identity(&cache.inventory)? {
        return Err("inventory cache identity is invalid".to_owned());
    }
    if cache.topology_identity != cache.inventory.topology.identity() {
        return Err("topology identity is invalid".to_owned());
    }
    let current_topology = discover_topology_with_mncs_policy(root, options, policy)?;
    let topology_changed = cache.topology_identity != current_topology.identity();
    let requested: std::collections::BTreeSet<String> = changed_paths
        .iter()
        .filter_map(|path| {
            let candidate = if path.is_absolute() {
                path.clone()
            } else {
                root.join(path)
            };
            candidate.canonicalize().ok().and_then(|value| {
                value
                    .strip_prefix(root)
                    .ok()
                    .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            })
        })
        .collect();
    let mut stale = Vec::new();
    for source in &cache.inventory.sources {
        let expected = cache
            .source_metadata
            .get(&source.relative)
            .ok_or_else(|| format!("source metadata is missing for {}", source.relative))?;
        if current_topology
            .sources
            .iter()
            .any(|candidate| candidate.relative == source.relative)
        {
            if let Ok(current) = source_metadata(&source.path) {
                if &current != expected && !requested.contains(&source.relative) {
                    stale.push(source.relative.clone());
                }
            } else if !topology_changed {
                stale.push(source.relative.clone());
            }
        }
    }
    Ok((cache, current_topology, stale, topology_changed))
}

/// Use the persisted digest-bound inventory for a changed-path request. A
/// missing or invalid cache deliberately falls back to one full discovery and
/// records the invalidation reason in the report; it is never trusted blindly.
fn discover_incremental_with_mncs_policy(
    root: &std::path::Path,
    options: &DiscoveryOptions,
    policy: &DoctorMncsRuntime,
    changed_paths: &[PathBuf],
) -> Result<Inventory, String> {
    if changed_paths.is_empty() {
        let mut inventory = discover_with_mncs_policy(root, options, policy)?;
        let identity = write_inventory_cache(root, &inventory, options, policy)?;
        inventory.metrics.cache_identity = Some(identity);
        inventory.metrics.topology_identity = Some(inventory.topology.identity());
        inventory.metrics.invalidation_reason = Some("repository_scan_requested".to_owned());
        return Ok(inventory);
    }

    let (cache, topology, stale, topology_changed) =
        match load_inventory_cache(root, options, policy, changed_paths) {
            Ok(cache) => cache,
            Err(reason) => {
                let mut inventory = discover_with_mncs_policy(root, options, policy)?;
                let identity = write_inventory_cache(root, &inventory, options, policy)?;
                inventory.metrics.cache_identity = Some(identity);
                inventory.metrics.invalidation_reason = Some(format!("cache_invalid:{reason}"));
                return Ok(inventory);
            }
        };

    let requested: std::collections::BTreeSet<String> = changed_paths
        .iter()
        .filter_map(|path| {
            let candidate = if path.is_absolute() {
                path.clone()
            } else {
                root.join(path)
            };
            candidate.canonicalize().ok().and_then(|value| {
                value
                    .strip_prefix(root)
                    .ok()
                    .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            })
        })
        .collect();
    let stale: std::collections::BTreeSet<String> = stale.into_iter().collect();
    let cached_by_relative: BTreeMap<String, SourceFile> = cache
        .inventory
        .sources
        .into_iter()
        .map(|source| (source.relative.clone(), source))
        .collect();
    let mut sources = Vec::new();
    let mut rescanned = 0usize;
    let mut reused = 0usize;
    for candidate in &topology.sources {
        let should_rescan = requested.contains(&candidate.relative)
            || stale.contains(&candidate.relative)
            || !cached_by_relative.contains_key(&candidate.relative);
        if should_rescan {
            let path = root.join(&candidate.relative);
            let refreshed = match read_source_file(root, &path, candidate.is_symlink) {
                Ok(source) => source,
                Err(error) => {
                    let mut inventory = discover_with_mncs_policy(root, options, policy)?;
                    let identity = write_inventory_cache(root, &inventory, options, policy)?;
                    inventory.metrics.cache_identity = Some(identity);
                    inventory.metrics.invalidation_reason = Some(format!(
                        "topology_changed:cannot rescan {}: {error}",
                        candidate.relative
                    ));
                    return Ok(inventory);
                }
            };
            sources.push(refreshed);
            rescanned += 1;
        } else if let Some(mut source) = cached_by_relative.get(&candidate.relative).cloned() {
            source.path = root.join(&candidate.relative);
            source.is_symlink = candidate.is_symlink;
            sources.push(source);
            reused += 1;
        }
    }
    sources.sort_by(|a, b| a.relative.cmp(&b.relative));
    let topology_identity = topology.identity();
    let mut inventory = Inventory {
        root: root.to_path_buf(),
        sources,
        manifests: topology.manifests.clone(),
        skipped: topology.skipped.clone(),
        extension_counts: topology.extension_counts.clone(),
        topology,
        metrics: Default::default(),
    };
    inventory.metrics.files_reused = reused;
    inventory.metrics.files_rescanned = rescanned;
    inventory.metrics.directories_revalidated = inventory.topology.directories.len();
    inventory.metrics.topology_reused = !topology_changed;
    inventory.metrics.topology_invalidated = topology_changed;
    inventory.metrics.topology_identity = Some(topology_identity);
    inventory.metrics.invalidation_reason = if topology_changed {
        Some("topology_invalidated".to_owned())
    } else if !stale.is_empty() {
        Some(format!(
            "unreported_source_metadata_changed:{}",
            stale.iter().cloned().collect::<Vec<_>>().join(",")
        ))
    } else {
        Some("changed_path".to_owned())
    };
    let identity = write_inventory_cache(root, &inventory, options, policy)?;
    inventory.metrics.cache_identity = Some(identity);
    Ok(inventory)
}

fn diagnose_all_with_mncs_policy(
    sources: &[SourceFile],
    with_backend: bool,
    policy: &DoctorMncsRuntime,
) -> Result<BTreeMap<String, Vec<mncs_doctor::diagnostics::Diagnostic>>, String> {
    let rust_backend = if with_backend {
        find_rust_cli().map(|exe| RustCliBackend { exe })
    } else {
        None
    };
    let mut map = BTreeMap::new();
    for file in sources {
        let mut diags = diagnose_one_with_mncs_policy(file, policy)?;
        if let Some(backend) = &rust_backend {
            diags.extend(backend.diagnose(file));
        }
        diags.sort_by(|a, b| {
            (a.span.start, a.span.end, &a.code).cmp(&(b.span.start, b.span.end, &b.code))
        });
        map.insert(file.relative.clone(), diags);
    }
    Ok(map)
}

fn diagnose_one_with_mncs_policy(
    file: &SourceFile,
    policy: &DoctorMncsRuntime,
) -> Result<Vec<Diagnostic>, String> {
    let scanner = ScannerBackend;
    let scan = policy
        .scan_bytes(&file.bytes)
        .map_err(|error| format!("MNCS byte scanner failed for {}: {error}", file.relative))?;
    let mut scanned_file = file.clone();
    scanned_file.has_bom = scan.has_bom;
    scanned_file.newline = scan.newline;
    scanner
        .diagnose_with_version_classifier(&scanned_file, |version| {
            policy
                .classify_version(version)
                .map_err(|error| error.to_string())
        })
        .map_err(|error| {
            format!(
                "MNCS diagnostic policy failed for {}: {error}",
                file.relative
            )
        })
}

fn emit(report: &Report, json: bool, quiet: bool) {
    if json {
        println!("{}", report.to_json());
    } else if !quiet {
        print!("{}", render_human(report, report_print_explain(report)));
    }
}

fn report_print_explain(report: &Report) -> bool {
    // `--explain` is recorded on the report via notes marker.
    report.notes.iter().any(|n| n == "explain")
}

fn migration_policy_check(unmigratable: &[String], fully_known: bool) -> CheckResult {
    let (status, title) = if !unmigratable.is_empty() {
        (Status::Fail, "Migration path has unplannable files")
    } else if !fully_known {
        (Status::Warning, "Migration path contains unknown edges")
    } else {
        (Status::Pass, "Migration path is fully known")
    };
    CheckResult {
        id: "migration-path".to_owned(),
        title: title.to_owned(),
        status,
        findings: Vec::new(),
    }
}

fn cmd_doctor(args: &[String]) -> Result<ExitCode, String> {
    let mut rest = Vec::new();
    let flags = parse_globals(args, 0, &mut rest)?;
    let mut check_only = false;
    for arg in &rest {
        match arg.as_str() {
            "--check" => check_only = true,
            _ => return Err(format!("doctor: unexpected argument {arg:?}")),
        }
    }
    let _ = check_only; // doctor never mutates; --check selects CI-oriented wording
    let policy = DoctorMncsRuntime::production()
        .map_err(|error| format!("MNCS policy runtime unavailable (fail-closed): {error}"))?;
    let root = workspace_root(&flags.root)?;
    let options = DiscoveryOptions::default();
    let inventory =
        discover_incremental_with_mncs_policy(&root, &options, policy, &flags.changed_paths)
            .map_err(|error| format!("discovery failed: {error}"))?;
    let sources = scoped_sources(&root, &inventory, &flags.changed_paths)?;
    let scoped_inventory = make_scoped_inventory(&inventory, &sources);
    let diags = diagnose_all_with_mncs_policy(&sources, flags.with_language_backend, policy)?;
    let toolchain = probe_toolchain_at(Some(&root));
    let ctx = HealthContext {
        inventory: &scoped_inventory,
        diagnostics: &diags,
        toolchain: &toolchain,
    };
    let mut checks = run_all_checks(&ctx);
    policy
        .apply_health_policy(&mut checks)
        .map_err(|error| format!("MNCS health policy failed (fail-closed): {error}"))?;
    let review_blocked = diags.values().flatten().any(|d| {
        matches!(
            d.applicability,
            mncs_doctor::fix::Applicability::Review | mncs_doctor::fix::Applicability::Manual
        ) && matches!(
            d.severity,
            mncs_doctor::diagnostics::Severity::Error | mncs_doctor::diagnostics::Severity::Warning
        )
    });
    let code = policy
        .exit_for(&checks, review_blocked, None)
        .map_err(|error| format!("MNCS report policy failed (fail-closed): {error}"))?;
    let mut report = Report::new("doctor", root.to_string_lossy());
    report.inventory = Some(scoped_inventory.summary());
    report.checks = checks;
    report.file_diagnostics = diags;
    report.toolchain = Some(toolchain);
    report.policy = Some(
        policy
            .provenance()
            .map_err(|error| format!("MNCS provenance unavailable (fail-closed): {error}"))?,
    );
    report.exit_code = code.as_i32();
    report.exit_meaning = exit_meaning(code).to_owned();
    if flags.explain {
        report.notes.push("explain".to_owned());
    }
    add_scope_note(
        &mut report.notes,
        &flags.changed_paths,
        sources.len(),
        inventory.sources.len(),
    );
    if flags.verbose {
        report.notes.push(format!(
            "scanned with {} backend(s)",
            if flags.with_language_backend {
                "scanner+rust-cli"
            } else {
                "scanner"
            }
        ));
    }
    emit(&report, flags.json, flags.quiet);
    Ok(code)
}

fn cmd_fix(args: &[String]) -> Result<ExitCode, String> {
    let mut rest = Vec::new();
    let flags = parse_globals(args, 0, &mut rest)?;
    let mut dry_run = false;
    let mut safe_only = true;
    let mut verify_cmd: Option<Vec<String>> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--dry-run" => dry_run = true,
            "--safe-only" => safe_only = true,
            "--proven" => safe_only = false,
            "--verify-cmd" => {
                i += 1;
                let cmd = rest.get(i).ok_or("--verify-cmd requires a value")?;
                verify_cmd = Some(cmd.split_whitespace().map(str::to_owned).collect());
            }
            _ => return Err(format!("fix: unexpected argument {:?}", rest[i])),
        }
        i += 1;
    }
    let eligibility = if safe_only {
        Eligibility::safe_only()
    } else {
        Eligibility::with_proven()
    };
    let policy = DoctorMncsRuntime::production()
        .map_err(|error| format!("MNCS policy runtime unavailable (fail-closed): {error}"))?;
    let root = workspace_root(&flags.root)?;
    let inventory = discover_incremental_with_mncs_policy(
        &root,
        &DiscoveryOptions::default(),
        policy,
        &flags.changed_paths,
    )
    .map_err(|error| format!("discovery failed: {error}"))?;
    let sources = scoped_sources(&root, &inventory, &flags.changed_paths)?;
    let scoped_inventory = make_scoped_inventory(&inventory, &sources);
    let diags = diagnose_all_with_mncs_policy(&sources, flags.with_language_backend, policy)?;
    let providers = providers_for_root(&root)?;
    let workspace = plan_workspace_with_mncs_policy(&sources, &diags, &providers, eligibility)
        .map_err(|error| format!("MNCS fix planning failed (fail-closed): {error}"))?;
    let plans = workspace.plans;
    let blocked_review = workspace.blocked_review;
    let blocked_manual = workspace.blocked_manual;

    let mut report = Report::new("fix", root.to_string_lossy());
    report.inventory = Some(scoped_inventory.summary());
    report.notes.extend(workspace.conflict_notes);
    add_scope_note(
        &mut report.notes,
        &flags.changed_paths,
        sources.len(),
        inventory.sources.len(),
    );
    report.file_diagnostics = diags.clone();
    report.toolchain = Some(probe_toolchain());
    for (file, plan) in &plans {
        for fix in &plan.fixes {
            report.fix_identities.push(fix.provider.clone());
            for address in &fix.addresses {
                if let Some(identity) = address.strip_prefix("MNCS-MIGRATION-") {
                    report.migration_rule_identities.push(identity.to_owned());
                }
            }
        }
        let base = file.text.as_deref().unwrap_or("");
        let next = union_apply_with_mncs(plan, base).map_err(|error| {
            format!(
                "MNCS fix plan failed for {} (fail-closed): {error}",
                file.relative
            )
        })?;
        report
            .planned_diffs
            .push(summarize_diff(&file.relative, base, &next));
    }
    report.fix_identities.sort();
    report.fix_identities.dedup();
    report.migration_rule_identities.sort();
    report.migration_rule_identities.dedup();

    if dry_run {
        report.notes.push(format!(
            "dry-run: {} file(s) would change; no mutation performed",
            plans.len()
        ));
        if flags.explain {
            report.notes.push("explain".to_owned());
        }
        let toolchain = probe_toolchain();
        let ctx = HealthContext {
            inventory: &scoped_inventory,
            diagnostics: &report.file_diagnostics,
            toolchain: &toolchain,
        };
        report.checks = run_all_checks(&ctx);
        policy
            .apply_health_policy(&mut report.checks)
            .map_err(|error| format!("MNCS health policy failed (fail-closed): {error}"))?;
        let code = policy
            .exit_for(&report.checks, blocked_review + blocked_manual > 0, None)
            .map_err(|error| format!("MNCS report policy failed (fail-closed): {error}"))?;
        report.policy = Some(
            policy
                .provenance()
                .map_err(|error| format!("MNCS provenance unavailable (fail-closed): {error}"))?,
        );
        report.exit_code = code.as_i32();
        report.exit_meaning = exit_meaning(code).to_owned();
        emit(&report, flags.json, flags.quiet);
        return Ok(code);
    }

    // Apply via transaction.
    let mut tx = Transaction::new();
    for (file, plan) in &plans {
        let base = file.text.as_deref().unwrap_or("");
        let next = union_apply_with_mncs(plan, base).map_err(|error| {
            format!(
                "MNCS fix plan failed for {} (fail-closed): {error}",
                file.relative
            )
        })?;
        tx.push(FileOp {
            relative: file.relative.clone(),
            old_fingerprint: Some(file.sha256.clone()),
            new_bytes: next.into_bytes(),
            mode: file.mode,
        });
    }
    let commit: Option<CommitReport> = if tx.is_empty() {
        None
    } else {
        Some(
            tx.commit(&root)
                .map_err(|e| format!("transaction failed: {e}"))?,
        )
    };
    if let Some(c) = &commit {
        report.notes.push(format!(
            "applied: {} written, {} skipped (identical)",
            c.written.len(),
            c.skipped_identical.len()
        ));
    } else {
        report.notes.push("nothing to apply".to_owned());
    }
    if flags.explain {
        report.notes.push("explain".to_owned());
    }

    // Re-read and verify: full convergence check + re-diagnose.
    let fresh = discover_incremental_with_mncs_policy(
        &root,
        &DiscoveryOptions::default(),
        policy,
        &flags.changed_paths,
    )
    .map_err(|error| format!("re-discovery failed: {error}"))?;
    let fresh_sources = scoped_sources(&root, &fresh, &flags.changed_paths)?;
    let fresh_inventory = make_scoped_inventory(&fresh, &fresh_sources);
    let fresh_diags =
        diagnose_all_with_mncs_policy(&fresh_sources, flags.with_language_backend, policy)?;
    // Convergence evidence: re-running the full repair loop over the
    // committed tree must reach an immediate fixpoint with nothing applied.
    let mut idempotent = true;
    let converged_providers = providers_for_root(&root)?;
    for file in &fresh_sources {
        if file.text.is_some() {
            let (_, conv) = repair_to_fixpoint_with_diagnose(
                file,
                &converged_providers,
                eligibility,
                &|planned_empty, fired_before, iterations, budget| {
                    policy
                        .fix_stop_rule(planned_empty, fired_before, iterations, budget)
                        .map_err(|error| error.to_string())
                },
                &|fired, id| {
                    policy
                        .fix_seen_before(fired, id)
                        .map_err(|error| error.to_string())
                },
                &|current| diagnose_one_with_mncs_policy(current, policy),
            )
            .map_err(|error| format!("MNCS fix policy failed (fail-closed): {error}"))?;
            if !conv.applied.is_empty() || conv.stopped != mncs_doctor::fix::StopReason::Fixpoint {
                idempotent = false;
            }
            report.convergence.push(conv);
        }
    }
    report
        .convergence
        .sort_by(|a, b| a.relative.cmp(&b.relative));
    let external: Vec<ExternalCheck> = verify_cmd
        .map(|cmd| vec![run_external(&cmd, &root)])
        .unwrap_or_default();
    let mut verification = verify_after_with_diagnostics(
        fresh_sources.len(),
        &diags,
        &fresh_diags,
        Some(idempotent),
        external,
    );
    let external_pass = verification.external.iter().all(|check| check.success);
    verification.passed = policy
        .verify_compose(
            verification.errors_before,
            verification.errors_after,
            verification.idempotent.unwrap_or(true),
            external_pass,
        )
        .map_err(|error| format!("MNCS verification policy failed (fail-closed): {error}"))?;
    report.verification = Some(verification);
    let toolchain = probe_toolchain();
    let ctx = HealthContext {
        inventory: &fresh_inventory,
        diagnostics: &fresh_diags,
        toolchain: &toolchain,
    };
    report.checks = run_all_checks(&ctx);
    report.toolchain = Some(toolchain);
    policy
        .apply_health_policy(&mut report.checks)
        .map_err(|error| format!("MNCS health policy failed (fail-closed): {error}"))?;
    let code = policy
        .exit_for(
            &report.checks,
            blocked_review + blocked_manual > 0,
            report.verification.as_ref(),
        )
        .map_err(|error| format!("MNCS report policy failed (fail-closed): {error}"))?;
    report.policy = Some(
        policy
            .provenance()
            .map_err(|error| format!("MNCS provenance unavailable (fail-closed): {error}"))?,
    );
    report.exit_code = code.as_i32();
    report.exit_meaning = exit_meaning(code).to_owned();
    emit(&report, flags.json, flags.quiet);
    Ok(code)
}

fn cmd_migrate(args: &[String]) -> Result<ExitCode, String> {
    let mut rest = Vec::new();
    let flags = parse_globals(args, 0, &mut rest)?;
    let mut target: Option<String> = None;
    let mut dry_run = false;
    let mut show_plan = false;
    let mut apply = false;
    let mut allow_review = false;
    let mut registry_path: Option<PathBuf> = None;
    let mut verify_cmd: Option<Vec<String>> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--to" => {
                i += 1;
                target = Some(rest.get(i).ok_or("--to requires a value")?.clone());
            }
            "--dry-run" => dry_run = true,
            "--plan" => show_plan = true,
            "--apply" => apply = true,
            "--check" => {}
            "--allow-review" => allow_review = true,
            "--registry" => {
                i += 1;
                registry_path = Some(PathBuf::from(
                    rest.get(i).ok_or("--registry requires a value")?,
                ));
            }
            "--verify-cmd" => {
                i += 1;
                let cmd = rest.get(i).ok_or("--verify-cmd requires a value")?;
                verify_cmd = Some(cmd.split_whitespace().map(str::to_owned).collect());
            }
            _ => return Err(format!("migrate: unexpected argument {:?}", rest[i])),
        }
        i += 1;
    }
    let target_raw = target.ok_or("migrate requires --to <version|latest>")?;
    let target_version =
        resolve_target(&target_raw).map_err(|e| format!("bad migration target: {e}"))?;
    let policy = DoctorMncsRuntime::production()
        .map_err(|error| format!("MNCS policy runtime unavailable (fail-closed): {error}"))?;
    let mut registry = default_registry();
    if let Some(path) = &registry_path {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read registry {}: {e}", path.display()))?;
        let extra = mncs_doctor::migration::load_registry_file(&json)
            .map_err(|e| format!("bad registry file: {e}"))?;
        for transition in extra {
            registry.insert(transition);
        }
    }
    let root = workspace_root(&flags.root)?;
    let inventory = discover_incremental_with_mncs_policy(
        &root,
        &DiscoveryOptions::default(),
        policy,
        &flags.changed_paths,
    )
    .map_err(|error| format!("discovery failed: {error}"))?;
    let sources = scoped_sources(&root, &inventory, &flags.changed_paths)?;
    let scoped_inventory = make_scoped_inventory(&inventory, &sources);
    let diags = diagnose_all_with_mncs_policy(&sources, flags.with_language_backend, policy)?;

    // Per-file: declared version -> plan to target.
    let mut report = Report::new("migrate", root.to_string_lossy());
    report.inventory = Some(scoped_inventory.summary());
    report.file_diagnostics = diags.clone();
    report.toolchain = Some(probe_toolchain());
    add_scope_note(
        &mut report.notes,
        &flags.changed_paths,
        sources.len(),
        inventory.sources.len(),
    );
    let mut file_plans: Vec<(SourceFile, mncs_doctor::migration::MigrationPlan)> = Vec::new();
    let mut unmigratable: Vec<String> = Vec::new();
    for file in &sources {
        let declared = file
            .text
            .as_deref()
            .map(scan_header)
            .and_then(|facts| facts.declared);
        let Some(from) = declared else {
            unmigratable.push(format!(
                "{}: no declared version (add a header first)",
                file.relative
            ));
            continue;
        };
        match plan_migration(from, target_version, &registry) {
            Ok(plan) => {
                let kinds: Vec<TransitionKind> = plan.steps.iter().map(|step| step.kind).collect();
                let observed = policy
                    .migration_plan_verdict(from, target_version, &kinds)
                    .map_err(|error| {
                        format!("MNCS migration policy failed (fail-closed): {error}")
                    })?;
                let expected = if plan.is_noop {
                    MigrationVerdict::Noop
                } else if !plan.fully_known {
                    MigrationVerdict::Blocked
                } else {
                    MigrationVerdict::Planned
                };
                if observed != expected {
                    return Err(format!(
                        "MNCS/Rust migration verdict mismatch for {}: MNCS={observed:?}, reference={expected:?}",
                        file.relative
                    ));
                }
                file_plans.push((file.clone(), plan));
            }
            Err(e) => unmigratable.push(format!("{}: {e}", file.relative)),
        }
    }
    file_plans.sort_by(|a, b| a.0.relative.cmp(&b.0.relative));
    unmigratable.sort();
    for entry in &unmigratable {
        report.notes.push(format!("unmigratable: {entry}"));
    }
    // Plan display always computed (inspect/plan/dry-run/apply share it).
    let fully_known = file_plans.iter().all(|(_, p)| p.fully_known);
    let noop_all = file_plans.iter().all(|(_, p)| p.is_noop);
    report.notes.push(format!(
        "migration path: {} file(s) planned, target {target_version}, fully_known={fully_known}, all_noop={noop_all}",
        file_plans.len()
    ));
    if show_plan || !apply {
        for (file, plan) in &file_plans {
            if plan.steps.is_empty() {
                report.notes.push(format!(
                    "{}: already at {target_version} (no-op)",
                    file.relative
                ));
            } else {
                for step in &plan.steps {
                    report.notes.push(format!(
                        "{}: {} [{}] {} ({})",
                        file.relative, step.transition, step.kind, step.title, step.provenance
                    ));
                }
            }
        }
    }
    if !apply {
        // Dry-run diffs for fully-known plans only; unknown edges refuse.
        for (file, plan) in &file_plans {
            if !plan.fully_known {
                continue;
            }
            let base = file.text.as_deref().unwrap_or("");
            match apply_plan(&file.relative, base, plan, &registry, allow_review) {
                Ok((next, _)) => {
                    if next != base {
                        report
                            .planned_diffs
                            .push(summarize_diff(&file.relative, base, &next));
                    }
                }
                Err(e) => report
                    .notes
                    .push(format!("{}: dry-run blocked: {e}", file.relative)),
            }
        }
        if !dry_run && !show_plan {
            report.notes.push(
                "no mutation performed (pass --apply to migrate; --dry-run shows diffs)".to_owned(),
            );
        }
        if flags.explain {
            report.notes.push("explain".to_owned());
        }
        let migration_check = migration_policy_check(&unmigratable, fully_known);
        report.checks.push(migration_check.clone());
        let code = policy
            .exit_for(&[migration_check], !fully_known, None)
            .map_err(|error| format!("MNCS report policy failed (fail-closed): {error}"))?;
        report.policy = Some(
            policy
                .provenance()
                .map_err(|error| format!("MNCS provenance unavailable (fail-closed): {error}"))?,
        );
        let code = if code == ExitCode::Healthy && !unmigratable.is_empty() {
            ExitCode::Findings
        } else {
            code
        };
        report.exit_code = code.as_i32();
        report.exit_meaning = exit_meaning(code).to_owned();
        emit(&report, flags.json, flags.quiet);
        return Ok(code);
    }

    // Apply path.
    let mut tx = Transaction::new();
    let mut records = Vec::new();
    for (file, plan) in &file_plans {
        if plan.is_noop {
            continue;
        }
        let base = file.text.as_deref().unwrap_or("");
        match apply_plan(&file.relative, base, plan, &registry, allow_review) {
            Ok((next, record)) => {
                if next != base {
                    tx.push(FileOp {
                        relative: file.relative.clone(),
                        old_fingerprint: Some(file.sha256.clone()),
                        new_bytes: next.into_bytes(),
                        mode: file.mode,
                    });
                    records.push(record);
                }
            }
            Err(e) => {
                return Err(format!("migration blocked for {}: {e}", file.relative));
            }
        }
    }
    if !tx.is_empty() {
        tx.commit(&root)
            .map_err(|e| format!("transaction failed: {e}"))?;
    }
    report.migrations = records;
    report.notes.push(format!(
        "applied {} migration record(s) to target {target_version}",
        report.migrations.len()
    ));
    if flags.explain {
        report.notes.push("explain".to_owned());
    }
    let fresh = discover_incremental_with_mncs_policy(
        &root,
        &DiscoveryOptions::default(),
        policy,
        &flags.changed_paths,
    )
    .map_err(|error| format!("re-discovery failed: {error}"))?;
    let fresh_sources = scoped_sources(&root, &fresh, &flags.changed_paths)?;
    let fresh_inventory = make_scoped_inventory(&fresh, &fresh_sources);
    let fresh_diags =
        diagnose_all_with_mncs_policy(&fresh_sources, flags.with_language_backend, policy)?;
    report.file_diagnostics = fresh_diags.clone();
    let fresh_toolchain = probe_toolchain();
    report.checks = run_all_checks(&HealthContext {
        inventory: &fresh_inventory,
        diagnostics: &fresh_diags,
        toolchain: &fresh_toolchain,
    });
    report.toolchain = Some(fresh_toolchain);
    // Idempotence: re-planning migrated files must yield no-op plans.
    let mut idempotent = true;
    for file in &fresh_sources {
        let declared = file
            .text
            .as_deref()
            .map(scan_header)
            .and_then(|facts| facts.declared);
        if let Some(from) = declared {
            if let Ok(replan) = plan_migration(from, target_version, &registry) {
                if !replan.is_noop {
                    idempotent = false;
                }
            }
        }
    }
    let external: Vec<ExternalCheck> = verify_cmd
        .map(|cmd| vec![run_external(&cmd, &root)])
        .unwrap_or_default();
    let mut verification = verify_after_with_diagnostics(
        fresh_sources.len(),
        &diags,
        &fresh_diags,
        Some(idempotent),
        external,
    );
    let external_pass = verification.external.iter().all(|check| check.success);
    verification.passed = policy
        .verify_compose(
            verification.errors_before,
            verification.errors_after,
            verification.idempotent.unwrap_or(true),
            external_pass,
        )
        .map_err(|error| format!("MNCS verification policy failed (fail-closed): {error}"))?;
    report.verification = Some(verification);
    policy
        .apply_health_policy(&mut report.checks)
        .map_err(|error| format!("MNCS health policy failed (fail-closed): {error}"))?;
    let migration_check = migration_policy_check(&unmigratable, fully_known);
    report.checks.push(migration_check.clone());
    let code = policy
        .exit_for(
            &[migration_check],
            !fully_known,
            report.verification.as_ref(),
        )
        .map_err(|error| format!("MNCS report policy failed (fail-closed): {error}"))?;
    let code = if code == ExitCode::Healthy && !unmigratable.is_empty() {
        ExitCode::Findings
    } else {
        code
    };
    report.policy = Some(
        policy
            .provenance()
            .map_err(|error| format!("MNCS provenance unavailable (fail-closed): {error}"))?,
    );
    report.exit_code = code.as_i32();
    report.exit_meaning = exit_meaning(code).to_owned();
    emit(&report, flags.json, flags.quiet);
    Ok(code)
}

fn cmd_verify(args: &[String]) -> Result<ExitCode, String> {
    let mut rest = Vec::new();
    let flags = parse_globals(args, 0, &mut rest)?;
    let mut verify_cmd: Option<Vec<String>> = None;
    for arg in &rest {
        if let Some(cmd) = arg.strip_prefix("--verify-cmd=") {
            verify_cmd = Some(cmd.split_whitespace().map(str::to_owned).collect());
        } else {
            return Err(format!("verify: unexpected argument {arg:?}"));
        }
    }
    let policy = DoctorMncsRuntime::production()
        .map_err(|error| format!("MNCS policy runtime unavailable (fail-closed): {error}"))?;
    let root = workspace_root(&flags.root)?;
    let inventory = discover_incremental_with_mncs_policy(
        &root,
        &DiscoveryOptions::default(),
        policy,
        &flags.changed_paths,
    )
    .map_err(|error| format!("discovery failed: {error}"))?;
    let sources = scoped_sources(&root, &inventory, &flags.changed_paths)?;
    let scoped_inventory = make_scoped_inventory(&inventory, &sources);
    let diags = diagnose_all_with_mncs_policy(&sources, flags.with_language_backend, policy)?;
    let external: Vec<ExternalCheck> = verify_cmd
        .map(|cmd| vec![run_external(&cmd, &root)])
        .unwrap_or_default();
    // Standalone verify judges the current tree against itself: the verdict
    // is about *new* breakage, so the pre-map is the present diagnosis.
    let mut verification =
        verify_after_with_diagnostics(sources.len(), &diags, &diags, None, external);
    let external_pass = verification.external.iter().all(|check| check.success);
    verification.passed = policy
        .verify_compose(
            verification.errors_before,
            verification.errors_after,
            true,
            external_pass,
        )
        .map_err(|error| format!("MNCS verification policy failed (fail-closed): {error}"))?;
    // Standalone verify passes on zero errors; pre-existing diagnostics are
    // reported as findings, not as verification failure, unless errors exist.
    let errors: usize = diags
        .values()
        .flatten()
        .filter(|d| matches!(d.severity, mncs_doctor::diagnostics::Severity::Error))
        .count();
    let mut report = Report::new("verify", root.to_string_lossy());
    report.inventory = Some(scoped_inventory.summary());
    report.file_diagnostics = diags;
    report.toolchain = Some(probe_toolchain());
    report.verification = Some(verification);
    if flags.explain {
        report.notes.push("explain".to_owned());
    }
    add_scope_note(
        &mut report.notes,
        &flags.changed_paths,
        sources.len(),
        inventory.sources.len(),
    );
    let code = policy
        .exit_for(&[], false, report.verification.as_ref())
        .map_err(|error| format!("MNCS report policy failed (fail-closed): {error}"))?;
    let code = if code == ExitCode::Healthy && errors > 0 {
        ExitCode::Findings
    } else {
        code
    };
    report.policy = Some(
        policy
            .provenance()
            .map_err(|error| format!("MNCS provenance unavailable (fail-closed): {error}"))?,
    );
    report.exit_code = code.as_i32();
    report.exit_meaning = exit_meaning(code).to_owned();
    emit(&report, flags.json, flags.quiet);
    Ok(code)
}

fn exit_meaning(code: ExitCode) -> &'static str {
    match code {
        ExitCode::Healthy => "healthy: no actionable findings",
        ExitCode::Findings => "findings present (repairable or informational)",
        ExitCode::ReviewRequired => "review required: automatic repair is unsafe for some items",
        ExitCode::VerificationFailed => "verification failed after mutation",
        ExitCode::ToolFailure => "internal/tool failure",
    }
}

fn print_help() {
    println!(
        "\
mncs-doctor {DOCTOR_VERSION} — MNCS repository health, repair, and migration

USAGE:
    mncs-doctor <command> [options]

COMMANDS:
    doctor    Inspect repository health (never mutates)
    fix       Plan and apply safe repairs
    migrate   Plan and apply version migrations
    verify    Re-diagnose and run verification commands

GLOBAL OPTIONS:
    --root <dir>              Workspace root (default: auto-detect upward)
    --json                    Machine-readable JSON report
    --explain                 Verbose finding explanations and diff hunks
    --quiet, -q               Suppress human output (use with --json)
    --verbose, -v             Extra provenance notes
    --changed-path <file>     Narrow diagnostics/repair/migration to this MNCS source
                              (repeat for a changed surface; default is repository scope)
    --with-language-backend   Also diagnose via the Rust language CLI
                              (MNCS_CLI or PATH `mncs` with source-study)
    --no-color                Accepted for scripting (output is uncolored)

DOCTOR OPTIONS:
    --check                   CI mode: same checks, nonzero exit on findings

FIX OPTIONS:
    --dry-run                 Show planned changes without mutating
    --safe-only               Apply safe fixes only (default)
    --proven                  Also apply semantically-proven fixes
    --verify-cmd \"<cmd>\"      Run a project verification command after apply

MIGRATE OPTIONS:
    --to <version|latest>     Target language version (required)
    --plan                    Show per-file transition paths
    --dry-run                 Show diffs without mutating
    --apply                   Perform the migration (default without
                              --apply/--plan/--dry-run is plan-only)
    --check                   Accepted; planning is already non-mutating
    --allow-review            Cross review-classified edges (never by default)
    --registry <file>         Load extra transitions from a JSON registry file
    --verify-cmd \"<cmd>\"      Run a project verification command after apply

VERIFY OPTIONS:
    --verify-cmd=<cmd>        External verification command to run

EXIT CODES:
    0  healthy            1  findings present
    2  review required    3  verification failed
    4  internal/tool failure

EXAMPLES:
    mncs-doctor doctor --explain
    mncs-doctor fix --dry-run
    mncs-doctor migrate --to latest --plan
    mncs-doctor verify --verify-cmd=\"cargo test --quiet\""
    );
}
