#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{Diagnostic, DiagnosticCode},
    eval::{botwork_detailed, evaluate_program_detailed, Context},
    grammar::{BWErr, BWParser, Literal, Rule},
    operation::{NativeOperation, OperationControl},
    run::{CleanupLimits, Engine, RunLimits, RunOptions, RunOutcome},
    signature::StatementSignature,
};
use pest::Parser;
use std::{
    collections::BTreeSet,
    future::pending,
    sync::{Arc, Mutex},
    time::Duration,
};

type Events = Arc<Mutex<Vec<String>>>;

fn engine() -> (Engine, Events) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::default();
    let observed = Arc::clone(&events);
    engine
        .register_native("Mark |value|", move |values, environment| {
            environment
                .control()
                .checkpoint()
                .map_err(Diagnostic::into_error)?;
            observed.lock().unwrap().push(values[0].to_string());
            Ok(Literal::None)
        })
        .unwrap();
    (engine, events)
}

fn options(steps: u64) -> RunOptions {
    RunOptions {
        inherit_environment: false,
        limits: RunLimits {
            cleanup: CleanupLimits {
                steps,
                timeout: Duration::from_secs(1),
            },
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

#[test]
fn cleanup_runs_once_after_normal_handled_and_unhandled_completion() {
    for (body, code, expected) in [
        ("Mark |1|", None, vec!["1", "3", "4"]),
        (
            "Try { Missing } Catch { Mark |2| }",
            None,
            vec!["2", "3", "4"],
        ),
        (
            "Missing",
            Some(DiagnosticCode::UndefinedStatement),
            vec!["3"],
        ),
        ("|x| = |1 / 0|", Some(DiagnosticCode::Arithmetic), vec!["3"]),
    ] {
        let (engine, events) = engine();
        let run = engine.run_source(
            "completion",
            &format!("Try {{ {body} }} Finally {{ Mark |3| }}\nMark |4|"),
            options(100),
        );
        assert_eq!(
            run.result.as_ref().err().map(Diagnostic::code),
            code,
            "{body}: {:?}",
            run.result
        );
        assert_eq!(*events.lock().unwrap(), expected, "{body}");
    }
}

#[test]
fn catch_precedes_cleanup_and_its_temporary_binding_is_restored() {
    let (engine, events) = engine();
    let run = engine.run_source(
        "catch-finally",
        r#"
|error| = |99|
Try { Missing } Catch |error| { Mark |error.code| } Finally { Mark |error| }
Try { Missing } Catch { Other } Finally { Mark |100| }
"#,
        options(100),
    );
    let error = run.result.unwrap_err();
    assert!(error.to_string().contains("Other"));
    assert_eq!(error.causes.len(), 1);
    assert!(error.causes[0].to_string().contains("Missing"));
    assert_eq!(*events.lock().unwrap(), ["BW2002", "99", "100"]);
}

#[test]
fn returns_keep_their_original_value_and_unwind_nested_owners_in_reverse_order() {
    let (engine, events) = engine();
    let run = engine.run_source(
        "return",
        r#"
Get {
    |value| = |41|
    Try {
        Try { Return |value + 1| } Finally { |value| = |0|
            Mark |1| }
    } Finally { Mark |value| }
    Mark |999|
}
|answer| = Get
Mark |answer|
"#,
        options(100),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded, "{:?}", run.result);
    assert_eq!(*events.lock().unwrap(), ["1", "0", "42"]);
    assert!(!run.variables.contains_key("value"));
}

#[test]
fn loop_controls_and_failed_return_expressions_release_the_current_owner() {
    let (engine, events) = engine();
    let run = engine.run_source(
        "controls",
        r#"
For |i| In |[1, 2, 3]| {
    Try { If |i == 1| { Continue }
        Break
    } Finally { Mark |i| }
    Mark |999|
}
Get { Try { Return |missing| } Finally { Mark |3| } }
Get
"#,
        options(100),
    );
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::UndefinedVariable
    );
    assert_eq!(*events.lock().unwrap(), ["1", "2", "3"]);
}

#[test]
fn cleanup_cannot_escape_into_an_enclosing_invocation_loop_or_catch() {
    for source in [
        "Mark |0|\nGet { Try {} Finally { Return |1| } }",
        "Mark |0|\nWhile |true| { Try {} Finally { Break } }",
        "Mark |0|\nFor |i| In |[]| { Try {} Finally { Continue } }",
        "Mark |0|\nTry {} Catch { Try {} Finally { Rethrow } }",
        "Mark |0|\nTry {} Finally { If |false| { Return } }",
    ] {
        let (engine, events) = engine();
        let run = engine.run_source("invalid-control", source, options(100));
        assert_eq!(
            run.result.unwrap_err().code(),
            DiagnosticCode::InvalidControl,
            "{source}"
        );
        assert_eq!(run.steps, 0);
        assert!(events.lock().unwrap().is_empty());
    }
    let (engine, events) = engine();
    let run = engine.run_source(
        "local-controls",
        r#"
Try {} Finally {
    Local { Return |7| }
    For |i| In |[1, 2]| { If |i == 1| { Continue }
        Mark |@{ Local }|
        Break
    }
    Try { Try { Missing } Catch { Rethrow } } Catch { Mark |8| }
}
"#,
        options(100),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded, "{:?}", run.result);
    assert_eq!(*events.lock().unwrap(), ["7", "8"]);
}

#[test]
fn cleanup_failure_preserves_body_identity_location_and_ordered_secondary_failures() {
    let (engine, events) = engine();
    let run = engine.run_source(
        "both",
        r#"
Try {
    Try { |x| = |missing| } Finally { |x| = |1 / 0| }
} Finally { Mark |1|
    Other
}
"#,
        options(100),
    );
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(error.span.as_ref().unwrap().text(), "missing");
    assert_eq!(
        error
            .causes
            .iter()
            .map(Diagnostic::code)
            .collect::<Vec<_>>(),
        [
            DiagnosticCode::Arithmetic,
            DiagnosticCode::UndefinedStatement
        ]
    );
    assert_eq!(error.causes[0].span.as_ref().unwrap().text(), "1 / 0");
    assert_eq!(*events.lock().unwrap(), ["1"]);
    for body in ["", "Return |42|"] {
        let run = engine.run_source(
            "cleanup-failure",
            &format!("Get {{ Try {{ {body} }} Finally {{ Other }} }}\nGet"),
            options(100),
        );
        let error = run.result.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::UndefinedStatement);
        assert!(error.causes.is_empty());
    }
}

#[test]
fn cleanup_diagnostic_truncation_keeps_the_body_failure_primary() {
    let (engine, _) = engine();
    let mut settings = options(100);
    settings.limits.diagnostics.depth = 1;
    let run = engine.run_source(
        "truncated",
        "Try { |x| = |missing| } Finally { Other }",
        settings,
    );
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable, "{error:?}");
    assert!(error.omissions.is_some());
    assert!(error.to_string().contains("omitted"));
}

#[test]
fn cleanup_releases_nested_host_resources_after_an_assertion_callback_failure() {
    let resources = Arc::new(Mutex::new(BTreeSet::new()));
    let (mut engine, events) = engine();
    let opened = Arc::clone(&resources);
    engine
        .register_native("Open |name|", move |values, _| {
            assert!(opened.lock().unwrap().insert(values[0].to_string()));
            Ok(values[0].clone())
        })
        .unwrap();
    let closed = Arc::clone(&resources);
    let observed = Arc::clone(&events);
    engine
        .register_native("Close |name|", move |values, _| {
            assert!(closed.lock().unwrap().remove(&values[0].to_string()));
            observed.lock().unwrap().push(values[0].to_string());
            Ok(Literal::None)
        })
        .unwrap();
    engine
        .register_native("Check", |_, _| {
            Err(BWErr::NativeError("assertion failed".into()))
        })
        .unwrap();
    let run = engine.run_source(
        "resources",
        r#"
Try {
    |outer| = Open |"outer"|
    Try {
        |inner| = Open |"inner"|
        Check
    } Finally { Close |inner| }
} Finally { Close |outer| }
"#,
        options(100),
    );
    assert_eq!(run.result.unwrap_err().code(), DiagnosticCode::Native);
    assert!(resources.lock().unwrap().is_empty());
    assert_eq!(*events.lock().unwrap(), ["inner", "outer"]);
}

#[test]
fn cleanup_runs_after_main_steps_are_exhausted_without_restarting_the_run() {
    let (engine, events) = engine();
    let mut settings = options(100);
    settings.limits.steps = 20;
    let run = engine.run_source(
        "limit",
        "Try { Try { While |true| {} } Catch { Mark |0| } } Finally { Mark |1| }\nMark |2|",
        settings,
    );
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
    assert_eq!(*events.lock().unwrap(), ["1"]);
    assert!(
        run.steps > 20,
        "cleanup steps are included in run statistics"
    );
}

#[test]
fn cleanup_limits_are_independent_for_prior_owners_and_shared_by_new_cleanup_owners() {
    let (engine, events) = engine();
    let run = engine.run_source(
        "nested-limit",
        "Try { Try {} Finally { While |true| {} } } Finally { Mark |1| }",
        options(20),
    );
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
    assert_eq!(*events.lock().unwrap(), ["1"]);
    let run = engine.run_source(
        "no-renewal",
        "Try {} Finally { While |true| { Try {} Finally { Mark |1| } } }",
        options(20),
    );
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
    assert!(events.lock().unwrap().len() < 10);
    for (steps, expected) in [(0, RunOutcome::LimitExceeded), (2, RunOutcome::Succeeded)] {
        let run = engine.run_source("exact", "Try {} Finally { Mark |9| }", options(steps));
        assert_eq!(run.outcome(), expected, "{steps}: {:?}", run.result);
        assert_eq!(run.steps, 1 + steps);
    }
}

#[test]
fn parsing_reserves_only_the_whole_keyword_and_rejects_invalid_clause_orders() {
    for source in [
        "Try {}",
        "Finally {}",
        "Try {} Finally {} Catch {}",
        "Try {} Finally {} Finally {}",
        "Try {} Catch {} Catch {}",
    ] {
        assert!(Program::parse_detailed("bad", source).is_err(), "{source}");
    }
    for source in [
        "try{}finally{}",
        "Try {}\n# next\nFiNaLlY {}",
        "Try {} Catch {}\r\nFinally {}",
        "Finally! {}\nFinally!",
    ] {
        let program = Program::parse_detailed("valid", source).unwrap();
        evaluate_program_detailed(&program, &mut Context::default()).unwrap();
    }
    let pair = BWParser::parse(Rule::stmt_finally, "Finally {}")
        .unwrap()
        .next()
        .unwrap();
    assert_eq!(
        botwork_detailed(pair, &mut Context::default())
            .unwrap_err()
            .code(),
        DiagnosticCode::InvalidControl
    );
}

#[test]
fn cleanup_retains_live_value_quotas_and_ast_admission_checks_all_branches() {
    let (engine, events) = engine();
    let mut settings = options(100);
    settings.limits.retained_values.values = 1;
    let run = engine.run_source(
        "values",
        "Try { |x| = |1| } Finally { |y| = |2| }",
        settings,
    );
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
    assert_eq!(run.variables["x"].to_string(), "1");
    assert!(!run.variables.contains_key("y"));
    let source = "Try {} Finally { Mark |1| }";
    let program = Program::parse("ast", source).unwrap();
    let mut settings = options(100);
    settings.limits.ast.nodes = 4;
    let run = engine.run_program(&program, settings);
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.steps, 0);
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn imported_cleanup_and_its_import_work_use_the_owning_runs_state() {
    let harness = cli_harness::Harness::new();
    std::fs::write(
        harness.workspace.join("one.botwork"),
        "Get { Try { Return |42| } Finally { Mark |1| } }",
    )
    .unwrap();
    std::fs::write(harness.workspace.join("two.botwork"), "Mark |2|").unwrap();
    let (engine, events) = engine();
    for asynchronous in [false, true] {
        events.lock().unwrap().clear();
        let mut settings = options(100);
        settings.working_directory = Some(harness.workspace.clone());
        settings.limits.imports.loads = 1;
        let source = "Try { Import |\"one.botwork\"| As |one|\nMark |@{ one::Get }| } Finally { Import |\"two.botwork\"| As |two| }";
        let run = if asynchronous {
            engine.run_source_async("imports", source, settings).await
        } else {
            engine.run_source("imports", source, settings)
        };
        assert_eq!(
            run.result.unwrap_err().code(),
            DiagnosticCode::ResourceLimit
        );
        assert_eq!(*events.lock().unwrap(), ["1", "42"]);
    }
}

#[tokio::test]
async fn a_started_blocking_callback_finishes_before_cleanup_begins() {
    let (mut engine, events) = engine();
    let cancellation = OperationControl::default();
    let stop = cancellation.clone();
    let observed = Arc::clone(&events);
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&entered);
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    engine
        .register_native("Block", move |_, _| {
            observed.lock().unwrap().push("entered".into());
            stop.cancel();
            notify.notify_one();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            observed.lock().unwrap().push("drained".into());
            Ok(Literal::None)
        })
        .unwrap();
    let mut settings = options(100);
    settings.control = cancellation;
    let mut run =
        Box::pin(engine.run_source_async("drain", "Try { Block } Finally { Mark |1| }", settings));
    tokio::select! {
        _ = entered.notified() => (),
        result = &mut run => panic!("callback was not drained: {:?}", result.result),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("callback did not start"),
    }
    assert_eq!(*events.lock().unwrap(), ["entered"]);
    release.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap();
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert_eq!(*events.lock().unwrap(), ["entered", "drained", "1"]);
}

#[test]
fn native_panics_release_owners_and_success_restores_the_original_run_budget() {
    let (mut engine, events) = engine();
    engine
        .register_native("Panic", |_, _| panic!("host failure"))
        .unwrap();
    let run = engine.run_source("panic", "Try { Panic } Finally { Mark |1| }", options(100));
    assert_eq!(run.result.unwrap_err().code(), DiagnosticCode::NativePanic);
    assert_eq!(*events.lock().unwrap(), ["1"]);
    events.lock().unwrap().clear();
    let mut settings = options(100);
    settings.limits.steps = 2;
    let run = engine.run_source(
        "restored",
        "Try {} Finally { Mark |1| }\nMark |2|",
        settings,
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(*events.lock().unwrap(), ["1"]);
}

#[test]
fn deep_cleanup_unwinding_respects_the_existing_stack_limit() {
    let (engine, events) = engine();
    let run = engine.run_source(
        "deep-cleanup",
        "Recur { Try { Recur } Finally { Mark |1| } }\nRecur",
        options(100),
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(
        !events.lock().unwrap().is_empty(),
        "outer owners must still attempt cleanup"
    );
    assert!(events.lock().unwrap().len() <= 32);
}

#[test]
fn cli_keeps_output_charges_across_cleanup_and_honors_cleanup_timeouts() {
    let harness = cli_harness::Harness::new();
    for (id, source) in [
        ("body-output", "Try { Log |1| } Finally { Log |2| }"),
        ("cleanup-output", "Try {} Finally { Log |1| }\nLog |2|"),
    ] {
        let output = harness
            .run_with_args(
                id,
                source,
                &["--max-output-bytes", "2"],
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(output.stdout, b"1\n");
        assert!(String::from_utf8_lossy(&output.stderr).contains("output total bytes"));
    }
    let output = harness
        .run_with_args(
            "zero-timeout",
            "Try {} Finally { Log |1| }",
            &["--cleanup-timeout-ms", "0"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("BW5002"));
}

#[tokio::test(start_paused = true)]
async fn cancellation_drains_work_then_awaits_cleanup_with_independent_control() {
    let (mut engine, events) = engine();
    let parent = OperationControl::default();
    let cancellation = parent.clone();
    let observed = Arc::clone(&events);
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Cancel").unwrap(),
                move |_, _| {
                    let cancellation = cancellation.clone();
                    async move {
                        cancellation.cancel();
                        Err(Diagnostic::new(BWErr::NativeError("body error".into())))
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Release").unwrap(),
                move |_, control| {
                    let observed = Arc::clone(&observed);
                    async move {
                        control.checkpoint()?;
                        observed.lock().unwrap().push("begin".into());
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        control.checkpoint()?;
                        observed.lock().unwrap().push("end".into());
                        Ok(Literal::None)
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
    let mut settings = options(100);
    settings.control = parent.clone();
    let run = engine
        .run_source_async(
            "cancel",
            "Try { Cancel } Catch { Mark |0| } Finally { Release\nMark |1| }\nMark |2|",
            settings,
        )
        .await;
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    assert_eq!(*events.lock().unwrap(), ["begin", "end", "1"]);
    assert!(parent.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn expired_body_and_failed_inner_cleanup_do_not_prevent_outer_cleanup() {
    let (mut engine, events) = engine();
    engine
        .register_operation(
            NativeOperation::asynchronous(StatementSignature::native("Wait").unwrap(), |_, _| {
                pending()
            })
            .unwrap(),
        )
        .unwrap();
    let mut settings = options(100);
    settings.timeout = Some(Duration::from_millis(10));
    settings.limits.cleanup.timeout = Duration::from_millis(20);
    let start = tokio::time::Instant::now();
    let run = engine
        .run_source_async(
            "timeout",
            "Try { Try { Wait } Finally { Wait } } Finally { Mark |1| }",
            settings,
        )
        .await;
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Timeout);
    assert_eq!(*events.lock().unwrap(), ["1"]);
    assert!(start.elapsed() >= Duration::from_millis(30));
}

#[tokio::test(start_paused = true)]
async fn cleanup_deadline_cannot_be_renewed_by_nested_finally() {
    let (mut engine, events) = engine();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Pause").unwrap(),
                |_, _| async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Ok(Literal::None)
                },
            )
            .unwrap(),
        )
        .unwrap();
    let mut settings = options(100);
    settings.limits.cleanup.timeout = Duration::from_millis(15);
    let run = engine
        .run_source_async(
            "deadline",
            "Try {} Finally { Pause\nTry {} Finally { Pause\nMark |1| } }",
            settings,
        )
        .await;
    assert_eq!(run.result.unwrap_err().code(), DiagnosticCode::Timeout);
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_new_parent_stop_during_cleanup_is_observed_after_cleanup_finishes() {
    let (mut engine, events) = engine();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Pause").unwrap(),
                |_, _| async {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(Literal::None)
                },
            )
            .unwrap(),
        )
        .unwrap();
    let mut settings = options(100);
    settings.timeout = Some(Duration::from_millis(10));
    let run = engine
        .run_source_async(
            "late-stop",
            "Try {} Finally { Pause\nMark |1| }\nMark |2|",
            settings,
        )
        .await;
    assert_eq!(run.outcome(), RunOutcome::TimedOut);
    assert_eq!(*events.lock().unwrap(), ["1"]);
}

#[test]
fn suite_cases_finish_their_cleanup_before_the_terminal_status() {
    let harness = cli_harness::Harness::new();
    let suite = harness.workspace.join("cleanup.suite.botwork");
    std::fs::write(
        &suite,
        r#"
Suite |"cleanup"| {
    Case |"failed"| { Try { Missing } Finally { Log |"failed cleanup"| } }
    Case |"passed"| { Try { Log |"body"| } Finally { Log |"passed cleanup"| } }
}
"#,
    )
    .unwrap();
    let output = harness
        .command(
            "suite-cleanup",
            &["--suite", suite.to_str().unwrap(), "--jobs", "1"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"failed cleanup\nbody\npassed cleanup\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("2 selected: 1 succeeded, 1 failed"));
}

#[tokio::test]
async fn dropping_a_suspended_run_does_not_claim_that_async_cleanup_ran() {
    let (mut engine, events) = engine();
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = Arc::clone(&entered);
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Wait").unwrap(),
                move |_, _| {
                    let observed = Arc::clone(&observed);
                    async move {
                        observed.notify_one();
                        pending().await
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
    let program = Program::parse("drop", "Try { Wait } Finally { Mark |1| }").unwrap();
    let mut future = Box::pin(engine.run_program_async(&program, options(100)));
    tokio::select! {
        _ = entered.notified() => (),
        result = &mut future => panic!("suspended run finished: {:?}", result.result),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("operation did not enter"),
    }
    drop(future);
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn cli_runs_cleanup_after_limits_and_retains_both_failure_diagnostics() {
    let harness = cli_harness::Harness::new();
    let output = harness
        .run_with_args(
            "cleanup-limit",
            "Try { While |true| {} } Finally { Log |\"released\"| }",
            &["--max-steps", "20"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"released\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("BW8001"));
    let output = harness
        .run(
            "cleanup-both",
            "Try { |x| = |missing| } Finally { Other }",
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(errors.find("BW2001").unwrap() < errors.find("BW2002").unwrap());
    let output = harness
        .run_with_args(
            "cleanup-zero",
            "Try {} Finally { Log |1| }",
            &["--max-cleanup-steps", "0"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}
