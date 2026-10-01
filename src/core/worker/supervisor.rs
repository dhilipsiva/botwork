//! Worker supervision shared by every platform: one owned OS thread per worker
//! drives launch, pipe I/O, termination, and reaping, while an observer
//! publishes the result. The platform backend owns the child itself.
use super::*;
use std::{
    io::{self, Read, Write},
    process::{Child, Command, Stdio},
};

mod observation;
mod owner;
#[cfg(unix)]
pub(super) mod unix;
#[cfg(all(test, unix))]
use unix::ChildOwner;
#[cfg(unix)]
use unix::{launch_worker, nonblocking};
#[cfg(windows)]
mod windows;
#[cfg(all(test, windows))]
use windows::ChildOwner;
#[cfg(windows)]
use windows::{launch_worker, nonblocking};
#[cfg(test)]
pub(super) type LaunchHook = Box<dyn FnOnce(WorkerCommand) -> io::Result<ChildOwner> + Send>;

const QUANTUM: Duration = Duration::from_millis(5);
#[cfg(test)]
mod tests;
const CHUNK: usize = 4096;

struct Completion {
    status: Option<ExitStatus>,
    cleanup: WorkerCleanup,
    error: Option<Diagnostic>,
}

fn stop(request: &Request, deadline: Instant) -> Option<(WorkerOutcome, Diagnostic)> {
    observe_stop(request, deadline, || request.control.checkpoint())
}

fn observe_stop(
    request: &Request,
    deadline: Instant,
    checkpoint: impl FnOnce() -> DiagnosticResult<()>,
) -> Option<(WorkerOutcome, Diagnostic)> {
    // Drop publishes abandonment before cancelling control. Sample control
    // first, then the flag, so Drop's wakeup cannot be mistaken for cancellation.
    let stopped = checkpoint();
    if request.abandoned.load(Ordering::Acquire) {
        return Some((
            WorkerOutcome::Interrupted,
            Diagnostic::formatted(
                BWErr::Cancelled,
                format_args!("Worker invocation abandoned; completed effects are not rolled back"),
            ),
        ));
    }
    if let Err(error) = stopped {
        let outcome = if error.code() == super::super::diagnostic::DiagnosticCode::Cancelled {
            WorkerOutcome::Cancelled
        } else {
            WorkerOutcome::TimedOut
        };
        return Some((outcome, error));
    }
    (Instant::now() >= deadline).then(|| {
        (
            WorkerOutcome::TimedOut,
            Diagnostic::formatted(
                BWErr::Timeout,
                format_args!("Isolated worker deadline expired"),
            ),
        )
    })
}

fn set_failure(report: &mut WorkerReport, outcome: WorkerOutcome, error: Diagnostic) {
    report.outcome = outcome;
    report.diagnostic = Some(match report.diagnostic.take() {
        Some(original) => error.while_handling(original),
        None => error,
    });
}

fn io_failure(error: io::Error) -> Diagnostic {
    runtime(format_args!("Worker I/O or cleanup failed: {error}"))
}

pub(super) fn supervise(
    id: u64,
    specification: WorkerCommand,
    input: RetainedInput,
    request: Arc<Request>,
    deadline: Instant,
    shared: Arc<Shared>,
    send: WorkerDelivery,
) {
    observation::supervise(id, specification, input, request, deadline, shared, send);
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point {
    Setup,
    Write,
    WriteComplete,
    Read,
    ReadComplete,
    Observe,
    Terminate,
    Reap,
    Close,
    // Only a guardian has a control channel.
    #[cfg(unix)]
    ControlClose,
    Drop,
}
#[cfg(test)]
pub(super) type IoHook = (Point, Box<dyn FnOnce(u32) + Send>);

#[cfg(test)]
fn append_cleanup(report: &mut WorkerReport, error: io::Error) {
    append_cause(report, io_failure(error));
}

fn append_cause(report: &mut WorkerReport, cause: Diagnostic) {
    if let Some(primary) = report.diagnostic.take() {
        report.diagnostic = Some(primary.while_handling(cause));
    } else {
        // A Pending report may already own the original diagnostic. Cleanup
        // reconciliation must preserve that published terminal outcome.
        if report.outcome == WorkerOutcome::Succeeded {
            report.outcome = WorkerOutcome::Failed;
        }
        report.diagnostic = Some(cause);
    }
}
