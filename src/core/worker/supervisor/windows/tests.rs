//! Gated launches on Windows, as unix/launch/tests.rs has them: process
//! creation that has not returned leaves its worker pending, and the pool keeps
//! owning it, and its capacity, until the late child is reaped.
use super::*;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    num::NonZeroUsize,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

struct Held {
    entered: Receiver<Option<u32>>,
    release: Option<Sender<()>>,
}

impl Held {
    /// Hold the pool's next launch until released, having started its child
    /// first when `child` is true, or failing it as a missing executable.
    fn new(pool: &WorkerPool, child: bool) -> Self {
        let (ready, entered) = mpsc::channel();
        let (release, wait) = mpsc::channel::<()>();
        *pool.0 .0.launcher.lock().unwrap() = Some(Box::new(move |specification| {
            let owner = child.then(|| spawn(specification, false)).transpose()?;
            ready.send(owner.as_ref().map(ChildOwner::id)).unwrap();
            // A failed test cannot strand the child or the supervisor.
            wait.recv_timeout(Duration::from_secs(5))
                .map_err(io::Error::other)?;
            owner.ok_or_else(|| io::Error::from_raw_os_error(2))
        }));
        Self {
            entered,
            release: Some(release),
        }
    }

    fn entered(&self) -> Option<u32> {
        self.entered.recv_timeout(Duration::from_secs(5)).unwrap()
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

/// sort.exe reads its input to the end, as cat does.
fn command() -> WorkerCommand {
    let root = std::env::var_os("SystemRoot").unwrap();
    WorkerCommand {
        executable: PathBuf::from(&root).join("System32").join("sort.exe"),
        arguments: vec![],
        directory: std::env::temp_dir(),
        environment: BTreeMap::from([(OsString::from("SystemRoot"), root)]),
    }
}

fn report(handle: WorkerHandle) -> RetainedReport {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), handle.wait_retained())
                .await
                .expect("startup observation is bounded")
        })
}

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "reconciliation timed out");
        std::thread::sleep(QUANTUM);
    }
}

#[test]
fn a_stalled_launch_is_pending_and_its_late_child_is_reaped_on_windows() {
    let pool = pool();
    let mut held = Held::new(&pool, true);
    let handle = pool
        .start(command(), vec![b'x'; 100], OperationControl::default())
        .unwrap();
    assert!(held.entered().is_some());
    let delivered = report(handle);
    assert_eq!(delivered.report.outcome, WorkerOutcome::TimedOut);
    assert_eq!(delivered.report.cleanup, WorkerCleanup::Pending);
    assert!(!delivered.report.io_complete);
    let snapshot = pool.snapshot();
    assert_eq!(snapshot.active[0].cleanup, Some(WorkerCleanup::Pending));
    // The pending worker keeps the pool's only slot.
    assert!(pool
        .start(command(), vec![], OperationControl::default())
        .is_err());
    drop(delivered);
    held.release();
    until(|| pool.snapshot().active.is_empty());
    let record = &pool.snapshot().completed[0];
    assert_eq!(record.outcome, WorkerOutcome::TimedOut);
    assert_eq!(record.cleanup, WorkerCleanup::Reaped);
}

#[test]
fn shutdown_wait_bounds_a_stalled_launch_on_windows() {
    let pool = pool();
    let mut held = Held::new(&pool, false);
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
    let snapshot = pool.shutdown_wait(Duration::from_secs(5)).unwrap();
    assert!(snapshot.active.is_empty());
    assert_eq!(snapshot.completed[0].outcome, WorkerOutcome::Cancelled);
    assert_eq!(snapshot.completed[0].cleanup, WorkerCleanup::NotStarted);
}
