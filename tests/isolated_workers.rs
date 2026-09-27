#![cfg(target_os = "linux")]

use botwork::core::{
    diagnostic::DiagnosticCode,
    operation::OperationControl,
    worker::{WorkerCleanup, WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool, WorkerReport},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    num::NonZeroUsize,
    path::PathBuf,
    time::{Duration, Instant},
};

fn limits() -> WorkerLimits {
    WorkerLimits {
        timeout: Duration::from_secs(3),
        cleanup_timeout: Duration::from_secs(1),
        ..Default::default()
    }
}

fn command(script: &str) -> WorkerCommand {
    WorkerCommand {
        executable: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".into(), script.into()],
        directory: std::env::temp_dir(),
        environment: BTreeMap::new(),
    }
}

fn wait(handle: botwork::core::worker::WorkerHandle) -> WorkerReport {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(6), handle.wait())
                .await
                .expect("supervision must finish without callback cooperation")
        })
}

fn until(mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < end, "worker cleanup/handshake timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn complete_binary_io_exit_status_and_empty_environment_are_preserved() {
    let pool = WorkerPool::new(limits()).unwrap();
    let mut specification = command("printf '%s' \"$WORKER_MARK\" >&2; /bin/cat");
    specification
        .environment
        .insert("WORKER_MARK".into(), "é".into());
    let input = b"hello\0\xff\n".to_vec();
    let report = wait(
        pool.start(specification, input.clone(), OperationControl::default())
            .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Succeeded);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert!(report.exit_status.unwrap().success());
    assert!(report.io_complete);
    assert_eq!(report.stdin_written, input.len());
    assert_eq!(report.stdout, input);
    assert_eq!(report.stderr, "é".as_bytes());
    assert!(report.diagnostic.is_none());
    assert!(pool.snapshot().active.is_empty());
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::Succeeded
    );
    let report = wait(
        pool.start(
            command("test -z \"$HOME\" && test -z \"$WORKER_MARK\""),
            vec![],
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Succeeded);
}

#[test]
fn nonzero_exit_and_incomplete_requests_are_failures_even_with_valid_output() {
    let pool = WorkerPool::new(limits()).unwrap();
    let report = wait(
        pool.start(
            command("printf complete; exit 7"),
            vec![],
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert_eq!(report.stdout, b"complete");
    assert_eq!(report.exit_status.unwrap().code(), Some(7));
    assert_eq!(report.diagnostic.unwrap().code(), DiagnosticCode::Native);
    let report = wait(
        pool.start(
            command("exec 0<&-; printf partial"),
            vec![1; 1024 * 1024],
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert!(!report.io_complete);
    assert!(report.stdin_written < 1024 * 1024);
    assert!(report.diagnostic.is_some());
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
}

#[test]
fn deadline_kills_cpu_bound_work_that_ignores_cooperative_signals() {
    let pool = WorkerPool::new(WorkerLimits {
        timeout: Duration::from_millis(100),
        ..limits()
    })
    .unwrap();
    let start = Instant::now();
    let handle = pool
        .start(
            command("trap '' TERM; printf begun; while :; do :; done"),
            vec![],
            OperationControl::default(),
        )
        .unwrap();
    let id = handle.id();
    let mut pid = None;
    until(|| {
        pid = pool
            .snapshot()
            .active
            .iter()
            .find(|active| active.id == id)
            .and_then(|active| active.pid);
        pid.is_some()
    });
    let pid = pid.unwrap();
    // The deadline progresses without polling a Tokio task.
    std::thread::sleep(Duration::from_millis(150));
    let report = wait(handle);
    assert_eq!(report.outcome, WorkerOutcome::TimedOut);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert_eq!(report.stdout, b"begun");
    assert_eq!(report.diagnostic.unwrap().code(), DiagnosticCode::Timeout);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "direct child must be reaped"
    );
}

#[test]
fn blocked_stdin_and_inherited_output_descriptors_do_not_hold_completion_forever() {
    let pool = WorkerPool::new(WorkerLimits {
        timeout: Duration::from_millis(80),
        ..limits()
    })
    .unwrap();
    let report = wait(
        pool.start(
            command("while :; do :; done"),
            vec![1; 1024 * 1024],
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::TimedOut);
    assert!(!report.io_complete);
    assert!(report.stdin_written < 1024 * 1024);
    let pool = WorkerPool::new(limits()).unwrap();
    let report = wait(
        pool.start(
            command("/bin/sleep 30 & printf returned"),
            vec![],
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.stdout, b"returned");
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert!(
        report.io_complete,
        "inherited pipes close after process-group termination"
    );
}

#[test]
fn cancellation_and_abandonment_reap_children_and_publish_distinct_records() {
    let pool = WorkerPool::new(limits()).unwrap();
    let control = OperationControl::default();
    let handle = pool
        .start(command("while :; do :; done"), vec![], control.clone())
        .unwrap();
    until(|| {
        pool.snapshot()
            .active
            .first()
            .is_some_and(|active| active.pid.is_some())
    });
    handle.cancel();
    let report = wait(handle);
    assert_eq!(report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert!(!control.is_cancelled());
    let handle = pool
        .start(command("while :; do :; done"), vec![], control.clone())
        .unwrap();
    let id = handle.id();
    until(|| {
        pool.snapshot()
            .active
            .first()
            .is_some_and(|active| active.pid.is_some())
    });
    drop(handle);
    until(|| pool.snapshot().active.is_empty());
    let snapshot = pool.snapshot();
    let record = snapshot
        .completed
        .iter()
        .find(|record| record.id == id)
        .unwrap();
    assert_eq!(record.outcome, WorkerOutcome::Interrupted);
    assert_eq!(record.cleanup, WorkerCleanup::Reaped);
    assert!(!control.is_cancelled());
}

#[test]
fn dropping_a_wait_future_also_interrupts_and_supervision_survives_runtime_shutdown() {
    let pool = WorkerPool::new(limits()).unwrap();
    let handle = pool
        .start(
            command("while :; do :; done"),
            vec![],
            OperationControl::default(),
        )
        .unwrap();
    let id = handle.id();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let future = handle.wait();
        tokio::pin!(future);
        tokio::select! { _ = &mut future => panic!("worker must still run"), _ = tokio::time::sleep(Duration::from_millis(20)) => {} }
    });
    drop(runtime);
    until(|| pool.snapshot().active.is_empty());
    let record = pool
        .snapshot()
        .completed
        .into_iter()
        .find(|record| record.id == id)
        .unwrap();
    assert_eq!(record.outcome, WorkerOutcome::Interrupted);
    assert_eq!(record.cleanup, WorkerCleanup::Reaped);
}

#[test]
fn byte_limits_admit_exact_streams_and_stop_floods_before_buffer_growth() {
    for (stdout_bytes, stderr_bytes, expected) in [
        (3, 2, None),
        (2, 2, Some("worker stdout bytes")),
        (3, 1, Some("worker stderr bytes")),
    ] {
        let pool = WorkerPool::new(WorkerLimits {
            stdout_bytes,
            stderr_bytes,
            ..limits()
        })
        .unwrap();
        let report = wait(
            pool.start(
                command("printf abc; printf xy >&2"),
                vec![],
                OperationControl::default(),
            )
            .unwrap(),
        );
        assert!(report.stdout.len() <= stdout_bytes && report.stderr.len() <= stderr_bytes);
        if let Some(resource) = expected {
            assert_eq!(report.outcome, WorkerOutcome::Failed);
            assert!(report.diagnostic.unwrap().to_string().contains(resource));
        } else {
            assert_eq!(report.outcome, WorkerOutcome::Succeeded);
        }
        assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    }
    let pool = WorkerPool::new(WorkerLimits {
        stdout_bytes: 17,
        stderr_bytes: 13,
        ..limits()
    })
    .unwrap();
    let report = wait(
        pool.start(
            command("while :; do printf 01234567890123456789; printf abcdefghijklmnop >&2; done"),
            vec![],
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert!(!report.io_complete);
    assert!(report.stdout.len() <= 17 && report.stderr.len() <= 13);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    let pool = WorkerPool::new(WorkerLimits {
        stdout_bytes: 0,
        stderr_bytes: 0,
        ..limits()
    })
    .unwrap();
    let report = wait(
        pool.start(command(":"), vec![], OperationControl::default())
            .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Succeeded);
}

#[test]
fn admission_capacity_and_shutdown_are_shared_across_clones_without_parent_cancellation() {
    let pool = WorkerPool::new(WorkerLimits {
        max_in_flight: NonZeroUsize::new(1).unwrap(),
        ..limits()
    })
    .unwrap();
    let sibling = pool.clone();
    let parent = OperationControl::default();
    let handle = pool
        .start(command("while :; do :; done"), vec![], parent.clone())
        .unwrap();
    assert!(sibling.start(command(":"), vec![], parent.clone()).is_err());
    assert!(sibling.shutdown().closed);
    assert!(pool.start(command(":"), vec![], parent.clone()).is_err());
    assert_eq!(wait(handle).outcome, WorkerOutcome::Cancelled);
    assert!(pool.snapshot().active.is_empty());
    assert!(!parent.is_cancelled());
}

#[test]
fn zero_deadlines_and_stopped_controls_skip_process_entry_and_capacity_is_reusable() {
    let pool = WorkerPool::new(WorkerLimits {
        timeout: Duration::ZERO,
        ..limits()
    })
    .unwrap();
    let report = wait(
        pool.start(command("exit 99"), vec![], OperationControl::default())
            .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::TimedOut);
    assert_eq!(report.cleanup, WorkerCleanup::NotStarted);
    assert!(report.exit_status.is_none());
    let cancelled = OperationControl::default();
    cancelled.cancel();
    assert_eq!(
        pool.start(command(":"), vec![], cancelled)
            .err()
            .unwrap()
            .code(),
        DiagnosticCode::Cancelled
    );
    let expired = OperationControl::default().child(Some(tokio::time::Instant::now()));
    assert_eq!(
        pool.start(command(":"), vec![], expired)
            .err()
            .unwrap()
            .code(),
        DiagnosticCode::Timeout
    );
    assert!(pool.snapshot().active.is_empty());
    assert_eq!(pool.snapshot().completed.len(), 1);
}

#[test]
fn command_arguments_are_not_shell_expanded_and_working_directory_is_local() {
    let pool = WorkerPool::new(limits()).unwrap();
    let argument = OsString::from("$(exit 93); 'quoted' é");
    let report = wait(
        pool.start(
            WorkerCommand {
                executable: PathBuf::from("/bin/printf"),
                arguments: vec!["%s".into(), argument.clone()],
                directory: PathBuf::from("/"),
                environment: BTreeMap::new(),
            },
            vec![],
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Succeeded);
    assert_eq!(report.stdout, argument.as_encoded_bytes());
    let cwd = std::env::current_dir().unwrap();
    let mut specification = command("pwd");
    specification.directory = PathBuf::from("/");
    assert_eq!(
        wait(
            pool.start(specification, vec![], OperationControl::default())
                .unwrap()
        )
        .stdout,
        b"/\n"
    );
    assert_eq!(std::env::current_dir().unwrap(), cwd);
}

#[test]
fn configuration_spawn_failure_and_bounded_history_preserve_explicit_evidence() {
    assert!(WorkerPool::new(WorkerLimits {
        timeout: Duration::MAX,
        ..limits()
    })
    .is_err());
    let pool = WorkerPool::new(WorkerLimits {
        request_bytes: 0,
        history_records: 1,
        ..limits()
    })
    .unwrap();
    assert!(pool
        .start(command(":"), vec![1], OperationControl::default())
        .is_err());
    let mut specification = command(":");
    specification.executable = "relative".into();
    assert!(pool
        .start(specification, vec![], OperationControl::default())
        .is_err());
    let mut specification = command(":");
    specification.executable = "/missing-botwork-worker".into();
    let report = wait(
        pool.start(specification, vec![], OperationControl::default())
            .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert_eq!(report.cleanup, WorkerCleanup::NotStarted);
    assert_eq!(
        report.diagnostic.unwrap().code(),
        DiagnosticCode::AsyncRuntime
    );
    let report = wait(
        pool.start(command(":"), vec![], OperationControl::default())
            .unwrap(),
    );
    let snapshot = pool.snapshot();
    assert_eq!(snapshot.completed.len(), 1);
    assert_eq!(snapshot.completed[0].id, report.id);
    assert_eq!(snapshot.omitted_records, 1);
    assert!(snapshot.active.is_empty());
}

#[test]
fn simultaneous_pipe_backpressure_is_drained_without_deadlock() {
    let pool = WorkerPool::new(limits()).unwrap();
    let input = vec![b'x'; 256 * 1024];
    let report = wait(
        pool.start(
            command("/usr/bin/tee /dev/stderr"),
            input.clone(),
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Succeeded);
    assert_eq!(report.stdout, input);
    assert_eq!(report.stderr, input);
    assert!(report.io_complete);
}

#[test]
fn inherited_deadlines_and_explicit_parent_cancellation_stop_only_their_workers() {
    let pool = WorkerPool::new(limits()).unwrap();
    let control = OperationControl::default().child(Some(
        tokio::time::Instant::now() + Duration::from_millis(60),
    ));
    let handle = pool
        .start(command("while :; do :; done"), vec![], control)
        .unwrap();
    let sibling = pool
        .start(
            command("printf sibling"),
            vec![],
            OperationControl::default(),
        )
        .unwrap();
    assert_eq!(wait(handle).outcome, WorkerOutcome::TimedOut);
    assert_eq!(wait(sibling).outcome, WorkerOutcome::Succeeded);
    let parent = OperationControl::default();
    let handle = pool
        .start(command("while :; do :; done"), vec![], parent.clone())
        .unwrap();
    parent.cancel();
    assert_eq!(wait(handle).outcome, WorkerOutcome::Cancelled);
}

#[test]
fn last_pool_owner_requests_shutdown_while_other_clones_keep_workers_alive() {
    let pool = WorkerPool::new(limits()).unwrap();
    let clone = pool.clone();
    let handle = pool
        .start(
            command("while :; do :; done"),
            vec![],
            OperationControl::default(),
        )
        .unwrap();
    drop(pool);
    until(|| {
        clone
            .snapshot()
            .active
            .first()
            .is_some_and(|active| active.pid.is_some())
    });
    assert!(!clone.snapshot().closed);
    drop(clone);
    let report = wait(handle);
    assert_eq!(report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
}

#[test]
fn bounded_shutdown_wait_reaps_workers_without_consuming_their_reports() {
    let pool = WorkerPool::new(limits()).unwrap();
    let parent = OperationControl::default();
    let mut handles = Vec::new();
    for _ in 0..3 {
        handles.push(
            pool.start(command("while :; do :; done"), vec![], parent.clone())
                .unwrap(),
        );
    }
    until(|| {
        pool.snapshot()
            .active
            .iter()
            .all(|worker| worker.pid.is_some())
    });
    let pids: Vec<_> = pool
        .snapshot()
        .active
        .iter()
        .map(|worker| worker.pid.unwrap())
        .collect();
    let start = Instant::now();
    let snapshot = pool.shutdown_wait(Duration::from_secs(2)).unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(snapshot.closed);
    assert!(snapshot.active.is_empty());
    assert_eq!(snapshot.completed.len(), 3);
    assert!(!parent.is_cancelled());
    for handle in handles {
        let report = wait(handle);
        assert_eq!(report.outcome, WorkerOutcome::Cancelled);
        assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    }
    for pid in pids {
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    }
    assert!(pool.start(command(":"), vec![], parent).is_err());
}

#[test]
fn shutdown_wait_validates_duration_before_closing_and_accepts_zero() {
    let pool = WorkerPool::new(limits()).unwrap();
    let error = pool.shutdown_wait(Duration::MAX).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    assert!(!pool.snapshot().closed);
    assert_eq!(
        wait(
            pool.start(command(":"), vec![], OperationControl::default())
                .unwrap()
        )
        .outcome,
        WorkerOutcome::Succeeded
    );
    let snapshot = pool.shutdown_wait(Duration::ZERO).unwrap();
    assert!(snapshot.closed);
    assert!(snapshot.active.is_empty());
    assert_eq!(
        pool.shutdown_wait(Duration::ZERO).unwrap().completed.len(),
        1
    );
}

#[test]
fn all_shutdown_waiters_wake_when_zero_history_pool_finishes_cleanup() {
    let pool = WorkerPool::new(WorkerLimits {
        history_records: 0,
        ..limits()
    })
    .unwrap();
    let handle = pool
        .start(
            command("while :; do :; done"),
            vec![],
            OperationControl::default(),
        )
        .unwrap();
    until(|| pool.snapshot().active[0].pid.is_some());
    let waiters: Vec<_> = (0..3)
        .map(|_| {
            let pool = pool.clone();
            std::thread::spawn(move || pool.shutdown_wait(Duration::from_secs(2)).unwrap())
        })
        .collect();
    for waiter in waiters {
        let snapshot = waiter.join().unwrap();
        assert!(snapshot.closed);
        assert!(snapshot.active.is_empty());
        assert!(snapshot.completed.is_empty());
        assert_eq!(snapshot.omitted_records, 1);
    }
    assert_eq!(wait(handle).outcome, WorkerOutcome::Cancelled);
}

#[test]
fn zero_cleanup_allowance_keeps_terminal_identity_until_eventual_reaping() {
    let pool = WorkerPool::new(WorkerLimits {
        cleanup_timeout: Duration::ZERO,
        max_in_flight: NonZeroUsize::new(1).unwrap(),
        ..limits()
    })
    .unwrap();
    for _ in 0..16 {
        let handle = pool
            .start(
                command("while :; do :; done"),
                vec![],
                OperationControl::default(),
            )
            .unwrap();
        let id = handle.id();
        until(|| {
            pool.snapshot()
                .active
                .first()
                .is_some_and(|active| active.pid.is_some())
        });
        handle.cancel();
        let report = wait(handle);
        assert_eq!(report.outcome, WorkerOutcome::Cancelled);
        assert!(matches!(
            report.cleanup,
            WorkerCleanup::Pending | WorkerCleanup::Reaped
        ));
        until(|| pool.snapshot().active.is_empty());
        let snapshot = pool.snapshot();
        let record = snapshot
            .completed
            .iter()
            .find(|record| record.id == id)
            .unwrap();
        assert_eq!(record.outcome, WorkerOutcome::Cancelled);
        assert_eq!(record.cleanup, WorkerCleanup::Reaped);
    }
}
