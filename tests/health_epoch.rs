//! Health epochs: cheap no-change verification with fail-open invalidation.
//!
//! Every test stages a fresh fixture tree. A "hit" (second no-change run)
//! must re-emit the validated outcome without running policy (proven by
//! empty entrypoints) and mark itself honestly; any input change must
//! fall back to the full path and reflect the new state.

mod common;

use common::{run, run_env, stage, stdout_json};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_dir(prefix: &str) -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "mncs-doctor-epoch-it-{}-{prefix}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn epoch_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".mncs/doctor/health-epoch.json")
}

fn has_reuse_note(report: &serde_json::Value) -> bool {
    report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|note| note.as_str().unwrap().starts_with("epoch reused: "))
}

#[test]
fn doctor_second_run_reuses_validated_epoch() {
    let root = stage("repos/healthy");
    let first = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(first.status.success());
    let run1 = stdout_json(&first);
    assert_eq!(run1["inventory"]["epoch_reused"], false);
    assert!(epoch_path(&root).is_file());

    let second = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(second.status.success());
    let run2 = stdout_json(&second);
    assert_eq!(run2["inventory"]["epoch_reused"], true);
    assert!(run2["inventory"]["epoch_digest"].is_string());
    assert!(has_reuse_note(&run2));
    // No policy ran in this process: the entrypoint trace is empty rather
    // than replayed.
    assert!(run2["policy"]["entrypoints"].as_array().unwrap().is_empty());
    assert_eq!(run2["policy"]["engine"], "mncs");
    // The re-emitted verdict equals the validated one.
    assert_eq!(run2["checks"], run1["checks"]);
    assert_eq!(run2["file_diagnostics"], run1["file_diagnostics"]);
    assert_eq!(run2["toolchain"], run1["toolchain"]);
    assert_eq!(run2["exit_code"], run1["exit_code"]);
    assert_eq!(run2["exit_meaning"], run1["exit_meaning"]);
}

#[test]
fn doctor_source_mutation_invalidates_epoch() {
    let root = stage("repos/healthy");
    let first = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(first.status.success());
    fs::write(
        root.join("src/lib.mncs"),
        "mncs 0.18;\nmodule healthy.lib;\n\nfn answer() -> (result: i64) {\n    return 42;\n",
    )
    .unwrap();

    let second = run(&root, &["doctor", "--root", ".", "--json"]);
    let run2 = stdout_json(&second);
    assert_eq!(run2["inventory"]["epoch_reused"], false);
    assert!(!has_reuse_note(&run2));
    // The full path ran and sees the new state (unbalanced delimiters).
    let text = serde_json::to_string(&run2["file_diagnostics"]).unwrap();
    assert!(text.contains("DOC106"), "{text}");
    assert!(!run2["policy"]["entrypoints"].as_array().unwrap().is_empty());

    // And the fresh state records a new epoch for the run after.
    let third = run(&root, &["doctor", "--root", ".", "--json"]);
    let run3 = stdout_json(&third);
    assert_eq!(run3["inventory"]["epoch_reused"], true);
}

#[test]
fn doctor_new_source_invalidates_epoch() {
    let root = stage("repos/healthy");
    let first = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(first.status.success());
    fs::write(
        root.join("src/new.mncs"),
        "mncs 0.18;\nmodule healthy.new;\n",
    )
    .unwrap();

    let second = run(&root, &["doctor", "--root", ".", "--json"]);
    let run2 = stdout_json(&second);
    assert_eq!(run2["inventory"]["epoch_reused"], false);
    assert_eq!(run2["inventory"]["files_checked"], 3);
}

#[test]
fn doctor_manifest_mutation_invalidates_epoch() {
    let root = stage("repos/healthy");
    let first = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(first.status.success());
    // Manifest health reads manifest bytes, not just paths.
    fs::write(root.join("mncs-forge.toml"), "[project]\nname = \"x\"\n").unwrap();

    let second = run(&root, &["doctor", "--root", ".", "--json"]);
    let run2 = stdout_json(&second);
    assert_eq!(run2["inventory"]["epoch_reused"], false);
    let manifest = run2["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "manifest-health")
        .expect("manifest-health check");
    assert_eq!(manifest["status"], "warning");
}

#[test]
fn doctor_project_manifest_invalidates_epoch() {
    let root = stage("repos/healthy");
    let first = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(first.status.success());
    // `.mncs` is excluded from topology, but the project manifest bytes
    // are fingerprinted separately.
    fs::create_dir_all(root.join(".mncs")).unwrap();
    fs::write(root.join(".mncs/project.json"), "{}").unwrap();

    let second = run(&root, &["doctor", "--root", ".", "--json"]);
    let run2 = stdout_json(&second);
    assert_eq!(run2["inventory"]["epoch_reused"], false);
    let text = serde_json::to_string(&run2["checks"]).unwrap();
    assert!(text.contains("family repository manifest"), "{text}");
}

#[test]
fn doctor_corrupt_or_tampered_epoch_falls_back_to_full_pass() {
    let root = stage("repos/healthy");
    let first = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(first.status.success());

    fs::write(epoch_path(&root), "corrupt").unwrap();
    let second = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(second.status.success());
    let run2 = stdout_json(&second);
    assert_eq!(run2["inventory"]["epoch_reused"], false);
    // The full pass rewrites a valid epoch.
    let epoch: serde_json::Value =
        serde_json::from_slice(&fs::read(epoch_path(&root)).unwrap()).unwrap();
    assert_eq!(epoch["schema_version"], "mncs.doctor.health-epoch/1");

    // Tampering with the validated state breaks the digest binding.
    let mut tampered = epoch;
    tampered["state"]["doctor_exit_code"] = serde_json::json!(3);
    fs::write(
        epoch_path(&root),
        serde_json::to_vec_pretty(&tampered).unwrap(),
    )
    .unwrap();
    let third = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(third.status.success());
    let run3 = stdout_json(&third);
    assert_eq!(run3["inventory"]["epoch_reused"], false);
    assert_eq!(run3["exit_code"], 0);
}

#[test]
fn doctor_aged_epoch_falls_back_to_full_pass() {
    let root = stage("repos/healthy");
    let first = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(first.status.success());

    let mut epoch: serde_json::Value =
        serde_json::from_slice(&fs::read(epoch_path(&root)).unwrap()).unwrap();
    epoch["recorded_at_unix"] = serde_json::json!(1);
    fs::write(
        epoch_path(&root),
        serde_json::to_vec_pretty(&epoch).unwrap(),
    )
    .unwrap();
    let second = run(&root, &["doctor", "--root", ".", "--json"]);
    let run2 = stdout_json(&second);
    assert_eq!(run2["inventory"]["epoch_reused"], false);
    assert!(!has_reuse_note(&run2));
}

#[test]
fn doctor_toolchain_override_change_invalidates_epoch() {
    let root = stage("repos/healthy");
    let stubs = unique_dir("toolchain");
    let stub_a = stubs.join("provider-a.sh");
    let stub_b = stubs.join("provider-b.sh");
    fs::write(&stub_a, "#!/bin/sh\necho \"stub provider A\"\n").unwrap();
    fs::write(&stub_b, "#!/bin/sh\necho \"stub provider B\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for stub in [&stub_a, &stub_b] {
            let mut perms = fs::metadata(stub).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(stub, perms).unwrap();
        }
    }
    let path_a = stub_a.to_str().unwrap().to_owned();
    let path_b = stub_b.to_str().unwrap().to_owned();

    let first = run_env(
        &root,
        &["doctor", "--root", ".", "--json"],
        &[("MNCS_TEST_BIN", path_a.as_str())],
    );
    assert!(first.status.success());
    let run1 = stdout_json(&first);
    assert_eq!(run1["inventory"]["epoch_reused"], false);

    // Same override: hit, and the recorded provider facts are reused.
    let second = run_env(
        &root,
        &["doctor", "--root", ".", "--json"],
        &[("MNCS_TEST_BIN", path_a.as_str())],
    );
    let run2 = stdout_json(&second);
    assert_eq!(run2["inventory"]["epoch_reused"], true);
    assert_eq!(
        run2["toolchain"]["test_provider"]["version"],
        run1["toolchain"]["test_provider"]["version"]
    );

    // Changed override: full path re-probes and observes the new binary.
    let third = run_env(
        &root,
        &["doctor", "--root", ".", "--json"],
        &[("MNCS_TEST_BIN", path_b.as_str())],
    );
    let run3 = stdout_json(&third);
    assert_eq!(run3["inventory"]["epoch_reused"], false);
    assert_ne!(
        run3["toolchain"]["test_provider"]["version"],
        run1["toolchain"]["test_provider"]["version"]
    );

    let fourth = run_env(
        &root,
        &["doctor", "--root", ".", "--json"],
        &[("MNCS_TEST_BIN", path_b.as_str())],
    );
    let run4 = stdout_json(&fourth);
    assert_eq!(run4["inventory"]["epoch_reused"], true);
}

#[test]
fn doctor_with_language_backend_never_reuses() {
    let root = stage("repos/healthy");
    // A priming scanner-only run records an epoch...
    let priming = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(priming.status.success());
    // ...which backend runs must neither use nor overwrite.
    for _ in 0..2 {
        let out = run_env(
            &root,
            &["doctor", "--root", ".", "--json", "--with-language-backend"],
            &[("MNCS_CLI", "true")],
        );
        let report = stdout_json(&out);
        assert_eq!(report["inventory"]["epoch_reused"], false);
        assert!(!has_reuse_note(&report));
    }
    // The scanner epoch survived the backend runs untouched.
    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(report["inventory"]["epoch_reused"], true);
}

#[test]
fn doctor_changed_path_preserves_full_epoch() {
    let root = stage("repos/healthy");
    let first = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(first.status.success());

    // Scoped runs neither reuse nor clobber the full-state epoch.
    let narrowed = run(
        &root,
        &[
            "doctor",
            "--root",
            ".",
            "--changed-path",
            "src/main.mncs",
            "--json",
        ],
    );
    let scoped = stdout_json(&narrowed);
    assert_eq!(scoped["inventory"]["epoch_reused"], false);
    assert_eq!(scoped["inventory"]["files_checked"], 1);

    let third = run(&root, &["doctor", "--root", ".", "--json"]);
    let run3 = stdout_json(&third);
    assert_eq!(run3["inventory"]["epoch_reused"], true);
}

#[test]
fn remediate_clean_state_reuses_post_state_epoch() {
    let root = stage("repos/outdated");
    let first = run(&root, &["remediate", "--root", ".", "--json"]);
    let run1 = stdout_json(&first);
    assert_eq!(run1["summary"]["repaired"], 1);

    // The first run validated its post-state fixpoint, so the second run
    // re-emits the clean outcome without running policy.
    let second = run(&root, &["remediate", "--root", ".", "--json"]);
    let run2 = stdout_json(&second);
    assert_eq!(run2["summary"]["repaired"], 0);
    assert_eq!(run2["summary"]["reconciled"], 0);
    assert!(run2["remaining"].as_array().unwrap().is_empty());
    assert!(has_reuse_note(&run2));
    assert_eq!(run2["exit_code"], run1["exit_code"]);
    // Evidence is rewritten for the current run with no hypotheticals.
    let evidence_path = run2["evidence"].as_str().unwrap().to_owned();
    let evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(&evidence_path).unwrap()).unwrap();
    assert_eq!(evidence["schema_version"], "mncs.remediation-evidence/1");
    assert!(evidence["repairs"].as_array().unwrap().is_empty());
    assert!(evidence["policy"]["entrypoints"]
        .as_array()
        .unwrap()
        .is_empty());

    // The hit envelope equals a fresh full pass modulo the reuse note.
    fs::remove_file(epoch_path(&root)).unwrap();
    let third = run(&root, &["remediate", "--root", ".", "--json"]);
    let run3 = stdout_json(&third);
    assert!(!has_reuse_note(&run3));
    assert_eq!(run3["summary"], run2["summary"]);
    assert_eq!(run3["remaining"], run2["remaining"]);
    assert_eq!(run3["validation"], run2["validation"]);
    assert_eq!(run3["exit_code"], run2["exit_code"]);
}

#[test]
fn remediate_repairs_then_doctor_hits() {
    let root = stage("repos/outdated");
    let first = run(&root, &["remediate", "--root", ".", "--json"]);
    let run1 = stdout_json(&first);
    assert_eq!(run1["summary"]["repaired"], 1);

    // Remediation records the diagnostic half too: the confirming
    // `doctor` is a hit, not another full scan.
    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(report["inventory"]["epoch_reused"], true);
    assert_eq!(report["exit_code"], 1);
}

#[test]
fn fix_then_doctor_hits() {
    let root = stage("repos/outdated");
    let fixed = run(&root, &["fix", "--root", ".", "--json"]);
    assert!(matches!(fixed.status.code(), Some(0) | Some(1)));

    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(report["inventory"]["epoch_reused"], true);
    assert!(report["policy"]["entrypoints"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn fix_records_doctor_exit_not_fix_exit() {
    // `fix --verify-cmd false` fails verification (exit 3) after repairing
    // the tree. The recorded doctor half must carry the exit a `doctor`
    // full pass would report (findings, exit 1), not the fix exit.
    let root = stage("repos/outdated");
    let fixed = run(
        &root,
        &["fix", "--root", ".", "--verify-cmd", "false", "--json"],
    );
    assert_eq!(fixed.status.code(), Some(3));

    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(report["inventory"]["epoch_reused"], true);
    assert_eq!(report["exit_code"], 1);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn remediate_dry_run_twice_second_reuses_hypothetical() {
    let root = stage("repos/outdated");
    let before = fs::read(root.join("src/messy.mncs")).unwrap();
    let first = run(&root, &["remediate", "--root", ".", "--dry-run", "--json"]);
    let run1 = stdout_json(&first);
    assert_eq!(run1["dry_run"], true);
    assert_eq!(run1["summary"]["repaired"], 1);

    let second = run(&root, &["remediate", "--root", ".", "--dry-run", "--json"]);
    let run2 = stdout_json(&second);
    assert_eq!(run2["dry_run"], true);
    assert_eq!(run2["summary"]["repaired"], 1);
    assert_eq!(run2["repairs"], run1["repairs"]);
    assert_eq!(run2["validation"], run1["validation"]);
    assert!(has_reuse_note(&run2));
    assert_eq!(
        fs::read(root.join("src/messy.mncs")).unwrap(),
        before,
        "dry-run hit mutated a file"
    );

    // A dry epoch never serves a mutating run: real work still runs full.
    let third = run(&root, &["remediate", "--root", ".", "--json"]);
    let run3 = stdout_json(&third);
    assert_eq!(run3["dry_run"], false);
    assert_eq!(run3["summary"]["repaired"], 1);
    assert!(!has_reuse_note(&run3));
}

#[test]
fn remediate_budget_exhaustion_never_skips_real_work() {
    let root = stage("repos/outdated");
    // A second dirty file so the budget binds.
    fs::copy(root.join("src/messy.mncs"), root.join("src/messy2.mncs")).unwrap();
    let first = run(
        &root,
        &["remediate", "--root", ".", "--budget", "1", "--json"],
    );
    let run1 = stdout_json(&first);
    assert_eq!(run1["summary"]["repaired"], 1);
    assert_eq!(run1["budget"]["exhausted"], true);

    // The post-state still has pending repairs, so no fast path: the
    // re-run continues where the first stopped.
    let second = run(&root, &["remediate", "--root", ".", "--json"]);
    let run2 = stdout_json(&second);
    assert_eq!(run2["summary"]["repaired"], 1);
    assert!(!has_reuse_note(&run2));
}

#[test]
fn remediate_verify_cmd_never_reuses() {
    let root = stage("repos/outdated");
    let first = run(
        &root,
        &["remediate", "--root", ".", "--verify-cmd", "true", "--json"],
    );
    let run1 = stdout_json(&first);
    assert_eq!(run1["summary"]["repaired"], 1);

    // External verification results are unvalidated inputs: every
    // `--verify-cmd` run is a full pass.
    let second = run(
        &root,
        &["remediate", "--root", ".", "--verify-cmd", "true", "--json"],
    );
    let run2 = stdout_json(&second);
    assert_eq!(run2["summary"]["repaired"], 0);
    assert!(!has_reuse_note(&run2));
    // The external command really ran on the second pass.
    let evidence_path = run2["evidence"].as_str().unwrap().to_owned();
    let evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(&evidence_path).unwrap()).unwrap();
    assert_eq!(evidence["verification"]["external"][0]["command"], "true");
    assert_eq!(evidence["verification"]["external"][0]["success"], true);
}
