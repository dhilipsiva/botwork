use super::*;
use crate::core::run::{blocking_io, SnapshotSize};
use std::sync::OnceLock;
use tokio::sync::Semaphore;

static NATIVE_CAPACITY: OnceLock<Arc<Semaphore>> = OnceLock::new();

impl Context {
    // Public callbacks receive only arguments/environment; Log needs output
    // budgets and diagnostic frames. No mutable DSL bindings enter a worker.
    fn native_worker(&self) -> EvaluationResult<Self> {
        let mut size = SnapshotSize::default();
        size.entries(self.calls.len());
        self.charge_snapshot(size)?;
        let mut worker = Self::with_directory_snapshot(Ok(PathBuf::new()));
        worker.calls = self.calls.clone();
        worker.budget = self.budget.as_ref().map(RunBudget::shared);
        worker.environment = self.environment.clone();
        Ok(worker)
    }

    pub(super) fn blocking<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&mut Context) -> EvaluationResult<T> + Send + 'static,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = EvaluationResult<T>> + Send + '_>> {
        // Keep the worker snapshot/future out of recursive evaluator poll frames.
        // Inlining it into every dispatch can exhaust the stack before depth admission.
        Box::pin(async move {
            self.checkpoint()?;
            let mut worker = self.native_worker()?;
            let control = self
                .budget
                .as_ref()
                .map(|budget| budget.control().clone())
                .unwrap_or_default();
            let capacity = Arc::clone(NATIVE_CAPACITY.get_or_init(|| Arc::new(Semaphore::new(32))));
            let result = blocking_io::execute(capacity, control, move |control| {
                worker.worker_control = Some(control.clone());
                worker.environment = worker
                    .environment
                    .as_ref()
                    .map(|environment| Arc::new(environment.with_control(control.clone())));
                work(&mut worker)
            })
            .await?;
            let value =
                match result.stop {
                    Some(stop) => match result.value {
                        Err(error) if error.code() == stop.code() => Err(error),
                        Err(error) => Err(RuntimeDiagnostic::from(stop)
                            .while_handling(error, self.budget.as_ref())),
                        _ => Err(stop.into()),
                    },
                    None => result.value,
                };
            self.after_evaluation(value)
        })
    }

    pub(super) async fn trace_statement(
        &mut self,
        statement: &Statement,
        source: &Arc<ast::SourceFile>,
    ) -> EvaluationResult<()> {
        if !self.trace_statements {
            return Ok(());
        }
        let (line, column) = statement.span.line_column();
        let kind = statement.kind_name();
        if self.asynchronous {
            // The span retains the already-admitted source owner without a text copy.
            let source = Arc::clone(source);
            self.blocking(move |context| {
                context.write_output(
                    &mut io::stderr().lock(),
                    format_args!("debug: {}:{line}:{column}: {kind}\n", source.name()),
                )?;
                Ok(())
            })
            .await
        } else {
            self.write_output(
                &mut io::stderr().lock(),
                format_args!("debug: {}:{line}:{column}: {kind}\n", source.name()),
            )?;
            Ok(())
        }
    }
}

pub(super) fn native_body(
    context: &mut Context,
    body: NativeBody,
    metadata: &StatementSignature,
    arguments: Vec<TemporaryValue>,
    span: &Span,
) -> TemporaryResult {
    match body {
        NativeBody::Builtin(builtin) => {
            let result = builtin
                .invoke(arguments, context)
                .map_err(|error| context.runtime_diagnostic(error, Some(span), false));
            let result = context.after_evaluation(result)?;
            validate_result(context, metadata, &result, span)?;
            Ok(result)
        }
        NativeBody::Callback(callback) => {
            let arguments = TemporaryArguments::new(arguments);
            let result = catch_unwind(AssertUnwindSafe(|| callback(&arguments, context)))
                .map_err(|_| {
                    context.detail_error(
                        BWErr::NativePanic,
                        metadata.normalized(),
                        Some(span),
                        false,
                    )
                })
                .and_then(|result| {
                    result.map_err(|error| context.runtime_diagnostic(error, Some(span), false))
                });
            let result = context.after_evaluation(result.map(Owned::new))?;
            validate_result(context, metadata, &result, span)?;
            context.temporary(result.into_inner())
        }
    }
}

fn validate_result(
    context: &Context,
    metadata: &StatementSignature,
    value: &Literal,
    span: &Span,
) -> EvaluationResult<()> {
    context.check_value(value)?;
    validate_numeric_values(value).map_err(|error| {
        context.formatted_error(
            BWErr::ArithmeticError,
            format_args!("{error}"),
            Some(span),
            false,
        )
    })?;
    metadata.validate_return(value, |message| {
        context.formatted_error(
            BWErr::OperationIncompatibleError,
            message,
            Some(span),
            false,
        )
    })
}

#[cfg(test)]
mod tests;
