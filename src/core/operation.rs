//! Async adapter boundary. Hosts supply a Tokio runtime with time enabled.

mod ownership;
#[cfg(test)]
mod tests;
use ownership::{Admission, AdmissionFailure, OperationResult, Reservation, Tracked};
pub use ownership::{OperationBudget, OperationOwnershipLimits, OperationUsage};

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
    grammar::{validate_numeric_values, ArithmeticFailure, BWErr, Literal},
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
            Err(Diagnostic::formatted(
                BWErr::Cancelled,
                format_args!("Operation cancellation requested"),
            ))
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(Diagnostic::formatted(
                BWErr::Timeout,
                format_args!("Operation deadline expired"),
            ))
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
                let timer = match catch_unwind(AssertUnwindSafe(|| {
                    tokio::time::sleep_until(deadline)
                })) {
                    Ok(timer) => timer,
                    Err(_) => {
                        return Diagnostic::formatted(
                            BWErr::AsyncRuntime,
                            format_args!("Enable the Tokio time driver for operation deadlines"),
                        )
                    }
                };
                tokio::select! {
                    biased;
                    _ = self.cancellation.cancelled() => Diagnostic::formatted(BWErr::Cancelled, format_args!("Operation cancellation requested")),
                    _ = timer => Diagnostic::formatted(BWErr::Timeout, format_args!("Operation deadline expired")),
                }
            }
            None => {
                self.cancellation.cancelled().await;
                Diagnostic::formatted(
                    BWErr::Cancelled,
                    format_args!("Operation cancellation requested"),
                )
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
    ownership: OperationBudget,
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
            return Err(signature.builder_error(format_args!(
                "Blocking capacity exceeds the runtime's supported maximum"
            )));
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
            return Err(signature.builder_error(format_args!(
                "Native operations require native signature metadata"
            )));
        }
        Ok(Self {
            signature: Arc::new(signature),
            implementation,
            value_limits: ValueLimits::default(),
            diagnostic_limits: DiagnosticLimits::default(),
            ownership: OperationBudget::default(),
        })
    }

    pub fn signature(&self) -> &StatementSignature {
        &self.signature
    }

    /// Replace this clone's shared ownership pool. Existing invocations keep their old pool.
    pub fn with_ownership_budget(mut self, budget: OperationBudget) -> Self {
        self.ownership = budget;
        self
    }

    pub fn ownership_budget(&self) -> &OperationBudget {
        &self.ownership
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
        // Admit at construction, so unpolled futures cannot retain unlimited arguments.
        let values = Owned::new(values);
        let prepared = (|| {
            control
                .checkpoint()
                .map_err(|error| AdmissionFailure::Other(error.into_error()))?;
            if values.len() != self.signature.parameters().len() {
                return Err(AdmissionFailure::Arity);
            }
            let scope = self
                .ownership
                .arguments(&values, &self.value_limits, |index, value| {
                    validate_numeric_values(value).map_err(AdmissionFailure::Numeric)?;
                    self.signature
                        .validate_argument(index, value, |_| AdmissionFailure::Argument {
                            index,
                            kind: value.kind(),
                        })
                })?;
            Ok(Admission {
                values,
                scope: Arc::new(scope),
            })
        })();
        async move {
            let child = control.child(None);
            // Covers all exits; the final checkpoint precedes this guard's cancellation.
            let _cancel_on_drop = child.cancellation.clone().drop_guard();
            let scope = prepared
                .as_ref()
                .ok()
                .map(|admission| Arc::clone(&admission.scope));
            let mut cleanup_stop = None;
            let result = match prepared {
                Ok(admission) => {
                    self.invoke_inner(admission, child.clone(), &mut cleanup_stop)
                        .await
                }
                Err(failure) => {
                    let error = match failure {
                        AdmissionFailure::Arity => self.detail_error(
                            BWErr::ParameterMissingError,
                            "The operation does not match the signature's parameter count",
                        ),
                        AdmissionFailure::Other(error) => Diagnostic::new(error),
                        AdmissionFailure::Numeric(error) => self.numeric_error(error),
                        AdmissionFailure::Argument { index, kind } => self
                            .signature
                            .validate_argument_kind(index, kind, |message| {
                                self.signature_error(message)
                            })
                            .unwrap_err(),
                    };
                    Err(self.track_error(error, false, true, &scope, None))
                }
            };
            // Cleanup cancellation must not replace the stop observed before draining.
            let stopped = if cleanup_stop.is_some() {
                None
            } else {
                child.checkpoint().err()
            };
            let preserve_stop = matches!(
                cleanup_stop,
                Some(DiagnosticCode::Cancelled | DiagnosticCode::Timeout)
            ) || stopped.as_ref().is_some_and(|error| {
                matches!(
                    error.code(),
                    DiagnosticCode::Cancelled | DiagnosticCode::Timeout
                )
            });
            let result = match (result, stopped) {
                (Ok(value), None) => return Ok(value.into_inner().into_inner()),
                (Ok(value), Some(error)) => {
                    drop(value);
                    self.track_error(error, preserve_stop, true, &scope, None)
                }
                (Err(error), Some(stopped)) if error.value.as_ref().code() != stopped.code() => {
                    self.attach_stop(stopped, error, preserve_stop, &scope)
                }
                (Err(error), _) => error,
            };
            // Payload and lease move together until this public host ownership boundary.
            Err(result.into_inner().into_inner())
        }
    }

    async fn invoke_inner(
        &self,
        admission: Admission,
        control: OperationControl,
        cleanup_stop: &mut Option<DiagnosticCode>,
    ) -> OperationResult {
        let scope = Some(Arc::clone(&admission.scope));
        let fail = |error| self.track_error(error, false, true, &scope, None);
        control.checkpoint().map_err(&fail)?;
        tokio::runtime::Handle::try_current().map_err(|_| {
            fail(self.detail_error(
                BWErr::AsyncRuntime,
                "Invoke operations inside a Tokio runtime",
            ))
        })?;
        let child = control;
        let value = match &self.implementation {
            Implementation::Async(callback) => {
                let future = catch_unwind(AssertUnwindSafe(|| {
                    callback(admission.values.into_inner(), child.clone())
                }))
                .map_err(|_| fail(panic_error(&self.diagnostic_limits, &self.signature)))?;
                let future = GuardedFuture {
                    future,
                    operation: self.clone(),
                    scope: Some(admission.scope),
                };
                tokio::select! {
                    biased;
                    error = child.stopped() => return Err(fail(error)),
                    value = future => value?,
                }
            }
            Implementation::Blocking { callback, capacity } => {
                let permit = tokio::select! {
                    biased;
                    error = child.stopped() => return Err(fail(error)),
                    permit = Arc::clone(capacity).acquire_owned() => permit.map_err(|_| fail(self.detail_error(BWErr::AsyncRuntime, "Blocking operation capacity is closed")))?,
                };
                child.checkpoint().map_err(&fail)?;
                let callback = Arc::clone(callback);
                let worker_control = child.clone();
                let operation = self.clone();
                let mut worker = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    operation.run_blocking(callback, admission, worker_control)
                });
                tokio::select! {
                    biased;
                    error = child.stopped() => {
                        // Preserve the observed deadline/runtime failure before cancellation
                        // is requested solely to drain the worker.
                        *cleanup_stop = Some(error.code());
                        let preserve = matches!(error.code(), DiagnosticCode::Cancelled | DiagnosticCode::Timeout);
                        let error = self.track_error(error, preserve, true, &scope, None);
                        child.cancel();
                        worker.abort(); // Cancels queued work; started callbacks must cooperate.
                        return Err(match worker.await {
                            Ok(Err(cause)) => self.combine_errors(error, cause, preserve, &scope),
                            Err(cause) if !cause.is_cancelled() => {
                                let Tracked { value, reservation, .. } = *error;
                                self.track_error(self.worker_cleanup_error(value.into_inner(), &cause), preserve, true, &scope, reservation)
                            },
                            _ => error,
                        });
                    }
                    value = &mut worker => value.map_err(|error| fail(self.worker_error(&error)))??,
                }
            }
        };
        child.checkpoint().map_err(&fail)?;
        Ok(value)
    }

    fn run_blocking(
        &self,
        callback: BlockingCallback,
        admission: Admission,
        control: OperationControl,
    ) -> OperationResult {
        let scope = Some(Arc::clone(&admission.scope));
        control
            .checkpoint()
            .map_err(|error| self.track_error(error, false, true, &scope, None))?;
        match catch_unwind(AssertUnwindSafe(|| {
            callback(admission.values.into_inner(), control)
        })) {
            Ok(result) => self.track_result(result, &scope),
            Err(_) => Err(self.track_error(
                panic_error(&self.diagnostic_limits, &self.signature),
                false,
                true,
                &scope,
                None,
            )),
        }
    }

    fn track_result(
        &self,
        result: DiagnosticResult<Literal>,
        scope: &Option<Arc<Reservation>>,
    ) -> OperationResult {
        let fail = |error| self.track_error(error, false, true, scope, None);
        match result {
            Err(error) => Err(self.track_error(error, false, false, scope, None)),
            Ok(value) => {
                let value = Owned::new(value);
                let size = self
                    .value_limits
                    .check(&value)
                    .map_err(|error| fail(Diagnostic::new(error)))?;
                validate_numeric_values(&value).map_err(|error| fail(self.numeric_error(error)))?;
                self.signature
                    .validate_return(&value, |message| self.signature_error(message))
                    .map_err(&fail)?;
                let reservation = self
                    .ownership
                    .value(size)
                    .map_err(|error| fail(Diagnostic::new(error)))?;
                Ok(Tracked {
                    value,
                    reservation: Some(reservation),
                    _scope: scope.clone(),
                })
            }
        }
    }

    fn track_error(
        &self,
        error: Diagnostic,
        preserve_stop: bool,
        admitted: bool,
        scope: &Option<Arc<Reservation>>,
        previous: Option<Reservation>,
    ) -> Box<Tracked<OwnedDiagnostic>> {
        let trusted_emergency = admitted && error.is_emergency();
        let original = Arc::clone(&error.error);
        let mut error = admit_error(
            &self.diagnostic_limits,
            &self.signature,
            error,
            preserve_stop,
            admitted,
        );
        let emergency =
            error.is_emergency() && (trusted_emergency || !Arc::ptr_eq(&original, &error.error));
        drop(original);
        let mut reservation = None;
        if !emergency {
            // Individual admission has already bounded this tree and its traversal work.
            let size = self
                .diagnostic_limits
                .check(&error)
                .expect("admitted diagnostic");
            match self.ownership.diagnostic(size, previous) {
                Ok(lease) => reservation = Some(lease),
                Err(rejection) => {
                    let (violation, previous) = *rejection;
                    // Free rejected owners before returning their previous allowance.
                    error = error.rejected(violation);
                    if preserve_stop {
                        error = preserve_primary(error);
                    }
                    drop(previous);
                }
            }
        } else {
            drop(previous);
        }
        Box::new(Tracked {
            value: OwnedDiagnostic::new(error),
            reservation,
            _scope: scope.clone(),
        })
    }

    fn attach_stop(
        &self,
        stopped: Diagnostic,
        error: Box<Tracked<OwnedDiagnostic>>,
        preserve_stop: bool,
        scope: &Option<Arc<Reservation>>,
    ) -> Box<Tracked<OwnedDiagnostic>> {
        let Tracked {
            value, reservation, ..
        } = *error;
        self.track_error(
            stopped.while_handling(value.into_inner()),
            preserve_stop,
            true,
            scope,
            reservation,
        )
    }

    fn combine_errors(
        &self,
        primary: Box<Tracked<OwnedDiagnostic>>,
        cause: Box<Tracked<OwnedDiagnostic>>,
        preserve_stop: bool,
        scope: &Option<Arc<Reservation>>,
    ) -> Box<Tracked<OwnedDiagnostic>> {
        let Tracked {
            value: primary,
            reservation: primary_lease,
            ..
        } = *primary;
        let Tracked {
            value: cause,
            reservation: cause_lease,
            ..
        } = *cause;
        let reservation = Reservation::merge(primary_lease, cause_lease);
        self.track_error(
            primary.into_inner().while_handling(cause.into_inner()),
            preserve_stop,
            true,
            scope,
            reservation,
        )
    }

    fn detail_error(&self, category: fn(String) -> BWErr, message: &str) -> Diagnostic {
        self.diagnostic_limits.borrowed_detail(
            category,
            message,
            Some(self.signature.header()),
            false,
            std::iter::empty(),
        )
    }

    fn numeric_error(&self, error: ArithmeticFailure) -> Diagnostic {
        self.diagnostic_limits.formatted_detail(
            BWErr::ArithmeticError,
            format_args!("{error}"),
            Some(self.signature.header()),
            false,
            std::iter::empty(),
        )
    }

    fn worker_error(&self, error: &tokio::task::JoinError) -> Diagnostic {
        self.diagnostic_limits.formatted_detail(
            BWErr::AsyncRuntime,
            format_args!("Blocking worker ended unexpectedly: {error}"),
            Some(self.signature.header()),
            false,
            std::iter::empty(),
        )
    }

    fn worker_cleanup_error(
        &self,
        primary: Diagnostic,
        cause: &tokio::task::JoinError,
    ) -> Diagnostic {
        // This stop was observed before requesting cancellation to drain the
        // worker. Its known context is part of initial cause admission.
        let preserve_stop = matches!(
            primary.code(),
            DiagnosticCode::Cancelled | DiagnosticCode::Timeout
        );
        self.diagnostic_limits
            .formatted_cause(
                primary,
                BWErr::AsyncRuntime,
                format_args!("Blocking worker ended during cleanup: {cause}"),
                self.signature.header(),
            )
            .unwrap_or_else(|rejection| {
                if preserve_stop {
                    preserve_primary(rejection)
                } else {
                    rejection
                }
            })
    }

    fn signature_error(&self, message: std::fmt::Arguments<'_>) -> Diagnostic {
        self.diagnostic_limits.formatted_detail(
            BWErr::OperationIncompatibleError,
            message,
            Some(self.signature.header()),
            false,
            std::iter::empty(),
        )
    }
}

struct GuardedFuture {
    future: OperationFuture,
    operation: NativeOperation,
    scope: Option<Arc<Reservation>>,
}

impl Future for GuardedFuture {
    type Output = OperationResult;
    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.future.as_mut().poll(context))) {
            Ok(result) => result.map(|result| self.operation.track_result(result, &self.scope)),
            Err(_) => Poll::Ready(Err(self.operation.track_error(
                panic_error(&self.operation.diagnostic_limits, &self.operation.signature),
                false,
                true,
                &self.scope,
                None,
            ))),
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
        Err(error) if preserve_stop => preserve_primary(error),
        Err(error) => error,
    }
}

fn preserve_primary(mut rejection: Diagnostic) -> Diagnostic {
    let mut original = rejection.causes.pop().expect("bounded original");
    original.causes.push(rejection);
    original
}
