//! `--listener PROGRAM`: the invocation's execution events as JSON Lines on a
//! listener process's stdin, delivered through a bounded dispatcher.
use super::{BWErr, CliError, Diagnostic};
use botwork::core::{
    acceptance::{CaseStatus, CaseTotals, RunVerdict},
    listener::{Dispatcher, Listener, ListenerOptions, ListenerSender},
    report::{timestamp, ErrorRecord, EventObserver, EventRecord},
};
use serde::Serialize;
use std::{
    ffi::OsString,
    io::{self, BufWriter, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, SystemTime},
};

pub(super) const FORMAT: &str = "botwork-events";
pub(super) const VERSION: u32 = 1;

/// Stream framing and suite fixture outcomes, alongside run events.
#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(super) enum StreamNotice {
    StreamStarted {
        format: &'static str,
        version: u32,
        mode: &'static str,
        started_at: String,
    },
    FixtureFinished {
        suite: String,
        status: CaseStatus,
        error: Option<ErrorRecord>,
    },
    /// The execution outcome of runs and fixtures, before report and listener
    /// delivery finish.
    StreamFinished {
        finished_at: String,
        status: CaseStatus,
        cases: CaseTotals,
        fixture_failures: usize,
    },
}

pub(super) enum Notice {
    Run { run: usize, event: EventRecord },
    Stream(StreamNotice),
}

/// One JSON line: its position in the stream, then the notice. Statement events
/// have their own `index`, so the stream position has a distinct name.
#[derive(Serialize)]
#[serde(untagged)]
enum Line<'a> {
    Run {
        position: u64,
        run: usize,
        #[serde(flatten)]
        event: &'a EventRecord,
    },
    Stream {
        position: u64,
        #[serde(flatten)]
        notice: &'a StreamNotice,
    },
}

fn child(child: &Mutex<Child>) -> MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(|error| error.into_inner())
}

struct Process {
    stdin: Option<BufWriter<ChildStdin>>,
    child: Arc<Mutex<Child>>,
    position: u64,
}

impl Listener<Notice> for Process {
    fn deliver(&mut self, notice: &Notice) -> Result<(), String> {
        let position = self.position;
        let line = match notice {
            Notice::Run { run, event } => Line::Run {
                position,
                run: *run,
                event,
            },
            Notice::Stream(notice) => Line::Stream { position, notice },
        };
        let stdin = self.stdin.as_mut().ok_or("stdin is closed")?;
        serde_json::to_writer(&mut *stdin, &line)
            .map_err(io::Error::from)
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("writing event {position}: {error}"))?;
        self.position += 1;
        Ok(())
    }

    /// Close stdin so the listener sees the end of the stream, then wait for it.
    fn finish(&mut self) -> Result<(), String> {
        drop(self.stdin.take());
        loop {
            // Release the lock before sleeping so a timed-out close can kill.
            let status = child(&self.child).try_wait();
            match status {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return Err(format!("listener exited with {status}")),
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(error) => return Err(format!("waiting for listener: {error}")),
            }
        }
    }
}

pub(super) struct Hub {
    label: String,
    sender: ListenerSender<Notice>,
    dispatcher: Mutex<Option<Dispatcher<Notice>>>,
    child: Arc<Mutex<Child>>,
}

impl Hub {
    /// Start `program` without a shell and queue the stream header.
    pub(super) fn begin(
        program: &Path,
        arguments: &[OsString],
        options: ListenerOptions,
        mode: &'static str,
    ) -> Result<Self, CliError> {
        let label = format!("Listener {}", program.display());
        let error = |cause: &dyn std::fmt::Display| -> CliError {
            Diagnostic::new(BWErr::OutputError(format!("{label}: {cause}"))).into()
        };
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(Stdio::piped())
            // Script output owns stdout; listener output joins the console report.
            .stdout(Stdio::from(io::stderr()));
        // Its own group, so a timeout also stops processes the listener started.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut process = command.spawn().map_err(|cause| error(&cause))?;
        let stdin = process.stdin.take().expect("piped stdin");
        let child = Arc::new(Mutex::new(process));
        let listener = Process {
            stdin: Some(BufWriter::new(stdin)),
            child: Arc::clone(&child),
            position: 0,
        };
        let dispatcher = match Dispatcher::spawn(listener, options) {
            Ok(dispatcher) => dispatcher,
            Err(cause) => {
                let mut process = self::child(&child);
                let _ = process.kill();
                let _ = process.wait();
                return Err(error(&cause));
            }
        };
        let hub = Self {
            label,
            sender: dispatcher.sender(),
            dispatcher: Mutex::new(Some(dispatcher)),
            child,
        };
        hub.sender.send(Notice::Stream(StreamNotice::StreamStarted {
            format: FORMAT,
            version: VERSION,
            mode,
            started_at: timestamp(SystemTime::now()),
        }));
        Ok(hub)
    }

    /// Forward run `number`'s events as they are recorded.
    pub(super) fn observer(&self, number: usize) -> EventObserver {
        let sender = self.sender.clone();
        EventObserver::new(move |event| {
            sender.send(Notice::Run {
                run: number,
                event: event.clone(),
            });
        })
    }

    /// Forward a run event recorded outside a live recording, such as a skip.
    pub(super) fn event(&self, number: usize, event: EventRecord) {
        self.sender.send(Notice::Run { run: number, event });
    }

    pub(super) fn fixture(&self, suite: &str, status: CaseStatus, error: Option<ErrorRecord>) {
        self.sender
            .send(Notice::Stream(StreamNotice::FixtureFinished {
                suite: suite.to_owned(),
                status,
                error,
            }));
    }

    /// Queue the trailer when the execution outcome is known, then close and
    /// wait for the listener. Blocks up to the close timeout; later calls do nothing.
    pub(super) fn finish(&self, verdict: Option<&RunVerdict>) -> Result<(), CliError> {
        let Some(dispatcher) = self
            .dispatcher
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        else {
            return Ok(());
        };
        if let Some(verdict) = verdict {
            self.sender
                .send(Notice::Stream(StreamNotice::StreamFinished {
                    finished_at: timestamp(SystemTime::now()),
                    status: verdict.status(),
                    cases: *verdict.cases(),
                    fixture_failures: verdict.fixture_failures(),
                }));
        }
        let outcome = dispatcher.close();
        if !outcome.finished {
            // Unblock the detached delivery thread and reap the listener.
            let mut process = child(&self.child);
            if let Ok(None) = process.try_wait() {
                // The leader is unreaped under this lock, so its group ID has not
                // been reused.
                #[cfg(target_os = "linux")]
                unsafe {
                    libc::kill(-(process.id() as libc::pid_t), libc::SIGKILL);
                }
                let _ = process.kill();
            }
            let _ = process.wait();
        }
        match outcome.failure {
            None => Ok(()),
            Some(failure) => Err(Diagnostic::new(BWErr::OutputError(format!(
                "{}: {failure}; {} of {} events delivered",
                self.label, outcome.delivered, outcome.accepted
            )))
            .into()),
        }
    }
}
