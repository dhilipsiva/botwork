use botwork::core::{
    ast::Program,
    diagnostic::{Diagnostic, DiagnosticCode},
    grammar::{BWErr, Literal},
    operation::{NativeOperation, OperationControl},
    signature::{StatementSignature, ValueKind},
};
use std::{
    future::{pending, Future},
    num::NonZeroUsize,
    pin::pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::time::Instant;

fn signature() -> StatementSignature {
    StatementSignature::native("Value |value|").unwrap()
}
fn never() -> NativeOperation {
    NativeOperation::asynchronous(signature(), |_, _| pending()).unwrap()
}
struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_and_expired_deadlines_prevent_callback_construction() {
    let operation = NativeOperation::asynchronous(signature(), |_, _| {
        panic!("must not construct callback future");
        #[allow(unreachable_code)]
        async {
            Ok(Literal::None)
        }
    })
    .unwrap();
    let cancelled = OperationControl::default();
    cancelled.cancel();
    let expired = OperationControl::default().child(Some(Instant::now()));
    for (control, code) in [
        (cancelled, DiagnosticCode::Cancelled),
        (expired, DiagnosticCode::Timeout),
    ] {
        let error = operation
            .invoke(vec![Literal::Int(1)], control)
            .await
            .unwrap_err();
        assert_eq!(error.code(), code);
        assert_eq!(error.span.unwrap().source().name(), "<native>");
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_drops_async_resources_before_return_and_preserves_parent_control() {
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = Arc::clone(&drops);
    let operation = NativeOperation::asynchronous(signature(), move |_, _| {
        let resource = Dropped(Arc::clone(&resource));
        async move {
            let _resource = resource;
            pending().await
        }
    })
    .unwrap();
    let parent = OperationControl::default();
    let child = parent.child(Some(Instant::now() + Duration::from_secs(5)));
    let error = operation
        .invoke(vec![Literal::Int(1)], child)
        .await
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(!parent.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn parent_cancellation_wakes_pending_work_and_drops_its_resource() {
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = Arc::clone(&drops);
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let operation = NativeOperation::asynchronous(signature(), move |_, _| {
        let resource = Dropped(Arc::clone(&resource));
        let started = started_tx.clone();
        async move {
            let _resource = resource;
            started.send(()).unwrap();
            pending().await
        }
    })
    .unwrap();
    let control = OperationControl::default();
    let parent = control.clone();
    let task = tokio::spawn(async move { operation.invoke(vec![Literal::None], control).await });
    started_rx.recv().await.unwrap();
    parent.cancel();
    assert_eq!(
        task.await.unwrap().unwrap_err().code(),
        DiagnosticCode::Cancelled
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn children_cannot_extend_deadlines_or_cancel_parents_and_siblings() {
    let parent = OperationControl::default().child(Some(Instant::now() + Duration::from_secs(3)));
    let later = parent.child(Some(Instant::now() + Duration::from_secs(30)));
    let sooner = parent.child(Some(Instant::now() + Duration::from_secs(1)));
    assert_eq!(later.deadline(), parent.deadline());
    assert!(sooner.deadline() < parent.deadline());
    sooner.cancel();
    assert!(!parent.is_cancelled());
    assert!(!later.is_cancelled());
    parent.cancel();
    assert!(later.is_cancelled());
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(
        later.checkpoint().unwrap_err().code(),
        DiagnosticCode::Cancelled
    );
}

#[tokio::test(start_paused = true)]
async fn self_cancellation_stops_async_work_without_cancelling_its_parent() {
    let parent = OperationControl::default();
    let operation = NativeOperation::asynchronous(signature(), |_, control| async move {
        control.cancel();
        pending().await
    })
    .unwrap();
    assert_eq!(
        operation
            .invoke(vec![Literal::None], parent.clone())
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::Cancelled
    );
    assert!(!parent.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn argument_and_return_contracts_preserve_typed_failures() {
    let operation = NativeOperation::asynchronous(
        signature()
            .parameter("value", ValueKind::Int)
            .unwrap()
            .returns(ValueKind::String),
        |values, _| async move { Ok(values[0].clone()) },
    )
    .unwrap();
    for (values, code) in [
        (vec![], DiagnosticCode::ParameterCount),
        (vec![Literal::Bool(true)], DiagnosticCode::IncompatibleType),
        (vec![Literal::Int(1)], DiagnosticCode::IncompatibleType),
    ] {
        assert_eq!(
            operation
                .invoke(values, OperationControl::default())
                .await
                .unwrap_err()
                .code(),
            code
        );
    }
    let identity =
        NativeOperation::asynchronous(
            signature(),
            |values, _| async move { Ok(values[0].clone()) },
        )
        .unwrap();
    assert!(matches!(
        identity
            .invoke(vec![Literal::None], OperationControl::default())
            .await,
        Ok(Literal::None)
    ));
    let invalid = Literal::Array(vec![Literal::Float(f32::NAN)]);
    assert_eq!(
        identity
            .invoke(vec![invalid.clone()], OperationControl::default())
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::Arithmetic
    );
    let invalid_return = NativeOperation::asynchronous(signature(), move |_, _| {
        let invalid = invalid.clone();
        async move { Ok(invalid) }
    })
    .unwrap();
    assert_eq!(
        invalid_return
            .invoke(vec![Literal::None], OperationControl::default())
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::Arithmetic
    );
}

#[tokio::test]
async fn diagnostic_spans_and_nested_causes_survive_the_async_boundary() {
    let original = Program::parse_detailed("original.botwork", "Return").unwrap_err();
    let mut error = Diagnostic::new(BWErr::NativeError("failed".into()));
    error.causes.push(original);
    let operation = NativeOperation::asynchronous(signature(), move |_, _| {
        let error = error.clone();
        async move { Err(error) }
    })
    .unwrap();
    let error = operation
        .invoke(vec![Literal::None], OperationControl::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Native);
    assert_eq!(
        error.causes[0].span.as_ref().unwrap().source().name(),
        "original.botwork"
    );
    assert_eq!(error.causes[0].code(), DiagnosticCode::InvalidControl);
}

#[tokio::test]
async fn callback_factory_and_future_panics_are_typed_failures() {
    let factory = NativeOperation::asynchronous(signature(), |_, _| {
        panic!("factory panic");
        #[allow(unreachable_code)]
        async {
            Ok(Literal::None)
        }
    })
    .unwrap();
    let future =
        NativeOperation::asynchronous(signature(), |_, _| async { panic!("poll panic") }).unwrap();
    for operation in [factory, future] {
        assert_eq!(
            operation
                .invoke(vec![Literal::None], OperationControl::default())
                .await
                .unwrap_err()
                .code(),
            DiagnosticCode::NativePanic
        );
    }
}

#[tokio::test]
async fn dropping_an_async_invocation_cancels_children_and_releases_resources() {
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = Arc::clone(&drops);
    let captured = Arc::new(Mutex::new(None));
    let callback_capture = Arc::clone(&captured);
    let operation = NativeOperation::asynchronous(signature(), move |_, control| {
        *callback_capture.lock().unwrap() = Some(control);
        let resource = Dropped(Arc::clone(&resource));
        async move {
            let _resource = resource;
            pending().await
        }
    })
    .unwrap();
    let mut future = Box::pin(operation.invoke(vec![Literal::None], OperationControl::default()));
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    drop(future);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(captured.lock().unwrap().as_ref().unwrap().is_cancelled());
}

#[tokio::test]
async fn blocking_cancellation_waits_for_completion_and_preserves_cleanup_errors() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let drops = Arc::new(AtomicUsize::new(0));
    let resource = Arc::clone(&drops);
    let operation = NativeOperation::blocking(
        signature(),
        NonZeroUsize::new(1).unwrap(),
        move |_, control| {
            let _resource = Dropped(Arc::clone(&resource));
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            assert!(control.is_cancelled());
            Err(BWErr::NativeError("cleanup failed".into()).into())
        },
    )
    .unwrap();
    let control = OperationControl::default();
    let cancel = control.clone();
    let task = tokio::spawn(async move { operation.invoke(vec![Literal::None], control).await });
    started_rx.recv().await.unwrap();
    cancel.cancel();
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    release_tx.send(()).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn blocking_capacity_is_shared_by_clones_and_waiting_calls_can_be_cancelled() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&count);
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let operation =
        NativeOperation::blocking(signature(), NonZeroUsize::new(1).unwrap(), move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            Ok(Literal::Int(7))
        })
        .unwrap();
    let first = operation.clone();
    let task = tokio::spawn(async move {
        first
            .invoke(vec![Literal::None], OperationControl::default())
            .await
    });
    started_rx.recv().await.unwrap();
    let control = OperationControl::default();
    let mut waiting = Box::pin(operation.invoke(vec![Literal::None], control.clone()));
    assert!(waiting
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    control.cancel();
    assert_eq!(waiting.await.unwrap_err().code(), DiagnosticCode::Cancelled);
    release_tx.send(()).unwrap();
    task.await.unwrap().unwrap();
    release_tx.send(()).unwrap();
    assert!(matches!(
        operation
            .invoke(vec![Literal::None], OperationControl::default())
            .await,
        Ok(Literal::Int(7))
    ));
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn blocking_panics_are_typed_and_release_capacity() {
    let operation =
        NativeOperation::blocking(signature(), NonZeroUsize::new(1).unwrap(), |values, _| {
            if matches!(values[0], Literal::Bool(true)) {
                panic!("worker panic");
            }
            Ok(Literal::None)
        })
        .unwrap();
    assert_eq!(
        operation
            .invoke(vec![Literal::Bool(true)], OperationControl::default())
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::NativePanic
    );
    assert!(matches!(
        operation
            .invoke(vec![Literal::Bool(false)], OperationControl::default())
            .await,
        Ok(Literal::None)
    ));
}

#[test]
fn missing_runtime_is_an_error_and_excessive_capacity_is_rejected() {
    let operation = never();
    let mut future = pin!(operation.invoke(vec![Literal::None], OperationControl::default()));
    let Poll::Ready(Err(error)) = future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("runtime error")
    };
    assert_eq!(error.code(), DiagnosticCode::AsyncRuntime);
    assert!(NativeOperation::blocking(
        signature(),
        NonZeroUsize::new(usize::MAX).unwrap(),
        |_, _| Ok(Literal::None)
    )
    .is_err());
}

#[test]
fn a_runtime_without_time_reports_a_typed_configuration_failure() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let error = runtime.block_on(async {
        never()
            .invoke(
                vec![Literal::None],
                OperationControl::default().child(Some(Instant::now() + Duration::from_secs(1))),
            )
            .await
            .unwrap_err()
    });
    assert_eq!(error.code(), DiagnosticCode::AsyncRuntime);
}

#[tokio::test(start_paused = true)]
async fn blocking_deadlines_wait_for_the_worker_and_keep_timeout_as_primary() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let operation = NativeOperation::blocking(
        signature(),
        NonZeroUsize::new(1).unwrap(),
        move |_, control| {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            control.checkpoint()?;
            Ok(Literal::None)
        },
    )
    .unwrap();
    let control = OperationControl::default().child(Some(Instant::now() + Duration::from_secs(5)));
    let task = tokio::spawn(async move { operation.invoke(vec![Literal::None], control).await });
    started_rx.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    release_tx.send(()).unwrap();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Cancelled);
}

#[tokio::test]
async fn dropping_a_blocking_invocation_signals_its_worker_and_retains_capacity_until_exit() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let (finished_tx, mut finished_rx) = tokio::sync::mpsc::unbounded_channel();
    let count = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&count);
    let operation = NativeOperation::blocking(
        signature(),
        NonZeroUsize::new(1).unwrap(),
        move |_, control| {
            calls.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            finished_tx.send(control.is_cancelled()).unwrap();
            Ok(Literal::None)
        },
    )
    .unwrap();
    let mut future = Box::pin(operation.invoke(vec![Literal::None], OperationControl::default()));
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    started_rx.recv().await.unwrap();
    drop(future);
    let mut waiting = Box::pin(operation.invoke(vec![Literal::None], OperationControl::default()));
    assert!(waiting
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    drop(waiting);
    release_tx.send(()).unwrap();
    assert!(finished_rx.recv().await.unwrap());
}
