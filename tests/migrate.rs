//! `migrate` command: planning, blocked unknown edges, fixture migration.

mod common;

use common::{read, run, stage, stdout_json};

/// Absolute path to the committed 9.x registry fixture.
fn registry_arg() -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/migration/registry-9x.json")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn plan_shows_composed_path_but_marks_unknown() {
    let root = stage("repos/outdated");
    let out = run(
        &root,
        &[
            "migrate", "--root", ".", "--to", "latest", "--plan", "--json",
        ],
    );
    // Unknown edges: planned but not fully known -> review required (2).
    assert_eq!(
        out.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout_json(&out);
    let notes = serde_json::to_string(&report["notes"]).unwrap();
    assert!(notes.contains("0.8 -> 0.9"), "{notes}");
    assert!(notes.contains("0.15 -> 0.16"), "{notes}");
    assert!(report["planned_diffs"].as_array().unwrap().is_empty());
    // No mutation from planning.
    assert!(read(&root, "src/main.mncs").starts_with("mncs 0.8;"));
}

#[test]
fn apply_refuses_unrecorded_edges() {
    let root = stage("repos/outdated");
    let out = run(
        &root,
        &[
            "migrate", "--root", ".", "--to", "latest", "--apply", "--json",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(4),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("blocked") || stderr.contains("unrecorded"),
        "{stderr}"
    );
    assert!(
        read(&root, "src/main.mncs").starts_with("mncs 0.8;"),
        "must not mutate"
    );
}

#[test]
fn downgrade_has_no_path() {
    let root = stage("repos/outdated");
    let out = run(
        &root,
        &["migrate", "--root", ".", "--to", "0.1", "--plan", "--json"],
    );
    assert_ne!(out.status.code(), Some(0));
    let report = stdout_json(&out);
    let notes = serde_json::to_string(&report["notes"]).unwrap();
    assert!(notes.contains("unmigratable"), "{notes}");
}

#[test]
fn headerless_file_is_unmigratable_with_guidance() {
    let root = stage("repos/mixed");
    let out = run(
        &root,
        &[
            "migrate", "--root", ".", "--to", "latest", "--plan", "--json",
        ],
    );
    let report = stdout_json(&out);
    let notes = serde_json::to_string(&report["notes"]).unwrap();
    assert!(
        notes.contains("headless.mncs") && notes.contains("header"),
        "{notes}"
    );
}

#[test]
fn fixture_migration_applies_and_converges() {
    let root = stage("repos/fixture-9x");
    let registry = registry_arg();
    // Plan: fully known, exit 0.
    let out = run(
        &root,
        &[
            "migrate",
            "--root",
            ".",
            "--to",
            "9.2",
            "--registry",
            &registry,
            "--plan",
            "--json",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan_report = stdout_json(&out);
    let notes = serde_json::to_string(&plan_report["notes"]).unwrap();
    assert!(notes.contains("fully_known=true"), "{notes}");
    // Dry-run predicts the change without mutating.
    let out = run(
        &root,
        &[
            "migrate",
            "--root",
            ".",
            "--to",
            "9.2",
            "--registry",
            &registry,
            "--dry-run",
            "--json",
        ],
    );
    let dry = stdout_json(&out);
    assert_eq!(dry["planned_diffs"].as_array().unwrap().len(), 1);
    assert!(
        read(&root, "src/demo.mncs").contains("oldfn"),
        "dry-run mutated"
    );
    // Apply.
    let out = run(
        &root,
        &[
            "migrate",
            "--root",
            ".",
            "--to",
            "9.2",
            "--registry",
            &registry,
            "--apply",
            "--json",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let applied = stdout_json(&out);
    let after = read(&root, "src/demo.mncs");
    assert!(after.starts_with("mncs 9.2;"), "{after:?}");
    assert!(
        !after.contains("oldfn") && after.contains("newfn"),
        "{after:?}"
    );
    assert_eq!(applied["verification"]["passed"], true);
    // migrate(old) -> valid; migrate again -> no changes (idempotent).
    let out = run(
        &root,
        &[
            "migrate",
            "--root",
            ".",
            "--to",
            "9.2",
            "--registry",
            &registry,
            "--plan",
            "--json",
        ],
    );
    let replan = stdout_json(&out);
    let notes = serde_json::to_string(&replan["notes"]).unwrap();
    assert!(notes.contains("no-op"), "{notes}");
    assert!(replan["planned_diffs"].as_array().unwrap().is_empty());
    // Migration record carries provenance.
    let records = applied["migrations"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    let steps = records[0]["steps"].as_array().unwrap();
    assert!(!steps.is_empty());
    for step in steps {
        assert!(!step["provenance"].as_str().unwrap().is_empty());
        assert!(!step["from_fingerprint"].as_str().unwrap().is_empty());
    }
}
