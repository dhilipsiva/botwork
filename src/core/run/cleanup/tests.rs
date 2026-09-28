use super::*;

#[test]
fn cleanup_shares_live_ownership_and_cumulative_work_but_not_stop_or_steps() {
    let control = OperationControl::default();
    let mut limits = RunLimits {
        steps: 1,
        ..RunLimits::default()
    };
    limits.output.total_bytes = 2;
    limits.imports.loads = 2;
    limits.snapshots.entries = 2;
    let normal = RunBudget::new(limits, control.clone());
    let guard = normal.enter().unwrap();
    normal.tick().unwrap();
    normal.charge_output(1).unwrap();
    normal
        .charge_imports(&[(ImportResource::Loads, 1)])
        .unwrap();
    let mut snapshot = SnapshotSize::default();
    snapshot.entries(1);
    normal.charge_snapshot(snapshot).unwrap();
    let cleanup = normal.for_cleanup().unwrap();
    control.cancel();
    assert!(normal.checkpoint().is_err());
    cleanup.checkpoint().unwrap();
    cleanup.tick().unwrap();
    assert_eq!(normal.used(), 2);
    assert_eq!(normal.0.used.load(Ordering::Relaxed), 1);
    assert_eq!(cleanup.0.active.load(Ordering::Relaxed), 1);
    for shared in [
        Arc::ptr_eq(&normal.0.retained_values, &cleanup.0.retained_values),
        Arc::ptr_eq(&normal.0.retained_names, &cleanup.0.retained_names),
        Arc::ptr_eq(
            &normal.0.retained_definitions,
            &cleanup.0.retained_definitions,
        ),
        Arc::ptr_eq(&normal.0.retained_registry, &cleanup.0.retained_registry),
        Arc::ptr_eq(&normal.0.temporary_values, &cleanup.0.temporary_values),
        Arc::ptr_eq(
            &normal.0.retained_diagnostics,
            &cleanup.0.retained_diagnostics,
        ),
    ] {
        assert!(shared);
    }
    cleanup.charge_output(1).unwrap();
    cleanup
        .charge_imports(&[(ImportResource::Loads, 1)])
        .unwrap();
    cleanup.charge_snapshot(snapshot).unwrap();
    assert_eq!(normal.0.output.load(Ordering::Relaxed), 2);
    assert_eq!(normal.import_remaining(ImportResource::Loads), 0);
    assert_eq!(*normal.0.snapshots.lock().unwrap(), [2, 0]);
    let nested = cleanup.for_cleanup().unwrap();
    assert!(Arc::ptr_eq(&nested.0, &cleanup.0));
    assert!(nested.charge_output(1).is_err());
    assert!(cleanup.checkpoint().is_err());
    let outer = normal.for_cleanup().unwrap();
    outer.checkpoint().unwrap();
    assert!(outer.charge_imports(&[(ImportResource::Loads, 1)]).is_err());
    drop(guard);
    assert_eq!(normal.0.active.load(Ordering::Relaxed), 0);
}

#[test]
fn context_clones_copy_cleanup_work_without_sharing_cumulative_output_counters() {
    let original = RunBudget::new(RunLimits::default(), OperationControl::default());
    original.tick().unwrap();
    original.for_cleanup().unwrap().tick().unwrap();
    let copy = original.clone();
    assert_eq!(copy.used(), 2);
    copy.for_cleanup().unwrap().tick().unwrap();
    copy.charge_output(1).unwrap();
    assert_eq!(copy.used(), 3);
    assert_eq!(original.used(), 2);
    assert_eq!(original.0.output.load(Ordering::Relaxed), 0);
}

#[test]
fn cleanup_configuration_rejects_overflow_and_zero_allowances_stop_before_work() {
    let mut limits = RunLimits::default();
    limits.cleanup.timeout = Duration::MAX;
    assert!(matches!(
        limits.validate().unwrap_err().error.as_ref(),
        BWErr::RunConfiguration(_)
    ));
    limits.cleanup.timeout = Duration::ZERO;
    limits.validate().unwrap();
    let budget = RunBudget::new(limits, OperationControl::default());
    assert!(matches!(
        budget
            .for_cleanup()
            .unwrap()
            .checkpoint()
            .unwrap_err()
            .error
            .as_ref(),
        BWErr::Timeout(_)
    ));
    budget.checkpoint().unwrap();
}
