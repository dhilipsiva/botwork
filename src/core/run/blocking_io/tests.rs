use super::*;
use crate::core::diagnostic::DiagnosticCode;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

fn code<T>(result: Result<T, SourceFailure>) -> DiagnosticCode {
    match result {
        Err(SourceFailure::Diagnostic(error)) => error.code(),
        _ => panic!("expected a structured failure"),
    }
}

#[tokio::test]
async fn blocked_work_leaves_the_executor_available_and_holds_capacity_until_drained() {
    let capacity = Arc::new(Semaphore::new(1));
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    let control = OperationControl::default();
    let stopping = control.clone();
    let pool = Arc::clone(&capacity);
    let mut first = Box::pin(run_in(pool, control, move |_| {
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(7)
    }));
    assert!(first
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(capacity.available_permits(), 0);
    stopping.cancel();
    assert!(first
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    assert_eq!(capacity.available_permits(), 0);
    release.send(()).unwrap();
    assert_eq!(code(first.await), DiagnosticCode::Cancelled);
    assert_eq!(capacity.available_permits(), 1);
}

#[tokio::test(start_paused = true)]
async fn queued_cancellation_and_deadline_never_enter_work_or_cancel_the_parent() {
    let capacity = Arc::new(Semaphore::new(0));
    for deadline in [false, true] {
        let parent = OperationControl::default();
        let child =
            parent.child(deadline.then(|| tokio::time::Instant::now() + Duration::from_secs(1)));
        let stopping = child.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::clone(&calls);
        let mut future = Box::pin(run_in(Arc::clone(&capacity), child, move |_| {
            entered.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        assert!(future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        if deadline {
            tokio::time::advance(Duration::from_secs(1)).await;
        } else {
            stopping.cancel();
        }
        assert_eq!(
            code(future.await),
            if deadline {
                DiagnosticCode::Timeout
            } else {
                DiagnosticCode::Cancelled
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!parent.is_cancelled());
    }
}

#[tokio::test]
async fn dropping_started_work_retains_capacity_and_requests_child_cancellation() {
    let capacity = Arc::new(Semaphore::new(1));
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    let (done, finished) = tokio::sync::oneshot::channel();
    let parent = OperationControl::default();
    let mut future = Box::pin(run_in(
        Arc::clone(&capacity),
        parent.clone(),
        move |control| {
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            done.send(control.is_cancelled()).unwrap();
            Ok(())
        },
    ));
    assert!(matches!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    ready.await.unwrap();
    drop(future);
    assert_eq!(capacity.available_permits(), 0);
    assert!(!parent.is_cancelled());
    release.send(()).unwrap();
    assert!(finished.await.unwrap());
    // Admission, rather than a timing-dependent counter read, proves the late
    // result was disposed and the original permit returned.
    let next = tokio::time::timeout(
        Duration::from_secs(5),
        run_in(Arc::clone(&capacity), parent, |_| Ok(9)),
    )
    .await
    .unwrap();
    assert!(matches!(next, Ok(9)));
    assert_eq!(capacity.available_permits(), 1);
}

#[tokio::test]
async fn panic_and_closed_capacity_are_structured_and_release_admission() {
    let capacity = Arc::new(Semaphore::new(1));
    let result = run_in(
        Arc::clone(&capacity),
        OperationControl::default(),
        |_| -> Result<(), SourceFailure> { panic!("filesystem test panic") },
    )
    .await;
    assert_eq!(code(result), DiagnosticCode::AsyncRuntime);
    assert_eq!(capacity.available_permits(), 1);
    capacity.close();
    let result = run_in(capacity, OperationControl::default(), |_| Ok(())).await;
    assert_eq!(code(result), DiagnosticCode::AsyncRuntime);
}

#[test]
fn missing_runtime_and_stopped_entry_fail_without_executing_work() {
    for cancelled in [false, true] {
        let control = OperationControl::default();
        if cancelled {
            control.cancel();
        }
        let future = run_in(
            Arc::new(Semaphore::new(1)),
            control,
            |_| -> Result<(), SourceFailure> { panic!("work must not enter") },
        );
        let mut future = std::pin::pin!(future);
        let Poll::Ready(result) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("entry failure must be immediate")
        };
        assert_eq!(
            code(result),
            if cancelled {
                DiagnosticCode::Cancelled
            } else {
                DiagnosticCode::AsyncRuntime
            }
        );
    }
}

#[tokio::test]
async fn completed_payload_keeps_its_permit_until_handoff_or_disposal() {
    struct Payload(Arc<AtomicUsize>);
    impl Drop for Payload {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let capacity = Arc::new(Semaphore::new(1));
    let drops = Arc::new(AtomicUsize::new(0));
    let owned = Arc::clone(&drops);
    let (done, ready) = tokio::sync::oneshot::channel();
    let mut future = Box::pin(execute(
        Arc::clone(&capacity),
        OperationControl::default(),
        move |_| {
            done.send(()).unwrap();
            Payload(owned)
        },
    ));
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    ready.await.unwrap();
    assert_eq!(capacity.available_permits(), 0);
    let outcome = future.await.unwrap();
    assert_eq!(capacity.available_permits(), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(outcome);
    assert_eq!(capacity.available_permits(), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn file_reads_keep_only_the_limit_and_one_probe_byte_and_stop_before_open() {
    let path = std::env::temp_dir().join(format!("botwork-read-limit-{}", std::process::id()));
    fs::write(&path, vec![0xff; 65536]).unwrap();
    for maximum in [0, 1, 16383, 16384, 16385, 65536] {
        let Ok(bytes) = read(&path, maximum, &OperationControl::default()) else {
            panic!("read failed");
        };
        assert_eq!(bytes.len(), (maximum + 1).min(65536));
    }
    fs::remove_file(&path).unwrap();
    let stopped = OperationControl::default();
    stopped.cancel();
    assert_eq!(code(read(&path, 1, &stopped)), DiagnosticCode::Cancelled);
}

#[tokio::test]
async fn cancellation_keeps_a_concurrent_worker_panic_as_cleanup_evidence() {
    let control = OperationControl::default();
    let stop = control.clone();
    let result = execute(Arc::new(Semaphore::new(1)), control, move |_| {
        stop.cancel();
        panic!("failed while stopping");
    })
    .await;
    let Err(error) = result else {
        panic!("panic must fail")
    };
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::AsyncRuntime);
}

#[test]
fn aborting_a_queued_runtime_job_does_not_invent_a_cleanup_failure() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_time()
        .build()
        .unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    runtime.block_on(async {
        let occupied = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        ready.await.unwrap();
        let control = OperationControl::default();
        let pool = Arc::new(Semaphore::new(1));
        let mut queued = Box::pin(execute(Arc::clone(&pool), control.clone(), |_| {
            panic!("queued work must never enter");
        }));
        assert!(queued
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        assert_eq!(pool.available_permits(), 0);
        control.cancel();
        // Poll cancellation while the sole worker is still occupied. Aborting
        // the queued task must finish before its callback can be scheduled.
        let result = queued
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));
        release.send(()).unwrap();
        let result = match result {
            Poll::Ready(result) => result,
            Poll::Pending => queued.await,
        };
        occupied.await.unwrap();
        let Err(error) = result else {
            panic!("cancelled job must fail")
        };
        assert_eq!(error.code(), DiagnosticCode::Cancelled);
        assert!(error.causes.is_empty());
        assert_eq!(pool.available_permits(), 1);
    });
}

#[test]
fn interrupted_reads_retry_but_other_errors_stop_without_extra_io() {
    struct Reader {
        calls: usize,
        control: OperationControl,
        stop: bool,
    }
    impl Read for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            // Bound a faulty retry loop without relying on a wall-clock race.
            if self.calls > 3 {
                self.control.cancel();
            }
            match self.calls {
                1 => {
                    if self.stop {
                        self.control.cancel();
                    }
                    Err(std::io::ErrorKind::Interrupted.into())
                }
                2 => {
                    let count = bytes.len().min(4);
                    bytes[..count].fill(b'x');
                    Ok(count)
                }
                _ => Err(std::io::ErrorKind::PermissionDenied.into()),
            }
        }
    }
    for (maximum, stop) in [(3, false), (4, false), (3, true)] {
        let control = OperationControl::default();
        let mut reader = Reader {
            calls: 0,
            control: control.clone(),
            stop,
        };
        let result = read_from(&mut reader, maximum, &control);
        if stop {
            assert_eq!(code(result), DiagnosticCode::Cancelled);
            assert_eq!(reader.calls, 1);
        } else if maximum == 3 {
            assert!(matches!(result, Ok(ref bytes) if bytes == b"xxxx"));
            assert_eq!(
                reader.calls, 2,
                "a completed bounded read must not probe again"
            );
        } else {
            assert!(matches!(result, Err(SourceFailure::Io(ref error))
                if error.kind() == std::io::ErrorKind::PermissionDenied));
            assert_eq!(reader.calls, 3);
        }
    }
}
