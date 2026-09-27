use super::*;

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
