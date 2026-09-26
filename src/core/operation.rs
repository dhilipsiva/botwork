//! Async adapter boundary. Hosts supply a Tokio runtime with time enabled.

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
    diagnostic::{Diagnostic, DiagnosticResult},
    grammar::{validate_value, BWErr, Literal},
    signature::{StatementOrigin, StatementSignature},
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
        })
    }

    pub fn signature(&self) -> &StatementSignature {
        &self.signature
    }

    /// Validate values before entry, run once, and validate the result before publication.
    /// Dropping an async invocation drops its future; dropping a blocking invocation
    /// requests cancellation but cannot synchronously join its worker.
    pub async fn invoke(
        &self,
        values: Vec<Literal>,
        control: OperationControl,
    ) -> DiagnosticResult<Literal> {
        self.invoke_inner(values, control)
            .await
            .map_err(|error| error.at(self.signature.header()))
    }

    async fn invoke_inner(
        &self,
        values: Vec<Literal>,
        control: OperationControl,
    ) -> DiagnosticResult<Literal> {
        control.checkpoint()?;
        if values.len() != self.signature.parameters().len() {
            return Err(BWErr::ParameterMissingError(
                "The operation does not match the signature's parameter count".into(),
            )
            .into());
        }
        for (index, value) in values.iter().enumerate() {
            validate_value(value)?;
            self.signature.validate_argument(index, value)?;
        }
        tokio::runtime::Handle::try_current()
            .map_err(|_| BWErr::AsyncRuntime("Invoke operations inside a Tokio runtime".into()))?;
        let child = control.child(None);
        // Covers normal completion, errors, and the host dropping this invocation.
        let _cancel_on_drop = child.cancellation.clone().drop_guard();
        let value = match &self.implementation {
            Implementation::Async(callback) => {
                let future = catch_unwind(AssertUnwindSafe(|| callback(values, child.clone())))
                    .map_err(|_| self.panic_error())?;
                let future = GuardedFuture {
                    future,
                    signature: Arc::clone(&self.signature),
                };
                tokio::select! {
                    biased;
                    error = child.stopped() => return Err(error),
                    value = future => value?,
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
                let mut worker = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    worker_control.checkpoint()?;
                    catch_unwind(AssertUnwindSafe(|| callback(values, worker_control)))
                        .unwrap_or_else(|_| {
                            Err(BWErr::NativePanic(signature.normalized().into()).into())
                        })
                });
                tokio::select! {
                    biased;
                    mut error = child.stopped() => {
                        child.cancel();
                        worker.abort(); // Cancels queued work; started callbacks must cooperate.
                        match worker.await {
                            Ok(Err(cause)) => error.causes.push(cause.at(self.signature.header())),
                            Err(cause) if !cause.is_cancelled() => error.causes.push(
                                Diagnostic::new(BWErr::AsyncRuntime(format!("Blocking worker ended during cleanup: {cause}"))).at(self.signature.header())
                            ),
                            _ => (),
                        }
                        return Err(error);
                    }
                    value = &mut worker => value.map_err(|error| BWErr::AsyncRuntime(format!("Blocking worker ended unexpectedly: {error}")))??,
                }
            }
        };
        child.checkpoint()?;
        validate_value(&value)?;
        self.signature.validate_return(&value)?;
        Ok(value)
    }

    fn panic_error(&self) -> Diagnostic {
        BWErr::NativePanic(self.signature.normalized().into()).into()
    }
}

struct GuardedFuture {
    future: OperationFuture,
    signature: Arc<StatementSignature>,
}

impl Future for GuardedFuture {
    type Output = DiagnosticResult<Literal>;
    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> Poll<Self::Output> {
        catch_unwind(AssertUnwindSafe(|| self.future.as_mut().poll(context))).unwrap_or_else(|_| {
            Poll::Ready(Err(
                BWErr::NativePanic(self.signature.normalized().into()).into()
            ))
        })
    }
}
