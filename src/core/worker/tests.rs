use super::*;

fn command() -> WorkerCommand {
    WorkerCommand {
        executable: "/missing-botwork-test-worker".into(),
        arguments: vec![],
        directory: std::env::temp_dir(),
        environment: Default::default(),
    }
}

#[test]
fn unverified_cleanup_stays_visible_and_keeps_capacity_even_without_history() {
    let pool = WorkerPool::new(WorkerLimits {
        max_in_flight: NonZeroUsize::new(1).unwrap(),
        history_records: 0,
        ..Default::default()
    })
    .unwrap();
    let request = Arc::new(Request {
        journal: None,
        control: OperationControl::default(),
        abandoned: AtomicBool::new(false),
    });
    let retained = Arc::new(());
    let weak = Arc::downgrade(&retained);
    // Inject a lost-ownership terminal transition without stealing a real child
    // from another test or changing the host's process-wide SIGCHLD disposition.
    pool.0 .0.state.lock().unwrap().active.insert(
        7,
        Active {
            _retention: Some(retained),
            request,
            pid: None,
            stopping: false,
            cleanup: None,
        },
    );
    pool.0 .0.finish(&WorkerReport::failure(
        7,
        WorkerOutcome::Interrupted,
        WorkerCleanup::Unverified,
        runtime(format_args!("Child ownership was lost")),
    ));
    let snapshot = pool.snapshot();
    assert!(snapshot.completed.is_empty());
    assert_eq!(snapshot.omitted_records, 1);
    assert_eq!(snapshot.active.len(), 1);
    assert_eq!(snapshot.active[0].id, 7);
    assert!(snapshot.active[0].stopping);
    assert_eq!(snapshot.active[0].cleanup, Some(WorkerCleanup::Unverified));
    let error = pool
        .start(command(), vec![], OperationControl::default())
        .err()
        .unwrap();
    assert!(matches!(
        error.error.as_ref(),
        BWErr::ResourceLimit {
            resource: "isolated workers",
            limit: 1
        }
    ));
    assert_eq!(pool.shutdown().active.len(), 1);
    assert!(weak.upgrade().is_some());
    drop(pool);
    assert!(weak.upgrade().is_none());
}

#[test]
fn exhausted_worker_ids_fail_before_entry_without_reusing_old_identity() {
    let pool = WorkerPool::new(WorkerLimits::default()).unwrap();
    pool.0 .0.state.lock().unwrap().next_id = u64::MAX;
    let error = pool
        .start(command(), vec![], OperationControl::default())
        .err()
        .unwrap();
    assert!(
        matches!(error.error.as_ref(), BWErr::RunConfiguration(reason) if reason.contains("identifier space"))
    );
    assert!(pool.snapshot().active.is_empty());
    assert!(pool.snapshot().completed.is_empty());
}
