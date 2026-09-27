//! Observe startup stops independently of the potentially stalled OS launch.
use super::*;
use std::sync::mpsc::{self, TryRecvError};

pub(super) struct Started {
    pub child: ChildOwner,
    pub report: WorkerReport,
    pub cleanup_started: Option<Instant>,
    pub observed_stop: bool,
}

// The child guard is destroyed before the capacity/retention owner if delivery
// fails. std::process::Child alone does not terminate or reap on drop.
struct LaunchResult {
    result: io::Result<ChildOwner>,
    _shared: Arc<Shared>,
}

pub(super) fn spawn(specification: WorkerCommand) -> io::Result<ChildOwner> {
    Command::new(&specification.executable)
        .args(&specification.arguments)
        .current_dir(&specification.directory)
        .env_clear()
        .envs(&specification.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map(|child| ChildOwner { child, owned: true })
}

pub(super) fn wait(
    id: u64,
    specification: WorkerCommand,
    request: &Arc<Request>,
    deadline: Instant,
    shared: &Arc<Shared>,
    delivery: &mut WorkerDelivery,
    launcher: impl FnOnce(WorkerCommand) -> io::Result<ChildOwner> + Send + 'static,
) -> Option<Started> {
    let (send, receive) = mpsc::sync_channel(1);
    let owner = Arc::clone(shared);
    let launch_request = Arc::clone(request);
    let launched = std::thread::Builder::new()
        .name(format!("botwork-launch-{id}"))
        .spawn(move || {
            let result = LaunchResult {
                result: if stop(&launch_request, deadline).is_some() {
                    Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Worker stopped before OS process creation",
                    ))
                } else {
                    launcher(specification)
                },
                _shared: owner,
            };
            // Failed send drops the child guard and then the retained slot owner.
            drop(send.send(result));
        });
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
    let mut observed_stop = false;
    if let Err(error) = launched {
        observe_stop(
            &mut report,
            request,
            deadline,
            &mut cleanup_started,
            &mut observed_stop,
            shared,
        );
        append_cause(
            &mut report,
            runtime(format_args!("Starting worker launcher failed: {error}")),
        );
        report.cleanup = WorkerCleanup::NotStarted;
        deliver(report, shared, delivery);
        return None;
    }
    loop {
        observe_stop(
            &mut report,
            request,
            deadline,
            &mut cleanup_started,
            &mut observed_stop,
            shared,
        );
        match receive.try_recv() {
            Ok(LaunchResult {
                result: Ok(child), ..
            }) => {
                return Some(Started {
                    child,
                    report,
                    cleanup_started,
                    observed_stop,
                });
            }
            Ok(LaunchResult {
                result: Err(error), ..
            }) => {
                append_cause(
                    &mut report,
                    runtime(format_args!("Starting isolated worker failed: {error}")),
                );
                report.cleanup = WorkerCleanup::NotStarted;
                deliver(report, shared, delivery);
                return None;
            }
            Err(TryRecvError::Disconnected) => {
                // A panicking launcher leaves process creation uncertain. Never
                // advertise unused capacity or signal a guessed numeric PID.
                if report.outcome == WorkerOutcome::Succeeded {
                    report.outcome = WorkerOutcome::Interrupted;
                }
                append_cleanup(
                    &mut report,
                    io::Error::other("Worker launcher ended without transferring child ownership"),
                );
                report.cleanup = WorkerCleanup::Unverified;
                deliver(report, shared, delivery);
                return None;
            }
            Err(TryRecvError::Empty) => {}
        }
        if cleanup_started.is_some_and(|start| start.elapsed() >= shared.limits.cleanup_timeout) {
            if let Some(sender) = delivery.send.take() {
                shared.update(id, None, true, Some(WorkerCleanup::Pending));
                let early = WorkerReport {
                    id,
                    outcome: report.outcome,
                    cleanup: WorkerCleanup::Pending,
                    exit_status: None,
                    stdin_written: 0,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    io_complete: false,
                    diagnostic: report.diagnostic.take(),
                };
                drop(sender.send(RetainedReport {
                    report: early,
                    _retention: delivery.retention.clone(),
                }));
            }
        }
        // The supervisor owns the slot and request until the launcher returns.
        // It never joins a thread stuck in process creation on the caller's path.
        std::thread::sleep(QUANTUM);
    }
}

fn observe_stop(
    report: &mut WorkerReport,
    request: &Request,
    deadline: Instant,
    cleanup_started: &mut Option<Instant>,
    observed_stop: &mut bool,
    shared: &Shared,
) {
    if !*observed_stop {
        if let Some((outcome, error)) = stop(request, deadline) {
            set_failure(report, outcome, error);
            *observed_stop = true;
            *cleanup_started = Some(Instant::now());
            shared.update(report.id, None, true, None);
        }
    }
}

#[cfg(test)]
mod tests;
