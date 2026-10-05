//! Stdlib composition diagnostics ride `toolchain-health` (Stage F).
//!
//! The MNCS policy window is fixed at 8 checks (see DOC-P-023), so
//! stdlib findings (missing/invalid/stale pin, incompatible profiles,
//! unresolved project imports) surface inside the toolchain check while
//! the structured `toolchain.stdlib` status carries the module facts.

use std::fs;

mod common;

use common::{run_env, stage, stdout_json};

fn toolchain_check(report: &serde_json::Value) -> serde_json::Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "toolchain-health")
        .expect("toolchain-health check")
        .clone()
}

fn stdlib_findings(check: &serde_json::Value) -> Vec<serde_json::Value> {
    check["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| {
            f["message"].as_str().unwrap_or("").contains("stdlib")
                || f["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("standard library")
        })
        .cloned()
        .collect()
}

/// Hermetic stdlib discovery: empty `MNCS_STDLIB_ROOT` is ignored by
/// Doctor's discovery, so staged fixtures resolve without ambient leaks.
const HERMETIC: [(&str, &str); 1] = [("MNCS_STDLIB_ROOT", "")];

#[test]
fn healthy_composition_has_no_stdlib_findings() {
    let root = stage("repos/stdlib-family");
    let out = run_env(
        &root,
        &["doctor", "--root", "project/ok", "--json"],
        &HERMETIC,
    );
    // Exit status reflects every check; this test pins stdlib silence.
    let report = stdout_json(&out);
    let check = toolchain_check(&report);
    assert!(
        stdlib_findings(&check).is_empty(),
        "findings: {}",
        check["findings"]
    );
    assert_eq!(report["toolchain"]["stdlib"]["state"], "ready");
    assert_eq!(report["toolchain"]["stdlib"]["module_count"], 2);
}

#[test]
fn missing_stdlib_warns() {
    let root = stage("repos/stdlib-lonely");
    let out = run_env(&root, &["doctor", "--root", "project", "--json"], &HERMETIC);
    let report = stdout_json(&out);
    let check = toolchain_check(&report);
    let findings = stdlib_findings(&check);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["message"], "no usable standard library found");
    assert_eq!(findings[0]["severity"], "warning");
}

#[test]
fn unresolved_import_in_stdlib_owned_namespace_fails() {
    let root = stage("repos/stdlib-family");
    let out = run_env(
        &root,
        &["doctor", "--root", "project/bad", "--json"],
        &HERMETIC,
    );
    let report = stdout_json(&out);
    let check = toolchain_check(&report);
    let findings = stdlib_findings(&check);
    assert!(findings.iter().any(|f| {
        f["message"]
            .as_str()
            .unwrap_or("")
            .contains("unresolved stdlib import: mncs.core.missing.v1")
    }));
    assert!(findings.iter().any(|f| f["severity"] == "error"));
}

#[test]
fn imports_from_other_selected_providers_are_not_misclassified_as_stdlib() {
    let root = stage("repos/stdlib-family");
    let source = root.join("project/ok/consumer.mncs");
    let mut text = fs::read_to_string(&source).unwrap();
    text.push_str("\nuse mncs.commons.family.artifact.v1;\n");
    fs::write(source, text).unwrap();

    let out = run_env(
        &root,
        &["doctor", "--root", "project/ok", "--json"],
        &HERMETIC,
    );
    let report = stdout_json(&out);
    let check = toolchain_check(&report);
    assert!(
        stdlib_findings(&check).is_empty(),
        "findings: {}",
        check["findings"]
    );
}

#[test]
fn stale_tree_bytes_fail() {
    let root = stage("repos/stdlib-family");
    let module = root.join("mncs-stdlib/library/core/status.mncs");
    let mut text = fs::read_to_string(&module).unwrap();
    text.push_str("\n// tampered\n");
    fs::write(&module, text).unwrap();
    let out = run_env(
        &root,
        &["doctor", "--root", "project/ok", "--json"],
        &HERMETIC,
    );
    let report = stdout_json(&out);
    let check = toolchain_check(&report);
    let findings = stdlib_findings(&check);
    assert!(findings.iter().any(|f| {
        f["message"]
            .as_str()
            .unwrap_or("")
            .contains("library/core/status.mncs")
    }));
    assert!(findings.iter().any(|f| f["severity"] == "error"));
}
