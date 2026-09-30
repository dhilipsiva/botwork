#![cfg(target_os = "linux")]
use botwork::core::{
    ast::Program,
    diagnostic::{CallFrame, Diagnostic, DiagnosticCode, DiagnosticLimits, RelatedLocation},
    grammar::{BWErr, Literal},
    operation::{
        NativeOperation, OperationBudget, OperationControl, OperationOwnershipLimits,
        OperationUsage,
    },
    signature::{StatementSignature, ValueKind},
    value_limits::ValueLimits,
    worker::{protocol::WorkerProtocol, WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool},
};
use std::{
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

fn pool() -> WorkerPool {
    WorkerPool::new(WorkerLimits {
        timeout: Duration::from_secs(5),
        cleanup_timeout: Duration::from_secs(1),
        ..Default::default()
    })
    .unwrap()
}
fn command(mode: &str) -> WorkerCommand {
    let executable = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("python3"))
        .find(|path| path.is_absolute() && path.is_file())
        .expect("Python3 used by repository checks");
    WorkerCommand {
        executable,
        arguments: vec![
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/support/typed_worker.py")
                .into_os_string(),
            mode.into(),
        ],
        directory: std::env::temp_dir(),
        environment: Default::default(),
    }
}
fn operation(
    pool: &WorkerPool,
    command: WorkerCommand,
    protocol: WorkerProtocol,
) -> NativeOperation {
    NativeOperation::isolated(
        StatementSignature::native("Echo |x|").unwrap(),
        pool.clone(),
        command,
        protocol,
    )
    .unwrap()
}
async fn invoke(operation: &NativeOperation, values: Vec<Literal>) -> Result<Literal, Diagnostic> {
    tokio::time::timeout(
        Duration::from_secs(8),
        operation.invoke(values, OperationControl::default()),
    )
    .await
    .expect("bounded invocation")
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("worker handshake/cleanup");
}
fn resource(error: &Diagnostic) -> &str {
    match error.error.as_ref() {
        BWErr::ResourceLimit { resource, .. } => resource,
        other => panic!("{other:?}"),
    }
}
fn fixture_error() -> Diagnostic {
    let parsed = Program::parse("worker-é.botwork", "|x| = |\"é\"|").unwrap();
    let span = &parsed.statements[0].span;
    let mut error = Diagnostic::new(BWErr::NativeError("worker failed".into())).at(span);
    error
        .causes
        .push(Diagnostic::new(BWErr::OutputError("partial".into())).at(span));
    error.call_stack.push(CallFrame {
        signature: "inner".into(),
        statement: None,
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    error.related.push(RelatedLocation {
        message: "related".into(),
        span: span.clone(),
    });
    error
}
fn with_error(mut command: WorkerCommand, error: &Diagnostic) -> WorkerCommand {
    let frame = WorkerProtocol::default()
        .encode_response(Err(error))
        .unwrap();
    command.environment.insert(
        "FRAME".into(),
        frame
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .into(),
    );
    command
}
struct Ready(PathBuf);
impl Ready {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "botwork-typed-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn command(&self, mut command: WorkerCommand) -> WorkerCommand {
        command
            .environment
            .insert("READY".into(), self.0.clone().into_os_string());
        command
    }
}
impl Drop for Ready {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[tokio::test]
async fn independent_worker_preserves_values_and_argument_order() {
    let pool = pool();
    let protocol = WorkerProtocol::default();
    let operation = operation(&pool, command("echo"), protocol.clone());
    for value in [
        Literal::None,
        Literal::Int(i32::MIN),
        Literal::Float(-0.0),
        Literal::Bool(true),
        Literal::String("é\0".into()),
        Literal::Array(vec![Literal::Int(3), Literal::None]),
        Literal::Map(std::collections::HashMap::from([(
            "é".into(),
            Literal::Float(1.0),
        )])),
    ] {
        let expected = protocol.encode_response(Ok(&value)).unwrap();
        let returned = invoke(&operation, vec![value]).await.unwrap();
        assert_eq!(protocol.encode_response(Ok(&returned)).unwrap(), expected);
    }
    let pack = NativeOperation::isolated(
        StatementSignature::native("Pack |first| |second|").unwrap(),
        pool,
        command("pack"),
        protocol,
    )
    .unwrap();
    let result = invoke(&pack, vec![Literal::Int(1), Literal::Int(2)])
        .await
        .unwrap();
    assert!(
        matches!(result, Literal::Array(values) if matches!(values.as_slice(), [Literal::Int(1), Literal::Int(2)]))
    );
}

#[tokio::test]
async fn request_signature_finite_and_frame_checks_happen_before_spawn() {
    let pool = pool();
    let operation = NativeOperation::isolated(
        StatementSignature::native("Echo |x|")
            .unwrap()
            .parameter("x", ValueKind::Int)
            .unwrap(),
        pool.clone(),
        command("echo"),
        WorkerProtocol::default(),
    )
    .unwrap();
    assert_eq!(
        invoke(&operation, vec![]).await.unwrap_err().code(),
        DiagnosticCode::ParameterCount
    );
    assert_eq!(
        invoke(&operation, vec![Literal::None])
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::IncompatibleType
    );
    assert_eq!(
        invoke(&operation, vec![Literal::Float(f32::NAN)])
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::Arithmetic
    );
    let mut protocol = WorkerProtocol::default();
    protocol.limits.frame_bytes = 0;
    let operation = NativeOperation::isolated(
        StatementSignature::native("Echo |x|").unwrap(),
        pool.clone(),
        command("echo"),
        protocol,
    )
    .unwrap();
    assert_eq!(
        resource(&invoke(&operation, vec![Literal::None]).await.unwrap_err()),
        "worker protocol frame bytes"
    );
    assert!(pool.snapshot().completed.is_empty());
}

#[tokio::test]
async fn invalid_results_never_publish_success() {
    let pool = pool();
    for mode in ["version", "truncated", "extra", "nan", "exit"] {
        let operation = operation(&pool, command(mode), WorkerProtocol::default());
        let error = invoke(&operation, vec![Literal::Int(1)]).await.unwrap_err();
        assert_eq!(
            error.code(),
            if mode == "exit" {
                DiagnosticCode::Native
            } else {
                DiagnosticCode::AsyncRuntime
            },
            "{mode}: {error}"
        );
        until(|| operation.isolated_in_flight_bytes() == Some(0)).await;
        assert_eq!(
            operation.ownership_budget().usage(),
            OperationUsage::default()
        );
    }
    let operation = NativeOperation::isolated(
        StatementSignature::native("Echo |x|")
            .unwrap()
            .returns(ValueKind::String),
        pool,
        command("echo"),
        WorkerProtocol::default(),
    )
    .unwrap();
    assert_eq!(
        invoke(&operation, vec![Literal::Int(1)])
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::IncompatibleType
    );
}

#[tokio::test]
async fn typed_diagnostics_preserve_codes_causes_and_original_source_context() {
    let pool = pool();
    let original = fixture_error();
    let operation = operation(
        &pool,
        with_error(command("echo"), &original),
        WorkerProtocol::default(),
    );
    let error = invoke(&operation, vec![Literal::None]).await.unwrap_err();
    let protocol = WorkerProtocol::default();
    assert_eq!(
        protocol.encode_response(Err(&error)).unwrap(),
        protocol.encode_response(Err(&original)).unwrap()
    );
    assert!(Arc::ptr_eq(
        error.span.as_ref().unwrap().source(),
        error.causes[0].span.as_ref().unwrap().source()
    ));
    assert_eq!(
        operation.ownership_budget().usage(),
        OperationUsage::default()
    );
}

#[tokio::test]
async fn source_free_errors_receive_the_native_header_and_local_limits_keep_evidence() {
    let pool = pool();
    let original = Diagnostic::new(BWErr::NativeError("worker failed".into()));
    let operation = operation(
        &pool,
        with_error(command("echo"), &original),
        WorkerProtocol::default(),
    );
    let error = invoke(&operation, vec![Literal::None]).await.unwrap_err();
    assert_eq!(error.span.as_ref().unwrap(), operation.signature().header());
    for dimension in 0..6 {
        let original = fixture_error();
        let mut limits = DiagnosticLimits::default();
        match dimension {
            0 => limits.diagnostics = 0,
            1 => limits.depth = 0,
            2 => limits.call_frames = 0,
            3 => limits.related_locations = 0,
            4 => limits.text_bytes = 0,
            _ => limits.source_bytes = 0,
        }
        let limited = operation.clone();
        let limited = NativeOperation::isolated(
            limited.signature().clone(),
            pool.clone(),
            with_error(command("echo"), &original),
            WorkerProtocol::default(),
        )
        .unwrap()
        .with_diagnostic_limits(limits)
        .unwrap();
        let error = invoke(&limited, vec![Literal::None]).await.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
        assert!(error.causes[0].span.is_none());
        assert_eq!(
            error.causes[0]
                .omissions
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap()
                .file,
            "worker-é.botwork"
        );
        assert_eq!(
            limited.ownership_budget().usage(),
            OperationUsage::default()
        );
    }
}

#[tokio::test]
async fn ownership_and_local_value_limits_reject_before_result_handoff() {
    let pool = pool();
    let value_limited = operation(&pool, command("large"), WorkerProtocol::default())
        .with_value_limits(ValueLimits {
            string_bytes: 3,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        resource(
            &invoke(&value_limited, vec![Literal::None])
                .await
                .unwrap_err()
        ),
        "value string bytes"
    );
    let value_limited = operation(&pool, command("large"), WorkerProtocol::default())
        .with_ownership_budget(OperationBudget::new(OperationOwnershipLimits {
            payload_bytes: 3,
            ..Default::default()
        }));
    assert_eq!(
        resource(
            &invoke(&value_limited, vec![Literal::None])
                .await
                .unwrap_err()
        ),
        "operation value payload bytes"
    );
    let error_limited = operation(
        &pool,
        with_error(command("echo"), &fixture_error()),
        WorkerProtocol::default(),
    )
    .with_ownership_budget(OperationBudget::new(OperationOwnershipLimits {
        source_bytes: 0,
        ..Default::default()
    }));
    let error = invoke(&error_limited, vec![Literal::None])
        .await
        .unwrap_err();
    assert_eq!(resource(&error), "operation diagnostic source bytes");
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    assert_eq!(
        error_limited.ownership_budget().usage(),
        OperationUsage::default()
    );
}

#[tokio::test]
async fn exact_wire_reservations_and_overflow_fail_before_entry() {
    let pool = pool();
    let protocol = WorkerProtocol::default();
    let required = protocol.encode_request(&[Literal::None]).unwrap().len()
        + pool.limits().stdout_bytes
        + pool.limits().stderr_bytes;
    for maximum in [0, required - 1, required] {
        let mut protocol = protocol.clone();
        protocol.limits.in_flight_bytes = maximum;
        let operation = operation(&pool, command("echo"), protocol);
        let result = invoke(&operation, vec![Literal::None]).await;
        if maximum < required {
            assert_eq!(
                resource(&result.unwrap_err()),
                "worker protocol in-flight bytes"
            );
        } else {
            assert!(result.is_ok());
        }
        until(|| operation.isolated_in_flight_bytes() == Some(0)).await;
    }
    assert_eq!(pool.snapshot().completed.len(), 1);
    let pool = WorkerPool::new(WorkerLimits {
        stdout_bytes: usize::MAX,
        ..Default::default()
    })
    .unwrap();
    let operation = operation(&pool, command("echo"), protocol);
    assert_eq!(
        resource(&invoke(&operation, vec![Literal::None]).await.unwrap_err()),
        "worker protocol in-flight bytes"
    );
    assert!(pool.snapshot().completed.is_empty());
}

#[tokio::test]
async fn cancellation_retains_complete_error_evidence_and_does_not_cancel_parent() {
    let pool = pool();
    let ready = Ready::new();
    let operation = operation(
        &pool,
        ready.command(with_error(command("reply_hang"), &fixture_error())),
        WorkerProtocol::default(),
    );
    let parent = OperationControl::default();
    let control = parent.child(None);
    let mut future = Box::pin(operation.invoke(vec![Literal::None], control.clone()));
    tokio::select! { result = &mut future => panic!("premature {result:?}"), _ = until(|| ready.0.exists()) => {} }
    control.cancel();
    let error = future.await.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert!(error
        .causes
        .iter()
        .any(|cause| cause.code() == DiagnosticCode::Native));
    assert!(!parent.is_cancelled());
    until(|| operation.isolated_in_flight_bytes() == Some(0)).await;
    assert!(pool.snapshot().active.is_empty());
}

#[tokio::test]
async fn deadlines_override_success_frames_and_preserve_timeout_category() {
    let pool = WorkerPool::new(WorkerLimits {
        timeout: Duration::from_millis(100),
        ..Default::default()
    })
    .unwrap();
    let operation = operation(&pool, command("reply_hang"), WorkerProtocol::default());
    assert_eq!(
        invoke(&operation, vec![Literal::Int(1)])
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::Timeout
    );
    until(|| operation.isolated_in_flight_bytes() == Some(0)).await;
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::TimedOut
    );
}

#[tokio::test]
async fn unpolled_and_abandoned_invocations_release_after_their_owners_finish() {
    let pool = pool();
    let ready = Ready::new();
    let operation = operation(
        &pool,
        ready.command(command("hang")),
        WorkerProtocol::default(),
    );
    let future = operation.invoke(
        vec![Literal::String("owned".into())],
        OperationControl::default(),
    );
    assert_eq!(operation.ownership_budget().usage().invocations, 1);
    assert_eq!(operation.isolated_in_flight_bytes(), Some(0));
    drop(future);
    assert_eq!(
        operation.ownership_budget().usage(),
        OperationUsage::default()
    );
    let mut future = Box::pin(operation.invoke(
        vec![Literal::String("owned".into())],
        OperationControl::default(),
    ));
    assert!(matches!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    until(|| ready.0.exists()).await;
    assert!(operation.isolated_in_flight_bytes().unwrap() > 0);
    drop(future);
    until(|| operation.isolated_in_flight_bytes() == Some(0)).await;
    assert_eq!(
        operation.ownership_budget().usage(),
        OperationUsage::default()
    );
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::Interrupted
    );
}

#[tokio::test]
async fn completed_unpolled_results_keep_shared_wire_and_argument_charges() {
    let pool = pool();
    let mut protocol = WorkerProtocol::default();
    protocol.limits.in_flight_bytes = protocol.encode_request(&[Literal::Int(1)]).unwrap().len()
        + pool.limits().stdout_bytes
        + pool.limits().stderr_bytes;
    let operation = operation(&pool, command("echo"), protocol);
    let mut future = Box::pin(operation.invoke(vec![Literal::Int(1)], OperationControl::default()));
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    until(|| !pool.snapshot().completed.is_empty()).await;
    assert_eq!(operation.ownership_budget().usage().invocations, 1);
    let clone = operation.clone();
    assert_eq!(
        resource(&invoke(&clone, vec![Literal::Int(2)]).await.unwrap_err()),
        "worker protocol in-flight bytes"
    );
    assert!(matches!(future.await.unwrap(), Literal::Int(1)));
    until(|| operation.isolated_in_flight_bytes() == Some(0)).await;
    assert_eq!(
        operation.ownership_budget().usage(),
        OperationUsage::default()
    );
    assert!(invoke(&clone, vec![Literal::Int(2)]).await.is_ok());
}

#[test]
fn runtime_shutdown_releases_typed_worker_ownership_after_reaping() {
    let pool = pool();
    let ready = Ready::new();
    let operation = operation(
        &pool,
        ready.command(command("hang")),
        WorkerProtocol::default(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let operation = operation.clone();
        tokio::spawn(async move {
            operation
                .invoke(vec![Literal::Int(1)], OperationControl::default())
                .await
        });
        until(|| ready.0.exists()).await;
    });
    assert!(operation.isolated_in_flight_bytes().unwrap() > 0);
    drop(runtime);
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while operation.isolated_in_flight_bytes() != Some(0) {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        operation.ownership_budget().usage(),
        OperationUsage::default()
    );
    assert!(pool.snapshot().active.is_empty());
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::Interrupted
    );
}

#[tokio::test]
async fn source_free_rejection_keeps_the_prospective_native_location() {
    let pool = pool();
    let operation = operation(
        &pool,
        with_error(
            command("echo"),
            &Diagnostic::new(BWErr::NativeError("failure".into())),
        ),
        WorkerProtocol::default(),
    )
    .with_diagnostic_limits(DiagnosticLimits {
        source_bytes: 0,
        ..Default::default()
    })
    .unwrap();
    let error = invoke(&operation, vec![Literal::None]).await.unwrap_err();
    assert_eq!(resource(&error), "diagnostic source bytes");
    let source = error.causes[0]
        .omissions
        .as_ref()
        .unwrap()
        .source
        .as_ref()
        .unwrap();
    assert_eq!(source.file, operation.signature().header().source().name());
    assert_eq!(source.start_byte, operation.signature().header().start());
    assert_eq!(source.end_byte, operation.signature().header().end());
}
