use super::*;
use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
};

// Process-tree guardians and PID namespaces need Linux (decision D12).
#[cfg(target_os = "linux")]
pub(super) mod guardian;
mod launch;
#[cfg(target_os = "linux")]
mod namespace;
mod observation;
mod owner;
mod process;
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
    child: process::Process,
    owned: bool,
    guardian: Option<std::os::unix::net::UnixStream>,
    #[cfg(test)]
    observer: Option<Arc<observation::Observation>>,
}
struct Completion {
    status: Option<ExitStatus>,
    cleanup: WorkerCleanup,
    error: Option<Diagnostic>,
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
        if self.child.namespaced() {
            // Killing this known namespace init asks the kernel to terminate
            // its entire namespace, even when the guardian cannot cooperate.
            return self.child.kill().or_else(|error| {
                if error.raw_os_error() == Some(libc::ESRCH) {
                    Ok(())
                } else {
                    Err(error)
                }
            });
        }
        if let Some(control) = &self.guardian {
            // Keep the guardian alive to reap its descendants, including detached ones.
            return control
                .shutdown(std::net::Shutdown::Write)
                .or_else(|error| {
                    if error.kind() == io::ErrorKind::NotConnected {
                        Ok(())
                    } else {
                        Err(error)
                    }
                });
        }
        // SAFETY: this is our unreaped child and initial process-group leader.
        // Never signal by this numeric PID after releasing child ownership.
        let result = unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
        let group_error = (result == -1)
            .then(io::Error::last_os_error)
            .filter(|error| !self.nothing_left(error));
        // Also stop the direct child if it moved itself out of the initial group.
        let direct = self.child.kill().or_else(|error| {
            if self.nothing_left(&error) {
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

    /// Whether a failed signal found nothing left to stop: no such process, or
    /// on macOS, which refuses to signal zombies, only the exited child that
    /// [`Self::exited`] holds for reaping.
    fn nothing_left(&self, error: &io::Error) -> bool {
        match error.raw_os_error() {
            Some(libc::ESRCH) => true,
            Some(libc::EPERM) if cfg!(target_os = "macos") => self.exited().unwrap_or(false),
            _ => false,
        }
    }

    fn reap(&mut self) -> io::Result<Option<Completion>> {
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        self.owned = false;
        #[cfg(target_os = "linux")]
        if let Some(control) = self.guardian.as_mut() {
            return match guardian::completion(control, status) {
                Ok(completion) => Ok(Some(completion)),
                Err(error) if self.child.namespaced() => Ok(Some(Completion {
                    status: None,
                    cleanup: WorkerCleanup::NamespaceReaped,
                    error: Some(runtime(format_args!(
                        "Namespace guardian ended without verified worker completion: {error}"
                    ))),
                })),
                Err(error) => Err(error),
            };
        }
        Ok(Some(Completion {
            status: Some(status),
            cleanup: WorkerCleanup::Reaped,
            error: None,
        }))
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
            // Only the owned OS thread reaches this fallback.
            // Its capacity stays retained if the kernel cannot finish reaping.
            let _ = self.child.wait();
        }
    }
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
