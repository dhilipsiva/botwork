use super::*;

#[cfg(test)]
mod tests;

/// Cooperative allowance for each Finally entered from ordinary execution.
/// Finally blocks entered during cleanup share the current allowance.
#[derive(Clone, Debug)]
pub struct CleanupLimits {
    pub steps: u64,
    pub timeout: Duration,
}

impl Default for CleanupLimits {
    fn default() -> Self {
        Self {
            steps: 10_000,
            timeout: Duration::from_secs(5),
        }
    }
}

impl CleanupLimits {
    pub(crate) fn validate(&self) -> DiagnosticResult<()> {
        self.deadline().map(|_| ())
    }

    fn deadline(&self) -> DiagnosticResult<tokio::time::Instant> {
        tokio::time::Instant::now()
            .checked_add(self.timeout)
            .ok_or_else(|| {
                Diagnostic::new(BWErr::RunConfiguration(
                    "Cleanup timeout exceeds the monotonic clock range".into(),
                ))
            })
    }
}

impl RunBudget {
    pub(crate) fn for_cleanup(&self) -> DiagnosticResult<Self> {
        if self.0.cleaning {
            // A cleanup loop cannot manufacture unlimited fresh allowances.
            return Ok(self.shared());
        }
        let control = OperationControl::default().child(Some(self.0.limits.cleanup.deadline()?));
        let mut limits = self.0.limits.clone();
        limits.steps = limits.cleanup.steps;
        Ok(Self(Arc::new(BudgetState {
            limits,
            control,
            used: Arc::new(AtomicU64::new(0)),
            active: AtomicUsize::new(self.0.active.load(Ordering::Relaxed)),
            output: Arc::clone(&self.0.output),
            imports: Arc::clone(&self.0.imports),
            snapshots: Arc::clone(&self.0.snapshots),
            cleanup_steps: Arc::clone(&self.0.cleanup_steps),
            cleaning: true,
            retained_values: Arc::clone(&self.0.retained_values),
            retained_definitions: Arc::clone(&self.0.retained_definitions),
            retained_names: Arc::clone(&self.0.retained_names),
            retained_registry: Arc::clone(&self.0.retained_registry),
            temporary_values: Arc::clone(&self.0.temporary_values),
            retained_diagnostics: Arc::clone(&self.0.retained_diagnostics),
            stopped: Mutex::new(None),
        })))
    }
}
