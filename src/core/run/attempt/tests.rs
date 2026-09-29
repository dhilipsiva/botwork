use super::*;

#[test]
fn attempts_share_steps_and_quotas_but_latch_stops_locally() {
    let limits = RunLimits {
        steps: 2,
        ..RunLimits::default()
    };
    let run = RunBudget::new(limits, OperationControl::default());
    let guard = run.enter().unwrap();
    run.tick().unwrap();
    let attempt = run.for_attempt(None);
    assert_eq!(attempt.0.active.load(Ordering::Relaxed), 1);
    attempt.tick().unwrap();
    assert_eq!(run.used(), 2, "attempt steps are charged to the run");
    let exhausted = attempt.tick().unwrap_err();
    assert_eq!(exhausted.code(), DiagnosticCode::ResourceLimit);
    assert!(matches!(
        attempt.latched(),
        Some(BWErr::ResourceLimit { .. })
    ));
    run.checkpoint().unwrap();
    assert!(run.latched().is_none(), "the owner decides propagation");
    assert!(run.tick().is_err(), "the shared counter remains exhausted");
    for shared in [
        Arc::ptr_eq(&run.0.used, &attempt.0.used),
        Arc::ptr_eq(&run.0.output, &attempt.0.output),
        Arc::ptr_eq(&run.0.retained_values, &attempt.0.retained_values),
        Arc::ptr_eq(&run.0.temporary_values, &attempt.0.temporary_values),
        Arc::ptr_eq(&run.0.retained_diagnostics, &attempt.0.retained_diagnostics),
    ] {
        assert!(shared);
    }
    drop(guard);
}

#[tokio::test(start_paused = true)]
async fn attempt_deadlines_are_local_while_parent_stops_propagate() {
    let control = OperationControl::default();
    let run = RunBudget::new(RunLimits::default(), control.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_millis(10);
    let attempt = run.for_attempt(Some(deadline));
    attempt.checkpoint().unwrap();
    tokio::time::advance(Duration::from_millis(10)).await;
    assert_eq!(
        attempt.checkpoint().unwrap_err().code(),
        DiagnosticCode::Timeout
    );
    assert!(matches!(attempt.latched(), Some(BWErr::Timeout(_))));
    run.checkpoint().unwrap();

    let unbounded = run.for_attempt(None);
    unbounded.checkpoint().unwrap();
    control.cancel();
    assert_eq!(
        unbounded.checkpoint().unwrap_err().code(),
        DiagnosticCode::Cancelled
    );
    assert!(run.checkpoint().is_err());
}

#[test]
fn nested_attempts_share_the_same_counter_and_clones_do_not() {
    let run = RunBudget::new(RunLimits::default(), OperationControl::default());
    let nested = run.for_attempt(None).for_attempt(None);
    nested.tick().unwrap();
    assert_eq!(run.used(), 1);
    let copy = run.clone();
    copy.tick().unwrap();
    assert_eq!((run.used(), copy.used()), (1, 2));
    let cleanup = run.for_cleanup().unwrap();
    assert!(!Arc::ptr_eq(&run.0.used, &cleanup.0.used));
}
