use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode,
    eval::{evaluate_program_async, Context},
    grammar::{BWErr, Literal},
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome, SnapshotLimits},
    signature::{StatementSignature, ValueKind},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

fn options() -> RunOptions {
    RunOptions {
        inherit_environment: false,
        ..RunOptions::default()
    }
}

#[tokio::test]
async fn ordinary_callbacks_run_off_executor_while_synchronous_calls_keep_the_calling_thread() {
    let caller = thread::current().id();
    let threads = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&threads);
    let mut engine = Engine::default();
    engine
        .register_native("Thread", move |_, _| {
            observed.lock().unwrap().push(thread::current().id());
            Ok(Literal::Int(7))
        })
        .unwrap();
    assert_eq!(
        engine.run_source("sync", "Thread", options()).outcome(),
        RunOutcome::Succeeded
    );
    assert_eq!(
        engine
            .run_source_async("async", "Thread", options())
            .await
            .outcome(),
        RunOutcome::Succeeded
    );
    let threads = threads.lock().unwrap();
    assert_eq!(threads[0], caller);
    assert_ne!(threads[1], caller);
}

#[tokio::test]
async fn blocked_native_callbacks_allow_sibling_runs_and_preserve_stop_causes_and_bindings() {
    let (entered, mut ready) = tokio::sync::mpsc::channel(1);
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    let mut engine = Engine::default();
    engine
        .register_native("Blocking", move |_, environment| {
            assert_eq!(
                environment.get("BOTWORK_BLOCKING"),
                Some(std::ffi::OsStr::new("first"))
            );
            entered.blocking_send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            assert!(environment.control().is_cancelled());
            Err(BWErr::NativeError("cleanup failed".into()))
        })
        .unwrap();
    let stop = OperationControl::default();
    let control = stop.clone();
    let task = tokio::spawn(async move {
        engine
            .run_source_async(
                "blocked",
                "|kept| = |7|\nOuter { Blocking }\nTry { Outer } Catch { |handled| = |true| }",
                RunOptions {
                    control,
                    environment: BTreeMap::from([(
                        "BOTWORK_BLOCKING".into(),
                        Some("first".into()),
                    )]),
                    ..options()
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await
        .unwrap()
        .unwrap();
    let sibling = Engine::default()
        .run_source_async("sibling", "|answer| = |42|", options())
        .await;
    assert_eq!(sibling.outcome(), RunOutcome::Succeeded);
    stop.cancel();
    release.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert_eq!(result.variables["kept"].to_string(), "7");
    assert!(!result.variables.contains_key("handled"));
    let error = result.result.unwrap_err();
    assert!(
        error
            .causes
            .iter()
            .any(|cause| cause.to_string().contains("cleanup failed")),
        "{error:?}"
    );
    assert_eq!(error.call_stack[0].signature, "blocking");
    assert_eq!(error.call_stack[1].signature, "outer");
    assert!(error
        .causes
        .iter()
        .any(|cause| cause.code() == DiagnosticCode::Native));
}

#[tokio::test]
async fn dropping_a_run_cancels_only_the_worker_child_and_keeps_arguments_until_cleanup() {
    let (entered, mut ready) = tokio::sync::mpsc::channel(1);
    let (release, wait) = mpsc::channel();
    let wait = Mutex::new(wait);
    let (done, mut finished) = tokio::sync::mpsc::channel(1);
    let mut engine = Engine::default();
    engine
        .register_native("Blocking |value|", move |values, environment| {
            entered.blocking_send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            done.blocking_send((environment.control().is_cancelled(), values[0].to_string()))
                .unwrap();
            Ok(values[0].clone())
        })
        .unwrap();
    let parent = OperationControl::default();
    let control = parent.clone();
    let task = tokio::spawn(async move {
        engine
            .run_source_async(
                "drop",
                "Blocking |[7, 8]|",
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
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(!parent.is_cancelled());
    release.send(()).unwrap();
    let (cancelled, argument) = tokio::time::timeout(Duration::from_secs(5), finished.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(cancelled);
    assert_eq!(argument, "[7, 8]");
    assert!(!parent.is_cancelled());
}

#[tokio::test]
async fn owned_context_callbacks_and_worker_value_failures_keep_language_contracts() {
    let mut context = Context::default();
    context
        .register_native("Echo |value|", |values| Ok(values[0].clone()))
        .unwrap();
    let program = Program::parse("context", "Echo |{a: [1, 2]}|").unwrap();
    let result = evaluate_program_async(&program, context).await.unwrap();
    assert_eq!(result.to_string(), "{\"a\": [1, 2]}");

    for (header, expected) in [
        ("Panic", DiagnosticCode::NativePanic),
        ("Bad", DiagnosticCode::IncompatibleType),
        ("Deep", DiagnosticCode::ResourceLimit),
    ] {
        let mut engine = Engine::default();
        engine
            .register_native_with_signature(
                StatementSignature::native(header)
                    .unwrap()
                    .returns(ValueKind::Int),
                move |_, _| match header {
                    "Panic" => panic!("native worker panic"),
                    "Bad" => Ok(Literal::String("wrong kind".into())),
                    _ => {
                        let mut value = Literal::None;
                        for _ in 0..100_000 {
                            value = Literal::Array(vec![value]);
                        }
                        Ok(value)
                    }
                },
            )
            .unwrap();
        let result = engine
            .run_source_async(
                "worker-error",
                &format!("|value| = |7|\n|value| = {header}"),
                options(),
            )
            .await;
        assert_eq!(result.variables["value"].to_string(), "7");
        let error = result.result.unwrap_err();
        assert_eq!(error.code(), expected);
        assert_eq!(error.call_stack[0].signature, header.to_lowercase());
    }
}

#[tokio::test]
async fn worker_call_frame_snapshots_are_admitted_before_callback_effects() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let mut engine = Engine::default();
    engine
        .register_native("Effect", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    for (entries, expected) in [(2, RunOutcome::LimitExceeded), (3, RunOutcome::Succeeded)] {
        let result = engine
            .run_source_async(
                "snapshot",
                "Effect",
                RunOptions {
                    limits: RunLimits {
                        snapshots: SnapshotLimits {
                            entries,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..options()
                },
            )
            .await;
        assert_eq!(result.outcome(), expected, "{:?}", result.result);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn blocked_output_destinations_leave_sibling_runs_responsive() {
    use std::{
        fs,
        io::Read,
        process::{Command, Stdio},
        time::{Instant, SystemTime, UNIX_EPOCH},
    };
    const PROBE: &str = "BOTWORK_IO_OUTPUT_PROBE";
    const MARKER: &str = "BOTWORK_IO_OUTPUT_MARKER";
    if let Ok(mode) = std::env::var(PROBE) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut context = Context::default();
            context.init_statements();
            let program = if mode == "log" {
                context
                    .set_input_variables(BTreeMap::from([(
                        "payload".into(),
                        Literal::String("x".repeat(512 * 1024)),
                    )]))
                    .unwrap();
                Program::parse("log", "Log |payload|").unwrap()
            } else {
                context.set_statement_tracing(true);
                Program::parse(&"t".repeat(512 * 1024), "|answer| = |7|").unwrap()
            };
            let (entered, ready) = tokio::sync::oneshot::channel();
            let writer = tokio::spawn(async move {
                entered.send(()).unwrap();
                evaluate_program_async(&program, context).await
            });
            ready.await.unwrap();
            let sibling = Engine::default()
                .run_source_async("sibling", "|answer| = |42|", options())
                .await;
            assert_eq!(sibling.outcome(), RunOutcome::Succeeded);
            fs::write(std::env::var_os(MARKER).unwrap(), "ready").unwrap();
            writer.await.unwrap().unwrap();
        });
        return;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("botwork-output-{}-{nonce}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    for mode in ["log", "trace"] {
        let marker = directory.join(mode);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "blocked_output_destinations_leave_sibling_runs_responsive",
                "--nocapture",
            ])
            .env(PROBE, mode)
            .env(MARKER, &marker)
            .stdin(Stdio::null());
        if mode == "log" {
            command.stdout(Stdio::piped()).stderr(Stdio::null());
        } else {
            command.stderr(Stdio::piped()).stdout(Stdio::null());
        }
        let mut child = command.spawn().unwrap();
        let mut pipe: Box<dyn Read + Send> = if mode == "log" {
            Box::new(child.stdout.take().unwrap())
        } else {
            Box::new(child.stderr.take().unwrap())
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline && child.try_wait().unwrap().is_none() {
            thread::sleep(Duration::from_millis(2));
        }
        let responsive = marker.exists();
        if !responsive {
            let _ = child.kill();
        }
        let capture = thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                break child.wait().unwrap();
            }
            thread::sleep(Duration::from_millis(2));
        };
        let output = capture.join().unwrap();
        assert!(
            responsive,
            "{mode} blocked the executor before its sibling could finish: {status}"
        );
        assert!(status.success(), "{mode}: {status}");
        assert!(
            output.len() > 512 * 1024,
            "{mode}: output was not fully drained"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test(start_paused = true)]
async fn native_deadlines_request_worker_cancellation_and_preserve_cleanup_errors() {
    let (entered, mut ready) = tokio::sync::mpsc::channel(1);
    let mut engine = Engine::default();
    engine
        .register_native("Wait", move |_, environment| {
            entered.blocking_send(()).unwrap();
            let watchdog = std::time::Instant::now() + Duration::from_secs(5);
            while !environment.control().is_cancelled() && std::time::Instant::now() < watchdog {
                thread::sleep(Duration::from_millis(1));
            }
            assert!(
                environment.control().is_cancelled(),
                "deadline did not request worker cancellation"
            );
            Err(BWErr::NativeError("deadline cleanup".into()))
        })
        .unwrap();
    let parent = OperationControl::default();
    let control = parent.clone();
    let task = tokio::spawn(async move {
        engine
            .run_source_async(
                "deadline",
                "|kept| = |7|\nWait",
                RunOptions {
                    control,
                    timeout: Some(Duration::from_secs(1)),
                    ..options()
                },
            )
            .await
    });
    ready.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    let result = task.await.unwrap();
    assert_eq!(result.outcome(), RunOutcome::TimedOut);
    assert_eq!(result.variables["kept"].to_string(), "7");
    assert_eq!(
        result.result.unwrap_err().causes[0].code(),
        DiagnosticCode::Native
    );
    assert!(!parent.is_cancelled());
}
