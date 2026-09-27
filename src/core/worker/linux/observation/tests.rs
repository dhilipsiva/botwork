use super::*;
use std::sync::mpsc::{self, Receiver, Sender};

struct Gate {
    entered: Receiver<u32>,
    release: Option<Sender<()>>,
}
impl Gate {
    fn new(pool: &WorkerPool, point: Point, panic: bool) -> Self {
        let (ready, entered) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        pool.0 .0.io_hooks.lock().unwrap().push_back((
            point,
            Box::new(move |pid| {
                // Drop gates also run during unwinding: never panic on channel errors.
                let _ = ready.send(pid);
                let _ = wait.recv_timeout(Duration::from_secs(5));
                if panic {
                    panic!("controlled OS owner panic");
                }
            }),
        ));
        Self {
            entered,
            release: Some(release),
        }
    }
    fn entered(&self) -> u32 {
        self.entered.recv_timeout(Duration::from_secs(2)).unwrap()
    }
    fn release(&mut self) {
        if let Some(sender) = self.release.take() {
            let _ = sender.send(());
        }
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        self.release();
    }
}
fn pool() -> WorkerPool {
    WorkerPool::new(WorkerLimits {
        max_in_flight: NonZeroUsize::new(1).unwrap(),
        timeout: Duration::from_secs(5),
        cleanup_timeout: Duration::from_millis(30),
        ..Default::default()
    })
    .unwrap()
}
fn command(script: Option<&str>) -> WorkerCommand {
    WorkerCommand {
        executable: if script.is_some() {
            "/bin/sh"
        } else {
            "/bin/cat"
        }
        .into(),
        arguments: script.map_or_else(Vec::new, |script| vec!["-c".into(), script.into()]),
        directory: std::env::temp_dir(),
        environment: Default::default(),
    }
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}
fn report(handle: WorkerHandle) -> RetainedReport {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(2), handle.wait_retained())
            .await
            .expect("OS stalls must not block report observation")
    })
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "reconciliation timed out");
        std::thread::sleep(QUANTUM);
    }
}

#[test]
fn every_post_launch_os_boundary_can_be_stalled_without_blocking_cancellation() {
    for point in [
        Point::Setup,
        Point::Write,
        Point::WriteComplete,
        Point::Read,
        Point::ReadComplete,
        Point::Observe,
        Point::Terminate,
        Point::Reap,
        Point::Close,
    ] {
        let pool = pool();
        let mut gate = Gate::new(&pool, point, false);
        let retained = Arc::new(());
        let weak = Arc::downgrade(&retained);
        let control = OperationControl::default();
        let cleanup = matches!(point, Point::Terminate | Point::Reap);
        let command = command(cleanup.then_some("while :; do :; done"));
        let handle = pool
            .start_retained(
                command.clone(),
                b"payload".to_vec(),
                control.clone(),
                Some(retained),
            )
            .unwrap();
        if cleanup {
            until(|| pool.snapshot().active[0].pid.is_some());
            control.cancel();
        }
        let pid = gate.entered();
        control.cancel();
        let delivered = report(handle);
        assert_eq!(
            delivered.report.outcome,
            WorkerOutcome::Cancelled,
            "{point:?}"
        );
        assert_eq!(
            delivered.report.cleanup,
            WorkerCleanup::Pending,
            "{point:?}"
        );
        assert!(!delivered.report.progress_complete);
        assert!(!delivered.report.io_complete);
        if point == Point::WriteComplete {
            // The syscall accepted bytes, but it has not committed that count yet.
            assert_eq!(delivered.report.stdin_written, 0);
        }
        if point == Point::ReadComplete {
            assert!(delivered.report.stdout.is_empty());
        }
        drop(delivered);
        assert!(weak.upgrade().is_some());
        assert!(pool
            .start(command, vec![], OperationControl::default())
            .is_err());
        let snapshot = pool.shutdown_wait(Duration::ZERO).unwrap();
        assert_eq!(snapshot.active[0].pid, Some(pid));
        assert_eq!(snapshot.active[0].cleanup, Some(WorkerCleanup::Pending));
        gate.release();
        until(|| pool.snapshot().active.is_empty() && weak.upgrade().is_none());
        assert_eq!(
            pool.snapshot().completed[0].outcome,
            WorkerOutcome::Cancelled
        );
        assert_eq!(pool.snapshot().completed[0].cleanup, WorkerCleanup::Reaped);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
}

#[test]
fn successful_exit_with_stalled_cleanup_publishes_failure_before_execution_deadline() {
    for point in [Point::Terminate, Point::Reap] {
        let pool = pool();
        let mut gate = Gate::new(&pool, point, false);
        let handle = pool
            .start(command(Some("exit 0")), vec![], OperationControl::default())
            .unwrap();
        gate.entered();
        let delivered = report(handle);
        assert_eq!(delivered.report.outcome, WorkerOutcome::Failed);
        assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
        assert!(!delivered.report.progress_complete);
        assert_eq!(
            delivered.report.diagnostic.unwrap().code().as_str(),
            "BW5003"
        );
        gate.release();
        until(|| pool.snapshot().active.is_empty());
        let record = &pool.snapshot().completed[0];
        assert_eq!(record.outcome, WorkerOutcome::Failed);
        assert_eq!(record.cleanup, WorkerCleanup::Reaped);
        assert!(record.exit_status.unwrap().success());
    }
}

#[test]
fn stalled_panic_guard_retains_ownership_and_quarantines_late_completion() {
    let pool = pool();
    let mut panic_gate = Gate::new(&pool, Point::Observe, true);
    let mut drop_gate = Gate::new(&pool, Point::Drop, false);
    let retained = Arc::new(());
    let weak = Arc::downgrade(&retained);
    let control = OperationControl::default();
    let handle = pool
        .start_retained(
            command(Some("while :; do :; done")),
            vec![],
            control.clone(),
            Some(retained),
        )
        .unwrap();
    let pid = panic_gate.entered();
    panic_gate.release();
    assert_eq!(drop_gate.entered(), pid);
    control.cancel();
    let delivered = report(handle);
    assert_eq!(delivered.report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
    drop(delivered);
    assert!(weak.upgrade().is_some());
    drop_gate.release();
    until(|| pool.snapshot().active[0].cleanup == Some(WorkerCleanup::Unverified));
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::Cancelled
    );
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    assert!(weak.upgrade().is_some());
    drop(pool);
    until(|| weak.upgrade().is_none());
}

#[test]
fn dropping_pool_and_runtime_during_blocked_io_keeps_child_and_reservation_owned() {
    let pool = pool();
    let mut gate = Gate::new(&pool, Point::Write, false);
    let retained = Arc::new(());
    let weak = Arc::downgrade(&retained);
    let handle = pool
        .start_retained(
            command(None),
            vec![1],
            OperationControl::default(),
            Some(retained),
        )
        .unwrap();
    let pid = gate.entered();
    drop(pool);
    let delivered = report(handle); // its temporary runtime is dropped too
    assert_eq!(delivered.report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
    drop(delivered);
    assert!(weak.upgrade().is_some());
    gate.release();
    until(|| weak.upgrade().is_none());
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

use crate::core::{
    grammar::Literal,
    operation::{NativeOperation, OperationUsage},
    signature::StatementSignature,
    worker::protocol::WorkerProtocol,
};
fn typed(pool: &WorkerPool, response: DiagnosticResult<Literal>) -> NativeOperation {
    let protocol = WorkerProtocol::default();
    let bytes = protocol.encode_response(response.as_ref()).unwrap();
    let mut script = String::from("/bin/cat >/dev/null; printf '");
    for byte in bytes {
        use std::fmt::Write;
        write!(script, "\\{byte:03o}").unwrap();
    }
    script.push('\'');
    NativeOperation::isolated(
        StatementSignature::native("Echo |x|").unwrap(),
        pool.clone(),
        command(Some(&script)),
        protocol,
    )
    .unwrap()
}

#[test]
fn typed_value_read_in_flight_cannot_escape_timeout_or_release_charges() {
    let pool = WorkerPool::new(WorkerLimits {
        timeout: Duration::from_millis(300),
        ..pool().limits().clone()
    })
    .unwrap();
    let mut gate = Gate::new(&pool, Point::ReadComplete, false);
    let operation = typed(&pool, Ok(Literal::Int(7)));
    let runtime = runtime();
    let result = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(2),
            operation.invoke(
                vec![Literal::String("retained".into())],
                OperationControl::default(),
            ),
        )
        .await
        .unwrap()
    });
    let pid = gate.entered();
    assert_eq!(
        result.unwrap_err().code(),
        crate::core::diagnostic::DiagnosticCode::Timeout
    );
    assert_eq!(operation.ownership_budget().usage().invocations, 1);
    assert!(operation.isolated_in_flight_bytes().unwrap() > 0);
    drop(runtime);
    gate.release();
    until(|| operation.isolated_in_flight_bytes() == Some(0));
    assert_eq!(
        operation.ownership_budget().usage(),
        OperationUsage::default()
    );
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::TimedOut
    );
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn complete_foreign_error_survives_cancellation_while_pipe_close_is_stalled() {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let pool = WorkerPool::new(WorkerLimits {
        cleanup_timeout: Duration::from_millis(150),
        ..pool().limits().clone()
    })
    .unwrap();
    let mut read_gate = Gate::new(&pool, Point::ReadComplete, false);
    let mut close_gate = Gate::new(&pool, Point::Close, false);
    let operation = typed(
        &pool,
        Err(Diagnostic::formatted(
            BWErr::NativeError,
            format_args!("foreign failure"),
        )),
    );
    let control = OperationControl::default();
    let runtime = runtime();
    let mut invocation = Box::pin(operation.invoke(vec![Literal::None], control.clone()));
    runtime.block_on(async {
        assert!(matches!(
            invocation
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
    });
    read_gate.entered();
    control.cancel();
    read_gate.release();
    close_gate.entered();
    let error = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), invocation)
            .await
            .unwrap()
            .unwrap_err()
    });
    assert_eq!(
        error.code(),
        crate::core::diagnostic::DiagnosticCode::Cancelled
    );
    assert!(error
        .causes
        .iter()
        .any(|cause| cause.error.to_string().contains("foreign failure")));
    assert_eq!(
        pool.snapshot().active[0].cleanup,
        Some(WorkerCleanup::Pending)
    );
    assert_eq!(operation.ownership_budget().usage().invocations, 1);
    close_gate.release();
    until(|| operation.isolated_in_flight_bytes() == Some(0));
    assert_eq!(
        operation.ownership_budget().usage(),
        OperationUsage::default()
    );
}

#[test]
fn lost_child_ownership_starts_cleanup_observation_before_stalled_pipe_closure() {
    let pool = pool();
    let mut observe_gate = Gate::new(&pool, Point::Observe, false);
    let mut close_gate = Gate::new(&pool, Point::Close, false);
    let retained = Arc::new(());
    let weak = Arc::downgrade(&retained);
    let handle = pool
        .start_retained(
            command(Some("while :; do :; done")),
            vec![],
            OperationControl::default(),
            Some(retained),
        )
        .unwrap();
    let pid = observe_gate.entered();
    // Deliberate host interference, while the owner is gated and cannot reap.
    // SAFETY: pid is this test's live child; waitpid receives a valid status pointer.
    unsafe {
        assert_eq!(libc::kill(pid as libc::pid_t, libc::SIGKILL), 0);
        let mut status = 0;
        assert_eq!(
            libc::waitpid(pid as libc::pid_t, &mut status, 0),
            pid as libc::pid_t
        );
    }
    observe_gate.release();
    assert_eq!(close_gate.entered(), pid);
    let delivered = report(handle);
    assert_eq!(delivered.report.outcome, WorkerOutcome::Interrupted);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
    assert!(!delivered.report.progress_complete);
    drop(delivered);
    assert!(weak.upgrade().is_some());
    close_gate.release();
    until(|| pool.snapshot().active[0].cleanup == Some(WorkerCleanup::Unverified));
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::Interrupted
    );
    assert!(weak.upgrade().is_some());
    drop(pool);
    until(|| weak.upgrade().is_none());
}

#[test]
fn final_result_owns_last_payload_reservation_before_waking_receiver() {
    use std::{
        future::Future,
        task::{Context, Poll, Wake, Waker},
    };
    struct WakeGate {
        entered: Sender<u32>,
        release: Mutex<Receiver<()>>,
    }
    impl Wake for WakeGate {
        fn wake(self: Arc<Self>) {
            let _ = self.entered.send(0);
            let _ = self
                .release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
        }
    }
    let pool = pool();
    let mut setup = Gate::new(&pool, Point::Setup, false);
    let retained = Arc::new(());
    let weak = Arc::downgrade(&retained);
    let handle = pool
        .start_retained(
            command(None),
            b"payload".to_vec(),
            OperationControl::default(),
            Some(retained),
        )
        .unwrap();
    setup.entered();
    let (ready, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let mut wake_gate = Gate {
        entered,
        release: Some(release),
    };
    let waker = Waker::from(Arc::new(WakeGate {
        entered: ready,
        release: Mutex::new(wait),
    }));
    let mut waiting = Box::pin(handle.wait_retained());
    assert!(waiting
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    setup.release();
    wake_gate.entered(); // The observer is still inside the receiver's wake callback.
    let Poll::Ready(delivered) = waiting
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("completed report must be available before its wake finishes");
    };
    assert_eq!(delivered.report.outcome, WorkerOutcome::Succeeded);
    assert!(delivered.report.progress_complete);
    assert!(weak.upgrade().is_some());
    drop(delivered);
    assert!(
        weak.upgrade().is_none(),
        "no redundant OS/observer reservation after final handoff"
    );
    wake_gate.release();
}
