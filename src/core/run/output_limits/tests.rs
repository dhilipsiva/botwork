use super::*;

#[test]
fn aggregate_charging_is_checked_atomic_and_shared_across_module_budgets() {
    let budget = RunBudget::new(
        RunLimits {
            output: OutputLimits {
                record_bytes: usize::MAX,
                total_bytes: usize::MAX,
            },
            ..Default::default()
        },
        OperationControl::default(),
    );
    budget.charge_output(usize::MAX - 1).unwrap();
    budget.charge_output(1).unwrap();
    assert!(budget.charge_output(1).is_err());
    assert_eq!(budget.0.output.load(Ordering::Relaxed), usize::MAX);

    let budget = RunBudget::new(
        RunLimits {
            output: OutputLimits {
                record_bytes: 6,
                total_bytes: 6,
            },
            ..Default::default()
        },
        OperationControl::default(),
    );
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let shared = budget.shared();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            shared.charge_output(5).is_ok()
        }));
    }
    assert_eq!(
        workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>(),
        1
    );
    assert_eq!(budget.0.output.load(Ordering::Relaxed), 5);
    assert!(budget.checkpoint().is_err());
}
