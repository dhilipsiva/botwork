use super::*;

#[test]
fn dropping_a_handle_during_stop_observation_preserves_interruption() {
    let request = Arc::new(Request {
        #[cfg(target_os = "linux")]
        journal: None,
        control: OperationControl::default(),
        abandoned: AtomicBool::new(false),
        limits: WorkerLimits::default(),
    });
    let handle = WorkerHandle {
        id: 1,
        request: request.clone(),
        receive: None,
    };
    // Schedule real Drop immediately before cancellation is sampled, rather
    // than relying on a thread winning the small gap between two reads.
    let (outcome, error) = observe_stop(&request, Instant::now() + Duration::from_secs(1), || {
        drop(handle);
        request.control.checkpoint()
    })
    .unwrap();
    assert_eq!(outcome, WorkerOutcome::Interrupted);
    assert!(error.error.to_string().contains("abandoned"));

    let request = Request {
        #[cfg(target_os = "linux")]
        journal: None,
        control: OperationControl::default(),
        abandoned: AtomicBool::new(false),
        limits: WorkerLimits::default(),
    };
    let later = Instant::now() + Duration::from_secs(1);
    assert!(stop(&request, later).is_none());
    assert_eq!(
        stop(&request, Instant::now()).unwrap().0,
        WorkerOutcome::TimedOut
    );
    request.control.cancel();
    assert_eq!(stop(&request, later).unwrap().0, WorkerOutcome::Cancelled);
}

#[test]
fn cleanup_errors_after_pending_handoff_preserve_the_published_terminal_outcome() {
    for outcome in [
        WorkerOutcome::TimedOut,
        WorkerOutcome::Cancelled,
        WorkerOutcome::Interrupted,
        WorkerOutcome::Failed,
    ] {
        let mut report = WorkerReport::failure(
            1,
            outcome,
            WorkerCleanup::Pending,
            runtime(format_args!("original")),
        );
        let original = report.diagnostic.take(); // Already transferred in the Pending report.
        append_cleanup(&mut report, io::Error::from_raw_os_error(libc::ECHILD));
        assert_eq!(report.outcome, outcome);
        assert!(report.diagnostic.is_some());
        assert_eq!(
            original.unwrap().error.to_string(),
            "Async runtime failure: original"
        );
    }
}

/// Whether no process has this ID: signal 0 reaches even a zombie.
pub(super) fn gone(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}
