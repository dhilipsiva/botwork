use super::*;

pub(super) struct CleanupContext<'a> {
    pub(super) context: &'a mut Context,
    budget: Option<RunBudget>,
    environment: Option<Arc<RunEnvironment>>,
}

impl<'a> CleanupContext<'a> {
    pub(super) fn enter(context: &'a mut Context) -> EvaluationResult<Self> {
        let budget = context
            .budget
            .as_ref()
            .map(RunBudget::for_cleanup)
            .transpose()?;
        let environment = context.environment.as_ref().map(|environment| {
            Arc::new(
                environment.with_control(
                    budget
                        .as_ref()
                        .map(|budget| budget.control().clone())
                        .unwrap_or_default(),
                ),
            )
        });
        let budget = std::mem::replace(&mut context.budget, budget);
        let environment = std::mem::replace(&mut context.environment, environment);
        Ok(Self {
            context,
            budget,
            environment,
        })
    }
}

impl Drop for CleanupContext<'_> {
    fn drop(&mut self) {
        self.context.budget = self.budget.take();
        self.context.environment = self.environment.take();
    }
}

// Keep phase state out of every recursive evaluator poll frame, including
// statements that do not use cleanup.
pub(super) fn evaluate<'a>(
    body: &'a Block,
    cleanup: &'a Block,
    context: &'a mut Context,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = CompletionResult> + Send + 'a>> {
    Box::pin(async move {
        let primary = execution::evaluate_block(body, context).await;
        // Observe the body's stop before installing independent cleanup control.
        let primary = context
            .after_evaluation(primary)
            .map_err(|error| context.runtime_diagnostic(error, Some(&body.span), false));
        let cleaned = match CleanupContext::enter(context) {
            Ok(guard) => {
                let result = execution::evaluate_block(cleanup, guard.context).await;
                guard.context.after_evaluation(result).map_err(|error| {
                    guard
                        .context
                        .runtime_diagnostic(error, Some(&cleanup.span), false)
                })
            }
            Err(error) => Err(error),
        };
        match (primary, cleaned) {
            (primary, Ok(Completion::Normal(_))) => primary,
            (Err(primary), Err(cleanup)) => {
                Err(primary.with_cleanup(cleanup, context.budget.as_ref()))
            }
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (_, Ok(_)) => unreachable!("cleanup controls are validated before execution"),
        }
    })
}
