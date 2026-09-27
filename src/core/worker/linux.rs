use super::*;
use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
};

mod launch;
mod observation;
mod owner;
#[cfg(test)]
pub(super) type LaunchHook = Box<dyn FnOnce(WorkerCommand) -> io::Result<ChildOwner> + Send>;

const QUANTUM: Duration = Duration::from_millis(5);
#[cfg(test)]
mod tests;
const CHUNK: usize = 4096;

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY: the borrowed pipe owns a live descriptor throughout both calls.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) struct ChildOwner {
    child: Child,
    owned: bool,
    #[cfg(test)]
    observer: Option<Arc<observation::Observation>>,
}
impl ChildOwner {
    fn exited(&self) -> io::Result<bool> {
        // WNOWAIT keeps the PID reserved until process-group cleanup is requested.
        // SAFETY: zero is a valid empty siginfo_t; waitid initializes the result.
        let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut information,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { information.si_pid() } != 0)
    }

    fn terminate(&mut self) -> io::Result<()> {
        if !self.owned {
            return Ok(());
        }
        // SAFETY: this is our unreaped child and initial process-group leader.
        // Never signal by this numeric PID after releasing child ownership.
        let result = unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
        let group_error = (result == -1)
            .then(io::Error::last_os_error)
            .filter(|error| error.raw_os_error() != Some(libc::ESRCH));
        // Also stop the direct child if it moved itself out of the initial group.
        let direct = self.child.kill().or_else(|error| {
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(error)
            }
        });
        match group_error {
            Some(error) => Err(error),
            None => direct,
        }
    }

    fn reap(&mut self) -> io::Result<Option<ExitStatus>> {
        let result = self.child.try_wait()?;
        if result.is_some() {
            self.owned = false;
        }
        Ok(result)
    }
}

impl Drop for ChildOwner {
    fn drop(&mut self) {
        if self.owned {
            #[cfg(test)]
            if let Some(observer) = self.observer.take() {
                observer.hook(Point::Drop, self.child.id());
            }
            let _ = self.terminate();
            // Only the owned OS thread reach this fallback.
            // Its capacity stays retained if the kernel cannot finish reaping.
            let _ = self.child.wait();
        }
    }
}

fn stop(request: &Request, deadline: Instant) -> Option<(WorkerOutcome, Diagnostic)> {
    if request.abandoned.load(Ordering::Acquire) {
        return Some((
            WorkerOutcome::Interrupted,
            Diagnostic::formatted(
                BWErr::Cancelled,
                format_args!("Worker invocation abandoned; completed effects are not rolled back"),
            ),
        ));
    }
    if let Err(error) = request.control.checkpoint() {
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
