use super::*;
use CaseStatus as S;
use FailureKind as F;
use PhaseOutcome::{Failed as Bad, Succeeded as Good};

const PHASES: [PhaseOutcome; 6] = [
    Good,
    Bad(F::Assertion),
    Bad(F::Other),
    Bad(F::Cancelled),
    Bad(F::TimedOut),
    Bad(F::LimitExceeded),
];
const FAILURES: [(FailureKind, CaseStatus); 5] = [
    (F::Assertion, S::Failed),
    (F::Other, S::Failed),
    (F::Cancelled, S::Cancelled),
    (F::TimedOut, S::TimedOut),
    (F::LimitExceeded, S::LimitExceeded),
];

#[test]
fn only_immediate_assertions_are_supported() {
    assert_eq!(AssertionMode::default(), AssertionMode::Immediate);
    assert_eq!(
        serde_json::to_string(&AssertionMode::default()).unwrap(),
        "\"immediate\""
    );
    assert_eq!(
        serde_json::from_str::<AssertionMode>("\"immediate\"").unwrap(),
        AssertionMode::Immediate
    );
    assert!(serde_json::from_str::<AssertionMode>("\"collected\"").is_err());
}

#[test]
fn expected_failure_requires_a_bounded_meaningful_reason() {
    assert_eq!(CaseExpectation::default().reason(), None);
    for reason in [
        "BUG-12",
        "  upstream regression  ",
        "தமிழ்",
        "<script>issue</script>",
    ] {
        assert_eq!(
            CaseExpectation::failure(reason).unwrap().reason(),
            Some(reason)
        );
    }
    for reason in [
        "".to_owned(),
        " \u{2003} ".into(),
        "line\nbreak".into(),
        "tab\there".into(),
        "null\0byte".into(),
        "escape\u{1b}".into(),
        "x".repeat(513),
        "é".repeat(257),
    ] {
        assert_eq!(
            CaseExpectation::failure(&reason).unwrap_err().code(),
            DiagnosticCode::RunConfiguration
        );
    }
    for reason in ["x".repeat(512), "é".repeat(256)] {
        assert_eq!(
            CaseExpectation::failure(&reason).unwrap().reason(),
            Some(reason.as_str())
        );
    }
}

#[test]
fn body_and_cleanup_matrix_enforces_strict_expectations_and_primary_failure() {
    // Columns: cleanup success, assertion, operational error, cancel, timeout, limit.
    // Rows have the same order for body outcomes. Cleanup cannot replace a primary.
    let normal = [
        [
            S::Succeeded,
            S::Failed,
            S::Failed,
            S::Cancelled,
            S::TimedOut,
            S::LimitExceeded,
        ],
        [S::Failed; 6],
        [S::Failed; 6],
        [S::Cancelled; 6],
        [S::TimedOut; 6],
        [S::LimitExceeded; 6],
    ];
    let expected = [
        [
            S::UnexpectedPass,
            S::Failed,
            S::Failed,
            S::Cancelled,
            S::TimedOut,
            S::LimitExceeded,
        ],
        [
            S::ExpectedFailure,
            S::Failed,
            S::Failed,
            S::Failed,
            S::Failed,
            S::Failed,
        ],
        [S::Failed; 6],
        [S::Cancelled; 6],
        [S::TimedOut; 6],
        [S::LimitExceeded; 6],
    ];
    for (expectation, table) in [
        (CaseExpectation::default(), normal),
        (CaseExpectation::failure("BUG-12").unwrap(), expected),
    ] {
        for (row, body) in PHASES.into_iter().enumerate() {
            for (column, cleanup) in PHASES.into_iter().enumerate() {
                let completion = CaseCompletion::executed(Good, Some(body), cleanup, None).unwrap();
                assert_eq!(
                    completion.decide(&expectation),
                    table[row][column],
                    "body={body:?}, cleanup={cleanup:?}, expectation={expectation:?}"
                );
                assert_eq!(completion.skip_reason(), None);
            }
        }
    }
}

#[test]
fn setup_failures_never_skip_or_satisfy_an_expectation() {
    for expectation in [
        CaseExpectation::default(),
        CaseExpectation::failure("BUG-12").unwrap(),
    ] {
        for (setup, status) in FAILURES {
            for cleanup in PHASES {
                assert_eq!(
                    CaseCompletion::executed(Bad(setup), None, cleanup, None)
                        .unwrap()
                        .decide(&expectation),
                    status
                );
            }
        }
    }
}

#[test]
fn late_parent_stops_override_the_verdict_after_all_phase_combinations() {
    let expected = CaseExpectation::failure("BUG-12").unwrap();
    for (stop, status) in [
        (ParentStop::Cancelled, S::Cancelled),
        (ParentStop::TimedOut, S::TimedOut),
        (ParentStop::LimitExceeded, S::LimitExceeded),
    ] {
        for setup in PHASES {
            for body in PHASES {
                for cleanup in PHASES {
                    let body = (setup == Good).then_some(body);
                    let completed =
                        CaseCompletion::executed(setup, body, cleanup, Some(stop)).unwrap();
                    assert_eq!(completed.decide(&expected), status);
                }
            }
        }
    }
}

#[test]
fn missing_or_unreachable_body_observations_are_configuration_errors() {
    for setup in PHASES {
        let body = if setup == Good { None } else { Some(Good) };
        assert_eq!(
            CaseCompletion::executed(setup, body, Good, None)
                .unwrap_err()
                .code(),
            DiagnosticCode::RunConfiguration
        );
    }
    for expectation in [
        CaseExpectation::default(),
        CaseExpectation::failure("BUG-12").unwrap(),
    ] {
        for reason in [SkipReason::SuiteSetupFailed, SkipReason::SuiteStopped] {
            let skipped = CaseCompletion::skipped(reason);
            assert_eq!(skipped.skip_reason(), Some(reason));
            assert_eq!(skipped.decide(&expectation), S::Skipped);
        }
        let interrupted = CaseCompletion::interrupted();
        assert_eq!(interrupted.decide(&expectation), S::Interrupted);
        assert_eq!(interrupted.skip_reason(), None);
    }
}

const STATUSES: [(CaseStatus, &str, &str, u8, bool); 9] = [
    (S::Succeeded, "succeeded", "succeeded", 0, true),
    (
        S::ExpectedFailure,
        "expected_failure",
        "expected failure",
        0,
        true,
    ),
    (S::Failed, "failed", "failed", 1, true),
    (
        S::UnexpectedPass,
        "unexpected_pass",
        "unexpected pass",
        1,
        true,
    ),
    (S::Skipped, "skipped", "skipped", 1, true),
    (S::Cancelled, "cancelled", "cancelled", 1, true),
    (S::TimedOut, "timed_out", "timed out", 1, true),
    (
        S::LimitExceeded,
        "limit_exceeded",
        "limit exceeded",
        1,
        true,
    ),
    (S::Interrupted, "interrupted", "interrupted", 1, false),
];

#[test]
fn console_json_html_and_exit_status_agree_for_every_verdict() {
    for (status, machine, label, exit, complete) in STATUSES {
        let failed = exit != 0;
        let view = status.view();
        assert_eq!(status.as_str(), machine);
        assert_eq!(status.label(), label);
        assert_eq!(status.exit_code(), exit);
        assert_eq!(view.status(), machine);
        assert_eq!(view.label(), label);
        assert_eq!(view.exit_code(), exit);
        assert_eq!(view.failed(), failed);
        assert_eq!(view.complete(), complete);
        assert_eq!(serde_json::to_value(status).unwrap(), machine);
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            serde_json::json!({"status": machine, "label": label, "failed": failed, "complete": complete})
        );
        assert_eq!(view.html().to_string(), format!("<span class=\"status status-{machine}\" data-outcome=\"{machine}\" data-failed=\"{failed}\" data-complete=\"{complete}\">{label}</span>"));
        assert!(view.html().to_string().len() < 256);
    }
}

#[test]
fn totals_keep_every_category_visible_and_never_count_expected_failure_as_success() {
    let mut all = CaseTotals::default();
    for (status, machine, _, exit, complete) in STATUSES {
        let mut single = CaseTotals::default();
        single.record_many(status, 3).unwrap();
        single.record(status).unwrap();
        assert_eq!(single.total(), 4);
        for (other, _, _, _, _) in STATUSES {
            assert_eq!(single.count(other), if other == status { 4 } else { 0 });
        }
        assert_eq!(serde_json::to_value(single).unwrap()[machine], 4);
        let verdict = single.finish(0, Delivery::Complete);
        assert_eq!(verdict.exit_code(), exit);
        assert_eq!(verdict.failed(), exit != 0);
        assert_eq!(verdict.complete(), complete);
        assert_eq!(verdict.cases(), &single);
        assert_eq!(verdict.fixture_failures(), 0);
        all.record(status).unwrap();
    }
    assert_eq!(all.total(), 9);
    assert_eq!(
        serde_json::to_value(all).unwrap(),
        serde_json::json!({
            "total": 9, "succeeded": 1, "expected_failure": 1, "failed": 1, "unexpected_pass": 1,
            "skipped": 1, "cancelled": 1, "timed_out": 1, "limit_exceeded": 1, "interrupted": 1
        })
    );
    assert_eq!(all.finish(0, Delivery::Complete).status(), S::Interrupted);
}

#[test]
fn fixtures_delivery_and_incomplete_observations_control_final_status() {
    for fixture_failures in [0, 1, usize::MAX] {
        for (delivery, status, complete) in [
            (
                Delivery::Complete,
                if fixture_failures == 0 {
                    S::Succeeded
                } else {
                    S::Failed
                },
                true,
            ),
            (Delivery::Failed, S::Failed, false),
            (Delivery::Interrupted, S::Interrupted, false),
        ] {
            for expected_count in [0, 5] {
                let mut totals = CaseTotals::default();
                totals
                    .record_many(S::ExpectedFailure, expected_count)
                    .unwrap();
                let verdict = totals.finish(fixture_failures, delivery);
                assert_eq!(verdict.status(), status);
                assert_eq!(verdict.complete(), complete);
                assert_eq!(verdict.fixture_failures(), fixture_failures);
                assert_eq!(verdict.view().complete(), complete);
                assert_eq!(verdict.view().exit_code(), verdict.exit_code());
                assert_eq!(
                    serde_json::to_value(verdict).unwrap(),
                    serde_json::json!({
                        "status": status, "complete": complete, "cases": totals,
                        "fixture_failures": fixture_failures, "delivery": delivery
                    })
                );
                assert!(verdict
                    .view()
                    .html()
                    .to_string()
                    .contains(&format!("data-complete=\"{complete}\"")));
                totals.record(S::Interrupted).unwrap();
                let interrupted = totals.finish(fixture_failures, delivery);
                assert_eq!(interrupted.status(), S::Interrupted);
                assert!(!interrupted.complete());
                assert_eq!(interrupted.exit_code(), 1);
            }
        }
    }
}

#[test]
fn count_overflow_is_atomic_and_zero_additions_are_valid() {
    for (status, _, _, _, _) in STATUSES {
        let mut totals = CaseTotals::default();
        totals.record_many(status, usize::MAX).unwrap();
        let before = totals;
        for (next, _, _, _, _) in STATUSES {
            totals.record_many(next, 0).unwrap();
            assert_eq!(
                totals.record(next).unwrap_err().code(),
                DiagnosticCode::RunConfiguration
            );
            assert_eq!(totals, before);
        }
    }
}

#[test]
fn existing_diagnostics_never_masquerade_as_expected_assertions() {
    for (code, kind, status) in [
        (DiagnosticCode::Cancelled, F::Cancelled, S::Cancelled),
        (DiagnosticCode::Timeout, F::TimedOut, S::TimedOut),
        (
            DiagnosticCode::ResourceLimit,
            F::LimitExceeded,
            S::LimitExceeded,
        ),
        (DiagnosticCode::Native, F::Other, S::Failed),
        (DiagnosticCode::NativePanic, F::Other, S::Failed),
        (DiagnosticCode::UndefinedVariable, F::Other, S::Failed),
        (DiagnosticCode::Output, F::Other, S::Failed),
    ] {
        assert_eq!(FailureKind::from_diagnostic_code(code), kind);
        assert_eq!(CaseStatus::from_diagnostic_code(code), status);
    }
}

#[test]
fn html_streaming_propagates_writer_failures() {
    struct Broken;
    impl std::fmt::Write for Broken {
        fn write_str(&mut self, _: &str) -> std::fmt::Result {
            Err(std::fmt::Error)
        }
    }
    use std::fmt::Write;
    assert!(write!(Broken, "{}", S::ExpectedFailure.view().html()).is_err());
}
