//! Async adapter boundary. Hosts supply a Tokio runtime with time enabled.

#[cfg(test)]
mod tests;

use std::{
    future::Future,
    num::NonZeroUsize,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};
use tokio::{sync::Semaphore, time::Instant};
use tokio_util::sync::CancellationToken;

use super::{
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLimits, DiagnosticResult, OwnedDiagnostic},
    grammar::{validate_value, BWErr, Literal},
    signature::{StatementOrigin, StatementSignature},
    value_limits::{Owned, ValueLimits},
};

/// Clones share cancellation; children receive parent cancellation without cancelling parents.
#[derive(Clone, Debug, Default)]
pub struct OperationControl {
    cancellation: CancellationToken,
    deadline: Option<Instant>,
}

impl OperationControl {
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// A child inherits the earlier deadline and cannot extend its parent's deadline.
    pub fn child(&self, deadline: Option<Instant>) -> Self {
        Self {
            cancellation: self.cancellation.child_token(),
            deadline: match (self.deadline, deadline) {
                (Some(parent), Some(child)) => Some(parent.min(child)),
                (parent, child) => parent.or(child),
            },
        }
    }

    pub fn checkpoint(&self) -> DiagnosticResult<()> {
        if self.is_cancelled() {
            Err(BWErr::Cancelled("Operation cancellation requested".into()).into())
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(BWErr::Timeout("Operation deadline expired".into()).into())
        } else {
            Ok(())
        }
    }

    /// Wait for a stop request. Cancellation has priority when both are ready.
    pub async fn stopped(&self) -> Diagnostic {
        if let Err(error) = self.checkpoint() {
            return error;
        }
        match self.deadline {
            Some(deadline) => {
                let timer =
                    match catch_unwind(AssertUnwindSafe(|| tokio::time::sleep_until(deadline))) {
                        Ok(timer) => timer,
                        Err(_) => {
                            return BWErr::AsyncRuntime(
                                "Enable the Tokio time driver for operation deadlines".into(),
                            )
                            .into()
                        }
                    };
                tokio::select! {
                    biased;
                    _ = self.cancellation.cancelled() => BWErr::Cancelled("Operation cancellation requested".into()).into(),
                    _ = timer => BWErr::Timeout("Operation deadline expired".into()).into(),
                }
            }
            None => {
                self.cancellation.cancelled().await;
                BWErr::Cancelled("Operation cancellation requested".into()).into()
            }
        }
    }
}

type OperationFuture = Pin<Box<dyn Future<Output = DiagnosticResult<Literal>> + Send>>;
type AsyncCallback = Arc<dyn Fn(Vec<Literal>, OperationControl) -> OperationFuture + Send + Sync>;
type BlockingCallback =
    Arc<dyn Fn(Vec<Literal>, OperationControl) -> DiagnosticResult<Literal> + Send + Sync>;

#[derive(Clone)]
enum Implementation {
    Async(AsyncCallback),
    Blocking {
        callback: BlockingCallback,
        capacity: Arc<Semaphore>,
    },
}

/// Host-callable operation contract. DSL async dispatch is a separate runtime integration.
#[derive(Clone)]
pub struct NativeOperation {
    signature: Arc<StatementSignature>,
    implementation: Implementation,
    value_limits: ValueLimits,
    diagnostic_limits: DiagnosticLimits,
}

impl NativeOperation {
    pub fn asynchronous<Fut>(
        signature: StatementSignature,
        callback: impl Fn(Vec<Literal>, OperationControl) -> Fut + Send + Sync + 'static,
    ) -> DiagnosticResult<Self>
    where
        Fut: Future<Output = DiagnosticResult<Literal>> + Send + 'static,
    {
        Self::new(
            signature,
            Implementation::Async(Arc::new(move |values, control| {
                Box::pin(callback(values, control))
            })),
        )
    }

    /// Blocking callbacks must cooperate with checkpoints. Stop requests drain
    /// started work before returning; an uncooperative callback has no hard bound.
    pub fn blocking(
        signature: StatementSignature,
        max_in_flight: NonZeroUsize,
        callback: impl Fn(Vec<Literal>, OperationControl) -> DiagnosticResult<Literal>
            + Send
            + Sync
            + 'static,
    ) -> DiagnosticResult<Self> {
        if max_in_flight.get() > Semaphore::MAX_PERMITS {
            return Err(Diagnostic::new(BWErr::SignatureError(
                "Blocking capacity exceeds the runtime's supported maximum".into(),
            ))
            .at(signature.header()));
        }
        Self::new(
            signature,
            Implementation::Blocking {
                callback: Arc::new(callback),
                capacity: Arc::new(Semaphore::new(max_in_flight.get())),
            },
        )
    }

    fn new(
        signature: StatementSignature,
        implementation: Implementation,
    ) -> DiagnosticResult<Self> {
        if signature.origin() != StatementOrigin::Native {
            return Err(Diagnostic::new(BWErr::SignatureError(
                "Native operations require native signature metadata".into(),
            ))
            .at(signature.header()));
        }
        Ok(Self {
            signature: Arc::new(signature),
            implementation,
            value_limits: ValueLimits::default(),
            diagnostic_limits: DiagnosticLimits::default(),
        })
    }

    pub fn signature(&self) -> &StatementSignature {
        &self.signature
    }

    pub fn with_value_limits(mut self, limits: ValueLimits) -> DiagnosticResult<Self> {
        limits.validate()?;
        self.value_limits = limits;
        Ok(self)
    }

    /// Configure individual callback/cleanup errors before worker handoff and publication.
    pub fn with_diagnostic_limits(mut self, limits: DiagnosticLimits) -> DiagnosticResult<Self> {
        limits.validate()?;
        self.diagnostic_limits = limits;
        Ok(self)
    }

    /// Validate values before entry, run once, and validate the result before publication.
    /// Dropping an async invocation drops its future; dropping a blocking invocation
    /// requests cancellation but cannot synchronously join its worker.
    pub fn invoke(
        &self,
        values: Vec<Literal>,
        control: OperationControl,
    ) -> impl Future<Output = DiagnosticResult<Literal>> + Send + '_ {
        // Wrap ownership before the first poll: dropping an unpolled invocation
        // must also release rejected deep host input without recursive destruction.
        let values = Owned::new(values);
        async move {
            let child = control.child(None);
            // Covers all exits; the final checkpoint precedes this guard's cancellation.
            let _cancel_on_drop = child.cancellation.clone().drop_guard();
            let mut cleanup_stop = None;
            let result = self
                .invoke_inner(values, child.clone(), &mut cleanup_stop)
                .await;
            let stopped = cleanup_stop.or_else(|| child.checkpoint().err());
            let preserve_stop = stopped.as_ref().is_some_and(|error| {
                matches!(
                    error.code(),
                    DiagnosticCode::Cancelled | DiagnosticCode::Timeout
                )
            });
            let result = match (result, stopped) {
                (Ok(value), None) => return Ok(value.into_inner()),
                (Ok(_), Some(error)) => error,
                (Err(error), Some(stopped)) if error.code() != stopped.code() => {
                    stopped.while_handling(error)
                }
                (Err(error), _) => error,
            };
            Err(admit_error(
                &self.diagnostic_limits,
                &self.signature,
                result,
                preserve_stop,
                true,
            ))
        }
    }

    async fn invoke_inner(
        &self,
        values: Owned<Vec<Literal>>,
        control: OperationControl,
        cleanup_stop: &mut Option<Diagnostic>,
    ) -> DiagnosticResult<Owned<Literal>> {
        control.checkpoint()?;
        if values.len() != self.signature.parameters().len() {
            return Err(BWErr::ParameterMissingError(
                "The operation does not match the signature's parameter count".into(),
            )
            .into());
        }
        for (index, value) in values.iter().enumerate() {
            self.value_limits.check(value)?;
            validate_value(value)?;
            self.signature.validate_argument(index, value)?;
        }
        tokio::runtime::Handle::try_current()
            .map_err(|_| BWErr::AsyncRuntime("Invoke operations inside a Tokio runtime".into()))?;
        let child = control;
        let value = match &self.implementation {
            Implementation::Async(callback) => {
                let future = catch_unwind(AssertUnwindSafe(|| {
                    callback(values.into_inner(), child.clone())
                }))
                .map_err(|_| panic_error(&self.diagnostic_limits, &self.signature))?;
                let future = GuardedFuture {
                    future,
                    signature: Arc::clone(&self.signature),
                    limits: self.diagnostic_limits.clone(),
                };
                tokio::select! {
                    biased;
                    error = child.stopped() => return Err(error),
                    value = future => value.map_err(OwnedDiagnostic::into_inner)?,
                }
            }
            Implementation::Blocking { callback, capacity } => {
                let permit = tokio::select! {
                    biased;
                    error = child.stopped() => return Err(error),
                    permit = Arc::clone(capacity).acquire_owned() => permit.map_err(|_| BWErr::AsyncRuntime("Blocking operation capacity is closed".into()))?,
                };
                child.checkpoint()?;
                let callback = Arc::clone(callback);
                let worker_control = child.clone();
                let signature = Arc::clone(&self.signature);
                let limits = self.diagnostic_limits.clone();
                let mut worker = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    worker_control.checkpoint().map_err(|error| {
                        OwnedDiagnostic::new(admit_error(&limits, &signature, error, false, false))
                    })?;
                    match catch_unwind(AssertUnwindSafe(|| {
                        callback(values.into_inner(), worker_control)
                    })) {
                        Ok(result) => admit_callback_result(result, &limits, &signature),
                        Err(_) => Err(OwnedDiagnostic::new(panic_error(&limits, &signature))),
                    }
                });
                tokio::select! {
                    biased;
                    mut error = child.stopped() => {
                        // Preserve the observed deadline/runtime failure before cancellation
                        // is requested solely to drain the worker.
                        *cleanup_stop = Some(error.clone());
                        child.cancel();
                        worker.abort(); // Cancels queued work; started callbacks must cooperate.
                        match worker.await {
                            Ok(Err(cause)) => error.causes.push(cause.into_inner().at(self.signature.header())),
                            Err(cause) if !cause.is_cancelled() => error.causes.push(
                                Diagnostic::new(BWErr::AsyncRuntime(format!("Blocking worker ended during cleanup: {cause}"))).at(self.signature.header())
                            ),
                            _ => (),
                        }
                        return Err(error);
                    }
                    value = &mut worker => value.map_err(|error| BWErr::AsyncRuntime(format!("Blocking worker ended unexpectedly: {error}")))?.map_err(OwnedDiagnostic::into_inner)?,
                }
            }
        };
        child.checkpoint()?;
        self.value_limits.check(&value)?;
        validate_value(&value)?;
        self.signature.validate_return(&value)?;
        Ok(value)
    }
}

struct GuardedFuture {
    future: OperationFuture,
    signature: Arc<StatementSignature>,
    limits: DiagnosticLimits,
}

impl Future for GuardedFuture {
    type Output = Result<Owned<Literal>, OwnedDiagnostic>;
    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.future.as_mut().poll(context))) {
            Ok(result) => {
                result.map(|result| admit_callback_result(result, &self.limits, &self.signature))
            }
            Err(_) => Poll::Ready(Err(OwnedDiagnostic::new(panic_error(
                &self.limits,
                &self.signature,
            )))),
        }
    }
}

fn panic_error(limits: &DiagnosticLimits, signature: &StatementSignature) -> Diagnostic {
    limits.borrowed_detail(
        BWErr::NativePanic,
        signature.normalized(),
        Some(signature.header()),
        false,
        std::iter::empty(),
    )
}

fn admit_callback_result(
    result: DiagnosticResult<Literal>,
    limits: &DiagnosticLimits,
    signature: &StatementSignature,
) -> Result<Owned<Literal>, OwnedDiagnostic> {
    result
        .map(Owned::new)
        .map_err(|error| OwnedDiagnostic::new(admit_error(limits, signature, error, false, false)))
}

// Only internal, previously admitted emergency records can skip another admission.
// Host callback records always pass measurement, even if their public fields imitate one.
fn admit_error(
    limits: &DiagnosticLimits,
    signature: &StatementSignature,
    error: Diagnostic,
    preserve_stop: bool,
    admitted: bool,
) -> Diagnostic {
    if admitted && error.is_emergency() {
        return error;
    }
    match limits.admit(error.at(signature.header())) {
        Ok(error) => error,
        Err(mut error) if preserve_stop => {
            let mut original = error.causes.pop().expect("bounded original");
            original.causes.push(error);
            original
        }
        Err(error) => error,
    }
}
