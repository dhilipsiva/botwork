use super::*;
use std::{
    num::NonZeroUsize,
    sync::mpsc::{Receiver, Sender},
};

enum Completion {
    Child,
    ChildWithOutput,
    Failure,
    Panic,
}
struct Held {
    entered: Receiver<Option<u32>>,
    release: Option<Sender<()>>,
}
impl Held {
    fn new(pool: &WorkerPool, completion: Completion) -> Self {
        let (ready, entered) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        *pool.0 .0.launcher.lock().unwrap() = Some(Box::new(move |specification| {
            let child = matches!(completion, Completion::Child | Completion::ChildWithOutput)
                .then(|| spawn(specification))
                .transpose()?;
            if matches!(completion, Completion::ChildWithOutput) {
                let mut descriptor = libc::pollfd {
                    fd: child
                        .as_ref()
                        .unwrap()
                        .child
                        .stdout
                        .as_ref()
                        .unwrap()
                        .as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: this live child owns the descriptor and pollfd is valid
                // for this bounded readiness check; no pipe bytes are consumed.
                let ready = unsafe { libc::poll(&mut descriptor, 1, 2000) };
                if ready <= 0 || descriptor.revents & libc::POLLIN == 0 {
                    return Err(io::Error::other("test child did not produce output"));
                }
            }
            ready
                .send(child.as_ref().map(|child| child.child.id()))
                .unwrap();
            // Test failure cannot strand the process or supervisor indefinitely.
            wait.recv_timeout(Duration::from_secs(5))
                .map_err(io::Error::other)?;
            match completion {
                Completion::Child | Completion::ChildWithOutput => Ok(child.unwrap()),
                Completion::Failure => Err(io::Error::from_raw_os_error(libc::ENOENT)),
                Completion::Panic => panic!("controlled launcher panic"),
            }
        }));
        Self {
            entered,
            release: Some(release),
        }
    }
    fn entered(&self) -> Option<u32> {
        self.entered.recv_timeout(Duration::from_secs(2)).unwrap()
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
impl Drop for Held {
    fn drop(&mut self) {
        self.release();
    }
}

fn pool() -> WorkerPool {
    WorkerPool::new(WorkerLimits {
        max_in_flight: NonZeroUsize::new(1).unwrap(),
        timeout: Duration::from_millis(500),
        cleanup_timeout: Duration::from_millis(20),
        ..Default::default()
    })
    .unwrap()
}
fn command() -> WorkerCommand {
    WorkerCommand {
        executable: "/bin/cat".into(),
        arguments: vec![],
        directory: std::env::temp_dir(),
        environment: Default::default(),
    }
}
fn report(handle: WorkerHandle) -> RetainedReport {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(2), handle.wait_retained())
                .await
                .expect("startup observation is bounded")
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
fn stalled_child_transfer_returns_pending_and_reaps_late_child_without_changing_timeout() {
    let pool = pool();
    let mut held = Held::new(&pool, Completion::Child);
    let retained = Arc::new(());
    let weak = Arc::downgrade(&retained);
    let handle = pool
        .start_retained(
            command(),
            vec![1; 100],
            OperationControl::default(),
            Some(retained),
        )
        .unwrap();
    let pid = held.entered().unwrap();
    let delivered = report(handle);
    assert_eq!(delivered.report.outcome, WorkerOutcome::TimedOut);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
    assert_eq!(
        delivered.report.diagnostic.as_ref().unwrap().code(),
        crate::core::diagnostic::DiagnosticCode::Timeout
    );
    assert_eq!(delivered.report.stdin_written, 0);
    assert!(!delivered.report.io_complete);
    let snapshot = pool.snapshot();
    assert_eq!(snapshot.active[0].pid, None);
    assert_eq!(snapshot.active[0].cleanup, Some(WorkerCleanup::Pending));
    assert!(snapshot.completed.is_empty());
    assert!(pool
        .start(command(), vec![], OperationControl::default())
        .is_err());
    drop(delivered);
    assert!(weak.upgrade().is_some());
    held.release();
    until(|| pool.snapshot().active.is_empty() && weak.upgrade().is_none());
    let record = &pool.snapshot().completed[0];
    assert_eq!(record.outcome, WorkerOutcome::TimedOut);
    assert_eq!(record.cleanup, WorkerCleanup::Reaped);
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn shutdown_wait_bounds_stalled_startup_and_preserves_cancellation_after_late_failure() {
    let pool = pool();
    let mut held = Held::new(&pool, Completion::Failure);
    let handle = pool
        .start(command(), vec![], OperationControl::default())
        .unwrap();
    assert_eq!(held.entered(), None);
    let before = Instant::now();
    let snapshot = pool.shutdown_wait(Duration::from_millis(40)).unwrap();
    assert!(before.elapsed() < Duration::from_secs(1));
    assert!(snapshot.closed);
    assert_eq!(snapshot.active.len(), 1);
    assert!(snapshot.active[0].stopping);
    let delivered = report(handle);
    assert_eq!(delivered.report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
    held.release();
    let snapshot = pool.shutdown_wait(Duration::from_secs(2)).unwrap();
    assert!(snapshot.active.is_empty());
    assert_eq!(snapshot.completed[0].outcome, WorkerOutcome::Cancelled);
    assert_eq!(snapshot.completed[0].cleanup, WorkerCleanup::NotStarted);
}

#[test]
fn dropping_handle_while_startup_is_stalled_records_interruption_after_late_failure() {
    let pool = pool();
    let mut held = Held::new(&pool, Completion::Failure);
    let handle = pool
        .start(command(), vec![], OperationControl::default())
        .unwrap();
    held.entered();
    drop(handle);
    until(|| pool.snapshot().active[0].cleanup == Some(WorkerCleanup::Pending));
    held.release();
    until(|| pool.snapshot().active.is_empty());
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::Interrupted
    );
    assert_eq!(
        pool.snapshot().completed[0].cleanup,
        WorkerCleanup::NotStarted
    );
}

#[test]
fn pool_drop_retains_launch_ownership_until_late_child_is_reaped() {
    let pool = pool();
    let mut held = Held::new(&pool, Completion::Child);
    let retention = Arc::new(());
    let weak = Arc::downgrade(&retention);
    let handle = pool
        .start_retained(
            command(),
            vec![],
            OperationControl::default(),
            Some(retention),
        )
        .unwrap();
    let pid = held.entered().unwrap();
    drop(pool);
    let delivered = report(handle);
    assert_eq!(delivered.report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
    drop(delivered);
    assert!(weak.upgrade().is_some());
    held.release();
    until(|| weak.upgrade().is_none());
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn launcher_panic_quarantines_uncertain_capacity_and_shutdown_wait_stays_bounded() {
    let pool = pool();
    let mut held = Held::new(&pool, Completion::Panic);
    let retention = Arc::new(());
    let weak = Arc::downgrade(&retention);
    let handle = pool
        .start_retained(
            command(),
            vec![],
            OperationControl::default(),
            Some(retention),
        )
        .unwrap();
    held.entered();
    held.release();
    let delivered = report(handle);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Unverified);
    assert_ne!(delivered.report.outcome, WorkerOutcome::Succeeded);
    drop(delivered);
    let snapshot = pool.shutdown_wait(Duration::from_millis(10)).unwrap();
    assert_eq!(snapshot.active[0].cleanup, Some(WorkerCleanup::Unverified));
    assert!(weak.upgrade().is_some());
    drop(pool);
    until(|| weak.upgrade().is_none());
}

#[test]
fn late_child_within_cleanup_allowance_is_reaped_before_terminal_report() {
    let pool = WorkerPool::new(WorkerLimits {
        timeout: Duration::from_secs(5),
        cleanup_timeout: Duration::from_secs(2),
        ..Default::default()
    })
    .unwrap();
    let mut held = Held::new(&pool, Completion::Child);
    let control = OperationControl::default();
    let handle = pool
        .start(command(), vec![1; 100], control.clone())
        .unwrap();
    held.entered();
    control.cancel();
    held.release();
    let delivered = report(handle);
    assert_eq!(delivered.report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Reaped);
    assert_eq!(delivered.report.stdin_written, 0);
    assert!(pool.snapshot().active.is_empty());
}

#[test]
fn failed_launch_result_delivery_destroys_and_reaps_the_owned_child() {
    let pool = pool();
    let child = spawn(command()).unwrap();
    let pid = child.child.id();
    let (send, receive) = mpsc::sync_channel(1);
    drop(receive);
    drop(send.send(LaunchResult {
        result: Ok(child),
        _shared: pool.0 .0.clone(),
    }));
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn typed_invocation_keeps_argument_and_wire_charges_after_startup_timeout() {
    use crate::core::{
        grammar::Literal,
        operation::{NativeOperation, OperationUsage},
        signature::StatementSignature,
        worker::protocol::WorkerProtocol,
    };
    let pool = pool();
    let mut held = Held::new(&pool, Completion::Child);
    let operation = NativeOperation::isolated(
        StatementSignature::native("Echo |x|").unwrap(),
        pool.clone(),
        command(),
        WorkerProtocol::default(),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
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
    let pid = held.entered().unwrap();
    assert_eq!(
        result.unwrap_err().code(),
        crate::core::diagnostic::DiagnosticCode::Timeout
    );
    assert_eq!(operation.ownership_budget().usage().invocations, 1);
    assert!(operation.isolated_in_flight_bytes().unwrap() > 0);
    drop(runtime);
    held.release();
    until(|| operation.isolated_in_flight_bytes() == Some(0));
    assert_eq!(
        operation.ownership_budget().usage(),
        OperationUsage::default()
    );
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn late_output_overflow_preserves_the_already_published_cancellation() {
    let pool = WorkerPool::new(WorkerLimits {
        stdout_bytes: 0,
        timeout: Duration::from_secs(5),
        cleanup_timeout: Duration::ZERO,
        ..Default::default()
    })
    .unwrap();
    let mut held = Held::new(&pool, Completion::ChildWithOutput);
    let specification = WorkerCommand {
        executable: "/bin/sh".into(),
        arguments: vec!["-c".into(), "printf output; exec /bin/cat".into()],
        ..command()
    };
    let handle = pool
        .start(specification, vec![], OperationControl::default())
        .unwrap();
    held.entered();
    handle.cancel();
    let delivered = report(handle);
    assert_eq!(delivered.report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
    held.release();
    until(|| pool.snapshot().active.is_empty());
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::Cancelled
    );
    assert_eq!(pool.snapshot().completed[0].cleanup, WorkerCleanup::Reaped);
}
