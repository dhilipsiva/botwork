use super::*;

#[test]
fn concurrent_admission_has_one_winner_and_refunds_the_last_owner() {
    use std::sync::Barrier;
    let budget = OperationBudget::new(OperationOwnershipLimits {
        invocations: 1,
        ..OperationOwnershipLimits::default()
    });
    let ready = Barrier::new(9);
    let admitted = Barrier::new(9);
    let release = Barrier::new(9);
    std::thread::scope(|scope| {
        let mut threads = vec![];
        for _ in 0..8 {
            threads.push(scope.spawn(|| {
                ready.wait();
                let reservation = budget.reserve([1, 0, 0, 0, 0, 0, 0, 0, 0]);
                admitted.wait();
                release.wait();
                reservation.is_ok()
            }));
        }
        ready.wait();
        admitted.wait();
        assert_eq!(budget.usage().invocations, 1);
        release.wait();
        assert_eq!(
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .filter(|won| *won)
                .count(),
            1
        );
    });
    assert_eq!(budget.usage(), OperationUsage::default());
}

#[test]
fn reservations_are_atomic_at_every_exact_boundary_and_release_after_merge() {
    let budget = OperationBudget::new(OperationOwnershipLimits {
        invocations: 1,
        values: 1,
        nodes: 1,
        payload_bytes: 1,
        diagnostics: 1,
        call_frames: 1,
        related_locations: 1,
        text_bytes: 1,
        source_bytes: 1,
    });
    let first = budget.reserve([1; 9]).unwrap();
    for dimension in 0..9 {
        let mut charge = [0; 9];
        charge[dimension] = 1;
        assert!(budget.reserve(charge).is_err());
        assert_eq!(*budget.0.used.lock().unwrap(), [1; 9]);
    }
    drop(first);
    assert_eq!(budget.usage(), OperationUsage::default());
    let first = budget.reserve([1, 1, 1, 1, 0, 0, 0, 0, 0]).unwrap();
    let second = budget.reserve([0, 0, 0, 0, 1, 1, 1, 1, 1]).unwrap();
    let merged = Reservation::merge(Some(first), Some(second)).unwrap();
    assert_eq!(*budget.0.used.lock().unwrap(), [1; 9]);
    drop(merged);
    assert_eq!(budget.usage(), OperationUsage::default());
}

#[test]
fn checked_counters_reject_overflow_without_partial_charges() {
    let budget = OperationBudget::new(OperationOwnershipLimits {
        invocations: usize::MAX,
        values: usize::MAX,
        nodes: usize::MAX,
        payload_bytes: usize::MAX,
        diagnostics: usize::MAX,
        call_frames: usize::MAX,
        related_locations: usize::MAX,
        text_bytes: usize::MAX,
        source_bytes: usize::MAX,
    });
    let first = budget.reserve([usize::MAX; 9]).unwrap();
    for dimension in 0..9 {
        let mut charge = [0; 9];
        charge[dimension] = 1;
        assert!(budget.reserve(charge).is_err());
        assert_eq!(*budget.0.used.lock().unwrap(), [usize::MAX; 9]);
    }
    drop(first);
    assert_eq!(budget.usage(), OperationUsage::default());
}

#[test]
fn diagnostic_replacement_preserves_old_charge_on_failure_then_releases() {
    let budget = OperationBudget::new(OperationOwnershipLimits {
        text_bytes: 4,
        ..OperationOwnershipLimits::default()
    });
    let first = budget
        .diagnostic(
            DiagnosticSize {
                diagnostics: 1,
                text_bytes: 4,
                ..DiagnosticSize::default()
            },
            None,
        )
        .unwrap();
    let (_, first) = *budget
        .diagnostic(
            DiagnosticSize {
                text_bytes: 5,
                ..DiagnosticSize::default()
            },
            Some(first),
        )
        .unwrap_err();
    assert_eq!(budget.usage().text_bytes, 4);
    assert_eq!(budget.usage().diagnostics, 1);
    let first = budget
        .diagnostic(
            DiagnosticSize {
                diagnostics: 2,
                text_bytes: 1,
                ..DiagnosticSize::default()
            },
            first,
        )
        .unwrap();
    assert_eq!(budget.usage().text_bytes, 1);
    assert_eq!(budget.usage().diagnostics, 2);
    drop(first);
    assert_eq!(budget.usage(), OperationUsage::default());
}
