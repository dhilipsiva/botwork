use botwork::core::{
    ast::Program,
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLimits},
    grammar::{BWErr, Literal},
    operation::{NativeOperation, OperationControl},
    signature::StatementSignature,
};
use std::{
    future::Future,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    task::{Context, Waker},
    time::Duration,
};

fn deep() -> (Diagnostic, std::sync::Weak<BWErr>) {
    let mut value = Diagnostic::new(BWErr::NativeError("leaf".into()));
    let leaf = Arc::downgrade(&value.error);
    for _ in 0..100_000 {
        let mut parent = Diagnostic::new(BWErr::NativeError("parent".into()));
        parent.causes.push(value);
        value = parent;
    }
    (value, leaf)
}

#[test]
fn checked_host_clone_retains_codes_context_and_source_after_original_disposal() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let owner = Arc::downgrade(&program.source);
    let mut original =
        Diagnostic::new(BWErr::NativeError("first".into())).at(&program.statements[0].span);
    original
        .causes
        .push(Diagnostic::new(BWErr::ArithmeticError("zero".into())));
    let identity = original.error.clone();
    let clone = original
        .try_clone_with_limits(&DiagnosticLimits::default())
        .unwrap();
    original.discard();
    drop(program);
    assert!(owner.upgrade().is_some());
    assert!(Arc::ptr_eq(&identity, &clone.error));
    assert_eq!(clone.causes[0].code(), DiagnosticCode::Arithmetic);
    assert_eq!(clone.span.as_ref().unwrap().text(), "|x| = |1|");
    clone.discard();
    assert!(owner.upgrade().is_none());
}

#[test]
fn rejected_clone_leaves_original_available_for_explicit_disposal() {
    let (original, leaf) = deep();
    let error = original
        .try_clone_with_limits(&DiagnosticLimits::default())
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(leaf.upgrade().is_some());
    original.discard();
    assert!(leaf.upgrade().is_none());
}

#[tokio::test]
async fn async_and_blocking_results_dispose_deep_host_diagnostics_before_publication() {
    for blocking in [false, true] {
        let (diagnostic, leaf) = deep();
        let value = Mutex::new(Some(diagnostic));
        let signature = StatementSignature::native("Fail").unwrap();
        let operation = if blocking {
            NativeOperation::blocking(signature, NonZeroUsize::new(1).unwrap(), move |_, _| {
                Err(value.lock().unwrap().take().unwrap())
            })
            .unwrap()
        } else {
            NativeOperation::asynchronous(signature, move |_, _| {
                let diagnostic = value.lock().unwrap().take().unwrap();
                async move { Err(diagnostic) }
            })
            .unwrap()
        };
        let error = operation
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
        assert!(error.causes[0].omissions.is_some());
        assert!(error.span.is_none());
        assert!(DiagnosticLimits::default().check(&error).is_ok());
        error.discard();
        assert!(leaf.upgrade().is_none());
    }
}

#[tokio::test]
async fn abandoned_blocking_worker_error_is_destroyed_iteratively_and_releases_capacity() {
    let (diagnostic, leaf) = deep();
    let value = Mutex::new(Some(diagnostic));
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let operation = NativeOperation::blocking(
        StatementSignature::native("Fail").unwrap(),
        NonZeroUsize::new(1).unwrap(),
        move |_, control| {
            let Some(diagnostic) = value.lock().unwrap().take() else {
                return Ok(Literal::None);
            };
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            assert!(control.is_cancelled());
            Err(diagnostic)
        },
    )
    .unwrap();
    let mut invocation = Box::pin(operation.invoke(vec![], OperationControl::default()));
    assert!(invocation
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
        .await
        .unwrap()
        .unwrap();
    drop(invocation);
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while leaf.upgrade().is_some() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(operation
        .invoke(vec![], OperationControl::default())
        .await
        .is_ok());
}

#[tokio::test]
async fn cancellation_cleanup_keeps_bounded_evidence_after_deep_worker_disposal() {
    let (diagnostic, leaf) = deep();
    let value = Mutex::new(Some(diagnostic));
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let operation = NativeOperation::blocking(
        StatementSignature::native("Fail").unwrap(),
        NonZeroUsize::new(1).unwrap(),
        move |_, _| {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Err(value.lock().unwrap().take().unwrap())
        },
    )
    .unwrap();
    let control = OperationControl::default();
    let mut invocation = Box::pin(operation.invoke(vec![], control.clone()));
    assert!(invocation
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
        .await
        .unwrap()
        .unwrap();
    control.cancel();
    assert!(invocation
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    release_tx.send(()).unwrap();
    let error = invocation.await.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].causes[0].code(), DiagnosticCode::Native);
    assert!(error.causes[0].causes[0].omissions.is_some());
    assert!(leaf.upgrade().is_none());
    error.discard();
    assert!(leaf.upgrade().is_none());
}
