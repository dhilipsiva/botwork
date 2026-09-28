#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{Diagnostic, DiagnosticCode},
    eval::{evaluate_program_async, evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
    operation::{NativeOperation, OperationControl},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
    signature::{StatementSignature, ValueKind},
};
use cli_harness::Harness;
use std::{
    collections::BTreeMap,
    future::{pending, Future},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context as TaskContext, Poll, Waker},
    time::Duration,
};

fn options() -> RunOptions {
    RunOptions {
        inherit_environment: false,
        ..RunOptions::default()
    }
}

fn delayed(events: Arc<Mutex<Vec<String>>>) -> NativeOperation {
    NativeOperation::asynchronous(
        StatementSignature::native("Later |value|").unwrap(),
        move |mut values, _| {
            let events = Arc::clone(&events);
            async move {
                let value = values.remove(0);
                events.lock().unwrap().push(format!("begin:{value}"));
                tokio::time::sleep(Duration::from_millis(1)).await;
                events.lock().unwrap().push(format!("end:{value}"));
                Ok(value)
            }
        },
    )
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn suspended_arguments_collections_keys_and_returns_preserve_effect_order() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::default();
    engine
        .register_operation(delayed(Arc::clone(&events)))
        .unwrap();
    let source = r#"
Choose |first| And |second| { Return |second| }
|items| = |[11, 22]|
|answer| = Choose |@{ Later |1| }| And |{a: @{ Later |2| }, a: items[@{ Later |1| }]}|
|done| = |@{ Later |answer.a| } + 1|
"#;
    let result = engine.run_source_async("order", source, options()).await;
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.result
    );
    assert_eq!(result.variables["answer"].to_string(), r#"{"a": 22}"#);
    assert_eq!(result.variables["done"].to_string(), "23");
    assert!(!result.variables.contains_key("first"));
    assert_eq!(
        *events.lock().unwrap(),
        ["begin:1", "end:1", "begin:2", "end:2", "begin:1", "end:1", "begin:22", "end:22"]
    );
}

#[tokio::test(start_paused = true)]
async fn short_circuiting_and_argument_failures_skip_unselected_operations() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::default();
    engine
        .register_operation(delayed(Arc::clone(&events)))
        .unwrap();
    let result = engine
        .run_source_async(
            "skip",
            r#"
|a| = |false and @{ Later |1| }|
|b| = |true or @{ Later |2| }|
Pair |a| And |b| { Return |a| }
|result| = Pair |missing| And |@{ Later |3| }|
"#,
            options(),
        )
        .await;
    assert_eq!(
        result.result.unwrap_err().code(),
        DiagnosticCode::UndefinedVariable
    );
    assert_eq!(result.variables["a"].to_string(), "false");
    assert_eq!(result.variables["b"].to_string(), "true");
    assert!(!result.variables.contains_key("result"));
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn suspension_preserves_lexical_scope_recursion_and_loop_completion() {
    let mut engine = Engine::default();
    engine
        .register_operation(delayed(Arc::new(Mutex::new(Vec::new()))))
        .unwrap();
    let result = engine
        .run_source_async(
            "scope",
            r#"
|outer| = |9|
Read { Return |@{ Later |outer| }| }
Recur |n| {
  If |n == 0| { Return |@{ Read }| }
  |outer| = |100|
  Return |@{ Recur |n - 1| }|
}
Find {
  |item| = |99|
  For |item| In |[1, 2, 3]| {
    |value| = Later |item|
    If |value == 1| { Continue }
    Return |value + @{ Recur |2| }|
  }
  Return |0|
}
|answer| = Find
|counter| = |0|
While |@{ Later |counter < 4| }| {
  |counter| = |counter + 1|
  If |counter == 3| { Break }
}
|iterator| = |7|
For |iterator| In |[1, 2]| { |last| = Later |iterator| }
"#,
            options(),
        )
        .await;
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.result
    );
    for (name, expected) in [
        ("outer", "9"),
        ("answer", "11"),
        ("counter", "3"),
        ("iterator", "7"),
        ("last", "2"),
    ] {
        assert_eq!(result.variables[name].to_string(), expected);
    }
    assert!(!result.variables.contains_key("item"));
}

#[tokio::test(start_paused = true)]
async fn handlers_preserve_original_diagnostics_and_restore_bindings_across_awaits() {
    let original_program = Program::parse("adapter-é", "|x| = |1|").unwrap();
    let original = Diagnostic::new(BWErr::NativeError("original failure".into()))
        .at(&original_program.statements[0].span);
    let mut engine = Engine::default();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Fail later").unwrap(),
                move |_, _| {
                    let original = original.clone();
                    async move {
                        tokio::task::yield_now().await;
                        Err(original)
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
    engine
        .register_operation(delayed(Arc::new(Mutex::new(Vec::new()))))
        .unwrap();
    let result = engine
        .run_source_async(
            "handler",
            r#"
|error| = |7|
Outer {
  Try { Fail later } Catch |error| {
    |ignored| = Later |error.code|
    Rethrow
  }
}
Try { Outer } Catch |caught| { |code| = Later |caught.code| }
"#,
            options(),
        )
        .await;
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.result
    );
    assert_eq!(result.variables["error"].to_string(), "7");
    assert_eq!(result.variables["code"].to_string(), "BW4002");
    assert!(!result.variables.contains_key("caught"));
    let result = engine
        .run_source_async("trace", "Outer { Fail later }\nOuter", options())
        .await;
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Native);
    assert_eq!(error.span.as_ref().unwrap().source().name(), "adapter-é");
    assert_eq!(
        error
            .call_stack
            .iter()
            .map(|frame| frame.signature.as_str())
            .collect::<Vec<_>>(),
        ["faillater", "outer"]
    );
}

#[tokio::test(start_paused = true)]
async fn imports_await_initialization_once_and_keep_module_globals_isolated() {
    let harness = Harness::new();
    std::fs::write(
        harness.workspace.join("module.botwork"),
        "|private| = Later |5|\nRead { Return |@{ Later |private| }| }",
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::default();
    engine
        .register_operation(delayed(Arc::clone(&events)))
        .unwrap();
    let run_options = RunOptions {
        working_directory: Some(harness.workspace.clone()),
        ..options()
    };
    let source = "|private| = |99|\nImport |\"module.botwork\"| As |one|\nImport |\"module.botwork\"| As |two|\n|a| = one::Read\n|b| = two::Read";
    let result = engine
        .run_source_async("entry.botwork", source, run_options.clone())
        .await;
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.result
    );
    assert_eq!(result.variables["private"].to_string(), "99");
    assert_eq!(result.variables["a"].to_string(), "5");
    assert_eq!(result.variables["b"].to_string(), "5");
    assert_eq!(events.lock().unwrap().len(), 6);
    let again = engine
        .run_source_async("entry.botwork", source, run_options)
        .await;
    assert_eq!(again.outcome(), RunOutcome::Succeeded);
    assert_eq!(events.lock().unwrap().len(), 12);
}

#[tokio::test(start_paused = true)]
async fn failed_async_module_initialization_is_unpublished_and_can_be_retried() {
    let harness = Harness::new();
    std::fs::write(
        harness.workspace.join("retry.botwork"),
        "|value| = Initialize\nRead { Return |value| }",
    )
    .unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&attempts);
    let mut engine = Engine::default();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Initialize").unwrap(),
                move |_, _| {
                    let attempt = observed.fetch_add(1, Ordering::SeqCst);
                    async move {
                        tokio::task::yield_now().await;
                        if attempt == 0 {
                            Err(BWErr::NativeError("first initialization failed".into()).into())
                        } else {
                            Ok(Literal::Int(5))
                        }
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
    let result = engine
        .run_source_async(
            "entry.botwork",
            r#"
Try { Import |"retry.botwork"| As |broken| } Catch |error| { |code| = |error.code| }
Try { broken::Read } Catch |error| { |unpublished| = |error.code| }
Import |"retry.botwork"| As |ready|
Import |"retry.botwork"| As |cached|
|value| = cached::Read
"#,
            RunOptions {
                working_directory: Some(harness.workspace.clone()),
                ..options()
            },
        )
        .await;
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.result
    );
    assert_eq!(result.variables["code"].to_string(), "BW4002");
    assert_eq!(result.variables["unpublished"].to_string(), "BW2002");
    assert_eq!(result.variables["value"].to_string(), "5");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn suspended_failures_keep_causes_and_native_panics_and_bad_returns_are_structured() {
    let mut engine = Engine::default();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Bad return")
                    .unwrap()
                    .returns(ValueKind::Int),
                |_, _| async {
                    tokio::task::yield_now().await;
                    Ok(Literal::Bool(true))
                },
            )
            .unwrap(),
        )
        .unwrap();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Panic later").unwrap(),
                |_, _| async {
                    tokio::task::yield_now().await;
                    panic!("deliberate async poll panic")
                },
            )
            .unwrap(),
        )
        .unwrap();
    for (statement, code) in [
        ("Bad return", DiagnosticCode::IncompatibleType),
        ("Panic later", DiagnosticCode::NativePanic),
    ] {
        let source = format!("Try {{ {statement} }} Catch {{ |value| = |missing| }}");
        let result = engine
            .run_source_async("handler-failure", &source, options())
            .await;
        let error = result.result.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
        assert_eq!(error.causes.len(), 1);
        assert_eq!(error.causes[0].code(), code);
        assert_eq!(
            error.causes[0].call_stack[0].call_site.text().trim(),
            statement
        );
        assert!(!result.variables.contains_key("value"));
    }
}

struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn waiting(drops: Arc<AtomicUsize>) -> NativeOperation {
    NativeOperation::asynchronous(
        StatementSignature::native("Wait forever").unwrap(),
        move |_, _| {
            let guard = Dropped(Arc::clone(&drops));
            async move {
                let _guard = guard;
                pending().await
            }
        },
    )
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn timeout_unwinds_pending_calls_and_bypasses_handlers_without_cancelling_the_parent() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut engine = Engine::default();
    engine
        .register_operation(waiting(Arc::clone(&drops)))
        .unwrap();
    let parent = OperationControl::default();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        engine.run_source_async(
            "timeout",
            r#"
|iterator| = |7|
Try {
  For |iterator| In |[1]| {
    Wait forever
  }
} Catch { |handled| = |true| }
|tail| = |true|
"#,
            RunOptions {
                timeout: Some(Duration::from_secs(1)),
                control: parent.clone(),
                ..options()
            },
        ),
    )
    .await
    .expect("run must finish by its deadline");
    assert_eq!(result.outcome(), RunOutcome::TimedOut);
    assert_eq!(result.variables["iterator"].to_string(), "7");
    assert!(!result.variables.contains_key("handled"));
    assert!(!result.variables.contains_key("tail"));
    assert_eq!(
        result
            .result
            .unwrap_err()
            .call_stack
            .last()
            .unwrap()
            .signature,
        "waitforever"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(!parent.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn dropping_a_suspended_run_releases_async_resources_and_operation_ownership() {
    let drops = Arc::new(AtomicUsize::new(0));
    let operation = waiting(Arc::clone(&drops));
    let budget = operation.ownership_budget().clone();
    let mut engine = Engine::default();
    engine.register_operation(operation).unwrap();
    let mut future =
        Box::pin(engine.run_source_async("abandon", "Outer { Wait forever }\nOuter", options()));
    assert!(matches!(
        future
            .as_mut()
            .poll(&mut TaskContext::from_waker(Waker::noop())),
        Poll::Pending
    ));
    assert_eq!(budget.usage().invocations, 1);
    drop(future);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(budget.usage().invocations, 0);
    assert_eq!(
        engine
            .run_source_async("fresh", "|x| = |8|", options())
            .await
            .variables["x"]
            .to_string(),
        "8"
    );
}

#[tokio::test(start_paused = true)]
async fn cpu_loops_yield_so_a_sibling_can_cancel_them() {
    let (entered, mut ready) = tokio::sync::mpsc::channel(1);
    let mut engine = Engine::default();
    engine
        .register_native("Entered", move |_, _| {
            entered.try_send(()).unwrap();
            Ok(Literal::None)
        })
        .unwrap();
    let engine = Arc::new(engine);
    let parent = OperationControl::default();
    let running = Arc::clone(&engine);
    let control = parent.clone();
    let task = tokio::spawn(async move {
        running
            .run_source_async(
                "loop",
                "|kept| = |7|\nEntered\nWhile |true| {}",
                RunOptions {
                    control,
                    limits: RunLimits {
                        steps: 10_000,
                        ..RunLimits::default()
                    },
                    ..options()
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await
        .expect("loop entered before the test deadline")
        .unwrap();
    parent.cancel();
    let result = task.await.unwrap();
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert_eq!(result.variables["kept"].to_string(), "7");
}

#[test]
fn synchronous_entry_points_reject_async_registries_before_any_effects() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let mut engine = Engine::default();
    engine
        .register_native("Effect", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    engine
        .register_operation(waiting(Arc::new(AtomicUsize::new(0))))
        .unwrap();
    let result = engine.run_source("sync", "Effect\nWait forever", options());
    assert_eq!(
        result.result.unwrap_err().code(),
        DiagnosticCode::AsyncRuntime
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut context = Context::default();
    context
        .register_operation(waiting(Arc::new(AtomicUsize::new(0))))
        .unwrap();
    let program = Program::parse("sync-context", "|changed| = |7|").unwrap();
    assert_eq!(
        evaluate_program_detailed(&program, &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::AsyncRuntime
    );
    let probe = Program::parse("probe", "|value| = |changed|").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    assert_eq!(
        runtime
            .block_on(evaluate_program_async(&probe, context))
            .unwrap_err()
            .code(),
        DiagnosticCode::UndefinedVariable
    );
}

#[test]
fn missing_runtime_is_a_structured_pre_effect_failure_and_unpolled_values_drop_safely() {
    let engine = Engine::default();
    let mut future = Box::pin(engine.run_source_async("no-runtime", "|x| = |7|", options()));
    let Poll::Ready(result) = future
        .as_mut()
        .poll(&mut TaskContext::from_waker(Waker::noop()))
    else {
        panic!("immediate rejection")
    };
    assert_eq!(
        result.result.unwrap_err().code(),
        DiagnosticCode::AsyncRuntime
    );
    assert!(!result.variables.contains_key("x"));
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let deep = (0..100_000).fold(Literal::None, |value, _| Literal::Array(vec![value]));
            let engine = Engine::default();
            drop(engine.run_source_async(
                "unpolled",
                "",
                RunOptions {
                    variables: BTreeMap::from([("deep".into(), deep)]),
                    ..options()
                },
            ));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn signatures_collisions_and_run_local_limits_are_enforced_before_publication() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let signature = StatementSignature::native("Number |value|")
        .unwrap()
        .parameter("value", ValueKind::Int)
        .unwrap()
        .returns(ValueKind::Int);
    let mut engine = Engine::default();
    engine
        .register_operation(
            NativeOperation::asynchronous(signature.clone(), move |values, _| {
                observed.fetch_add(1, Ordering::SeqCst);
                async move { Ok(values.into_iter().next().unwrap()) }
            })
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine
            .register_operation(
                NativeOperation::asynchronous(signature, |_, _| async { Ok(Literal::Int(99)) })
                    .unwrap()
            )
            .unwrap_err()
            .code(),
        DiagnosticCode::DuplicateStatement
    );
    let rejected = engine
        .run_source_async("kind", "|answer| = Number |false|", options())
        .await;
    assert_eq!(
        rejected.result.unwrap_err().code(),
        DiagnosticCode::IncompatibleType
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let accepted = engine
        .run_source_async("value", "|answer| = Number |7|", options())
        .await;
    assert_eq!(accepted.variables["answer"].to_string(), "7");
    let rejected = engine
        .run_source_async(
            "registry",
            "Number |7|",
            RunOptions {
                limits: RunLimits {
                    retained_registry: botwork::core::run::RetainedRegistryLimits {
                        entries: 0,
                        ..Default::default()
                    },
                    ..RunLimits::default()
                },
                ..options()
            },
        )
        .await;
    assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn source_program_file_and_owned_context_share_async_semantics() {
    let harness = Harness::new();
    let source = "|answer| = Later |8|";
    let program = Program::parse("owned", source).unwrap();
    std::fs::write(harness.workspace.join("entry.botwork"), source).unwrap();
    let operation = delayed(Arc::new(Mutex::new(Vec::new())));
    let mut engine = Engine::default();
    engine.register_operation(operation.clone()).unwrap();
    assert_eq!(
        engine
            .run_program_async(&program, options())
            .await
            .variables["answer"]
            .to_string(),
        "8"
    );
    assert_eq!(
        engine
            .run_file_async(harness.workspace.join("entry.botwork"), options())
            .await
            .variables["answer"]
            .to_string(),
        "8"
    );
    let mut context = Context::default();
    context.register_operation(operation).unwrap();
    assert_eq!(
        evaluate_program_async(&program, context)
            .await
            .unwrap()
            .to_string(),
        "8"
    );
}

#[test]
fn cli_keeps_output_tracing_and_pre_effect_timeout_contracts() {
    let harness = Harness::new();
    let output = harness
        .run_with_args(
            "async-cli",
            "|value| = |7|\nLog |value|",
            &["--debug"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"7\n");
    let trace = String::from_utf8(output.stderr).unwrap();
    assert_eq!(trace.lines().count(), 2);
    assert!(trace.contains("async-cli.botwork:2:1: call"));
    let expired = harness
        .run_with_args(
            "expired",
            "Log |7|",
            &["--timeout-ms", "0"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(expired.status.code(), Some(1));
    assert!(expired.stdout.is_empty());
    assert!(String::from_utf8(expired.stderr)
        .unwrap()
        .contains("BW5002"));
    let valid = harness
        .run("plain", "Log |8|", Duration::from_secs(5))
        .unwrap();
    assert!(valid.status.success());
    assert_eq!(valid.stdout, b"8\n");
    assert!(valid.stderr.is_empty());
}

#[tokio::test(start_paused = true)]
async fn cancelled_and_successful_siblings_keep_separate_variables_and_environments() {
    let (started, mut ready) = tokio::sync::mpsc::channel(1);
    let mut engine = Engine::default();
    engine
        .register_native("Environment", |_, environment| {
            Ok(Literal::String(
                environment
                    .get("BOTWORK_ASYNC_TEST")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
            ))
        })
        .unwrap();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Select |value|").unwrap(),
                move |values, _| {
                    let started = started.clone();
                    async move {
                        if matches!(values[0], Literal::Int(1)) {
                            started.send(()).await.unwrap();
                            pending().await
                        } else {
                            tokio::task::yield_now().await;
                            Ok(values.into_iter().next().unwrap())
                        }
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
    let engine = Arc::new(engine);
    let stopped = OperationControl::default();
    let sibling = Arc::clone(&engine);
    let control = stopped.clone();
    let source = "|configuration| = Environment\n|answer| = Select |input|";
    let pending = tokio::spawn(async move {
        sibling
            .run_source_async(
                "first",
                source,
                RunOptions {
                    control,
                    variables: BTreeMap::from([("input".into(), Literal::Int(1))]),
                    environment: BTreeMap::from([(
                        "BOTWORK_ASYNC_TEST".into(),
                        Some("first".into()),
                    )]),
                    ..options()
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await
        .expect("first operation entered before the test deadline")
        .unwrap();
    let success = engine
        .run_source_async(
            "second",
            source,
            RunOptions {
                variables: BTreeMap::from([("input".into(), Literal::Int(2))]),
                environment: BTreeMap::from([("BOTWORK_ASYNC_TEST".into(), Some("second".into()))]),
                ..options()
            },
        )
        .await;
    stopped.cancel();
    let cancelled = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .expect("cancellation reached the pending operation")
        .unwrap();
    assert_eq!(cancelled.outcome(), RunOutcome::Cancelled);
    assert_eq!(cancelled.variables["configuration"].to_string(), "first");
    assert!(!cancelled.variables.contains_key("answer"));
    assert_eq!(success.outcome(), RunOutcome::Succeeded);
    assert_eq!(success.variables["configuration"].to_string(), "second");
    assert_eq!(success.variables["answer"].to_string(), "2");
}

#[tokio::test]
async fn blocking_operations_leave_the_runtime_available_and_drain_before_return() {
    let (started, mut ready) = tokio::sync::mpsc::channel(1);
    let (release, receive) = std::sync::mpsc::channel();
    let receive = Arc::new(Mutex::new(receive));
    let finished = Arc::new(AtomicUsize::new(0));
    let finished_worker = Arc::clone(&finished);
    let mut engine = Engine::default();
    engine
        .register_operation(
            NativeOperation::blocking(
                StatementSignature::native("Blocking").unwrap(),
                std::num::NonZeroUsize::new(1).unwrap(),
                move |_, control| {
                    started.blocking_send(()).unwrap();
                    receive
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                    finished_worker.fetch_add(1, Ordering::SeqCst);
                    control.checkpoint()?;
                    Ok(Literal::None)
                },
            )
            .unwrap(),
        )
        .unwrap();
    let control = OperationControl::default();
    let cancelled = control.clone();
    let task = tokio::spawn(async move {
        engine
            .run_source_async(
                "blocking",
                "|kept| = |7|\nBlocking",
                RunOptions {
                    control,
                    ..options()
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await
        .unwrap()
        .unwrap();
    // This task runs on the same single-thread runtime while the OS callback waits.
    cancelled.cancel();
    release.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert_eq!(result.variables["kept"].to_string(), "7");
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn operation_and_result_limits_stop_before_handlers_or_destination_publication() {
    use botwork::core::{
        operation::{OperationBudget, OperationOwnershipLimits},
        value_limits::ValueLimits,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Large").unwrap(), move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Ok(Literal::String("12345".into())) }
        })
        .unwrap();
    let mut engine = Engine::default();
    engine
        .register_operation(
            operation
                .clone()
                .with_ownership_budget(OperationBudget::new(OperationOwnershipLimits {
                    invocations: 0,
                    ..Default::default()
                })),
        )
        .unwrap();
    let source = "Try { |answer| = Large } Catch { |handled| = |true| }";
    let rejected = engine
        .run_source_async("ownership", source, options())
        .await;
    assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
    assert!(rejected.variables.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut engine = Engine::default();
    engine.register_operation(operation).unwrap();
    let rejected = engine
        .run_source_async(
            "local-value",
            source,
            RunOptions {
                limits: RunLimits {
                    values: ValueLimits {
                        string_bytes: 4,
                        ..Default::default()
                    },
                    ..RunLimits::default()
                },
                ..options()
            },
        )
        .await;
    assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
    assert!(rejected.variables.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn isolated_operations_round_trip_typed_values_through_dsl_calls() {
    use botwork::core::worker::{
        protocol::WorkerProtocol, WorkerCommand, WorkerLimits, WorkerPool,
    };
    let executable = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("python3"))
        .find(|path| path.is_absolute() && path.is_file())
        .unwrap();
    let pool = WorkerPool::new(WorkerLimits {
        timeout: Duration::from_secs(5),
        cleanup_timeout: Duration::from_secs(1),
        ..Default::default()
    })
    .unwrap();
    let command = WorkerCommand {
        executable,
        arguments: vec![
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/support/typed_worker.py")
                .into_os_string(),
            "echo".into(),
        ],
        directory: std::env::temp_dir(),
        environment: Default::default(),
    };
    let operation = NativeOperation::isolated(
        StatementSignature::native("Echo |value|").unwrap(),
        pool.clone(),
        command,
        WorkerProtocol::default(),
    )
    .unwrap();
    let mut engine = Engine::default();
    engine.register_operation(operation).unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        engine.run_source_async(
            "isolated",
            "Wrap |value| { Return |@{ Echo |value| }| }\n|answer| = Wrap |{a: [1, \"é\"]}|",
            options(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.result
    );
    assert_eq!(result.variables["answer"].to_string(), r#"{"a": [1, "é"]}"#);
    assert!(pool
        .shutdown_wait(Duration::from_secs(2))
        .unwrap()
        .active
        .is_empty());
}
