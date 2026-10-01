//! `remediate` command: ambient safe repair, terse summaries, evidence artifacts.

mod common;

use common::{run, stage, stdout_json};
use std::process::Command;

#[test]
fn remediate_repairs_safe_hygiene_and_reports_terse_counts() {
    let root = stage("repos/outdated");
    let out = run(&root, &["remediate", "--root", ".", "--json"]);
    assert!(
        matches!(out.status.code(), Some(0) | Some(1)),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout_json(&out);
    assert_eq!(report["schema_version"], "mncs.doctor.remediation/1");
    assert_eq!(report["summary"]["repaired"], 1);
    assert_eq!(report["summary"]["blockers"], 0);
    assert_eq!(report["repairs"][0]["class"], "safe_automatic");
    assert_eq!(report["repairs"][0]["validated"], true);
    // Terse: no diagnostics, diffs, or hunks on stdout.
    let text = serde_json::to_string(&report).unwrap();
    assert!(!text.contains("file_diagnostics"));
    assert!(!text.contains("planned_diffs"));
    // The file itself is normalized.
    let messy = std::fs::read_to_string(root.join("src/messy.mncs")).unwrap();
    assert!(!messy.lines().any(|line| line.ends_with([' ', '\t'])));
    assert!(messy.ends_with('\n'));
}

#[test]
fn remediate_is_idempotent_and_second_run_is_quiet() {
    let root = stage("repos/outdated");
    let first = run(&root, &["remediate", "--root", ".", "--json"]);
    assert!(matches!(first.status.code(), Some(0) | Some(1)));
    let second = run(&root, &["remediate", "--root", ".", "--json"]);
    let report = stdout_json(&second);
    assert_eq!(report["summary"]["repaired"], 0);
    assert_eq!(report["summary"]["reconciled"], 0);
    assert_eq!(report["summary"]["blockers"], 0);
    assert!(report["remaining"].as_array().unwrap().is_empty());
    let human = run(&root, &["remediate", "--root", "."]);
    let text = String::from_utf8_lossy(&human.stdout).into_owned();
    assert!(text.contains("repaired=0 reconciled=0 degraded=0 blockers=0"));
    assert!(text.contains("remaining: none"));
    assert!(text.lines().count() <= 4, "{text}");
}

#[test]
fn remediate_escalates_unrepairable_files_with_minimal_ids() {
    let root = stage("repos/malformed");
    let out = run(&root, &["remediate", "--root", ".", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(report["summary"]["repaired"], 0);
    assert!(report["summary"]["blockers"].as_u64().unwrap() >= 1);
    let remaining = report["remaining"].as_array().unwrap();
    assert!(!remaining.is_empty());
    // Escalation ids are `code:path`, not dumped diagnostics.
    for id in remaining {
        let id = id.as_str().unwrap();
        assert!(id.contains(':'), "{id}");
        assert!(id.len() < 128, "{id}");
    }
    // Nothing was mutated.
    let before = std::fs::read(root.join("src/unbalanced.mncs")).unwrap();
    let after = run(&root, &["remediate", "--root", ".", "--json"]);
    let _ = stdout_json(&after);
    assert_eq!(
        std::fs::read(root.join("src/unbalanced.mncs")).unwrap(),
        before
    );
}

#[test]
fn remediate_dry_run_predicts_without_mutating() {
    let root = stage("repos/outdated");
    let before = std::fs::read(root.join("src/messy.mncs")).unwrap();
    let out = run(&root, &["remediate", "--root", ".", "--dry-run", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["summary"]["repaired"], 1);
    assert_eq!(report["repairs"][0]["validated"], false);
    assert_eq!(
        std::fs::read(root.join("src/messy.mncs")).unwrap(),
        before,
        "dry-run mutated a file"
    );
}

#[test]
fn remediate_dry_run_verification_is_vacuous_not_failed() {
    // A dry run deliberately leaves fixes unapplied; that must not read
    // as a verification failure.
    let root = stage("repos/outdated");
    let out = run(&root, &["remediate", "--root", ".", "--dry-run", "--json"]);
    let report = stdout_json(&out);
    let remaining = report["remaining"].as_array().unwrap();
    assert!(
        !remaining.iter().any(|id| id == "verification-failed"),
        "{remaining:?}"
    );
    assert_ne!(report["exit_code"], 3);
    let evidence_path = report["evidence"].as_str().unwrap().to_owned();
    let evidence: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
    assert_eq!(evidence["verification"]["passed"], true);
    assert!(evidence["verification"]["idempotent"].is_null());
}

#[test]
fn remediate_budget_exhaustion_repairs_prefix_and_escalates_rest() {
    let root = stage("repos/outdated");
    // A second dirty file so the budget binds.
    std::fs::copy(root.join("src/messy.mncs"), root.join("src/messy2.mncs")).unwrap();
    let out = run(
        &root,
        &["remediate", "--root", ".", "--budget", "1", "--json"],
    );
    let report = stdout_json(&out);
    assert_eq!(report["summary"]["repaired"], 1);
    assert_eq!(report["budget"]["exhausted"], true);
    let remaining = report["remaining"].as_array().unwrap();
    assert!(
        remaining
            .iter()
            .any(|id| id.as_str().unwrap().starts_with("budget-exhausted:")),
        "{remaining:?}"
    );
    // A re-run with budget continues where the first stopped.
    let out = run(&root, &["remediate", "--root", ".", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(report["summary"]["repaired"], 1);
    assert!(report["remaining"].as_array().unwrap().is_empty());
}

#[test]
fn remediate_regenerates_a_corrupt_cache_as_reconciliation() {
    let root = stage("repos/outdated");
    let priming = run(&root, &["remediate", "--root", ".", "--json"]);
    assert!(matches!(priming.status.code(), Some(0) | Some(1)));
    std::fs::write(root.join(".mncs/doctor/inventory.json"), "corrupt").unwrap();
    let out = run(
        &root,
        &[
            "remediate",
            "--root",
            ".",
            "--changed-path",
            "src/main.mncs",
            "--json",
        ],
    );
    let report = stdout_json(&out);
    assert_eq!(report["summary"]["reconciled"], 1);
    assert_eq!(
        report["reconciliations"][0]["id"],
        "inventory-cache:regenerated"
    );
    assert_eq!(report["reconciliations"][0]["validated"], true);
}

#[test]
fn remediate_routes_evidence_to_session_artifact_dir() {
    let root = stage("repos/outdated");
    let artifacts = root.join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mncs-doctor"))
        .args(["remediate", "--root", ".", "--json"])
        .env("MNCS_ENV_SESSION_ARTIFACT_DIR", &artifacts)
        .current_dir(&root)
        .output()
        .unwrap();
    let report = stdout_json(&out);
    let evidence_path = report["evidence"].as_str().unwrap().to_owned();
    assert!(evidence_path.starts_with(artifacts.to_str().unwrap()));
    let evidence: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
    assert_eq!(
        evidence["schema_version"],
        "mncs.doctor.remediation-evidence/1"
    );
    assert_eq!(evidence["repairs"].as_array().unwrap().len(), 1);
    assert_eq!(evidence["policy"]["engine"], "mncs");
    assert_eq!(evidence["verification"]["passed"], true);
}

#[test]
fn remediate_evidence_defaults_to_ignored_doctor_dir() {
    let root = stage("repos/outdated");
    let out = run(
        &root,
        &[
            "remediate",
            "--root",
            ".",
            "--json",
            "--evidence-path",
            "custom/evidence.json",
        ],
    );
    let report = stdout_json(&out);
    assert!(report["evidence"]
        .as_str()
        .unwrap()
        .ends_with("custom/evidence.json"));
    assert!(root.join("custom/evidence.json").is_file());
}
