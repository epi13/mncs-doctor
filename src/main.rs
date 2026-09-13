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
use std::path::PathBuf;
use std::process::ExitCode as ProcExit;

use mncs_doctor::diagnostics::{scan_header, LanguageBackend, ScannerBackend};
use mncs_doctor::discovery::{discover, find_root, DiscoveryOptions, SourceFile};
use mncs_doctor::fix::{
    default_providers, plan_workspace, repair_to_fixpoint, union_apply, Eligibility,
};
use mncs_doctor::health::{run_all_checks, HealthContext};
use mncs_doctor::migration::{
    apply_plan, default_registry, plan as plan_migration, resolve_target,
};
use mncs_doctor::report::{exit_for, render_human, ExitCode, Report};
use mncs_doctor::toolchain::{find_rust_cli, probe_toolchain, RustCliBackend};
use mncs_doctor::transaction::{summarize_diff, CommitReport, FileOp, Transaction};
use mncs_doctor::verify::{run_external, verify_after, ExternalCheck};
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
            "--no-color" => {}
            _ => out.push(args[i].clone()),
        }
        i += 1;
    }
    Ok(flags)
}

fn workspace_root(flag: &Option<PathBuf>) -> Result<PathBuf, String> {
    let start = flag.clone().unwrap_or_else(|| PathBuf::from("."));
    find_root(&start).map_err(|e| format!("cannot locate workspace root: {e}"))
}

fn diagnose_all(
    sources: &[SourceFile],
    with_backend: bool,
) -> BTreeMap<String, Vec<mncs_doctor::diagnostics::Diagnostic>> {
    let scanner = ScannerBackend;
    let rust_backend = if with_backend {
        find_rust_cli().map(|exe| RustCliBackend { exe })
    } else {
        None
    };
    let mut map = BTreeMap::new();
    for file in sources {
        let mut diags = scanner.diagnose(file);
        if let Some(backend) = &rust_backend {
            diags.extend(backend.diagnose(file));
        }
        diags.sort_by(|a, b| {
            (a.span.start, a.span.end, &a.code).cmp(&(b.span.start, b.span.end, &b.code))
        });
        map.insert(file.relative.clone(), diags);
    }
    map
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
    let root = workspace_root(&flags.root)?;
    let inventory = discover(&root, &DiscoveryOptions::default())
        .map_err(|e| format!("discovery failed: {e}"))?;
    let diags = diagnose_all(&inventory.sources, flags.with_language_backend);
    let toolchain = probe_toolchain();
    let ctx = HealthContext {
        inventory: &inventory,
        diagnostics: &diags,
        toolchain: &toolchain,
    };
    let checks = run_all_checks(&ctx);
    let review_blocked = diags.values().flatten().any(|d| {
        matches!(
            d.applicability,
            mncs_doctor::fix::Applicability::Review | mncs_doctor::fix::Applicability::Manual
        ) && matches!(
            d.severity,
            mncs_doctor::diagnostics::Severity::Error | mncs_doctor::diagnostics::Severity::Warning
        )
    });
    let code = exit_for(&checks, review_blocked, None);
    let mut report = Report::new("doctor", root.to_string_lossy());
    report.inventory = Some(inventory.summary());
    report.checks = checks;
    report.file_diagnostics = diags;
    report.toolchain = Some(toolchain);
    report.exit_code = code.as_i32();
    report.exit_meaning = exit_meaning(code).to_owned();
    if flags.explain {
        report.notes.push("explain".to_owned());
    }
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
    let root = workspace_root(&flags.root)?;
    let inventory = discover(&root, &DiscoveryOptions::default())
        .map_err(|e| format!("discovery failed: {e}"))?;
    let diags = diagnose_all(&inventory.sources, flags.with_language_backend);
    let providers = default_providers();
    let workspace = plan_workspace(&inventory.sources, &diags, &providers, eligibility);
    let plans = workspace.plans;
    let blocked_review = workspace.blocked_review;
    let blocked_manual = workspace.blocked_manual;

    let mut report = Report::new("fix", root.to_string_lossy());
    report.inventory = Some(inventory.summary());
    report.notes.extend(workspace.conflict_notes);
    report.file_diagnostics = diags.clone();
    report.toolchain = Some(probe_toolchain());
    for (file, plan) in &plans {
        let base = file.text.as_deref().unwrap_or("");
        let next = union_apply(plan, base)
            .ok_or_else(|| format!("internal error: plan failed for {}", file.relative))?;
        report
            .planned_diffs
            .push(summarize_diff(&file.relative, base, &next));
    }

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
            inventory: &inventory,
            diagnostics: &report.file_diagnostics,
            toolchain: &toolchain,
        };
        report.checks = run_all_checks(&ctx);
        let code = exit_for(&report.checks, blocked_review + blocked_manual > 0, None);
        report.exit_code = code.as_i32();
        report.exit_meaning = exit_meaning(code).to_owned();
        emit(&report, flags.json, flags.quiet);
        return Ok(code);
    }

    // Apply via transaction.
    let mut tx = Transaction::new();
    for (file, plan) in &plans {
        let base = file.text.as_deref().unwrap_or("");
        let next = union_apply(plan, base)
            .ok_or_else(|| format!("internal error: plan failed for {}", file.relative))?;
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
    let fresh = discover(&root, &DiscoveryOptions::default())
        .map_err(|e| format!("re-discovery failed: {e}"))?;
    let fresh_diags = diagnose_all(&fresh.sources, flags.with_language_backend);
    // Convergence evidence: re-running the full repair loop over the
    // committed tree must reach an immediate fixpoint with nothing applied.
    let mut idempotent = true;
    let converged_providers = default_providers();
    for file in &fresh.sources {
        if file.text.is_some() {
            let (_, conv) =
                repair_to_fixpoint(file, &ScannerBackend, &converged_providers, eligibility);
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
    let verification = verify_after(
        &fresh.sources,
        &ScannerBackend,
        &diags,
        Some(idempotent),
        external,
    );
    report.verification = Some(verification);
    let toolchain = probe_toolchain();
    let ctx = HealthContext {
        inventory: &fresh,
        diagnostics: &fresh_diags,
        toolchain: &toolchain,
    };
    report.checks = run_all_checks(&ctx);
    report.toolchain = Some(toolchain);
    let code = exit_for(
        &report.checks,
        blocked_review + blocked_manual > 0,
        report.verification.as_ref(),
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
    let inventory = discover(&root, &DiscoveryOptions::default())
        .map_err(|e| format!("discovery failed: {e}"))?;
    let diags = diagnose_all(&inventory.sources, flags.with_language_backend);

    // Per-file: declared version -> plan to target.
    let mut report = Report::new("migrate", root.to_string_lossy());
    report.inventory = Some(inventory.summary());
    report.file_diagnostics = diags.clone();
    report.toolchain = Some(probe_toolchain());
    let mut file_plans: Vec<(SourceFile, mncs_doctor::migration::MigrationPlan)> = Vec::new();
    let mut unmigratable: Vec<String> = Vec::new();
    for file in &inventory.sources {
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
            Ok(plan) => file_plans.push((file.clone(), plan)),
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
        let code = if unmigratable.is_empty() && fully_known {
            ExitCode::Healthy
        } else if fully_known {
            ExitCode::Findings
        } else {
            ExitCode::ReviewRequired
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
    let fresh = discover(&root, &DiscoveryOptions::default())
        .map_err(|e| format!("re-discovery failed: {e}"))?;
    let fresh_diags = diagnose_all(&fresh.sources, flags.with_language_backend);
    report.file_diagnostics = fresh_diags.clone();
    let fresh_toolchain = probe_toolchain();
    report.checks = run_all_checks(&HealthContext {
        inventory: &fresh,
        diagnostics: &fresh_diags,
        toolchain: &fresh_toolchain,
    });
    report.toolchain = Some(fresh_toolchain);
    // Idempotence: re-planning migrated files must yield no-op plans.
    let mut idempotent = true;
    for file in &fresh.sources {
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
    let verification = verify_after(
        &fresh.sources,
        &ScannerBackend,
        &diags,
        Some(idempotent),
        external,
    );
    report.verification = Some(verification);
    let code = exit_for(&[], false, report.verification.as_ref());
    let code = if code == ExitCode::Healthy && !unmigratable.is_empty() {
        ExitCode::Findings
    } else {
        code
    };
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
    let root = workspace_root(&flags.root)?;
    let inventory = discover(&root, &DiscoveryOptions::default())
        .map_err(|e| format!("discovery failed: {e}"))?;
    let diags = diagnose_all(&inventory.sources, flags.with_language_backend);
    let external: Vec<ExternalCheck> = verify_cmd
        .map(|cmd| vec![run_external(&cmd, &root)])
        .unwrap_or_default();
    // Standalone verify judges the current tree against itself: the verdict
    // is about *new* breakage, so the pre-map is the present diagnosis.
    let verification = verify_after(&inventory.sources, &ScannerBackend, &diags, None, external);
    // Standalone verify passes on zero errors; pre-existing diagnostics are
    // reported as findings, not as verification failure, unless errors exist.
    let errors: usize = diags
        .values()
        .flatten()
        .filter(|d| matches!(d.severity, mncs_doctor::diagnostics::Severity::Error))
        .count();
    let mut report = Report::new("verify", root.to_string_lossy());
    report.inventory = Some(inventory.summary());
    report.file_diagnostics = diags;
    report.toolchain = Some(probe_toolchain());
    report.verification = Some(verification);
    if flags.explain {
        report.notes.push("explain".to_owned());
    }
    let code = match &report.verification {
        Some(v) if !v.external.iter().all(|c| c.success) => ExitCode::VerificationFailed,
        _ if errors > 0 => ExitCode::Findings,
        _ => ExitCode::Healthy,
    };
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
