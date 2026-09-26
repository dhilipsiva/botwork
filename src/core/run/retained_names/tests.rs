use super::*;
use std::collections::HashMap;

#[test]
fn exact_zero_and_overflow_budgets_leave_failed_reservations_atomic() {
    let tracker = Arc::new(RetainedNames::new(RetainedNameLimits {
        names: 2,
        name_bytes: 4,
        total_bytes: 6,
    }));
    let first = tracker.reserve(2).unwrap();
    let second = tracker.reserve(4).unwrap();
    assert_eq!(*tracker.used.lock().unwrap(), [2, 6]);
    assert!(tracker.reserve(0).is_err());
    drop(second);
    for bytes in [5, usize::MAX] {
        assert!(tracker.reserve(bytes).is_err());
    }
    assert_eq!(*tracker.used.lock().unwrap(), [1, 2]);
    drop(first);
    assert_eq!(*tracker.used.lock().unwrap(), [0, 0]);
    let zero = Arc::new(RetainedNames::new(RetainedNameLimits {
        names: 0,
        name_bytes: 0,
        total_bytes: 0,
    }));
    assert!(zero.reserve(0).is_err());
    let tracker = Arc::new(RetainedNames::new(RetainedNameLimits {
        names: usize::MAX,
        name_bytes: usize::MAX,
        total_bytes: usize::MAX,
    }));
    for used in [[usize::MAX, 0], [0, usize::MAX]] {
        *tracker.used.lock().unwrap() = used;
        assert!(tracker.reserve(1).is_err());
        assert_eq!(*tracker.used.lock().unwrap(), used);
    }
}

#[test]
fn names_share_storage_and_reservations_and_keep_exact_hash_lookup() {
    let budget = RunBudget::new(
        RunLimits {
            retained_names: RetainedNameLimits {
                names: 2,
                name_bytes: 4,
                total_bytes: 6,
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    );
    let name = budget.retain_name("é").unwrap();
    let alias = name.clone();
    assert!(Arc::ptr_eq(&name.0, &alias.0));
    let other = budget.retain_name("🙂").unwrap();
    let weak = Arc::downgrade(&name.0);
    // This test pins immutable hash identity while reservation counters change.
    #[allow(clippy::mutable_key_type)]
    let mut map = HashMap::from([(name, 1), (other, 2)]);
    assert_eq!(map.get("é"), Some(&1));
    assert_eq!(map.get("🙂"), Some(&2));
    assert_eq!(map.get("e\u{301}"), None);
    assert_eq!(alias, RetainedName::untracked("é"));
    map.remove("é");
    assert!(weak.upgrade().is_some());
    assert_eq!(*budget.0.retained_names.used.lock().unwrap(), [2, 6]);
    drop(alias);
    assert!(weak.upgrade().is_none());
    assert_eq!(*budget.0.retained_names.used.lock().unwrap(), [1, 4]);
    assert_eq!(map.get("🙂"), Some(&2));
    drop(map);
    assert_eq!(*budget.0.retained_names.used.lock().unwrap(), [0, 0]);
}

#[test]
fn aggregate_name_bytes_and_clone_failures_release_cleanly() {
    let budget = RunBudget::new(
        RunLimits {
            retained_names: RetainedNameLimits {
                names: 3,
                name_bytes: 4,
                total_bytes: 4,
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    );
    let held = budget.retain_name("abc").unwrap();
    let clone = budget.clone();
    assert!(clone
        .retain_name("de")
        .err()
        .unwrap()
        .to_string()
        .contains("retained variable name bytes"));
    assert_eq!(*budget.0.retained_names.used.lock().unwrap(), [1, 3]);
    assert!(budget.checkpoint().is_ok());
    drop(held);
    assert!(budget.retain_name("long").is_ok());
    let weak = Arc::downgrade(&budget.0.retained_names);
    drop(budget);
    drop(clone);
    assert!(weak.upgrade().is_none());
}
