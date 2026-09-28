use botwork::core::{
    diagnostic::{Diagnostic, DiagnosticCode},
    eval::{evaluate_program_async, evaluate_suite_fixture_async, Context},
    grammar::Literal,
    operation::OperationControl,
    run::RunLimits,
    suite::Suite,
};
use std::sync::{Arc, Mutex};

type Events = Arc<Mutex<Vec<String>>>;
fn context(limits: RunLimits, control: OperationControl, events: &Events) -> Context {
    let mut context = Context::with_control(limits, control).unwrap();
    let events = Arc::clone(events);
    context
        .register_native("Mark |n|", move |values| {
            events.lock().unwrap().push(values[0].to_string());
            Ok(Literal::None)
        })
        .unwrap();
    context
}

#[tokio::test]
async fn setup_and_teardown_failure_matrix_preserves_owner_primary_and_body_admission() {
    for suite_owned in [false, true] {
        for setup in ["", "|x| = |missing|", "Try { |x| = |missing| } Catch {}"] {
            for cleanup in ["", "|x| = |1 / 0|", "Try { |x| = |1 / 0| } Catch {}"] {
                let events = Events::default();
                let prefix = if suite_owned { "Suite" } else { "Case" };
                let suite = Suite::parse(
                    "matrix",
                    &format!(
                        r#"Suite |"s"| {{
{prefix}Setup {{ Mark |1|
    {setup} }}
{prefix}Teardown {{ Mark |3|
    {cleanup} }}
Case |"a"| {{ Mark |2| }}
}}"#
                    ),
                )
                .unwrap();
                let program = suite.program(0).unwrap();
                let root = context(RunLimits::default(), OperationControl::default(), &events);
                let failed_setup = setup.starts_with('|');
                let failed_cleanup = cleanup.starts_with('|');
                let result = if suite_owned {
                    let cases = Arc::clone(&events);
                    let owner = evaluate_suite_fixture_async(&suite, root, |inputs| async move {
                        let mut case =
                            context(RunLimits::default(), inputs.control().child(None), &cases);
                        inputs.inherit_into(&mut case).unwrap();
                        evaluate_program_async(&program, case).await
                    })
                    .await;
                    assert_eq!(owner.body.is_none(), failed_setup);
                    if let Some(case) = owner.body {
                        case.unwrap();
                    }
                    owner.result
                } else {
                    evaluate_program_async(&program, root).await.map(|_| ())
                };
                let expected = if failed_setup {
                    Some(DiagnosticCode::UndefinedVariable)
                } else if failed_cleanup {
                    Some(DiagnosticCode::Arithmetic)
                } else {
                    None
                };
                assert_eq!(
                    result.as_ref().err().map(Diagnostic::code),
                    expected,
                    "{prefix}: {setup}; {cleanup}"
                );
                assert_eq!(
                    *events.lock().unwrap(),
                    if failed_setup {
                        vec!["1", "3"]
                    } else {
                        vec!["1", "2", "3"]
                    }
                );
                if let Err(error) = result {
                    assert_eq!(
                        error.span.as_ref().unwrap().text(),
                        if failed_setup { "missing" } else { "1 / 0" }
                    );
                    assert_eq!(
                        error.causes.len(),
                        usize::from(failed_setup && failed_cleanup)
                    );
                    if failed_setup && failed_cleanup {
                        assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
                        assert_eq!(error.causes[0].span.as_ref().unwrap().text(), "1 / 0");
                        let rendered = error.to_string();
                        assert!(
                            rendered.find("BW2001").unwrap() < rendered.find("BW3002").unwrap(),
                            "{rendered}"
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn bounded_cleanup_evidence_keeps_the_setup_category_and_declares_omission() {
    for suite_owned in [false, true] {
        let prefix = if suite_owned { "Suite" } else { "Case" };
        let suite = Suite::parse(
            "bounded",
            &format!(
                r#"Suite |"s"| {{
{prefix}Setup {{ |x| = |missing| }}
{prefix}Teardown {{ |x| = |1 / 0| }}
Case |"a"| {{}}
}}"#
            ),
        )
        .unwrap();
        let mut limits = RunLimits::default();
        limits.diagnostics.depth = 1;
        let context = Context::with_limits(limits).unwrap();
        let error = if suite_owned {
            let result =
                evaluate_suite_fixture_async(&suite, context, |_| async { panic!("setup failed") })
                    .await;
            assert!(result.body.is_none());
            result.result.unwrap_err()
        } else {
            evaluate_program_async(&suite.program(0).unwrap(), context)
                .await
                .unwrap_err()
        };
        assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
        assert!(error.omissions.is_some());
        let rendered = error.to_string();
        assert!(rendered.contains("omitted"), "{rendered}");
        assert!(rendered.contains("BW2001"), "{rendered}");
    }
}

#[tokio::test]
async fn case_and_shared_cleanup_failures_remain_distinct_results() {
    let suite = Suite::parse(
        "separate",
        r#"Suite |"s"| {
SuiteTeardown { SuiteCleanupFailed }
CaseSetup { |x| = |missing| }
CaseTeardown { |x| = |1 / 0| }
Case |"a"| { BodyMustNotRun }
}"#,
    )
    .unwrap();
    let program = suite.program(0).unwrap();
    let owner = Context::with_limits(RunLimits::default()).unwrap();
    let result = evaluate_suite_fixture_async(&suite, owner, |inputs| async move {
        let mut case =
            Context::with_control(RunLimits::default(), inputs.control().child(None)).unwrap();
        inputs.inherit_into(&mut case).unwrap();
        evaluate_program_async(&program, case).await
    })
    .await;
    let case = result.body.unwrap().unwrap_err();
    assert_eq!(case.code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(case.causes.len(), 1);
    assert_eq!(case.causes[0].code(), DiagnosticCode::Arithmetic);
    let owner = result.result.unwrap_err();
    assert_eq!(owner.code(), DiagnosticCode::UndefinedStatement);
    assert!(owner.to_string().contains("SuiteCleanupFailed"));
    assert!(owner.causes.is_empty());
}

#[tokio::test]
async fn later_parent_cancellation_keeps_setup_and_cleanup_failures_as_evidence() {
    let events = Events::default();
    let suite = Suite::parse(
        "late-stop",
        r#"Suite |"s"| {
SuiteSetup { |x| = |missing| }
SuiteTeardown { Cancel
    |x| = |1 / 0| }
Case |"a"| { BodyMustNotRun }
}"#,
    )
    .unwrap();
    let control = OperationControl::default();
    let mut owner = context(RunLimits::default(), control.clone(), &events);
    owner
        .register_native("Cancel", move |_| {
            control.cancel();
            Ok(Literal::None)
        })
        .unwrap();
    let result =
        evaluate_suite_fixture_async(&suite, owner, |_| async { panic!("setup failed") }).await;
    assert!(result.body.is_none());
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(error.causes[0].causes.len(), 1);
    assert_eq!(error.causes[0].causes[0].code(), DiagnosticCode::Arithmetic);
}
