use super::*;
use crate::core::diagnostic::DiagnosticCode;

fn budget(entries: usize, path_bytes: usize) -> RunBudget {
    RunBudget::new(
        RunLimits {
            snapshots: SnapshotLimits {
                entries,
                path_bytes,
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    )
}

fn size(entries: usize, path_bytes: usize) -> SnapshotSize {
    let mut size = SnapshotSize::default();
    size.entries(entries);
    size.path_bytes(path_bytes);
    size
}

#[test]
fn combined_admission_is_atomic_and_latches_failures() {
    let budget = budget(5, 8);
    budget.charge_snapshot(size(2, 5)).unwrap();
    let error = budget.charge_snapshot(size(3, 4)).unwrap_err();
    assert!(error.to_string().contains("snapshot path bytes"));
    assert_eq!(*budget.0.snapshots.lock().unwrap(), [2, 5]);
    assert_eq!(
        budget.charge_snapshot(size(0, 0)).unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn checked_measurement_and_accumulation_reject_overflow() {
    for paths in [false, true] {
        let budget = budget(usize::MAX, usize::MAX);
        let mut amount = size(usize::MAX, usize::MAX);
        if paths {
            amount.path_bytes(1);
        } else {
            amount.entries(1);
        }
        assert!(budget.charge_snapshot(amount).is_err());
        assert_eq!(*budget.0.snapshots.lock().unwrap(), [0, 0]);
        let budget = self::budget(usize::MAX, usize::MAX);
        budget
            .charge_snapshot(size(usize::MAX, usize::MAX))
            .unwrap();
        assert!(budget
            .charge_snapshot(if paths { size(0, 1) } else { size(1, 0) })
            .is_err());
    }
    let budget = budget(0, 0);
    budget.charge_snapshot(SnapshotSize::default()).unwrap();
}

#[test]
fn module_work_is_shared_but_host_clone_work_is_independent() {
    let budget = budget(8_000, 8_000);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let shared = budget.shared();
            scope.spawn(move || {
                for _ in 0..1_000 {
                    shared.charge_snapshot(size(1, 1)).unwrap();
                }
            });
        }
    });
    assert_eq!(*budget.0.snapshots.lock().unwrap(), [8_000, 8_000]);
    let clone = budget.clone();
    assert!(clone.charge_snapshot(size(1, 0)).is_err());
    budget.checkpoint().unwrap();
    assert_eq!(*budget.0.snapshots.lock().unwrap(), [8_000, 8_000]);
}
