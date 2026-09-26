use super::*;

fn size(nodes: usize, payload_bytes: usize) -> ValueSize {
    ValueSize {
        nodes,
        depth: 1,
        payload_bytes,
    }
}

#[test]
fn reservations_are_atomic_exact_and_released_on_last_owner() {
    let owner = Arc::new(RetainedValues::new(RetainedValueLimits {
        values: 2,
        nodes: 3,
        payload_bytes: 5,
    }));
    let first = Arc::new(StoredValue::new(
        Literal::None,
        Some(owner.reserve(size(1, 2)).unwrap()),
    ));
    let alias = first.clone();
    let second = owner.reserve(size(2, 3)).unwrap();
    assert_eq!(*owner.used.lock().unwrap(), [2, 3, 5]);
    assert!(owner.reserve(size(0, 0)).is_err());
    drop(first);
    assert_eq!(*owner.used.lock().unwrap(), [2, 3, 5]);
    drop(second);
    assert_eq!(*owner.used.lock().unwrap(), [1, 1, 2]);
    for rejected in [size(3, 0), size(1, 4)] {
        assert!(owner.reserve(rejected).is_err());
        assert_eq!(*owner.used.lock().unwrap(), [1, 1, 2]);
    }
    drop(alias);
    assert_eq!(*owner.used.lock().unwrap(), [0; 3]);
    assert!(owner.reserve(size(3, 5)).is_ok());
}

#[test]
fn every_counter_rejects_overflow_without_partial_reservation() {
    let owner = Arc::new(RetainedValues::new(RetainedValueLimits {
        values: usize::MAX,
        nodes: usize::MAX,
        payload_bytes: usize::MAX,
    }));
    for index in 0..3 {
        let mut used = [0; 3];
        used[index] = usize::MAX;
        *owner.used.lock().unwrap() = used;
        assert!(owner.reserve(size(1, 1)).is_err());
        assert_eq!(*owner.used.lock().unwrap(), used);
    }
}

#[test]
fn cloned_budgets_share_storage_but_keep_stop_latches_separate() {
    let budget = RunBudget::new(
        RunLimits {
            retained_values: RetainedValueLimits {
                values: 1,
                ..RetainedValueLimits::default()
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    );
    let clone = budget.clone();
    let held = budget.reserve_value(size(1, 0)).unwrap();
    assert!(clone.reserve_value(size(1, 0)).is_err());
    assert!(budget.checkpoint().is_ok());
    drop(held);
    assert!(budget.reserve_value(size(1, 0)).is_ok());
    assert!(clone.checkpoint().is_err());
    let weak = Arc::downgrade(&budget.0.retained_values);
    drop(budget);
    drop(clone);
    assert!(weak.upgrade().is_none());
}

#[test]
fn concurrent_reservations_never_exceed_shared_capacity() {
    let owner = Arc::new(RetainedValues::new(RetainedValueLimits {
        values: 1,
        nodes: 1,
        payload_bytes: 1,
    }));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let successes = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let owner = owner.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let reservation = owner.reserve(size(1, 1));
                    barrier.wait();
                    reservation.is_ok()
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|success| *success)
            .count()
    });
    assert_eq!(successes, 1);
    assert_eq!(*owner.used.lock().unwrap(), [0; 3]);
}
