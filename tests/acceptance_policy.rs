use botwork::core::{
    acceptance::{CaseCompletion, CaseExpectation, CaseStatus, FailureKind, PhaseOutcome},
    diagnostic::DiagnosticCode,
    grammar::{BWErr, Literal},
    run::{Engine, RunOptions, RunOutcome},
};
use std::sync::{Arc, Mutex};

#[test]
fn immediate_callback_failures_stop_the_body_but_cleanup_and_explicit_catch_still_run() {
    for (body, expected_events, recovered) in [
        (
            "Check\nMark |\"unreachable\"|",
            vec!["check", "cleanup"],
            false,
        ),
        (
            "Try { Check } Catch { Mark |\"caught\"| }\nMark |\"continued\"|",
            vec!["check", "caught", "continued", "cleanup", "after"],
            true,
        ),
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::default();
        let observed = Arc::clone(&events);
        engine
            .register_native("Check", move |_, _| {
                observed.lock().unwrap().push("check".to_owned());
                Err(BWErr::NativeError("assertion failure".into()))
            })
            .unwrap();
        let observed = Arc::clone(&events);
        engine
            .register_native("Mark |value|", move |values, _| {
                observed.lock().unwrap().push(values[0].to_string());
                Ok(Literal::None)
            })
            .unwrap();
        let run = engine.run_source(
            "immediate.botwork",
            &format!("Try {{ {body} }} Finally {{ Mark |\"cleanup\"| }}\nMark |\"after\"|"),
            RunOptions::default(),
        );
        assert_eq!(*events.lock().unwrap(), expected_events);
        let outcome = match &run.result {
            Ok(_) => {
                assert!(recovered);
                PhaseOutcome::Succeeded
            }
            Err(error) => {
                assert!(!recovered);
                assert_eq!(error.code(), DiagnosticCode::Native);
                assert_eq!(
                    error.span.as_ref().unwrap().source().name(),
                    "immediate.botwork"
                );
                PhaseOutcome::Failed(FailureKind::from_diagnostic_code(error.code()))
            }
        };
        assert_eq!(
            run.outcome(),
            if recovered {
                RunOutcome::Succeeded
            } else {
                RunOutcome::Failed
            }
        );
        let completion = CaseCompletion::executed(
            PhaseOutcome::Succeeded,
            Some(outcome),
            PhaseOutcome::Succeeded,
            None,
        )
        .unwrap();
        // Text alone never establishes a typed assertion. Handled failures pass
        // ordinarily but fail strict expectations as an unexpected pass.
        assert_eq!(
            completion.decide(&CaseExpectation::failure("BUG-42").unwrap()),
            if recovered {
                CaseStatus::UnexpectedPass
            } else {
                CaseStatus::Failed
            }
        );
    }
}

#[test]
fn runtime_and_acceptance_classification_preserve_primary_cleanup_evidence() {
    let mut engine = Engine::default();
    engine
        .register_native("Check", |_, _| Err(BWErr::NativeError("assertion".into())))
        .unwrap();
    engine
        .register_native("CleanupTimedOut", |_, _| {
            Err(BWErr::Timeout("cleanup deadline".into()))
        })
        .unwrap();
    let run = engine.run_source(
        "secondary.botwork",
        "Try { Check } Finally { CleanupTimedOut }",
        RunOptions::default(),
    );
    assert_eq!(run.outcome(), RunOutcome::Failed);
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Native);
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Timeout);
    assert_eq!(
        CaseStatus::from_diagnostic_code(error.code()),
        CaseStatus::Failed
    );
    let typed = CaseCompletion::executed(
        PhaseOutcome::Succeeded,
        Some(PhaseOutcome::Failed(FailureKind::Assertion)),
        PhaseOutcome::Failed(FailureKind::TimedOut),
        None,
    )
    .unwrap();
    assert_eq!(
        typed.decide(&CaseExpectation::failure("BUG-42").unwrap()),
        CaseStatus::Failed
    );
}
