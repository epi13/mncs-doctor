use mncs_doctor::diagnostics::Severity;
use mncs_doctor::fix::Applicability;
use mncs_doctor::mncs_runtime::DoctorMncsRuntime;
use mncs_doctor::remediate::RemediationClass;

#[test]
fn commons_repair_classes_match_independent_vectors() {
    let policy = DoctorMncsRuntime::production().expect("frozen MNCS policy opens");
    let cases = [
        (Applicability::Safe, true, RemediationClass::SafeAutomatic),
        (Applicability::Safe, false, RemediationClass::Escalation),
        (
            Applicability::SemanticallyProven,
            true,
            RemediationClass::BoundedReconciliation,
        ),
        (
            Applicability::SemanticallyProven,
            false,
            RemediationClass::Escalation,
        ),
        (Applicability::Review, true, RemediationClass::Escalation),
        (Applicability::Manual, true, RemediationClass::Escalation),
    ];

    for (applicability, granted, expected) in cases {
        assert_eq!(
            policy.classify_repair(applicability, granted).unwrap(),
            expected,
            "applicability={applicability:?}, granted={granted}"
        );
    }
    assert_eq!(
        policy.classify_repair_code(99, true).unwrap(),
        RemediationClass::Escalation
    );
}

#[test]
fn commons_stop_dispositions_match_independent_vectors() {
    let policy = DoctorMncsRuntime::production().expect("frozen MNCS policy opens");
    let cases = [
        (0, false, false, Severity::Info),
        (1, false, true, Severity::Warning),
        (2, true, true, Severity::Warning),
        (2, false, true, Severity::Error),
        (3, true, true, Severity::Error),
        (99, true, true, Severity::Error),
    ];

    for (code, validated, escalate, severity) in cases {
        let result = policy.classify_stop_code(code, validated).unwrap();
        assert_eq!(result.escalate, escalate, "stop code {code}");
        assert_eq!(result.severity, severity, "stop code {code}");
    }
}

#[test]
fn commons_residual_dispositions_match_independent_vectors() {
    let policy = DoctorMncsRuntime::production().expect("frozen MNCS policy opens");
    let cases = [
        (0, true, false),
        (1, false, true),
        (2, false, false),
        (99, true, false),
    ];

    for (code, escalate, degraded) in cases {
        let result = policy.classify_residual_code(code).unwrap();
        assert_eq!(result.escalate, escalate, "severity code {code}");
        assert_eq!(result.degraded, degraded, "severity code {code}");
    }
}
