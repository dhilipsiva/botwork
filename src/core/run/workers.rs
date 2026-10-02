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
