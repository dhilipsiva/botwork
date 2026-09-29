//! Bounded filesystem work that never runs on an asynchronous executor thread.
use super::SourceFailure;
use crate::core::{diagnostic::Diagnostic, grammar::BWErr, operation::OperationControl};
use std::{
    fs,
    io::Read,
    path::Path,
    sync::{Arc, OnceLock},
};
use tokio::{sync::Semaphore, task::JoinHandle};

const MAX_JOBS: usize = 32;
static CAPACITY: OnceLock<Arc<Semaphore>> = OnceLock::new();

struct Completed<T> {
    result: Result<T, Diagnostic>,
    // Keep capacity through delivery or disposal, including an abandoned result.
    _permit: tokio::sync::OwnedSemaphorePermit,
}

struct Pending<T> {
    control: OperationControl,
    worker: JoinHandle<Completed<T>>,
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        self.control.cancel();
        // Queued work can be stopped; a started syscall still has to return.
        self.worker.abort();
    }
}

fn runtime_error(reason: &'static str) -> Diagnostic {
    Diagnostic::formatted(BWErr::AsyncRuntime, format_args!("{reason}"))
}

pub(crate) async fn run<T: Send + 'static>(
    control: OperationControl,
    work: impl FnOnce(&OperationControl) -> Result<T, SourceFailure> + Send + 'static,
) -> Result<T, SourceFailure> {
    let capacity = Arc::clone(CAPACITY.get_or_init(|| Arc::new(Semaphore::new(MAX_JOBS))));
    run_in(capacity, control, work).await
}

async fn run_in<T: Send + 'static>(
    capacity: Arc<Semaphore>,
    control: OperationControl,
    work: impl FnOnce(&OperationControl) -> Result<T, SourceFailure> + Send + 'static,
) -> Result<T, SourceFailure> {
    let outcome = execute(capacity, control, work)
        .await
        .map_err(SourceFailure::Diagnostic)?;
    match (outcome.stop, outcome.value) {
        (Some(stop), Ok(_)) => Err(SourceFailure::Diagnostic(stop)),
        (Some(stop), Err(SourceFailure::Diagnostic(cause))) => {
            // Synthetic child cancellation used to drain work never changes an
            // observed timeout into cancellation.
            if cause.code() == stop.code() {
                Err(SourceFailure::Diagnostic(cause))
            } else if cause.code() == crate::core::diagnostic::DiagnosticCode::Cancelled {
                Err(SourceFailure::Diagnostic(stop))
            } else {
                Err(SourceFailure::Diagnostic(stop.while_handling(cause)))
            }
        }
        // The caller admits OS paths/messages before applying parent stop priority.
        (_, result) => result,
    }
}

pub(crate) struct Outcome<T> {
    pub(crate) value: T,
    pub(crate) stop: Option<Diagnostic>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Transfer a result with its permit still held. Callers apply stop priority
/// under their own diagnostic limits, preserving any completed error as a cause.
pub(crate) async fn execute<T: Send + 'static>(
    capacity: Arc<Semaphore>,
    control: OperationControl,
    work: impl FnOnce(&OperationControl) -> T + Send + 'static,
) -> Result<Outcome<T>, Diagnostic> {
    let failure = runtime_error;
    control.checkpoint()?;
    tokio::runtime::Handle::try_current()
        .map_err(|_| failure("Run blocking jobs inside a Tokio runtime"))?;
    let child = control.child(None);
    let permit = tokio::select! {
        biased;
        stop = child.stopped() => return Err(stop),
        permit = capacity.acquire_owned() => permit.map_err(|_| failure("Blocking job capacity is closed"))?,
    };
    child.checkpoint()?;
    let worker_control = child.clone();
    let mut pending = Pending {
        control: child,
        worker: tokio::task::spawn_blocking(move || Completed {
            result: worker_control.checkpoint().map(|()| work(&worker_control)),
            _permit: permit,
        }),
    };
    let (completed, stop) = tokio::select! {
        biased;
        stop = pending.control.stopped() => {
            pending.control.cancel();
            pending.worker.abort();
            // Drain started work within the stop grace. Cancellation never means a
            // syscall was killed: work still running afterwards is abandoned, keeping
            // its permit until it returns, and its result is discarded.
            match control.within_grace(&mut pending.worker).await {
                Some(result) => (result, Some(stop)),
                None => return Err(control.abandoned(stop)),
            }
        }
        result = &mut pending.worker => (result, control.checkpoint().err()),
    };
    match completed {
        Ok(Completed {
            result: Ok(value),
            _permit,
        }) => Ok(Outcome {
            value,
            stop,
            _permit,
        }),
        Ok(Completed {
            result: Err(error), ..
        }) => Err(stop.unwrap_or(error)),
        Err(error) => {
            let failure = failure("Blocking worker failed before completing");
            Err(match stop {
                Some(stop) if error.is_cancelled() => stop,
                Some(stop) => stop.while_handling(failure),
                None => failure,
            })
        }
    }
}

pub(crate) fn read(
    path: &Path,
    maximum: usize,
    control: &OperationControl,
) -> Result<Vec<u8>, SourceFailure> {
    control.checkpoint().map_err(SourceFailure::Diagnostic)?;
    let mut file = fs::File::open(path).map_err(SourceFailure::Io)?;
    read_from(&mut file, maximum, control)
}

fn read_from(
    file: &mut impl Read,
    maximum: usize,
    control: &OperationControl,
) -> Result<Vec<u8>, SourceFailure> {
    let mut bytes = Vec::new();
    let limit = maximum.saturating_add(1);
    let mut buffer = [0; 16 * 1024];
    while bytes.len() < limit {
        control.checkpoint().map_err(SourceFailure::Diagnostic)?;
        let count = (limit - bytes.len()).min(buffer.len());
        let read = match file.read(&mut buffer[..count]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.map_err(SourceFailure::Io)?,
        };
        control.checkpoint().map_err(SourceFailure::Diagnostic)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
