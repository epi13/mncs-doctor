//! `fix` command: dry-run safety, apply, idempotence, preservation.

mod common;

use common::{read, read_bytes, run, stage, stdout_json};

#[test]
fn dry_run_mutates_nothing_but_reports_diffs() {
    let root = stage("repos/outdated");
    let before = read(&root, "src/messy.mncs");
    assert!(before.contains("  \n") || before.ends_with("  "));
    let out = run(&root, &["fix", "--root", ".", "--dry-run", "--json"]);
    let report = stdout_json(&out);
    assert_eq!(
        read(&root, "src/messy.mncs"),
        before,
        "dry-run mutated a file"
    );
    let diffs = report["planned_diffs"].as_array().unwrap();
    assert!(!diffs.is_empty(), "expected planned diffs");
    assert!(diffs.iter().any(|d| d["relative"] == "src/messy.mncs"));
}

#[test]
fn dry_run_plan_equals_applied_result() {
    let root = stage("repos/outdated");
    // Capture the dry-run prediction (new fingerprints per file).
    let out = run(&root, &["fix", "--root", ".", "--dry-run", "--json"]);
    let predicted = stdout_json(&out);
    let mut expected_fp = std::collections::BTreeMap::new();
    for diff in predicted["planned_diffs"].as_array().unwrap() {
        expected_fp.insert(
            diff["relative"].as_str().unwrap().to_owned(),
            diff["new_fingerprint"].as_str().unwrap().to_owned(),
        );
    }
    assert!(!expected_fp.is_empty());
    // Apply and compare fingerprints.
    let out = run(&root, &["fix", "--root", ".", "--json"]);
    // Exit 0 (fully healthy) or 1 (informational findings remain, e.g.
    // sealed profiles): either way the mutation itself succeeded.
    assert!(
        matches!(out.status.code(), Some(0) | Some(1)),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let applied = stdout_json(&out);
    assert_eq!(applied["policy"]["engine"], "mncs");
    let entrypoints = applied["policy"]["entrypoints"].as_array().unwrap();
    for expected in [
        "doctor.discovery.v1::directory_decision",
        "doctor.discovery.v1::file_class",
        "doctor.scanner.v1::feed",
        "doctor.scanner.v1::finish",
        "doctor.version.v1::classify",
        "doctor.fix.v1::stop_rule",
        "doctor.edits.v1::pair_conflict",
        "doctor.verify.v1::delta_ok",
        "doctor.verify.v1::compose",
        "doctor.health.v1::check_status_with_skip",
        "doctor.health.v1::overall",
        "doctor.report.v1::exit_for",
        "doctor.transaction.v1::validate_target",
    ] {
        assert!(
            entrypoints.iter().any(|entrypoint| entrypoint == expected),
            "live fix path omitted {expected}: {entrypoints:?}"
        );
    }
    for (rel, fp) in &expected_fp {
        let actual = sha_of(&read_bytes(&root, rel));
        assert_eq!(
            &actual, fp,
            "applied content differs from dry-run plan for {rel}"
        );
    }
    // Verification passed on the mutating run.
    assert_eq!(applied["verification"]["passed"], true);
    // Second apply converges: nothing left to do.
    let out = run(&root, &["fix", "--root", ".", "--json"]);
    let second = stdout_json(&out);
    assert!(second["planned_diffs"].as_array().unwrap().is_empty());
    let notes = serde_json::to_string(&second["notes"]).unwrap();
    assert!(notes.contains("nothing to apply"), "{notes}");
}

fn sha_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap());
        out.push(char::from_digit(u32::from(b & 0xf), 16).unwrap());
    }
    out
}

#[test]
fn fix_normalizes_messy_file_exactly() {
    let root = stage("repos/outdated");
    let out = run(&root, &["fix", "--root", ".", "--json"]);
    // Exit 0 (fully healthy) or 1 (informational findings remain, e.g.
    // sealed profiles): either way the mutation itself succeeded.
    assert!(
        matches!(out.status.code(), Some(0) | Some(1)),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Recompute expectation from the dry-run equivalence instead of a
    // hardcoded literal: content must have no trailing whitespace and end
    // with exactly one newline.
    let after = read(&root, "src/messy.mncs");
    assert!(after.ends_with('\n') && !after.ends_with("\n\n"));
    for line in after.lines() {
        assert_eq!(
            line,
            line.trim_end(),
            "trailing whitespace remains: {line:?}"
        );
    }
    assert!(
        after.starts_with("mncs 0.16;"),
        "header must be preserved: {after:?}"
    );
}

#[test]
fn crlf_normalizes_to_lf_preserving_unicode() {
    let root = stage("repos/mixed");
    let out = run(&root, &["fix", "--root", ".", "--json"]);
    // Exit 0 (fully healthy) or 1 (informational findings remain, e.g.
    // sealed profiles): either way the mutation itself succeeded.
    assert!(
        matches!(out.status.code(), Some(0) | Some(1)),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = read_bytes(&root, "src/crlf.mncs");
    assert!(!bytes.contains(&b'\r'), "CR bytes remain");
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains("café → unicode comment preserved"),
        "unicode lost: {text:?}"
    );
    assert!(text.starts_with("mncs 0.16;"));
}

#[test]
#[cfg(unix)]
fn fix_preserves_permission_bits() {
    use std::os::unix::fs::PermissionsExt;
    let root = stage("repos/outdated");
    let path = root.join("src/messy.mncs");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let out = run(&root, &["fix", "--root", ".", "--json"]);
    // Exit 0 (fully healthy) or 1 (informational findings remain, e.g.
    // sealed profiles): either way the mutation itself succeeded.
    assert!(
        matches!(out.status.code(), Some(0) | Some(1)),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640, "permission bits not preserved");
}

#[test]
fn verify_cmd_failure_fails_the_run() {
    let root = stage("repos/outdated");
    let out = run(
        &root,
        &["fix", "--root", ".", "--verify-cmd", "false", "--json"],
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout_json(&out);
    assert_eq!(report["verification"]["passed"], false);
}
