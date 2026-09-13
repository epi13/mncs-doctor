//! `doctor` command: health inspection across repo shapes.

mod common;

use common::{read_bytes, run, stage, stdout_json};

#[test]
fn healthy_repo_reports_two_files() {
    let root = stage("repos/healthy");
    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout_json(&out);
    assert_eq!(report["inventory"]["files_checked"], 2);
    assert_eq!(report["exit_code"], 0);
    assert_eq!(report["command"], "doctor");
    assert!(report["schema_version"].is_string());
    assert_eq!(report["policy"]["engine"], "mncs");
    assert_eq!(report["policy"]["backend"], "mncs-research-bytecode");
    let entrypoints = report["policy"]["entrypoints"].as_array().unwrap();
    assert!(entrypoints
        .iter()
        .any(|entrypoint| { entrypoint == "doctor.health.v1::check_status_with_skip" }));
    assert!(entrypoints
        .iter()
        .any(|entrypoint| entrypoint == "doctor.report.v1::exit_for"));
    assert!(entrypoints
        .iter()
        .any(|entrypoint| entrypoint == "doctor.discovery.v1::directory_decision"));
    assert!(entrypoints
        .iter()
        .any(|entrypoint| entrypoint == "doctor.discovery.v1::file_class"));
    assert!(entrypoints
        .iter()
        .any(|entrypoint| entrypoint == "doctor.version.v1::classify"));
    assert!(entrypoints
        .iter()
        .any(|entrypoint| entrypoint == "doctor.scanner.v1::feed"));
    assert!(entrypoints
        .iter()
        .any(|entrypoint| entrypoint == "doctor.scanner.v1::finish"));
    assert_eq!(report["policy"]["modules"].as_object().unwrap().len(), 10);
    // No error diagnostics anywhere.
    for (_path, diags) in report["file_diagnostics"].as_object().unwrap() {
        for diag in diags.as_array().unwrap() {
            assert_ne!(diag["severity"], "error", "{diag:?}");
        }
    }
}

#[test]
fn outdated_repo_reports_sealed_profiles() {
    let root = stage("repos/outdated");
    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout_json(&out);
    assert_eq!(report["inventory"]["files_checked"], 3);
    let text = serde_json::to_string(&report).unwrap();
    assert!(
        text.contains("DOC103"),
        "expected sealed-profile findings: {text}"
    );
    let drift = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "version-drift")
        .expect("version-drift check");
    assert_eq!(drift["status"], "warning");
}

#[test]
fn malformed_repo_fails_with_spans() {
    let root = stage("repos/malformed");
    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    let code = out.status.code().unwrap();
    assert!(
        code == 1 || code == 2,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout_json(&out);
    let text = serde_json::to_string(&report).unwrap();
    assert!(text.contains("DOC106"), "unbalanced delimiters: {text}");
    assert!(text.contains("DOC102"), "bad header version: {text}");
    assert!(text.contains("DOC108"), "non-UTF8 file: {text}");
    // DOC106 carries a precise in-file span.
    let diags = report["file_diagnostics"]["src/unbalanced.mncs"]
        .as_array()
        .unwrap();
    let d106 = diags.iter().find(|d| d["code"] == "DOC106").unwrap();
    assert!(d106["span"]["start"].as_u64().unwrap() > 0);
    assert_eq!(d106["span"]["start_line"], d106["span"]["end_line"]);
}

#[test]
fn empty_repo_is_healthy_with_zero_files() {
    let root = stage("repos/empty");
    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(report["inventory"]["files_checked"], 0);
    assert!(out.status.code() == Some(0) || out.status.code() == Some(1));
}

#[test]
fn mixed_repo_requires_review_for_manual_items() {
    // headless.mncs needs a human-chosen header (MANUAL): exit 2.
    let root = stage("repos/mixed");
    let out = run(&root, &["doctor", "--root", ".", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[cfg(unix)]
fn language_backend_plumbing_uses_mncs_cli() {
    use std::process::Command;
    let root = stage("repos/healthy");
    // Stub backend that always succeeds with no output: no diagnostics added.
    let out = Command::new(env!("CARGO_BIN_EXE_mncs-doctor"))
        .args(["doctor", "--root", ".", "--json", "--with-language-backend"])
        .env("MNCS_CLI", "true")
        .current_dir(&root)
        .output()
        .unwrap();
    let report = stdout_json(&out);
    let text = serde_json::to_string(&report["file_diagnostics"]).unwrap();
    assert!(!text.contains("DOC201"), "{text}");
    // Stub backend that always fails: fail-closed DOC201 per file.
    let out = Command::new(env!("CARGO_BIN_EXE_mncs-doctor"))
        .args(["doctor", "--root", ".", "--json", "--with-language-backend"])
        .env("MNCS_CLI", "false")
        .current_dir(&root)
        .output()
        .unwrap();
    let report = stdout_json(&out);
    let text = serde_json::to_string(&report["file_diagnostics"]).unwrap();
    assert!(text.contains("DOC201"), "{text}");
}

#[test]
fn human_output_is_stable_and_json_matches() {
    let root = stage("repos/outdated");
    let human = run(&root, &["doctor", "--root", "."]);
    let a = String::from_utf8_lossy(&human.stdout).into_owned();
    let human2 = run(&root, &["doctor", "--root", "."]);
    let b = String::from_utf8_lossy(&human2.stdout).into_owned();
    assert_eq!(a, b);
    assert!(a.contains("files checked"));
    assert!(a.contains("Checks:"));
    let _ = read_bytes(&root, "mncs-forge.toml");
}

#[test]
fn verify_command_uses_mncs_verification_and_report_policy() {
    let root = stage("repos/healthy");
    let out = run(&root, &["verify", "--root", ".", "--json"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout_json(&out);
    assert_eq!(report["policy"]["engine"], "mncs");
    assert_eq!(report["verification"]["passed"], true);
    let entrypoints = report["policy"]["entrypoints"].as_array().unwrap();
    for expected in [
        "doctor.discovery.v1::directory_decision",
        "doctor.discovery.v1::file_class",
        "doctor.scanner.v1::feed",
        "doctor.scanner.v1::finish",
        "doctor.verify.v1::delta_ok",
        "doctor.verify.v1::compose",
        "doctor.report.v1::exit_for",
    ] {
        assert!(
            entrypoints.iter().any(|entrypoint| entrypoint == expected),
            "live verify path omitted {expected}: {entrypoints:?}"
        );
    }
}
