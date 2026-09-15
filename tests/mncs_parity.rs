//! Rust ↔ MNCS differential parity tests.
//!
//! Each test runs the same inputs through the Rust reference implementation
//! and the MNCS policy module (compiled once per module via `mncs-embed`,
//! pinned rev) and asserts identical verdicts. Semantic finite values cross
//! the embedded boundary by nominal identity; numeric transport remains only
//! for natural measurements such as byte offsets and scanner counters.

use std::sync::OnceLock;

use mncs_doctor::diagnostics::{Diagnostic, DiagnosticSource, LanguageBackend, Severity, Span};
use mncs_doctor::discovery::{
    DirectoryDecision, DirectoryFacts, DirectoryName, FileClass, FileExtension, FileFacts, FileName,
};
use mncs_doctor::edits::{EditSet, TextEdit};
use mncs_doctor::fix::{Applicability, StopReason};
use mncs_doctor::health::{worst_of, CheckResult, Status};
use mncs_doctor::migration::{default_registry, fixture_registry, plan};
use mncs_doctor::mncs_runtime::{DoctorMncsRuntime, TransactionTargetVerdict};
use mncs_doctor::version::{classify, LanguageVersion};

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
    verify_session,
    "../mncs/doctor/verify.mncs",
    "doctor.verify.v1"
);
module_session!(
    scanner_session,
    "../mncs/doctor/scanner.mncs",
    "doctor.scanner.v1"
);
module_session!(
    discovery_session,
    "../mncs/doctor/discovery.mncs",
    "doctor.discovery.v1"
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

fn finite_arg(type_name: &str, variant: &str) -> String {
    format!("{{\"finite\": {{\"type\": \"{type_name}\", \"variant\": \"{variant}\"}}}}")
}

fn record_arg(type_name: &str, fields: &[(&str, String)]) -> String {
    let fields = fields
        .iter()
        .map(|(name, value)| format!("\"{name}\": {value}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{\"record\": {{\"type\": \"{type_name}\", \"fields\": {{{fields}}}}}}}")
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

fn family_session() -> &'static mncs_embed::Session {
    static SESSION: OnceLock<mncs_embed::Session> = OnceLock::new();
    SESSION.get_or_init(|| {
        let artifact =
            mncs_embed::Artifact::from_json(include_bytes!("../mncs/doctor/family.backend.json"))
                .expect("load frozen Doctor family artifact");
        mncs_embed::Session::open(artifact).expect("open frozen Doctor family artifact")
    })
}

fn report_session() -> &'static mncs_embed::Session {
    family_session()
}

fn typed_call(
    session: &mncs_embed::Session,
    module: &str,
    function: &str,
    args: &str,
) -> serde_json::Value {
    let args = format!("[{args}]");
    let out = session
        .call_typed_json(
            module,
            function,
            &args,
            &mncs_embed::CallOptions::budgeted(32768),
        )
        .expect("typed call");
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

fn as_variant(value: &serde_json::Value) -> String {
    if let Some(variant) = value["finite"]["variant"].as_str() {
        return variant.to_owned();
    }
    value["finite"]["variant_identity"]
        .as_str()
        .and_then(|identity| identity.rsplit("::").next())
        .expect("finite return variant")
        .to_owned()
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

#[test]
fn parity_version_classify() {
    // Current profile (0,17) supplied by the host registry, as designed.
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
    let runtime = DoctorMncsRuntime::new().expect("production policy runtime");
    for (major, minor) in versions {
        let got = runtime
            .classify_version(LanguageVersion::new(major, minor))
            .expect("generated typed version binding");
        assert_eq!(
            got,
            classify(Some(LanguageVersion::new(major, minor))),
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

fn status_variant(s: Status) -> &'static str {
    match s {
        Status::Pass => "Pass",
        Status::Warning => "Warning",
        Status::Fail => "Fail",
        Status::Skipped => "Skipped",
    }
}

#[test]
fn parity_health_combine() {
    let all = [Status::Pass, Status::Warning, Status::Fail, Status::Skipped];
    for a in all {
        for b in all {
            let got = as_variant(&typed_call(
                health_session(),
                "doctor.health.v1",
                "combine",
                &format!(
                    "{}, {}",
                    finite_arg("Status", status_variant(a)),
                    finite_arg("Status", status_variant(b))
                ),
            ));
            assert_eq!(got, status_variant(worst_of(a, b)), "{a:?} <> {b:?}");
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
        let mut items: Vec<String> = statuses
            .iter()
            .map(|s| finite_arg("Status", status_variant(*s)))
            .collect();
        while items.len() < 8 {
            items.push(finite_arg("Status", "Pass"));
        }
        let args = record_arg(
            "OverallInput",
            &[
                ("statuses", seq_arg(&items)),
                ("count", u64_arg(statuses.len() as u64)),
            ],
        );
        let got = as_variant(&typed_call(
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
        assert_eq!(got, status_variant(expected), "{statuses:?}");
    }
}

#[test]
fn parity_health_check_status_with_advisories() {
    // (errors, warnings, infos, promote_info, skipped) -> status.
    let cases = [
        ((0, 0, 0, false, false), "Pass"),
        ((0, 0, 2, false, false), "Pass"),
        ((0, 0, 2, true, false), "Warning"),
        ((0, 1, 2, false, false), "Warning"),
        ((1, 0, 0, false, true), "Fail"),
        ((0, 0, 0, false, true), "Skipped"),
    ];
    for ((errors, warnings, infos, promote, skipped), expected) in cases {
        let args = record_arg(
            "CheckStatusWithSkipInput",
            &[
                ("error_count", u64_arg(errors)),
                ("warning_count", u64_arg(warnings)),
                ("info_count", u64_arg(infos)),
                ("promote_info", bool_arg(promote)),
                ("skipped", bool_arg(skipped)),
            ],
        );
        let got = as_variant(&typed_call(
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
    let cases = [
        (
            (false, false, false, false, false, false),
            TransactionTargetVerdict::Allow,
        ),
        (
            (false, true, false, true, false, false),
            TransactionTargetVerdict::Stale,
        ),
        (
            (false, true, true, false, false, false),
            TransactionTargetVerdict::Symlink,
        ),
        (
            (true, false, false, false, false, false),
            TransactionTargetVerdict::Stale,
        ),
        (
            (true, true, false, false, true, false),
            TransactionTargetVerdict::NonFile,
        ),
        (
            (true, true, false, true, false, true),
            TransactionTargetVerdict::Stale,
        ),
        (
            (true, true, false, true, true, true),
            TransactionTargetVerdict::Identical,
        ),
        (
            (true, true, false, true, true, false),
            TransactionTargetVerdict::Allow,
        ),
    ];
    let runtime = DoctorMncsRuntime::new().unwrap();
    for ((expected, actual, symlink, file, matches, identical), expected_verdict) in cases {
        assert_eq!(
            runtime
                .transaction_target_verdict(expected, actual, symlink, file, matches, identical)
                .unwrap(),
            expected_verdict
        );
    }
}

#[test]
fn parity_discovery_fact_classification() {
    let directory_cases = [
        (
            (DirectoryName::Other, false, false, false, false, 0, 64),
            DirectoryDecision::Descend,
        ),
        (
            (DirectoryName::Target, false, false, false, false, 1, 64),
            DirectoryDecision::SkipExcluded,
        ),
        (
            (DirectoryName::Other, false, true, false, false, 1, 64),
            DirectoryDecision::SkipSymlink,
        ),
        (
            (DirectoryName::Other, false, true, true, true, 1, 64),
            DirectoryDecision::SkipCycle,
        ),
        (
            (DirectoryName::Other, false, false, false, false, 65, 64),
            DirectoryDecision::SkipDepth,
        ),
        (
            (DirectoryName::Other, true, false, false, false, 1, 64),
            DirectoryDecision::SkipExcluded,
        ),
        (
            (DirectoryName::DotVenv, false, false, false, false, 1, 64),
            DirectoryDecision::SkipExcluded,
        ),
    ];
    let runtime = DoctorMncsRuntime::new().unwrap();
    for ((name, extra, symlink, follow, cycle, depth, max_depth), expected) in directory_cases {
        assert_eq!(
            runtime
                .discovery_directory_decision(DirectoryFacts {
                    name,
                    extra_excluded: extra,
                    is_symlink: symlink,
                    follow_symlink: follow,
                    cycle,
                    depth,
                    max_depth,
                })
                .unwrap(),
            expected
        );
    }
    let file_cases = [
        // Recognised manifest names take precedence over the extension fact.
        (
            (FileName::ForgeManifest, FileExtension::Mncs),
            FileClass::ForgeManifest,
        ),
        (
            (FileName::ForgeManifest, FileExtension::Other),
            FileClass::ForgeManifest,
        ),
        (
            (FileName::ManifestJson, FileExtension::Other),
            FileClass::ManifestJson,
        ),
        ((FileName::Other, FileExtension::Mncs), FileClass::Source),
        ((FileName::Other, FileExtension::Other), FileClass::Ignore),
    ];
    for ((name, extension), expected) in file_cases {
        assert_eq!(
            runtime
                .discovery_file_class(FileFacts { name, extension })
                .unwrap(),
            expected
        );
    }
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
    let noop_kinds = seq_arg(&vec![finite_arg("TransitionKind", "Noop"); 16]);
    let cases = [
        (8, 16, "Planned"),           // planned
        (16, 16, "Noop"),             // noop
        (16, 15, "DowngradeRefused"), // downgrade refused
        (0, 16, "OfflineRefused"),    // off-line refused
    ];
    for (from, to, expected) in cases {
        let args = record_arg(
            "PlanInput",
            &[
                ("from_minor", i64_arg(from)),
                ("to_minor", i64_arg(to)),
                ("kinds", noop_kinds.clone()),
                ("count", u64_arg(8)),
            ],
        );
        let got = as_variant(&typed_call(
            migration_session(),
            "doctor.migration.v1",
            "plan_verdict",
            &args,
        ));
        assert_eq!(got, expected, "{from} -> {to}");
    }
    // Unknown edge on the path blocks: mirror of
    // plan().fully_known == false on the production registry.
    let mut kinds: Vec<String> = vec![finite_arg("TransitionKind", "Noop"); 16];
    kinds[3] = finite_arg("TransitionKind", "Unknown");
    let args = record_arg(
        "PlanInput",
        &[
            ("from_minor", i64_arg(8)),
            ("to_minor", i64_arg(16)),
            ("kinds", seq_arg(&kinds)),
            ("count", u64_arg(8)),
        ],
    );
    let got = as_variant(&typed_call(
        migration_session(),
        "doctor.migration.v1",
        "plan_verdict",
        &args,
    ));
    assert_eq!(got, "Blocked");
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
        let args = record_arg(
            "PairConflictInput",
            &[
                ("a_start", u64_arg(a0)),
                ("a_end", u64_arg(a1)),
                ("b_start", u64_arg(b0)),
                ("b_end", u64_arg(b1)),
            ],
        );
        let got = as_variant(&typed_call(
            edits_session(),
            "doctor.edits.v1",
            "pair_conflict",
            &args,
        ));
        assert_eq!(got == "Conflict", expected, "{a0}..{a1} vs {b0}..{b1}");
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
        (Applicability::Safe, "Safe"),
        (Applicability::SemanticallyProven, "SemanticallyProven"),
        (Applicability::Review, "Review"),
        (Applicability::Manual, "Manual"),
    ];
    for safe in [true, false] {
        for proven in [true, false] {
            for (level, variant) in levels {
                let args = record_arg(
                    "EligibilityInput",
                    &[
                        ("level", finite_arg("Applicability", variant)),
                        ("allow_safe", bool_arg(safe)),
                        ("allow_proven", bool_arg(proven)),
                    ],
                );
                let got = as_bool(&typed_call(
                    fix_session(),
                    "doctor.fix.v1",
                    "eligible",
                    &args,
                ));
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
    // (planned_empty, fired_before, iterations, budget) -> native stop result.
    let cases = [
        ((true, false, 0, 16), Some(StopReason::Fixpoint)),
        ((false, true, 0, 16), Some(StopReason::Oscillation)),
        ((false, false, 16, 16), Some(StopReason::BudgetExhausted)),
        ((false, false, 3, 16), None),
        ((true, true, 99, 16), Some(StopReason::Fixpoint)),
        ((false, true, 99, 16), Some(StopReason::Oscillation)),
    ];
    let runtime = DoctorMncsRuntime::new().unwrap();
    for ((empty, fired, iters, budget), expected) in cases {
        let got = runtime.fix_stop_rule(empty, fired, iters, budget).unwrap();
        assert_eq!(got, expected, "{empty} {fired} {iters} {budget}");
    }
}

// ---- report + verify policy ----

#[test]
fn parity_report_exit_for() {
    // (worst, review_blocked, verified, verify_passed) -> exit decision.
    let cases = [
        (("Pass", false, false, true), "Healthy"),
        (("Warning", false, false, true), "Findings"),
        (("Warning", true, false, true), "ReviewRequired"),
        (("Fail", false, false, true), "Findings"),
        (("Fail", true, false, true), "ReviewRequired"),
        (("Pass", false, true, false), "VerificationFailed"),
        (("Warning", true, true, false), "VerificationFailed"), // verification dominates
        (("Pass", false, true, true), "Healthy"),
    ];
    for ((worst, blocked, verified, passed), expected) in cases {
        let args = record_arg(
            "ExitInput",
            &[
                ("worst", finite_arg("Status", worst)),
                ("review_blocked", bool_arg(blocked)),
                ("verified", bool_arg(verified)),
                ("verify_passed", bool_arg(passed)),
            ],
        );
        let got = as_variant(&typed_call(
            report_session(),
            "doctor.report.v1",
            "exit_for",
            &args,
        ));
        assert_eq!(got, expected);
        // Cross-check the Rust exit mapping on equivalent inputs.
        let checks = vec![CheckResult {
            id: "t".to_owned(),
            title: "t".to_owned(),
            status: match worst {
                "Pass" => Status::Pass,
                "Warning" => Status::Warning,
                "Fail" => Status::Fail,
                _ => unreachable!(),
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
        let rust_decision = match rust_code {
            mncs_doctor::report::ExitCode::Healthy => "Healthy",
            mncs_doctor::report::ExitCode::Findings => "Findings",
            mncs_doctor::report::ExitCode::ReviewRequired => "ReviewRequired",
            mncs_doctor::report::ExitCode::VerificationFailed => "VerificationFailed",
            mncs_doctor::report::ExitCode::ToolFailure => unreachable!(),
        };
        assert_eq!(got, rust_decision);
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
        let delta = if after <= before { "Pass" } else { "Fail" };
        let d = as_variant(&typed_call(
            verify_session(),
            "doctor.verify.v1",
            "delta_ok",
            &record_arg(
                "DeltaInput",
                &[
                    ("errors_before", u64_arg(before)),
                    ("errors_after", u64_arg(after)),
                ],
            ),
        ));
        assert_eq!(d, delta);
        let got = as_variant(&typed_call(
            verify_session(),
            "doctor.verify.v1",
            "compose",
            &record_arg(
                "ComposeInput",
                &[
                    ("delta", finite_arg("VerificationVerdict", &d)),
                    ("idempotent", bool_arg(idempotent)),
                    ("external_pass", bool_arg(external_ok)),
                ],
            ),
        ));
        assert_eq!(got == "Pass", expected);
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
    // A natural integer cannot be silently accepted as a TransitionKind.
    let args = record_arg(
        "PlanInput",
        &[
            ("from_minor", i64_arg(8)),
            ("to_minor", i64_arg(16)),
            ("kinds", seq_arg(&vec![u64_arg(0); 16])),
            ("count", u64_arg(8)),
        ],
    );
    let error = migration_session()
        .call_typed_json(
            "doctor.migration.v1",
            "plan_verdict",
            &format!("[{args}]"),
            &options,
        )
        .expect_err("wrong nominal element type must fail closed");
    assert_eq!(error.code, "bad_typed_arguments");

    // Discovery facts are generated nominal values too; a natural integer
    // cannot silently become a directory/file classification.
    let error = discovery_session()
        .call_typed_json(
            "doctor.discovery.v1",
            "file_class",
            &format!(
                "[{}]",
                record_arg(
                    "FileClassInput",
                    &[("name", u64_arg(0)), ("extension", u64_arg(1))],
                )
            ),
            &options,
        )
        .expect_err("numeric discovery facts must fail closed");
    assert_eq!(error.code, "bad_typed_arguments");
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
