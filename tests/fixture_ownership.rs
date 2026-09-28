use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode,
    eval::{evaluate_program_async, evaluate_suite_fixture_async, Context},
    grammar::{BWErr, Literal},
    operation::{NativeOperation, OperationControl},
    run::RunLimits,
    signature::StatementSignature,
    suite::Suite,
};
use std::{
    collections::BTreeMap,
    future::pending,
    sync::{Arc, Mutex},
    time::Duration,
};

type Events = Arc<Mutex<Vec<String>>>;
fn context(limits: RunLimits, control: OperationControl, events: &Events) -> Context {
    let mut context = Context::with_control(limits, control).unwrap();
    let marks = Arc::clone(events);
    context
        .register_native("Mark |value|", move |values| {
            marks.lock().unwrap().push(values[0].to_string());
            Ok(Literal::None)
        })
        .unwrap();
    context
        .register_native("Check", |_| {
            Err(BWErr::NativeError("assertion failed".into()))
        })
        .unwrap();
    context
        .register_native("Panic", |_| panic!("host failure"))
        .unwrap();
    context
}
fn suite(setup: &str, body: &str, teardown: &str) -> Suite {
    Suite::parse(
        "fixtures",
        &format!(
            r#"Suite |"s"| {{
SuiteSetup {{ {setup} }}
SuiteTeardown {{ {teardown} }}
CaseSetup {{ Mark |"case setup"| }}
CaseTeardown {{ Mark |"case teardown"| }}
Case |"a"| {{ {body} }}
}}"#
        ),
    )
    .unwrap()
}

#[tokio::test]
async fn suite_waits_for_case_cleanup_after_success_assertion_returns_errors_and_panics() {
    for (body, code) in [
        ("Mark |7|", None),
        ("Check", Some(DiagnosticCode::Native)),
        ("Get { Return |7| }\nMark |@{ Get }|", None),
        ("Try { Missing } Catch { Mark |7| }", None),
        ("Missing", Some(DiagnosticCode::UndefinedStatement)),
        ("Panic", Some(DiagnosticCode::NativePanic)),
    ] {
        let events = Events::default();
        let suite = suite("Mark |\"suite setup\"|", body, "Mark |\"suite teardown\"|");
        let program = suite.program(0).unwrap();
        let root = context(RunLimits::default(), OperationControl::default(), &events);
        let case_events = Arc::clone(&events);
        let result = evaluate_suite_fixture_async(&suite, root, |inputs| async move {
            let mut case = context(
                RunLimits::default(),
                inputs.control().child(None),
                &case_events,
            );
            inputs.inherit_into(&mut case).unwrap();
            evaluate_program_async(&program, case).await
        })
        .await;
        assert!(result.result.is_ok());
        assert_eq!(result.body.unwrap().err().map(|e| e.code()), code);
        let mut expected = vec!["suite setup", "case setup"];
        if code.is_none() {
            expected.push("7");
        }
        expected.extend(["case teardown", "suite teardown"]);
        assert_eq!(*events.lock().unwrap(), expected, "{body}");
    }
}

#[tokio::test]
async fn setup_failure_keeps_primary_location_and_skips_borrowers_but_cleans() {
    let events = Events::default();
    let suite = suite("|x| = |missing|", "Mark |999|", "Mark |1|\nOther");
    let root = context(RunLimits::default(), OperationControl::default(), &events);
    let result =
        evaluate_suite_fixture_async(&suite, root, |_| async { panic!("no borrower") }).await;
    assert!(result.body.is_none());
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(error.span.as_ref().unwrap().text(), "missing");
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
    assert_eq!(*events.lock().unwrap(), ["1"]);
}

#[tokio::test]
async fn snapshots_are_independent_and_explicit_case_inputs_override_before_copying() {
    let events = Events::default();
    let suite = suite(
        "|shared| = |42|\n|large| = |\"a large string\"|",
        "",
        "Mark |shared|",
    );
    let root = context(RunLimits::default(), OperationControl::default(), &events);
    let case_events = Arc::clone(&events);
    let result = evaluate_suite_fixture_async(&suite, root, |inputs| async move {
        for value in [1, 2] {
            let mut limits = RunLimits::default();
            limits.values.string_bytes = 1; // Replaced fixture value must never be copied.
            let mut case = context(limits, inputs.control().child(None), &case_events);
            case.set_input_variables(BTreeMap::from([("large".into(), Literal::Int(value))]))
                .unwrap();
            inputs.inherit_into(&mut case).unwrap();
            let program =
                Program::parse("case", "Mark |large|\nMark |shared|\n|shared| = |0|").unwrap();
            evaluate_program_async(&program, case).await.unwrap();
        }
    })
    .await;
    result.result.unwrap();
    assert_eq!(*events.lock().unwrap(), ["1", "42", "2", "42", "42"]);
}

#[tokio::test]
async fn export_and_snapshot_limits_fail_before_borrowers_but_after_cleanup_is_armed() {
    for snapshot in [false, true] {
        let events = Events::default();
        let suite = suite("|x| = |1|", "", "Release");
        let mut limits = RunLimits::default();
        if snapshot {
            limits.snapshots.entries = 0;
        } else {
            limits.results.values = 0;
        }
        let mut root = context(limits, OperationControl::default(), &events);
        let marks = Arc::clone(&events);
        root.register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Release").unwrap(),
                move |_, _| {
                    let marks = Arc::clone(&marks);
                    async move {
                        marks.lock().unwrap().push("9".into());
                        Ok(Literal::None)
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
        let result =
            evaluate_suite_fixture_async(&suite, root, |_| async { panic!("no borrower") }).await;
        assert_eq!(
            result.result.unwrap_err().code(),
            DiagnosticCode::ResourceLimit
        );
        assert!(result.body.is_none());
        assert_eq!(*events.lock().unwrap(), ["9"]);
    }
}

#[tokio::test]
async fn fixture_preflight_checks_teardown_before_any_setup_effects() {
    let events = Events::default();
    let suite = suite("Mark |1|", "", "Mark |2|\nMark |3|\nMark |4|");
    let mut limits = RunLimits::default();
    limits.ast.nodes = 4;
    let root = context(limits, OperationControl::default(), &events);
    let result =
        evaluate_suite_fixture_async(&suite, root, |_| async { panic!("no borrower") }).await;
    assert_eq!(
        result.result.unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_drains_started_case_work_before_both_teardowns() {
    let events = Events::default();
    let suite = suite("", "Block", "Mark |\"suite teardown\"|");
    let program = suite.program(0).unwrap();
    let control = OperationControl::default();
    let root = context(RunLimits::default(), control.clone(), &events);
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&entered);
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    let marks = Arc::clone(&events);
    let stop = control.clone();
    let case_events = Arc::clone(&events);
    let mut future = Box::pin(evaluate_suite_fixture_async(
        &suite,
        root,
        |inputs| async move {
            let mut case = context(
                RunLimits::default(),
                inputs.control().child(None),
                &case_events,
            );
            inputs.inherit_into(&mut case).unwrap();
            case.register_native("Block", move |_| {
                marks.lock().unwrap().push("entered".into());
                stop.cancel();
                notify.notify_one();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                marks.lock().unwrap().push("drained".into());
                Ok(Literal::None)
            })
            .unwrap();
            evaluate_program_async(&program, case).await
        },
    ));
    tokio::select! {
        _ = entered.notified() => (),
        _ = &mut future => panic!("not drained"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("not entered"),
    }
    assert_eq!(*events.lock().unwrap(), ["case setup", "entered"]);
    release.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .unwrap();
    assert_eq!(result.result.unwrap_err().code(), DiagnosticCode::Cancelled);
    assert_eq!(
        result.body.unwrap().unwrap_err().code(),
        DiagnosticCode::Cancelled
    );
    assert_eq!(
        *events.lock().unwrap(),
        [
            "case setup",
            "entered",
            "drained",
            "case teardown",
            "suite teardown"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn suite_deadline_covers_borrowers_and_cleanup_gets_its_own_allowance() {
    let events = Events::default();
    let suite = suite("", "", "Mark |1|");
    let control = OperationControl::default()
        .child(Some(tokio::time::Instant::now() + Duration::from_secs(1)));
    let root = context(RunLimits::default(), control, &events);
    let result = evaluate_suite_fixture_async(&suite, root, |inputs| async move {
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            inputs.control().checkpoint().unwrap_err().code(),
            DiagnosticCode::Timeout
        );
    })
    .await;
    assert_eq!(result.result.unwrap_err().code(), DiagnosticCode::Timeout);
    assert!(result.body.is_some());
    assert_eq!(*events.lock().unwrap(), ["1"]);
}

#[tokio::test]
async fn parent_cancellation_during_suite_teardown_is_observed_after_teardown() {
    let events = Events::default();
    let suite = suite("", "", "Cancel\nMark |1|");
    let control = OperationControl::default();
    let mut root = context(RunLimits::default(), control.clone(), &events);
    root.register_native("Cancel", move |_| {
        control.cancel();
        Ok(Literal::None)
    })
    .unwrap();
    let result = evaluate_suite_fixture_async(&suite, root, |_| async {}).await;
    assert_eq!(result.result.unwrap_err().code(), DiagnosticCode::Cancelled);
    assert_eq!(*events.lock().unwrap(), ["1"]);
}

#[tokio::test]
async fn cleanup_steps_are_independent_bounded_and_never_hide_the_setup_failure() {
    let events = Events::default();
    let suite = suite("While |true| {}", "", "Mark |1|\nWhile |true| {}");
    let mut limits = RunLimits {
        steps: 10,
        ..RunLimits::default()
    };
    limits.cleanup.steps = 20;
    let root = context(limits, OperationControl::default(), &events);
    let result =
        evaluate_suite_fixture_async(&suite, root, |_| async { panic!("no borrower") }).await;
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert_eq!(*events.lock().unwrap(), ["1"]);
}

#[tokio::test]
async fn dropped_owner_future_does_not_claim_async_teardown_executed() {
    let events = Events::default();
    let suite = suite("Mark |1|", "", "Mark |2|");
    let root = context(RunLimits::default(), OperationControl::default(), &events);
    let entered = tokio::sync::Notify::new();
    let future = evaluate_suite_fixture_async(&suite, root, |_| async {
        entered.notify_one();
        pending::<()>().await;
    });
    let mut future = Box::pin(future);
    tokio::select! { _ = entered.notified() => (), _ = &mut future => panic!("body returned") }
    drop(future);
    assert_eq!(*events.lock().unwrap(), ["1"]);
}
