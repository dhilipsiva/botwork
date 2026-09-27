//! In-memory observation never holds a lock across a worker OS operation.
use super::*;
use std::sync::MutexGuard;

struct Flight {
    report: WorkerReport,
    pid: Option<u32>,
    cleanup_started: Option<Instant>,
    observed_stop: bool,
    published: bool,
    finished: bool,
}

pub(super) struct Observation {
    pub shared: Arc<Shared>,
    pub request: Arc<Request>,
    deadline: Instant,
    flight: Mutex<Flight>,
}

#[derive(Clone, Copy)]
pub(super) struct Status {
    pub stopped: bool,
    pub expired: bool,
    pub published: bool,
}

#[derive(Clone, Copy)]
pub(super) enum Stream {
    Stdout,
    Stderr,
}

impl Observation {
    fn lock(&self) -> MutexGuard<'_, Flight> {
        self.flight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
    fn observe(&self, flight: &mut Flight) {
        if !flight.published && !flight.observed_stop {
            if let Some((outcome, error)) = stop(&self.request, self.deadline) {
                set_failure(&mut flight.report, outcome, error);
                flight.observed_stop = true;
                flight.cleanup_started.get_or_insert_with(Instant::now);
                self.shared.update(flight.report.id, flight.pid, true, None);
            }
        }
    }
    pub fn status(&self) -> Status {
        let mut flight = self.lock();
        self.observe(&mut flight);
        Status {
            stopped: flight.report.outcome != WorkerOutcome::Succeeded,
            expired: flight
                .cleanup_started
                .is_some_and(|start| start.elapsed() >= self.shared.limits.cleanup_timeout),
            published: flight.published,
        }
    }
    pub fn started(&self, pid: u32) {
        let mut flight = self.lock();
        self.observe(&mut flight);
        flight.pid = Some(pid);
        self.shared.update(
            flight.report.id,
            Some(pid),
            flight.report.outcome != WorkerOutcome::Succeeded,
            flight.published.then_some(WorkerCleanup::Pending),
        );
    }
    pub fn cleanup(&self) {
        let mut flight = self.lock();
        self.observe(&mut flight);
        flight.cleanup_started.get_or_insert_with(Instant::now);
        self.shared.update(
            flight.report.id,
            flight.pid,
            true,
            flight.published.then_some(WorkerCleanup::Pending),
        );
    }
    pub fn error(&self, error: Diagnostic) {
        let mut flight = self.lock();
        self.observe(&mut flight);
        if !flight.published {
            append_cause(&mut flight.report, error);
        }
        flight.cleanup_started.get_or_insert_with(Instant::now);
    }
    pub fn wrote(&self, total: usize) {
        self.lock().report.stdin_written = total;
    }
    pub fn reaped(&self, status: ExitStatus) {
        self.lock().report.exit_status = Some(status);
    }
    pub fn remaining(&self, stream: Stream) -> usize {
        let flight = self.lock();
        match stream {
            Stream::Stdout => self.shared.limits.stdout_bytes - flight.report.stdout.len(),
            Stream::Stderr => self.shared.limits.stderr_bytes - flight.report.stderr.len(),
        }
    }
    pub fn capture(&self, stream: Stream, bytes: &[u8]) -> DiagnosticResult<()> {
        let mut flight = self.lock();
        if flight.published {
            return Ok(());
        }
        let (output, maximum, resource) = match stream {
            Stream::Stdout => (
                &mut flight.report.stdout,
                self.shared.limits.stdout_bytes,
                "worker stdout bytes",
            ),
            Stream::Stderr => (
                &mut flight.report.stderr,
                self.shared.limits.stderr_bytes,
                "worker stderr bytes",
            ),
        };
        let allowed = bytes.len().min(maximum - output.len());
        output.extend_from_slice(&bytes[..allowed]);
        if allowed == bytes.len() {
            Ok(())
        } else {
            Err(limit(resource, maximum))
        }
    }
    pub fn finish(&self, cleanup: WorkerCleanup, io_complete: bool) {
        let mut flight = self.lock();
        self.observe(&mut flight);
        if matches!(cleanup, WorkerCleanup::Reaped | WorkerCleanup::TreeReaped)
            && !io_complete
            && flight.report.outcome == WorkerOutcome::Succeeded
        {
            append_cause(
                &mut flight.report,
                runtime(format_args!("Worker cleanup ended with incomplete I/O")),
            );
        }
        flight.report.cleanup = cleanup;
        flight.report.progress_complete = cleanup != WorkerCleanup::Unverified;
        flight.report.io_complete =
            io_complete && !flight.published && flight.report.progress_complete;
        flight.finished = true;
    }
    pub fn lost_ownership(&self, error: Diagnostic) {
        let mut flight = self.lock();
        self.observe(&mut flight);
        if !flight.published {
            if flight.report.outcome == WorkerOutcome::Succeeded {
                flight.report.outcome = WorkerOutcome::Interrupted;
            }
            append_cause(&mut flight.report, error);
        }
        flight.cleanup_started.get_or_insert_with(Instant::now);
        self.shared.update(
            flight.report.id,
            flight.pid,
            true,
            flight.published.then_some(WorkerCleanup::Pending),
        );
    }
    pub fn unverified(&self, error: Diagnostic) {
        self.lost_ownership(error);
        self.finish(WorkerCleanup::Unverified, false);
    }
    fn take_report(flight: &mut Flight, cleanup: WorkerCleanup) -> WorkerReport {
        WorkerReport {
            id: flight.report.id,
            outcome: flight.report.outcome,
            cleanup,
            exit_status: flight.report.exit_status,
            stdin_written: flight.report.stdin_written,
            stdout: std::mem::take(&mut flight.report.stdout),
            stderr: std::mem::take(&mut flight.report.stderr),
            diagnostic: flight.report.diagnostic.take(),
            io_complete: flight.report.io_complete,
            progress_complete: flight.report.progress_complete,
        }
    }
    fn watch(&self, mut delivery: WorkerDelivery) {
        loop {
            let (finished, report) = {
                let mut flight = self.lock();
                self.observe(&mut flight);
                if flight.finished {
                    self.shared.finish(&flight.report);
                    let cleanup = flight.report.cleanup;
                    let report =
                        (!flight.published).then(|| Self::take_report(&mut flight, cleanup));
                    (true, report)
                } else if !flight.published
                    && flight
                        .cleanup_started
                        .is_some_and(|start| start.elapsed() >= self.shared.limits.cleanup_timeout)
                {
                    if flight.report.outcome == WorkerOutcome::Succeeded {
                        append_cause(
                            &mut flight.report,
                            runtime(format_args!(
                                "Worker cleanup allowance expired; completion is unverified"
                            )),
                        );
                    }
                    flight.report.progress_complete = false;
                    flight.report.io_complete = false;
                    flight.published = true;
                    self.shared.update(
                        flight.report.id,
                        flight.pid,
                        true,
                        Some(WorkerCleanup::Pending),
                    );
                    (
                        false,
                        Some(Self::take_report(&mut flight, WorkerCleanup::Pending)),
                    )
                } else {
                    (false, None)
                }
            };
            // Receiver wakeups can run host code; never invoke one under the state lock.
            if let Some(report) = report {
                if let Some(sender) = delivery.send.take() {
                    drop(sender.send(RetainedReport {
                        report,
                        _retention: if finished {
                            delivery.retention.take()
                        } else {
                            delivery.retention.clone()
                        },
                    }));
                }
            }
            if finished {
                break;
            }
            std::thread::sleep(QUANTUM);
        }
    }
    #[cfg(test)]
    pub fn hook(&self, point: Point, pid: u32) {
        let callback = {
            let mut hook = self.shared.io_hooks.lock().unwrap();
            if hook.front().is_some_and(|(at, _)| *at == point) {
                hook.pop_front().map(|(_, callback)| callback)
            } else {
                None
            }
        };
        if let Some(callback) = callback {
            callback(pid);
        }
    }
}

pub(super) fn supervise(
    id: u64,
    specification: WorkerCommand,
    input: RetainedInput,
    request: Arc<Request>,
    deadline: Instant,
    shared: Arc<Shared>,
    delivery: WorkerDelivery,
) {
    let observation = Arc::new(Observation {
        shared,
        request,
        deadline,
        flight: Mutex::new(Flight {
            report: WorkerReport {
                id,
                outcome: WorkerOutcome::Succeeded,
                cleanup: WorkerCleanup::Pending,
                exit_status: None,
                stdin_written: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
                io_complete: false,
                progress_complete: false,
                diagnostic: None,
            },
            pid: None,
            cleanup_started: None,
            observed_stop: false,
            published: false,
            finished: false,
        }),
    });
    let owner = observation.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("botwork-os-{id}"))
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::owner::run(specification, input, &owner);
            }));
            if result.is_err() {
                owner.unverified(runtime(format_args!(
                    "Worker OS owner panicked; cleanup is unverified"
                )));
            }
        });
    if let Err(error) = spawned {
        observation.error(runtime(format_args!(
            "Starting worker OS owner failed: {error}"
        )));
        observation.finish(WorkerCleanup::NotStarted, false);
    }
    observation.watch(delivery);
}

#[cfg(test)]
mod tests;
