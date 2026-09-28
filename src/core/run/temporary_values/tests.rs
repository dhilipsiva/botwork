use super::*;

fn size(nodes: usize, payload_bytes: usize) -> ValueSize {
    ValueSize {
        nodes,
        depth: 1,
        payload_bytes,
    }
}

#[test]
fn streaming_growth_is_atomic_and_does_not_charge_an_extra_value_handle() {
    let owner = Arc::new(TemporaryValues::new(TemporaryLimits {
        values: 1,
        nodes: 3,
        payload_bytes: 8,
    }));
    let mut reservation = owner.reserve(size(1, 0)).unwrap();
    reservation.grow(2, 8).unwrap();
    assert_eq!(*owner.used.lock().unwrap(), [1, 3, 8]);
    for (nodes, bytes) in [(1, 0), (0, 1), (usize::MAX, usize::MAX)] {
        assert!(reservation.grow(nodes, bytes).is_err());
        assert_eq!(*owner.used.lock().unwrap(), [1, 3, 8]);
    }
    drop(reservation);
    assert_eq!(*owner.used.lock().unwrap(), [0; 3]);
}

#[test]
fn exact_admission_merge_and_release_preserve_all_counters() {
    let owner = Arc::new(TemporaryValues::new(TemporaryLimits {
        values: 2,
        nodes: 5,
        payload_bytes: 9,
    }));
    let mut container = owner.reserve(size(1, 1)).unwrap();
    let child = owner.reserve(size(4, 8)).unwrap();
    assert_eq!(*owner.used.lock().unwrap(), [2, 5, 9]);
    assert!(owner.reserve(size(0, 0)).is_err());
    container.absorb(child);
    assert_eq!(*owner.used.lock().unwrap(), [1, 5, 9]);
    container.release_child(size(4, 8));
    assert_eq!(*owner.used.lock().unwrap(), [1, 1, 1]);
    drop(container);
    assert_eq!(*owner.used.lock().unwrap(), [0; 3]);
}

#[test]
fn failures_never_partially_charge_and_maximum_counters_do_not_overflow() {
    let owner = Arc::new(TemporaryValues::new(TemporaryLimits {
        values: usize::MAX,
        nodes: usize::MAX,
        payload_bytes: usize::MAX,
    }));
    for index in 0..3 {
        let mut current = [0; 3];
        current[index] = usize::MAX;
        *owner.used.lock().unwrap() = current;
        assert!(owner.reserve(size(1, 1)).is_err());
        assert_eq!(*owner.used.lock().unwrap(), current);
    }
    let zero = Arc::new(TemporaryValues::new(TemporaryLimits {
        values: 0,
        nodes: 0,
        payload_bytes: 0,
    }));
    assert!(zero.reserve(size(0, 0)).is_err());
}

#[test]
fn public_transfer_releases_tracking_without_destroying_owned_payload() {
    let budget = RunBudget::new(RunLimits::default(), OperationControl::default());
    let owner = Arc::clone(&budget.0.temporary_values);
    let value = Literal::String("payload".into());
    let Literal::String(string) = &value else {
        panic!()
    };
    let pointer = string.as_ptr();
    let temporary = TemporaryValue::new(value, Some(budget.reserve_temporary(size(1, 7)).unwrap()));
    assert_eq!(*owner.used.lock().unwrap(), [1, 1, 7]);
    let Literal::String(value) = temporary.into_inner() else {
        panic!()
    };
    assert_eq!(value.as_ptr(), pointer);
    assert_eq!(*owner.used.lock().unwrap(), [0; 3]);
    let weak = Arc::downgrade(&owner);
    drop(owner);
    drop(budget);
    assert!(weak.upgrade().is_none());
}

#[test]
fn cloned_context_budgets_share_live_values_with_independent_stops() {
    let budget = RunBudget::new(
        RunLimits {
            temporaries: TemporaryLimits {
                values: 1,
                ..TemporaryLimits::default()
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    );
    let cloned = budget.clone();
    let held = budget.reserve_temporary(size(1, 1)).unwrap();
    assert!(cloned.reserve_temporary(size(1, 1)).is_err());
    budget.checkpoint().unwrap();
    drop(held);
    drop(budget.reserve_temporary(size(1, 1)).unwrap());
    assert!(cloned.checkpoint().is_err());
    assert_eq!(*budget.0.temporary_values.used.lock().unwrap(), [0; 3]);
}

#[test]
fn concurrent_admission_has_one_winner_and_releases_without_cycles() {
    let owner = Arc::new(TemporaryValues::new(TemporaryLimits {
        values: 1,
        nodes: 1,
        payload_bytes: 1,
    }));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let winners = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let owner = Arc::clone(&owner);
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    let result = owner.reserve(size(1, 1));
                    barrier.wait();
                    result.is_ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| usize::from(handle.join().unwrap()))
            .sum::<usize>()
    });
    assert_eq!(winners, 1);
    assert_eq!(*owner.used.lock().unwrap(), [0; 3]);
}

#[test]
fn argument_placeholders_refund_unfilled_slots_after_failure() {
    let owner = Arc::new(TemporaryValues::new(TemporaryLimits::default()));
    let mut slots = owner.reserve_counts([3, 3, 0]).unwrap();
    slots.release_argument_slot();
    let first = owner.reserve(size(5, 4)).unwrap();
    assert_eq!(*owner.used.lock().unwrap(), [3, 7, 4]);
    drop(slots);
    assert_eq!(*owner.used.lock().unwrap(), [1, 5, 4]);
    drop(first);
    assert_eq!(*owner.used.lock().unwrap(), [0; 3]);
}
