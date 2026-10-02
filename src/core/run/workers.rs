//! Settling a run's workers as it ends: the worker processes its statements
//! started and did not see finish cleanup. See docs/shutdown.md#run-end.
use super::*;
use crate::core::worker::{ActiveWorker, WorkerCleanup, WorkerLedger};

/// The most unresolved workers a run's failure names one by one.
const NAMED_WORKERS: usize = 8;

impl Context {
    /// The run's worker ledger and the cleanup allowance it waits within.
    fn worker_ledger(&self) -> Option<(Arc<WorkerLedger>, Duration)> {
        let environment = self.environment.as_ref()?;
        let allowance = self
            .budget
            .as_ref()
            .map_or(CleanupLimits::default().timeout, |budget| {
                budget.limits().cleanup.timeout
            });
        Some((Arc::clone(&environment.workers), allowance))
    }

    /// Wait, within the run's cleanup allowance, for the workers its
    /// statements started and did not see finish cleanup; the failure for any
    /// still active then. A run whose workers all ended returns at once.
    pub(crate) fn settle_workers_blocking(&self) -> Option<Diagnostic> {
        let (ledger, allowance) = self.worker_ledger()?;
        if !ledger.pending() {
            return None;
        }
        unsettled(&ledger.settle(allowance), allowance)
    }

    /// As [`Context::settle_workers_blocking`], waiting on a blocking thread.
    pub(crate) async fn settle_workers(&self) -> Option<Diagnostic> {
        let (ledger, allowance) = self.worker_ledger()?;
        if !ledger.pending() {
            return None;
        }
        let workers = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                let waiting = Arc::clone(&ledger);
                runtime
                    .spawn_blocking(move || waiting.settle(allowance))
                    .await
                    .unwrap_or_else(|_| ledger.settle(Duration::ZERO))
            }
            Err(_) => ledger.settle(allowance),
        };
        unsettled(&workers, allowance)
    }
}

/// Workers left unresolved fail a run that otherwise succeeded, and are a
/// cause of one that failed, as a failed cleanup is.
pub(crate) fn with_unsettled<T>(
    result: EvaluationResult<T>,
    unsettled: Option<Diagnostic>,
    budget: Option<&RunBudget>,
) -> EvaluationResult<T> {
    let Some(unsettled) = unsettled else {
        return result;
    };
    let unsettled = RuntimeDiagnostic::constructed(unsettled, None);
    match result {
        Ok(_) => Err(unsettled),
        Err(primary) => Err(primary.with_cleanup(unsettled, budget)),
    }
}

/// The failure for workers still active when the run ended.
fn unsettled(workers: &[ActiveWorker], allowance: Duration) -> Option<Diagnostic> {
    use std::fmt::Write;
    if workers.is_empty() {
        return None;
    }
    let mut text = format!(
        "The run ended with {} worker process{} not cleaned up within its {} ms cleanup allowance:",
        workers.len(),
        if workers.len() == 1 { "" } else { "es" },
        allowance.as_millis()
    );
    for (index, worker) in workers.iter().take(NAMED_WORKERS).enumerate() {
        let state = match worker.cleanup {
            Some(WorkerCleanup::Pending) => "cleanup pending",
            Some(WorkerCleanup::Unverified) => "ownership lost",
            _ if worker.stopping => "stopping",
            _ => "running",
        };
        let separator = if index == 0 { " " } else { "; " };
        let _ = match worker.pid {
            Some(pid) => write!(
                text,
                "{separator}worker {} (process {pid}): {state}",
                worker.id
            ),
            None => write!(text, "{separator}worker {}: {state}", worker.id),
        };
    }
    if workers.len() > NAMED_WORKERS {
        let _ = write!(text, "; and {} more", workers.len() - NAMED_WORKERS);
    }
    Some(Diagnostic::new(BWErr::AsyncRuntime(text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(
        id: u64,
        pid: Option<u32>,
        stopping: bool,
        cleanup: Option<WorkerCleanup>,
    ) -> ActiveWorker {
        ActiveWorker {
            id,
            pid,
            stopping,
            cleanup,
        }
    }

    #[test]
    fn the_failure_names_each_state_and_at_most_eight_workers() {
        assert!(unsettled(&[], Duration::from_secs(5)).is_none());
        let mut workers = vec![
            worker(1, Some(10), false, None),
            worker(2, Some(20), true, None),
            worker(3, None, true, Some(WorkerCleanup::Pending)),
            worker(4, Some(40), true, Some(WorkerCleanup::Unverified)),
        ];
        let error = unsettled(&workers, Duration::from_millis(1500)).unwrap();
        assert_eq!(
            error.code(),
            crate::core::diagnostic::DiagnosticCode::AsyncRuntime
        );
        assert_eq!(
            error.error.to_string(),
            "Async runtime failure: The run ended with 4 worker processes not cleaned up within its 1500 ms cleanup allowance: worker 1 (process 10): running; worker 2 (process 20): stopping; worker 3: cleanup pending; worker 4 (process 40): ownership lost"
        );
        workers.extend((5..=10).map(|id| worker(id, None, false, None)));
        let text = unsettled(&workers, Duration::ZERO)
            .unwrap()
            .error
            .to_string();
        assert!(
            text.contains("The run ended with 10 worker processes"),
            "{text}"
        );
        assert!(text.contains("worker 8: running; and 2 more"), "{text}");
        assert!(!text.contains("worker 9"), "{text}");
    }
}
