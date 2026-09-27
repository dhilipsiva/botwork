use super::*;
use std::{
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
};

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

struct ChildOwner {
    child: Child,
    owned: bool,
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
            let _ = self.terminate();
            // Only the dedicated supervisor thread can reach this fallback.
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

fn read_pipe(
    pipe: &mut Option<impl Read>,
    bytes: &mut Vec<u8>,
    limit_bytes: usize,
    resource: &'static str,
) -> DiagnosticResult<bool> {
    let Some(reader) = pipe.as_mut() else {
        return Ok(true);
    };
    let mut buffer = [0; CHUNK];
    // Read at most one excess byte, detecting overflow before growing the buffer.
    let size = CHUNK.min(limit_bytes.saturating_sub(bytes.len()).saturating_add(1));
    match reader.read(&mut buffer[..size]) {
        Ok(0) => {
            pipe.take();
            Ok(true)
        }
        Ok(count) => {
            let allowed = count.min(limit_bytes - bytes.len());
            bytes.extend_from_slice(&buffer[..allowed]);
            if allowed < count {
                Err(limit(resource, limit_bytes))
            } else {
                Ok(false)
            }
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(io_failure(error)),
    }
}

pub(super) fn supervise(
    id: u64,
    specification: WorkerCommand,
    input: Vec<u8>,
    request: Arc<Request>,
    deadline: Instant,
    shared: Arc<Shared>,
    send: oneshot::Sender<WorkerReport>,
) {
    let mut send = Some(send);
    if let Some((outcome, error)) = stop(&request, deadline) {
        deliver(
            WorkerReport::failure(id, outcome, WorkerCleanup::NotStarted, error),
            &shared,
            &mut send,
        );
        return;
    }
    let spawn = Command::new(&specification.executable)
        .args(&specification.arguments)
        .current_dir(&specification.directory)
        .env_clear()
        .envs(&specification.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();
    drop(specification);
    let child = match spawn {
        Ok(child) => child,
        Err(error) => {
            let mut report = WorkerReport::failure(
                id,
                WorkerOutcome::Failed,
                WorkerCleanup::NotStarted,
                runtime(format_args!("Starting isolated worker failed: {error}")),
            );
            if let Some((outcome, error)) = stop(&request, deadline) {
                set_failure(&mut report, outcome, error);
            }
            deliver(report, &shared, &mut send);
            return;
        }
    };
    let mut child = ChildOwner { child, owned: true };
    let pid = child.child.id();
    shared.update(id, Some(pid), false, None);
    let mut stdin = child.child.stdin.take();
    let mut stdout = child.child.stdout.take();
    let mut stderr = child.child.stderr.take();
    let setup = nonblocking(stdin.as_ref().expect("piped stdin"))
        .and_then(|()| nonblocking(stdout.as_ref().expect("piped stdout")))
        .and_then(|()| nonblocking(stderr.as_ref().expect("piped stderr")));
    let mut report = WorkerReport {
        id,
        outcome: WorkerOutcome::Succeeded,
        cleanup: WorkerCleanup::Pending,
        exit_status: None,
        stdin_written: 0,
        stdout: Vec::new(),
        stderr: Vec::new(),
        io_complete: false,
        diagnostic: None,
    };
    let mut cleanup_started = None;
    let mut signalled = false;
    let mut observed_stop = false;
    let mut output_failed = false;
    if let Err(error) = setup {
        set_failure(&mut report, WorkerOutcome::Failed, io_failure(error));
        stdout.take();
        stderr.take();
        stdin.take();
        output_failed = true;
    }
    loop {
        if !observed_stop {
            if let Some((outcome, error)) = stop(&request, deadline) {
                set_failure(&mut report, outcome, error);
                observed_stop = true;
            }
        }
        if report.diagnostic.is_some() {
            stdin.take();
            cleanup_started.get_or_insert_with(Instant::now);
        }
        if !signalled && report.diagnostic.is_some() {
            shared.update(id, Some(pid), true, None);
            if let Err(error) = child.terminate() {
                append_cleanup(&mut report, error);
            }
            signalled = true;
        }
        if let Some(writer) = stdin.as_mut() {
            let end = input.len().min(report.stdin_written.saturating_add(CHUNK));
            if report.stdin_written == input.len() {
                stdin.take();
            } else {
                match writer.write(&input[report.stdin_written..end]) {
                    Ok(0) => set_failure(
                        &mut report,
                        WorkerOutcome::Failed,
                        io_failure(io::ErrorKind::WriteZero.into()),
                    ),
                    Ok(count) => report.stdin_written += count,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => {
                        set_failure(&mut report, WorkerOutcome::Failed, io_failure(error))
                    }
                }
            }
        }
        // Each stream gets one bounded read per turn, so floods cannot starve stops.
        for result in [
            read_pipe(
                &mut stdout,
                &mut report.stdout,
                shared.limits.stdout_bytes,
                "worker stdout bytes",
            ),
            read_pipe(
                &mut stderr,
                &mut report.stderr,
                shared.limits.stderr_bytes,
                "worker stderr bytes",
            ),
        ] {
            if let Err(error) = result {
                if report.diagnostic.is_none() {
                    set_failure(&mut report, WorkerOutcome::Failed, error);
                }
                output_failed = true;
                stdout.take();
                stderr.take();
            }
        }
        if child.owned {
            match child.exited() {
                Ok(true) => {
                    // Signal inherited descendants BEFORE reaping the group leader.
                    if !signalled {
                        if let Err(error) = child.terminate() {
                            append_cleanup(&mut report, error);
                        }
                        signalled = true;
                    }
                    match child.reap() {
                        Ok(Some(status)) => {
                            report.exit_status = Some(status);
                            cleanup_started.get_or_insert_with(Instant::now);
                            if !status.success() && report.diagnostic.is_none() && send.is_some() {
                                set_failure(
                                    &mut report,
                                    WorkerOutcome::Failed,
                                    Diagnostic::formatted(
                                        BWErr::NativeError,
                                        format_args!("Isolated worker exited with {status}"),
                                    ),
                                );
                            }
                            if report.stdin_written != input.len()
                                && report.diagnostic.is_none()
                                && send.is_some()
                            {
                                set_failure(
                                    &mut report,
                                    WorkerOutcome::Failed,
                                    runtime(format_args!(
                                        "Worker exited before accepting its complete request"
                                    )),
                                );
                            }
                            stdin.take();
                        }
                        Ok(None) => {}
                        Err(error) => {
                            lost_ownership(&mut child, &mut report, error);
                            break;
                        }
                    }
                }
                Ok(false) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    lost_ownership(&mut child, &mut report, error);
                    break;
                }
            }
        }
        if !child.owned && stdout.is_none() && stderr.is_none() {
            report.cleanup = WorkerCleanup::Reaped;
            report.io_complete = !output_failed && report.stdin_written == input.len();
            if !observed_stop {
                if let Some((outcome, error)) = stop(&request, deadline) {
                    set_failure(&mut report, outcome, error);
                }
            }
            break;
        }
        if cleanup_started.is_some_and(|start| start.elapsed() >= shared.limits.cleanup_timeout) {
            stdout.take();
            stderr.take();
            stdin.take();
            output_failed = true;
            if report.diagnostic.is_none() && send.is_some() {
                set_failure(
                    &mut report,
                    WorkerOutcome::Failed,
                    runtime(format_args!(
                        "Worker cleanup allowance expired; output is incomplete"
                    )),
                );
            }
            if !child.owned {
                report.cleanup = WorkerCleanup::Reaped;
                break;
            }
            if let Some(send) = send.take() {
                shared.update(id, Some(pid), true, Some(WorkerCleanup::Pending));
                // Publish the interrupted outcome, retaining child ownership and capacity.
                let early = WorkerReport {
                    id,
                    outcome: report.outcome,
                    cleanup: WorkerCleanup::Pending,
                    exit_status: report.exit_status,
                    stdin_written: report.stdin_written,
                    stdout: std::mem::take(&mut report.stdout),
                    stderr: std::mem::take(&mut report.stderr),
                    io_complete: false,
                    diagnostic: report.diagnostic.take(),
                };
                let _ = send.send(early);
                // Do not revisit stop priority or change the already published outcome.
                observed_stop = true;
            }
        }
        std::thread::sleep(QUANTUM);
    }
    deliver(report, &shared, &mut send);
}

fn append_cleanup(report: &mut WorkerReport, error: io::Error) {
    let cause = io_failure(error);
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

fn lost_ownership(child: &mut ChildOwner, report: &mut WorkerReport, error: io::Error) {
    // Another host reaper or SIGCHLD policy invalidated ownership. Never risk
    // signalling a recycled PID; keep the capacity slot quarantined explicitly.
    child.owned = false;
    report.cleanup = WorkerCleanup::Unverified;
    if report.outcome == WorkerOutcome::Succeeded {
        report.outcome = WorkerOutcome::Interrupted;
    }
    append_cleanup(report, error);
}

fn deliver(
    report: WorkerReport,
    shared: &Shared,
    send: &mut Option<oneshot::Sender<WorkerReport>>,
) {
    shared.finish(&report);
    if let Some(send) = send.take() {
        let _ = send.send(report);
    }
}
