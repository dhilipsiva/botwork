//! Scoped budgets for Eventually/Retry attempts.
use super::*;

#[cfg(test)]
mod tests;

impl RunBudget {
    /// An attempt shares the run's step counter and live quotas, observes the
    /// parent's cancellation and the earlier of both deadlines, and latches its
    /// own stops. The polling owner decides which attempt stops propagate.
    pub(crate) fn for_attempt(&self, deadline: Option<tokio::time::Instant>) -> Self {
        Self(Arc::new(BudgetState {
            limits: self.0.limits.clone(),
            control: self.0.control.child(deadline),
            used: Arc::clone(&self.0.used),
            active: AtomicUsize::new(self.0.active.load(Ordering::Relaxed)),
            output: Arc::clone(&self.0.output),
            imports: Arc::clone(&self.0.imports),
            snapshots: Arc::clone(&self.0.snapshots),
            cleanup_steps: Arc::clone(&self.0.cleanup_steps),
            cleaning: self.0.cleaning,
            retained_values: Arc::clone(&self.0.retained_values),
            retained_definitions: Arc::clone(&self.0.retained_definitions),
            retained_names: Arc::clone(&self.0.retained_names),
            retained_registry: Arc::clone(&self.0.retained_registry),
            temporary_values: Arc::clone(&self.0.temporary_values),
            retained_diagnostics: Arc::clone(&self.0.retained_diagnostics),
            stopped: Mutex::new(None),
        }))
    }

    /// The stop already latched by this budget, without consulting its control.
    pub(crate) fn latched(&self) -> Option<BWErr> {
        self.0
            .stopped
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
