use super::*;
use botwork::core::diagnostic::DiagnosticCode;

fn totals(statuses: &[(CaseStatus, usize)]) -> CaseTotals {
    let mut totals = CaseTotals::default();
    for &(status, count) in statuses {
        totals.record_many(status, count).unwrap();
    }
    totals
}

#[test]
fn summary_and_exit_decision_share_one_verdict() {
    use CaseStatus::*;
    for (statuses, fixtures, cases, line, success) in [
        (
            vec![],
            0,
            false,
            "[batch] 0 runs: 0 succeeded, 0 failed",
            true,
        ),
        (
            vec![(Succeeded, 3)],
            0,
            false,
            "[batch] 3 runs: 3 succeeded, 0 failed",
            true,
        ),
        (
            vec![(Succeeded, 2), (Failed, 1)],
            0,
            false,
            "[batch] 3 runs: 2 succeeded, 1 failed",
            false,
        ),
        (
            vec![
                (Succeeded, 1),
                (TimedOut, 1),
                (Cancelled, 2),
                (LimitExceeded, 1),
            ],
            0,
            false,
            "[batch] 5 runs: 1 succeeded, 0 failed, 2 cancelled, 1 timed out, 1 limit exceeded",
            false,
        ),
        (
            vec![(Succeeded, 2), (Skipped, 1)],
            0,
            true,
            "[cases] 3 selected: 2 succeeded, 0 failed, 1 skipped; 0 suite fixtures failed",
            false,
        ),
        (
            vec![(Succeeded, 2)],
            1,
            true,
            "[cases] 2 selected: 2 succeeded, 0 failed, 0 skipped; 1 suite fixtures failed",
            false,
        ),
        (
            vec![(Succeeded, 1), (ExpectedFailure, 1), (Interrupted, 1)],
            0,
            true,
            "[cases] 3 selected: 1 succeeded, 0 failed, 1 expected failure, 1 interrupted",
            false,
        ),
        (
            vec![(Succeeded, 1), (ExpectedFailure, 2)],
            0,
            true,
            "[cases] 3 selected: 1 succeeded, 0 failed, 2 expected failure",
            true,
        ),
    ] {
        let totals = totals(&statuses);
        assert_eq!(summary(cases, &totals, fixtures), line);
        let outcome = Outcome {
            totals,
            fixtures_failed: fixtures,
            failed_cases: vec![],
            selected: totals.total(),
            stop: None,
        };
        assert_eq!(outcome.result().is_ok(), success, "{line}");
    }
    let error = Outcome {
        totals: totals(&[(CaseStatus::Succeeded, 1), (CaseStatus::TimedOut, 2)]),
        fixtures_failed: 0,
        failed_cases: vec![],
        selected: 3,
        stop: None,
    }
    .result()
    .unwrap_err();
    assert!(matches!(
        error,
        CliError::Batch {
            failed: 2,
            total: 3
        }
    ));
}

#[test]
fn failure_recap_names_each_unsuccessful_run_and_bounds_its_length() {
    let identity = |number: usize, case: Option<&str>| Identity {
        number,
        path: Arc::new(PathBuf::from("a.botwork")),
        case: case.map(|id| (id.to_owned(), id.to_owned())),
        dataset: None,
        artifacts: None,
    };
    let source = botwork::core::run::Engine::default()
        .run_source(
            "a.botwork",
            "No Operation\nAssert |false|",
            Default::default(),
        )
        .result
        .unwrap_err();
    let mut tally = Tally::default();
    tally.finished(&identity(1, None), &Ok(())).unwrap();
    tally
        .finished(&identity(2, None), &Err(CliError::Script(source.clone())))
        .unwrap();
    tally
        .finished(
            &identity(3, Some("s/c")),
            &Err(CliError::Script(Diagnostic::new(BWErr::Timeout(
                "t".into(),
            )))),
        )
        .unwrap();
    tally.fixture("s", &Err(CliError::Script(source)));
    tally.fixture("ok", &Ok(()));
    tally.skipped().unwrap();
    let Message::Summary {
        totals,
        fixtures_failed,
        recap,
        omitted,
        ..
    } = tally.summary(true)
    else {
        panic!("summary")
    };
    assert_eq!(
        recap,
        [
            "[run 2] failed \"a.botwork\" (BW9001 at a.botwork:2:1)",
            "[case s/c] timed out (BW5002)",
            "[suite s] fixture failed (BW9001 at a.botwork:2:1)",
        ]
    );
    assert_eq!((omitted, fixtures_failed, totals.total()), (0, 1, 4));
    let mut many = Tally::default();
    for number in 0..RECAP_LINES + 3 {
        many.finished(&identity(number, None), &Err(task_failure()))
            .unwrap();
    }
    let Message::Summary { recap, omitted, .. } = many.summary(false) else {
        panic!("summary")
    };
    assert_eq!((recap.len(), omitted), (RECAP_LINES, 3));
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
                report: None,
                listener: None,
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
