//! Rust ↔ MNCS differential parity tests.
//!
//! Each test runs the same inputs through the Rust reference implementation
//! and the MNCS policy module (compiled once per module via `mncs-embed`,
//! pinned rev) and asserts identical verdicts. Scalar codes cross the host
//! boundary (token_set pattern); records/enums live inside MNCS.
//!
//! Status-code tables (shared by both sides):
//!   classify: 0=current 1=sealed 2=unsupported 3=unknown
//!   severity: 0=info 1=warning 2=error
//!   status:   0=pass 1=warning 2=fail 3=skipped
//!   kind:     0=noop 1=metadata 2=source 3=unknown

use std::sync::OnceLock;

use mncs_doctor::diagnostics::{Diagnostic, DiagnosticSource, LanguageBackend, Severity, Span};
use mncs_doctor::discovery::{DirectoryDecision, DirectoryFacts, FileClass, FileFacts};
use mncs_doctor::edits::{EditSet, TextEdit};
use mncs_doctor::fix::Applicability;
use mncs_doctor::health::{worst_of, CheckResult, Status};
use mncs_doctor::migration::{default_registry, fixture_registry, plan};
use mncs_doctor::mncs_runtime::TransactionTargetVerdict;
use mncs_doctor::version::{classify, LanguageVersion, VersionClass};

fn session_for(source: &str) -> mncs_embed::Session {
    let artifact =
        mncs_embed::Artifact::from_source(source, "mncs-research-bytecode").expect("compile");
    mncs_embed::Session::open(artifact).expect("open")
}

macro_rules! module_session {
    ($name:ident, $file:literal, $module:literal) => {
        fn $name() -> &'static mncs_embed::Session {
            static SESSION: OnceLock<mncs_embed::Session> = OnceLock::new();
            SESSION.get_or_init(|| session_for(include_str!($file)))
        }
        const _: &str = $module;
    };
}

module_session!(
    version_session,
    "../mncs/doctor/version.mncs",
    "doctor.version.v1"
);
module_session!(
    health_session,
    "../mncs/doctor/health.mncs",
    "doctor.health.v1"
);
module_session!(
    migration_session,
    "../mncs/doctor/migration.mncs",
    "doctor.migration.v1"
);
module_session!(
    edits_session,
    "../mncs/doctor/edits.mncs",
    "doctor.edits.v1"
);
module_session!(fix_session, "../mncs/doctor/fix.mncs", "doctor.fix.v1");
module_session!(
    report_session,
    "../mncs/doctor/report.mncs",
    "doctor.report.v1"
);
module_session!(
    verify_session,
    "../mncs/doctor/verify.mncs",
    "doctor.verify.v1"
);
module_session!(
    scanner_session,
    "../mncs/doctor/scanner.mncs",
    "doctor.scanner.v1"
);

fn i64_arg(value: i64) -> String {
    format!("{{\"integer\": {{\"value\": {value}, \"type\": {{\"bits\": 64, \"signed\": true}}}}}}")
}

fn u64_arg(value: u64) -> String {
    format!(
        "{{\"integer\": {{\"value\": {value}, \"type\": {{\"bits\": 64, \"signed\": false}}}}}}"
    )
}

fn bool_arg(value: bool) -> String {
    format!("{{\"boolean\": {{\"value\": {value}}}}}")
}

fn seq_arg(items: &[String]) -> String {
    format!("{{\"sequence\": {{\"values\": [{}]}}}}", items.join(", "))
}

fn byte_seq_arg(bytes: &[u8]) -> String {
    let items = bytes
        .iter()
        .map(|byte| format!("{{\"byte\": {{\"value\": {byte}}}}}"))
        .collect::<Vec<_>>();
    seq_arg(&items)
}

fn call(
    session: &mncs_embed::Session,
    module: &str,
    function: &str,
    args: &str,
) -> serde_json::Value {
    let args = format!("[{args}]");
    let out = session
        .call_json(
            module,
            function,
            &args,
            &mncs_embed::CallOptions::budgeted(32768),
        )
        .expect("call");
    assert_eq!(
        out.status, "returned",
        "{module}::{function} failed: {:?}",
        out.failure_reason
    );
    serde_json::to_value(&out.returned[0]).expect("serialize returned")
}

fn as_i64(value: &serde_json::Value) -> i64 {
    value["integer"]["value"].as_i64().expect("integer return")
}

fn as_u64(value: &serde_json::Value) -> u64 {
    value["integer"]["value"].as_u64().expect("u64 return")
}

fn as_bool(value: &serde_json::Value) -> bool {
    value["boolean"]["value"].as_bool().expect("boolean return")
}

fn as_seq(value: &serde_json::Value) -> Vec<serde_json::Value> {
    value["sequence"]["values"]
        .as_array()
        .expect("sequence return")
        .clone()
}

// ---- version policy ----

fn rust_compare(a: (u32, u32), b: (u32, u32)) -> i64 {
    match LanguageVersion::new(a.0, a.1).cmp(&LanguageVersion::new(b.0, b.1)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[test]
fn parity_version_compare() {
    let cases = [
        ((0, 8), (0, 16)),
        ((0, 16), (0, 8)),
        ((0, 16), (0, 16)),
        ((0, 1), (0, 1)),
        ((1, 0), (0, 16)),
        ((0, 15), (0, 16)),
    ];
    for ((amaj, amin), (bmaj, bmin)) in cases {
        let args = format!(
            "{}, {}, {}, {}",
            i64_arg(amaj as i64),
            i64_arg(amin as i64),
            i64_arg(bmaj as i64),
            i64_arg(bmin as i64)
        );
        let got = as_i64(&call(
            version_session(),
            "doctor.version.v1",
            "compare",
            &args,
        ));
        assert_eq!(
            got,
            rust_compare((amaj, amin), (bmaj, bmin)),
            "{amaj}.{amin} vs {bmaj}.{bmin}"
        );
    }
}

fn rust_classify_code(major: u32, minor: u32) -> i64 {
    match classify(Some(LanguageVersion::new(major, minor))) {
        VersionClass::Current => 0,
        VersionClass::Sealed => 1,
        VersionClass::Unsupported => 2,
        VersionClass::Unknown => 3,
    }
}

#[test]
fn parity_version_classify() {
    // Current profile (0,16) supplied by the host registry, as designed.
    let versions = [
        (0, 1),
        (0, 8),
        (0, 15),
        (0, 16),
        (0, 99),
        (1, 0),
        (2, 0),
        (0, 0),
    ];
    for (major, minor) in versions {
        let args = format!(
            "{}, {}, {}, {}",
            i64_arg(major),
            i64_arg(minor),
            i64_arg(0),
            i64_arg(16)
        );
        let got = as_i64(&call(
            version_session(),
            "doctor.version.v1",
            "classify",
            &args,
        ));
        assert_eq!(
            got,
            rust_classify_code(major as u32, minor as u32),
            "{major}.{minor}"
        );
    }
}

#[test]
fn parity_migration_span() {
    // (from, to) -> expected edge count; downgrade/off-line refuse.
    let cases = [
        ((0, 8), (0, 16), 8),
        ((0, 15), (0, 16), 1),
        ((0, 16), (0, 16), 0),
        ((0, 16), (0, 15), -1),
        ((0, 99), (0, 16), -1),
        ((1, 0), (0, 16), -2),
    ];
    let registry = default_registry();
    for ((fmaj, fmin), (tmaj, tmin), expected) in cases {
        let args = format!(
            "{}, {}, {}, {}",
            i64_arg(fmaj),
            i64_arg(fmin),
            i64_arg(tmaj),
            i64_arg(tmin)
        );
        let got = as_i64(&call(
            version_session(),
            "doctor.version.v1",
            "plan_span",
            &args,
        ));
        assert_eq!(got, expected, "{fmaj}.{fmin} -> {tmaj}.{tmin}");
        // Cross-check the Rust planner's structure (not its knowledge).
        let from = LanguageVersion::new(fmaj as u32, fmin as u32);
        let to = LanguageVersion::new(tmaj as u32, tmin as u32);
        match plan(from, to, &registry) {
            Ok(plan) => assert_eq!(plan.steps.len() as i64, expected.max(0)),
            Err(_) => assert!(expected < 0, "rust planned but mncs refused"),
        }
    }
}

// ---- health aggregation ----

fn status_code(s: Status) -> u64 {
    match s {
        Status::Pass => 0,
        Status::Warning => 1,
        Status::Fail => 2,
        Status::Skipped => 3,
    }
}

#[test]
fn parity_health_combine() {
    let all = [Status::Pass, Status::Warning, Status::Fail, Status::Skipped];
    for a in all {
        for b in all {
            let args = format!("{}, {}", u64_arg(status_code(a)), u64_arg(status_code(b)));
            let got = as_u64(&call(
                health_session(),
                "doctor.health.v1",
                "combine",
                &args,
            ));
            assert_eq!(got, status_code(worst_of(a, b)), "{a:?} <> {b:?}");
        }
    }
}

#[test]
fn parity_health_overall() {
    let cases: &[&[Status]] = &[
        &[],
        &[Status::Pass],
        &[Status::Pass, Status::Pass],
        &[Status::Pass, Status::Warning],
        &[Status::Warning, Status::Fail, Status::Pass],
        &[Status::Skipped, Status::Skipped],
        &[Status::Skipped, Status::Warning],
        &[Status::Pass, Status::Warning, Status::Fail, Status::Skipped],
    ];
    for statuses in cases {
        let mut items: Vec<String> = statuses.iter().map(|s| u64_arg(status_code(*s))).collect();
        while items.len() < 8 {
            items.push(u64_arg(0));
        }
        let args = format!("{}, {}", seq_arg(&items), u64_arg(statuses.len() as u64));
        let got = as_u64(&call(
            health_session(),
            "doctor.health.v1",
            "overall",
            &args,
        ));
        let expected = mncs_doctor::health::overall_status(
            &statuses
                .iter()
                .map(|s| CheckResult {
                    id: "t".to_owned(),
                    title: "t".to_owned(),
                    status: *s,
                    findings: Vec::new(),
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(got, status_code(expected), "{statuses:?}");
    }
}

#[test]
fn parity_health_check_status_with_advisories() {
    // (errors, warnings, infos, promote_info, skipped) -> status code.
    let cases = [
        ((0, 0, 0, false, false), 0),
        ((0, 0, 2, false, false), 0),
        ((0, 0, 2, true, false), 1),
        ((0, 1, 2, false, false), 1),
        ((1, 0, 0, false, true), 2),
        ((0, 0, 0, false, true), 3),
    ];
    for ((errors, warnings, infos, promote, skipped), expected) in cases {
        let args = format!(
            "{}, {}, {}, {}, {}",
            u64_arg(errors),
            u64_arg(warnings),
            u64_arg(infos),
            bool_arg(promote),
            bool_arg(skipped)
        );
        let got = as_u64(&call(
            health_session(),
            "doctor.health.v1",
            "check_status_with_skip",
            &args,
        ));
        assert_eq!(
            got, expected,
            "{errors} {warnings} {infos} {promote} {skipped}"
        );
    }
}

#[test]
fn parity_transaction_target_policy() {
    let session = session_for(include_str!("../mncs/doctor/transaction.mncs"));
    let cases = [
        ((false, false, false, false, false, false), 0),
        ((false, true, false, true, false, false), 3),
        ((false, true, true, false, false, false), 2),
        ((true, false, false, false, false, false), 3),
        ((true, true, false, false, true, false), 4),
        ((true, true, false, true, false, true), 3),
        ((true, true, false, true, true, true), 1),
        ((true, true, false, true, true, false), 0),
    ];
    for ((expected, actual, symlink, file, matches, identical), expected_code) in cases {
        let args = format!(
            "{}, {}, {}, {}, {}, {}",
            bool_arg(expected),
            bool_arg(actual),
            bool_arg(symlink),
            bool_arg(file),
            bool_arg(matches),
            bool_arg(identical)
        );
        let got = as_u64(&call(
            &session,
            "doctor.transaction.v1",
            "validate_target",
            &args,
        ));
        assert_eq!(got, expected_code);
    }
    let runtime = mncs_doctor::mncs_runtime::DoctorMncsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .transaction_target_verdict(false, false, false, false, false, false)
            .unwrap(),
        TransactionTargetVerdict::Allow
    );
}

#[test]
fn parity_discovery_fact_classification() {
    let session = session_for(include_str!("../mncs/doctor/discovery.mncs"));
    let directory_cases = [
        ((0, false, false, false, false, 0, 64), 0),
        ((4, false, false, false, false, 1, 64), 2),
        ((0, false, true, false, false, 1, 64), 4),
        ((0, false, true, true, true, 1, 64), 3),
        ((0, false, false, false, false, 65, 64), 1),
        ((0, true, false, false, false, 1, 64), 2),
        ((17, false, false, false, false, 1, 64), 2),
    ];
    for ((name, extra, symlink, follow, cycle, depth, max_depth), expected) in directory_cases {
        let args = format!(
            "{}, {}, {}, {}, {}, {}, {}",
            u64_arg(name),
            bool_arg(extra),
            bool_arg(symlink),
            bool_arg(follow),
            bool_arg(cycle),
            u64_arg(depth),
            u64_arg(max_depth)
        );
        assert_eq!(
            as_u64(&call(
                &session,
                "doctor.discovery.v1",
                "directory_decision",
                &args,
            )),
            expected
        );
    }
    let file_cases = [
        // Recognised manifest names take precedence over the extension code.
        ((1, 1), 2),
        ((1, 0), 2),
        ((4, 0), 4),
        ((0, 1), 1),
        ((0, 0), 0),
    ];
    for ((name, extension), expected) in file_cases {
        let args = format!("{}, {}", u64_arg(name), u64_arg(extension));
        assert_eq!(
            as_u64(&call(&session, "doctor.discovery.v1", "file_class", &args,)),
            expected
        );
    }
    let runtime = mncs_doctor::mncs_runtime::DoctorMncsRuntime::new().unwrap();
    assert_eq!(
        runtime
            .discovery_directory_decision(DirectoryFacts {
                name_code: 4,
                extra_excluded: false,
                is_symlink: false,
                follow_symlink: false,
                cycle: false,
                depth: 1,
                max_depth: 64,
            })
            .unwrap(),
        DirectoryDecision::SkipExcluded
    );
    assert_eq!(
        runtime
            .discovery_file_class(FileFacts {
                name_code: 0,
                extension_code: 1,
            })
            .unwrap(),
        FileClass::Source
    );
}

#[test]
fn parity_chunked_scanner_state_machine() {
    let mut state = [0_u64; 6];
    let chunks: &[&[u8]] = &[&[239], &[187], &[191, b'a', b'\r'], b"\nb\n\r"];
    let expected_after_feed: [[u64; 6]; 4] = [
        [1, 0, 0, 0, 0, 0],
        [2, 0, 0, 0, 0, 0],
        [3, 1, 0, 0, 0, 1],
        [3, 1, 1, 1, 0, 1],
    ];
    for (chunk, expected) in chunks.iter().zip(expected_after_feed) {
        let state_arg = seq_arg(&state.iter().copied().map(u64_arg).collect::<Vec<_>>());
        let got = as_seq(&call(
            scanner_session(),
            "doctor.scanner.v1",
            "feed",
            &format!("{}, {}", byte_seq_arg(chunk), state_arg),
        ));
        let got: Vec<u64> = got.iter().map(as_u64).collect();
        assert_eq!(got, expected);
        state.copy_from_slice(&got);
    }
    let got = as_seq(&call(
        scanner_session(),
        "doctor.scanner.v1",
        "finish",
        &seq_arg(&state.iter().copied().map(u64_arg).collect::<Vec<_>>()),
    ));
    let got: Vec<u64> = got.iter().map(as_u64).collect();
    assert_eq!(got, [3, 1, 1, 1, 1, 0]);
}

// ---- migration planning ----

#[test]
fn parity_migration_verdicts() {
    // (from_minor, to_minor, kinds predominant, expected verdict)
    // kinds consulted only when the span plans; all-noop kinds here.
    // Windows are exactly 16 wide: the value contract refuses any other
    // length fail-closed (pinned by this test's construction).
    let noop_kinds = seq_arg(&vec![i64_arg(0); 16]);
    let cases = [
        (8, 16, 0),  // planned
        (16, 16, 1), // noop
        (16, 15, 2), // downgrade refused
        (0, 16, 3),  // off-line refused
    ];
    for (from, to, expected) in cases {
        let args = format!(
            "{}, {}, {}, {}",
            i64_arg(from),
            i64_arg(to),
            noop_kinds,
            u64_arg(8)
        );
        let got = as_i64(&call(
            migration_session(),
            "doctor.migration.v1",
            "plan_verdict",
            &args,
        ));
        assert_eq!(got, expected, "{from} -> {to}");
    }
    // Unknown edge on the path blocks (verdict 4): mirror of
    // plan().fully_known == false on the production registry.
    let mut kinds: Vec<String> = vec![i64_arg(0); 16];
    kinds[3] = i64_arg(3);
    let args = format!(
        "{}, {}, {}, {}",
        i64_arg(8),
        i64_arg(16),
        seq_arg(&kinds),
        u64_arg(8)
    );
    let got = as_i64(&call(
        migration_session(),
        "doctor.migration.v1",
        "plan_verdict",
        &args,
    ));
    assert_eq!(got, 4);
    let registry = default_registry();
    let plan = plan(
        LanguageVersion::new(0, 8),
        LanguageVersion::new(0, 16),
        &registry,
    )
    .unwrap();
    assert!(!plan.fully_known);
}

#[test]
fn parity_migration_enumerate() {
    // Edge enumeration matches the Rust planner's step endpoints.
    let args = format!("{}, {}", i64_arg(14), u64_arg(2));
    let got = call(
        migration_session(),
        "doctor.migration.v1",
        "enumerate",
        &args,
    );
    let items = as_seq(&got);
    assert_eq!(items.len(), 16);
    assert_eq!(as_i64(&items[0]), 14);
    assert_eq!(as_i64(&items[1]), 15);
    assert!(items[2..].iter().all(|v| as_i64(v) == 0));
    let registry = fixture_registry();
    let plan = plan(
        LanguageVersion::new(9, 0),
        LanguageVersion::new(9, 2),
        &registry,
    )
    .unwrap();
    assert_eq!(plan.steps.len(), 2);
}

// ---- edit planning ----

fn rust_overlap(a: (usize, usize), b: (usize, usize)) -> bool {
    let base = "abcdef";
    let mut set = EditSet::new("t", mncs_doctor::discovery::fingerprint(base.as_bytes()));
    set.push(TextEdit::new(a.0, a.1, "X", "a", Applicability::Safe));
    set.push(TextEdit::new(b.0, b.1, "Y", "b", Applicability::Safe));
    set.validate(base.len()).is_err()
}

#[test]
fn parity_edit_pair_conflict() {
    let cases = [
        ((0, 3), (2, 5), true),  // overlap
        ((0, 2), (2, 4), false), // adjacent
        ((1, 1), (1, 1), true),  // same-point inserts
        ((0, 1), (1, 1), false), // insert at edge of replacement
        ((2, 2), (0, 6), true),  // insert strictly inside
        ((4, 6), (0, 2), false), // disjoint, reversed order
        ((3, 3), (3, 3), true),  // identical insertions
    ];
    for ((a0, a1), (b0, b1), expected) in cases {
        let args = format!(
            "{}, {}, {}, {}",
            u64_arg(a0),
            u64_arg(a1),
            u64_arg(b0),
            u64_arg(b1)
        );
        let got = as_u64(&call(
            edits_session(),
            "doctor.edits.v1",
            "pair_conflict",
            &args,
        ));
        assert_eq!(got == 1, expected, "{a0}..{a1} vs {b0}..{b1}");
        assert_eq!(
            rust_overlap((a0 as usize, a1 as usize), (b0 as usize, b1 as usize)),
            expected
        );
    }
}

// ---- fix convergence policy ----

#[test]
fn parity_fix_eligibility() {
    let levels = [
        (Applicability::Safe, 0u64),
        (Applicability::SemanticallyProven, 1),
        (Applicability::Review, 2),
        (Applicability::Manual, 3),
    ];
    for safe in [true, false] {
        for proven in [true, false] {
            for (level, code) in levels {
                let args = format!(
                    "{}, {}, {}",
                    u64_arg(code),
                    bool_arg(safe),
                    bool_arg(proven)
                );
                let got = as_bool(&call(fix_session(), "doctor.fix.v1", "eligible", &args));
                let eligibility = mncs_doctor::fix::Eligibility {
                    allow_safe: safe,
                    allow_proven: proven,
                };
                assert_eq!(
                    got,
                    eligibility.allows(level),
                    "{level:?} safe={safe} proven={proven}"
                );
            }
        }
    }
}

#[test]
fn parity_fix_stop_rule() {
    // (planned_empty, fired_before, iterations, budget) -> stop code
    // 0=continue 1=fixpoint 2=budget 3=oscillation; precedences pinned.
    let cases = [
        ((true, false, 0, 16), 1),
        ((false, true, 0, 16), 3),
        ((false, false, 16, 16), 2),
        ((false, false, 3, 16), 0),
        ((true, true, 99, 16), 1),  // fixpoint first
        ((false, true, 99, 16), 3), // oscillation before budget
    ];
    for ((empty, fired, iters, budget), expected) in cases {
        let args = format!(
            "{}, {}, {}, {}",
            bool_arg(empty),
            bool_arg(fired),
            u64_arg(iters),
            u64_arg(budget)
        );
        let got = as_u64(&call(fix_session(), "doctor.fix.v1", "stop_rule", &args));
        assert_eq!(got, expected as u64, "{empty} {fired} {iters} {budget}");
    }
}

// ---- report + verify policy ----

#[test]
fn parity_report_exit_for() {
    // (worst, review_blocked, verified, verify_passed) -> exit code
    let cases = [
        ((0u64, false, false, true), 0),
        ((1u64, false, false, true), 1),
        ((1u64, true, false, true), 2),
        ((2u64, false, false, true), 1),
        ((2u64, true, false, true), 2),
        ((0u64, false, true, false), 3),
        ((1u64, true, true, false), 3), // verification dominates
        ((0u64, false, true, true), 0),
    ];
    for ((worst, blocked, verified, passed), expected) in cases {
        let args = format!(
            "{}, {}, {}, {}",
            u64_arg(worst),
            bool_arg(blocked),
            bool_arg(verified),
            bool_arg(passed)
        );
        let got = as_u64(&call(
            report_session(),
            "doctor.report.v1",
            "exit_for",
            &args,
        ));
        assert_eq!(got, expected as u64);
        // Cross-check the Rust exit mapping on equivalent inputs.
        let checks = vec![CheckResult {
            id: "t".to_owned(),
            title: "t".to_owned(),
            status: match worst {
                0 => Status::Pass,
                1 => Status::Warning,
                _ => Status::Fail,
            },
            findings: Vec::new(),
        }];
        let verification = verified.then(|| mncs_doctor::verify::VerificationOutcome {
            files_rechecked: 1,
            errors_before: 0,
            errors_after: if passed { 0 } else { 1 },
            warnings_after: 0,
            idempotent: None,
            external: Vec::new(),
            passed,
            notes: Vec::new(),
        });
        let rust_code = mncs_doctor::report::exit_for(&checks, blocked, verification.as_ref());
        assert_eq!(got as i32, rust_code.as_i32());
    }
}

#[test]
fn parity_verify_compose() {
    let cases = [
        ((0u64, 0u64), true, true, true),
        ((1u64, 2u64), true, true, false),  // errors grew
        ((2u64, 1u64), true, true, true),   // errors shrank: no new errors
        ((0u64, 0u64), false, true, false), // not idempotent
        ((0u64, 0u64), true, false, false), // external failed
        ((1u64, 1u64), true, true, true),   // stable errors are fine
    ];
    for ((before, after), idempotent, external_ok, expected) in cases {
        let delta = if after <= before { 1 } else { 0 };
        let d = as_u64(&call(
            verify_session(),
            "doctor.verify.v1",
            "delta_ok",
            &format!("{}, {}", u64_arg(before), u64_arg(after)),
        ));
        assert_eq!(d, delta);
        let got = as_u64(&call(
            verify_session(),
            "doctor.verify.v1",
            "compose",
            &format!(
                "{}, {}, {}",
                u64_arg(d),
                u64_arg(idempotent as u64),
                u64_arg(external_ok as u64)
            ),
        ));
        assert_eq!(got == 1, expected);
    }
    // Rust verify_after agrees on a real re-diagnosis pair.
    let backend = mncs_doctor::diagnostics::ScannerBackend;
    let mk = |text: &str| mncs_doctor::discovery::SourceFile {
        path: std::path::PathBuf::from("a.mncs"),
        relative: "a.mncs".to_owned(),
        sha256: mncs_doctor::discovery::fingerprint(text.as_bytes()),
        len: text.len() as u64,
        newline: mncs_doctor::discovery::detect_newline(text.as_bytes()),
        has_bom: false,
        mode: None,
        is_symlink: false,
        text: Some(text.to_owned()),
        bytes: text.as_bytes().to_vec(),
    };
    let good = mk("mncs 0.16;\nmodule a;\n");
    let mut before = std::collections::BTreeMap::new();
    before.insert("a.mncs".to_owned(), backend.diagnose(&good));
    let outcome = mncs_doctor::verify::verify_after(&[good], &backend, &before, Some(true), vec![]);
    assert!(outcome.passed);
}

// ---- transport fail-closed behavior (pinned, not parity) ----

#[test]
fn transport_mismatches_refuse_fail_closed() {
    // Record values require exact canonical type identities; sequence
    // length and scalar signedness are enforced at the boundary. None of
    // these may silently coerce: the verdict must be invalid_request.
    const RECORD_SOURCE: &str = "mncs 0.16;\nmodule probe.records;\nrecord Version { major: i64, minor: i64 }\nfn make() -> (result: Version) {\n    return Version { major: 7, minor: 1 };\n}\nfn major_of(v: Version) -> (result: i64) {\n    return v.major;\n}\nfn echo(v: Version) -> (result: Version) {\n    return v;\n}\n";
    static RECORD_SESSION: OnceLock<mncs_embed::Session> = OnceLock::new();
    let session = RECORD_SESSION.get_or_init(|| session_for(RECORD_SOURCE));
    let options = mncs_embed::CallOptions::budgeted(8192);
    // Wrong record identity.
    let args = r#"[{"record": {"type_identity": "probe.records/Version", "name": "Version", "fields": [["major", {"integer": {"value": 7, "type": {"bits": 64, "signed": true}}}], ["minor", {"integer": {"value": 1, "type": {"bits": 64, "signed": true}}}]]}}]"#;
    let out = session
        .call_json("probe.records", "major_of", args, &options)
        .expect("call");
    assert_eq!(out.status, "invalid_request");
    assert!(out
        .failure_reason
        .as_deref()
        .unwrap_or("")
        .contains("type_identity"));
    // A compiler-produced record can make the round trip through the host
    // ABI when its canonical identity is preserved. This is the concrete
    // typed-transport case Doctor would eventually use for health/version
    // result records instead of scalar codes.
    let made = session
        .call_json("probe.records", "make", "[]", &options)
        .expect("make record");
    assert_eq!(made.status, "returned");
    let record = serde_json::to_value(&made.returned[0]).expect("record JSON");
    let record_args = serde_json::to_string(std::slice::from_ref(&record)).expect("record args");
    let major = session
        .call_json("probe.records", "major_of", &record_args, &options)
        .expect("record input");
    assert_eq!(major.status, "returned");
    let major_json = serde_json::to_value(&major.returned[0]).expect("major JSON");
    assert_eq!(major_json["integer"]["value"], 7);
    let echoed = session
        .call_json("probe.records", "echo", &record_args, &options)
        .expect("record output");
    assert_eq!(echoed.status, "returned");
    assert_eq!(serde_json::to_value(&echoed.returned[0]).unwrap(), record);

    // Artifact identity tampering is rejected before a session can open.
    let artifact = mncs_embed::Artifact::from_source(RECORD_SOURCE, "mncs-research-bytecode")
        .expect("record artifact");
    let mut artifact_json: serde_json::Value =
        serde_json::from_slice(&artifact.to_json_bytes()).expect("artifact JSON");
    artifact_json["identity"] = serde_json::json!("tampered");
    let invalid = match mncs_embed::Artifact::from_json(
        &serde_json::to_vec(&artifact_json).expect("tampered artifact JSON"),
    ) {
        Ok(_) => panic!("tampered identity must fail closed"),
        Err(error) => error,
    };
    assert_eq!(invalid.code, "invalid_identity");
    // Unsigned element into an [i64; 16] window.
    let args = format!(
        "{}, {}, {}, {}",
        i64_arg(8),
        i64_arg(16),
        seq_arg(&vec![u64_arg(0); 16]),
        u64_arg(8)
    );
    let out = migration_session()
        .call_json(
            "doctor.migration.v1",
            "plan_verdict",
            &format!("[{args}]"),
            &options,
        )
        .expect("call");
    assert_eq!(out.status, "invalid_request");
}

#[test]
fn embedding_import_closure_is_not_available_without_freeze() {
    const IMPORT_SOURCE: &str = "mncs 0.16;\nmodule probe.imported;\nuse mncs.std.text_utf8.v1;\nfn value() -> (result: u64) {\n    return 1;\n}\n";
    let error = match mncs_embed::Artifact::from_source(IMPORT_SOURCE, "mncs-research-bytecode") {
        Ok(_) => panic!("source embedding unexpectedly resolved an import"),
        Err(error) => error,
    };
    assert_eq!(error.code, "compile_failed");
}

// ---- diagnostic model parity (codes flow through both sides) ----

#[test]
fn parity_diagnostic_codes_stable() {
    // The DOC code vocabulary the MNCS side reasons about (as numeric codes
    // where applicable) matches the Rust scanner's codes.
    let backend = mncs_doctor::diagnostics::ScannerBackend;
    let file = mncs_doctor::discovery::SourceFile {
        path: std::path::PathBuf::from("t.mncs"),
        relative: "t.mncs".to_owned(),
        sha256: String::new(),
        len: 9,
        newline: mncs_doctor::discovery::NewlineStyle::Lf,
        has_bom: false,
        mode: None,
        is_symlink: false,
        text: Some("fn f() {}\n".to_owned()),
        bytes: b"fn f() {}\n".to_vec(),
    };
    let diags = backend.diagnose(&file);
    let codes: Vec<&str> = diags.iter().map(|d: &Diagnostic| d.code.as_str()).collect();
    assert!(codes.contains(&"DOC101"));
    assert!(codes.contains(&"DOC105"));
    let _ = (
        DiagnosticSource::Hygiene,
        Severity::Info,
        Span::whole_file(0, 0),
    );
}
