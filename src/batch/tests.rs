use super::*;
use botwork::core::diagnostic::DiagnosticCode;

#[test]
fn aggregate_verdict_controls_cli_success_and_rejects_inconsistent_counts() {
    for (total, failed, skipped, fixtures_failed, success) in [
        (0, 0, 0, 0, true),
        (3, 0, 0, 0, true),
        (3, 1, 0, 0, false),
        (3, 0, 1, 0, false),
        (3, 0, 0, 1, false),
        (3, 1, 1, 2, false),
    ] {
        let outcome = Outcome {
            total,
            failed,
            skipped,
            fixtures_failed,
            failed_cases: vec![],
        };
        assert_eq!(outcome.result().is_ok(), success);
    }
    for (total, failed, skipped) in [(0, 1, 0), (0, 0, 1), (2, 1, 2), (usize::MAX, usize::MAX, 1)] {
        let error = Outcome {
            total,
            failed,
            skipped,
            fixtures_failed: 0,
            failed_cases: vec![],
        }
        .result()
        .unwrap_err();
        assert!(
            matches!(error, CliError::Script(ref error) if error.code() == DiagnosticCode::RunConfiguration)
        );
    }
}

#[test]
fn console_classifies_execution_stops_without_inspecting_error_text() {
    for (error, expected) in [
        (BWErr::Cancelled("example".into()), "cancelled"),
        (BWErr::Timeout("example".into()), "timed out"),
        (
            BWErr::ResourceLimit {
                resource: "example",
                limit: 1,
            },
            "limit exceeded",
        ),
        (
            BWErr::NativeError("expected assertion timed out".into()),
            "failed",
        ),
    ] {
        let diagnostic = Diagnostic::new(error);
        assert_eq!(outcome(&CliError::Script(diagnostic.clone())), expected);
        assert_eq!(
            outcome(&CliError::SourceLimit {
                file: "source".into(),
                source: Box::new(diagnostic)
            }),
            expected
        );
    }
    assert_eq!(
        outcome(&CliError::Batch {
            failed: 1,
            total: 1
        }),
        "failed"
    );
}

#[tokio::test]
async fn invalid_admission_limits_fail_before_preparing_inputs_or_paths() {
    for jobs in [0, 65, usize::MAX] {
        let error = run(
            vec![PathBuf::from("unreachable.botwork")],
            jobs,
            Configuration {
                artifacts: None,
                debug: false,
                files: vec![],
                settings: vec!["malformed input".into()],
                limits: RunLimits::default(),
                timeout_ms: None,
                suite_timeout_ms: None,
            },
        )
        .await
        .unwrap_err();
        let CliError::Script(error) = error else {
            panic!("structured configuration failure")
        };
        assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
        assert!(error.to_string().contains("Parallel jobs"));
    }
}
