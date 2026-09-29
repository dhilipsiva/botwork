//! Run and statement results. Each run produces one ordered event stream, which
//! folds into a bounded, versioned [`RunRecord`]. Report formats and listeners
//! build on these types; outcomes reuse the [acceptance](super::acceptance) vocabulary.
use super::{
    acceptance::{
        CaseCompletion, CaseExpectation, CaseStatus, FailureKind, PhaseOutcome, SkipReason,
    },
    ast::Span,
    diagnostic::Diagnostic,
};
use serde::Serialize;
use std::{
    fmt::{self, Write},
    sync::{Arc, Mutex},
    time::{Instant, SystemTime},
};

#[cfg(test)]
mod tests;

/// The `format` field of every serialized [`RunRecord`].
pub const RECORD_FORMAT: &str = "botwork-run";
/// Incremented for incompatible record changes; additions keep the version.
pub const RECORD_VERSION: u32 = 1;
const TRUNCATED: &str = "…[truncated]";

/// Stable run identity: a script name or `suite/case[/row]` ID, a display name,
/// and the dataset row when present.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RunIdentity {
    pub id: String,
    pub name: String,
    pub dataset: Option<String>,
    pub row: Option<String>,
}

impl RunIdentity {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            dataset: None,
            row: None,
        }
    }

    pub fn with_row(mut self, dataset: impl Into<String>, row: impl Into<String>) -> Self {
        self.dataset = Some(dataset.into());
        self.row = Some(row.into());
        self
    }
}

/// A source range with byte offsets and one-based line/Unicode-scalar columns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceLocation {
    pub file: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

impl SourceLocation {
    pub fn from_span(span: &Span) -> Self {
        let (line, column) = span.line_column();
        let (end_line, end_column) = span.end_line_column();
        Self {
            file: span.source().name().to_owned(),
            start_byte: span.start(),
            end_byte: span.end(),
            line,
            column,
            end_line,
            end_column,
        }
    }
}

/// Retention bounds. Work beyond them is counted, never silently dropped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecordLimits {
    /// Top-level statement records per run.
    pub statements: usize,
    /// Log records per run.
    pub logs: usize,
    /// Captured log text per run, in UTF-8 bytes.
    pub log_bytes: usize,
    /// Captured text of one log record, in UTF-8 bytes.
    pub log_record_bytes: usize,
    /// Error message text, in UTF-8 bytes.
    pub message_bytes: usize,
    /// Cause codes recorded for the run error.
    pub causes: usize,
    /// Artifact records per run.
    pub artifacts: usize,
}

impl Default for RecordLimits {
    fn default() -> Self {
        Self {
            statements: 1024,
            logs: 256,
            log_bytes: 256 * 1024,
            log_record_bytes: 4096,
            message_bytes: 4096,
            causes: 16,
            artifacts: 256,
        }
    }
}

/// Opt-in recording for an embedded run.
#[derive(Clone, Debug, Default)]
pub struct RecordOptions {
    /// An empty `id` defaults to the run's source name.
    pub identity: RunIdentity,
    /// Decides the terminal status with the acceptance policy.
    pub expectation: CaseExpectation,
    pub limits: RecordLimits,
}

/// One unhandled error: stable code, derived status, bounded message, location,
/// and the codes of its causes in depth-first order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ErrorRecord {
    pub code: &'static str,
    pub status: CaseStatus,
    pub message: String,
    pub truncated: bool,
    pub location: Option<SourceLocation>,
    pub causes: Vec<&'static str>,
    pub omitted_causes: u64,
}

impl ErrorRecord {
    pub fn from_diagnostic(diagnostic: &Diagnostic, limits: &RecordLimits) -> Self {
        let (message, truncated) = bounded(&diagnostic.error, limits.message_bytes);
        let mut causes = Vec::new();
        let mut omitted = 0u64;
        let mut pending: Vec<&Diagnostic> = diagnostic.causes.iter().rev().collect();
        while let Some(cause) = pending.pop() {
            if causes.len() < limits.causes {
                causes.push(cause.code().as_str());
            } else {
                omitted += 1;
            }
            pending.extend(cause.causes.iter().rev());
        }
        Self {
            code: diagnostic.code().as_str(),
            status: CaseStatus::from_diagnostic_code(diagnostic.code()),
            message,
            truncated,
            location: diagnostic.span.as_ref().map(SourceLocation::from_span),
            causes,
            omitted_causes: omitted,
        }
    }
}

/// A top-level statement. `status` is absent only when the run stopped without
/// finishing the statement; loop iterations and nested statements are not recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StatementRecord {
    pub index: u64,
    pub kind: &'static str,
    pub location: SourceLocation,
    pub offset_us: u64,
    pub duration_us: Option<u64>,
    pub status: Option<CaseStatus>,
    pub code: Option<&'static str>,
}

/// A written Log record: a bounded text prefix and the full UTF-8 byte count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogRecord {
    /// The top-level statement that was running, if any.
    pub statement: Option<u64>,
    pub offset_us: u64,
    pub bytes: u64,
    pub text: String,
    pub truncated: bool,
}

/// A file or resource produced by the run, such as an assertion artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ArtifactRecord {
    pub kind: String,
    pub path: String,
}

/// One observation, in run order. Offsets are microseconds since `RunStarted`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        identity: RunIdentity,
        started_at: String,
    },
    StatementStarted {
        index: u64,
        kind: &'static str,
        location: SourceLocation,
        offset_us: u64,
    },
    StatementFinished {
        index: u64,
        status: CaseStatus,
        code: Option<&'static str>,
        offset_us: u64,
    },
    Log(LogRecord),
    Artifact(ArtifactRecord),
    RunFinished {
        status: CaseStatus,
        finished_at: String,
        duration_us: u64,
        error: Option<ErrorRecord>,
    },
    /// Work that never started. It is the only event of its run.
    RunSkipped {
        identity: RunIdentity,
        reason: SkipReason,
        recorded_at: String,
    },
}

/// An event with its zero-based position in the run's stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EventRecord {
    pub sequence: u64,
    #[serde(flatten)]
    pub event: Event,
}

/// An event that breaks run ordering; the record is left unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    #[error("event {found} arrived where sequence {expected} was expected")]
    Sequence { expected: u64, found: u64 },
    #[error("a run must start or be skipped before other events")]
    NotStarted,
    #[error("a run starts or is skipped once")]
    AlreadyStarted,
    #[error("no event follows a terminal outcome")]
    AfterTerminal,
    #[error("statements start in index order after the previous one finished")]
    StatementOrder,
}

/// The folded result of one run. Until a terminal event arrives, its status is
/// `interrupted` and `complete` is false: partial output never implies a pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunRecord {
    pub format: &'static str,
    pub version: u32,
    pub identity: RunIdentity,
    pub status: CaseStatus,
    pub complete: bool,
    pub skip_reason: Option<SkipReason>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_us: Option<u64>,
    pub statements: Vec<StatementRecord>,
    pub omitted_statements: u64,
    pub logs: Vec<LogRecord>,
    pub omitted_logs: u64,
    pub logged_bytes: u64,
    pub error: Option<ErrorRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub omitted_artifacts: u64,
    pub events: u64,
    #[serde(skip)]
    limits: RecordLimits,
    #[serde(skip)]
    captured_log_bytes: usize,
    #[serde(skip)]
    open_statement: Option<u64>,
    #[serde(skip)]
    next_statement: u64,
}

impl RunRecord {
    pub fn new(identity: RunIdentity, limits: RecordLimits) -> Self {
        Self {
            format: RECORD_FORMAT,
            version: RECORD_VERSION,
            identity,
            status: CaseStatus::Interrupted,
            complete: false,
            skip_reason: None,
            started_at: None,
            finished_at: None,
            duration_us: None,
            statements: Vec::new(),
            omitted_statements: 0,
            logs: Vec::new(),
            omitted_logs: 0,
            logged_bytes: 0,
            error: None,
            artifacts: Vec::new(),
            omitted_artifacts: 0,
            events: 0,
            limits,
            captured_log_bytes: 0,
            open_statement: None,
            next_statement: 0,
        }
    }

    /// Fold events in order, rejecting the first one that breaks run ordering.
    pub fn from_events<'a>(
        identity: RunIdentity,
        limits: RecordLimits,
        events: impl IntoIterator<Item = &'a EventRecord>,
    ) -> Result<Self, RecordError> {
        let mut record = Self::new(identity, limits);
        for event in events {
            record.apply(event)?;
        }
        Ok(record)
    }

    fn terminal(&self) -> bool {
        self.complete
    }

    pub fn apply(&mut self, record: &EventRecord) -> Result<(), RecordError> {
        if record.sequence != self.events {
            return Err(RecordError::Sequence {
                expected: self.events,
                found: record.sequence,
            });
        }
        if self.terminal() {
            return Err(RecordError::AfterTerminal);
        }
        let started = self.started_at.is_some();
        match &record.event {
            Event::RunStarted { .. } | Event::RunSkipped { .. } if started => {
                return Err(RecordError::AlreadyStarted)
            }
            Event::RunStarted { .. } | Event::RunSkipped { .. } => (),
            _ if !started => return Err(RecordError::NotStarted),
            _ => (),
        }
        match &record.event {
            Event::RunStarted {
                identity,
                started_at,
            } => {
                self.identity = identity.clone();
                self.started_at = Some(started_at.clone());
            }
            Event::RunSkipped {
                identity,
                reason,
                recorded_at,
            } => {
                self.identity = identity.clone();
                self.status = CaseStatus::Skipped;
                self.skip_reason = Some(*reason);
                self.finished_at = Some(recorded_at.clone());
                self.complete = true;
            }
            Event::StatementStarted {
                index,
                kind,
                location,
                offset_us,
            } => {
                if self.open_statement.is_some() || *index != self.next_statement {
                    return Err(RecordError::StatementOrder);
                }
                self.open_statement = Some(*index);
                self.next_statement += 1;
                if self.statements.len() < self.limits.statements {
                    self.statements.push(StatementRecord {
                        index: *index,
                        kind,
                        location: location.clone(),
                        offset_us: *offset_us,
                        duration_us: None,
                        status: None,
                        code: None,
                    });
                } else {
                    self.omitted_statements += 1;
                }
            }
            Event::StatementFinished {
                index,
                status,
                code,
                offset_us,
            } => {
                if self.open_statement != Some(*index) {
                    return Err(RecordError::StatementOrder);
                }
                self.open_statement = None;
                if let Some(statement) = self.statements.last_mut().filter(|s| s.index == *index) {
                    statement.duration_us = Some(offset_us.saturating_sub(statement.offset_us));
                    statement.status = Some(*status);
                    statement.code = *code;
                }
            }
            Event::Log(log) => {
                self.logged_bytes = self.logged_bytes.saturating_add(log.bytes);
                let room = self
                    .limits
                    .log_bytes
                    .saturating_sub(self.captured_log_bytes);
                if self.logs.len() < self.limits.logs && log.text.len() <= room {
                    self.captured_log_bytes += log.text.len();
                    self.logs.push(log.clone());
                } else {
                    self.omitted_logs += 1;
                }
            }
            Event::Artifact(artifact) => {
                if self.artifacts.len() < self.limits.artifacts {
                    self.artifacts.push(artifact.clone());
                } else {
                    self.omitted_artifacts += 1;
                }
            }
            Event::RunFinished {
                status,
                finished_at,
                duration_us,
                error,
            } => {
                self.status = *status;
                self.finished_at = Some(finished_at.clone());
                self.duration_us = Some(*duration_us);
                self.error = error.clone();
                self.complete = true;
            }
        }
        self.events += 1;
        Ok(())
    }
}

/// The acceptance decision for a run's single phase.
pub fn decide(result: Result<(), &Diagnostic>, expectation: &CaseExpectation) -> CaseStatus {
    let body = match result {
        Ok(()) => PhaseOutcome::Succeeded,
        Err(error) => PhaseOutcome::Failed(FailureKind::from_diagnostic(error)),
    };
    CaseCompletion::executed(
        PhaseOutcome::Succeeded,
        Some(body),
        PhaseOutcome::Succeeded,
        None,
    )
    .expect("a single executed phase is consistent")
    .decide(expectation)
}

/// Wall-clock time as RFC 3339 UTC with microseconds.
pub fn timestamp(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Format into at most `maximum` bytes; the returned text ends with a marker when cut.
fn bounded(value: &dyn fmt::Display, maximum: usize) -> (String, bool) {
    struct Capped {
        text: String,
        remaining: usize,
        truncated: bool,
    }
    impl Write for Capped {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            if text.len() <= self.remaining {
                self.text.push_str(text);
                self.remaining -= text.len();
                return Ok(());
            }
            let mut end = self.remaining;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.push_str(&text[..end]);
            self.truncated = true;
            Err(fmt::Error)
        }
    }
    let mut output = Capped {
        text: String::new(),
        remaining: maximum,
        truncated: false,
    };
    let _ = write!(output, "{value}");
    if output.truncated {
        output.text.push_str(TRUNCATED);
    }
    (output.text, output.truncated)
}

/// Count the UTF-8 bytes a value formats to, without retaining them.
fn measure(value: &dyn fmt::Display) -> u64 {
    struct Counter(u64);
    impl Write for Counter {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            self.0 = self.0.saturating_add(text.len() as u64);
            Ok(())
        }
    }
    let mut counter = Counter(0);
    let _ = write!(counter, "{value}");
    counter.0
}

struct Live {
    record: RunRecord,
    start: Instant,
    statement: Option<(u64, Instant)>,
}

impl Live {
    fn emit(&mut self, event: Event) {
        let sequence = self.record.events;
        // The recorder only emits well-ordered events.
        let _ = self.record.apply(&EventRecord { sequence, event });
    }
}

/// A run being recorded by a host that prepares its own `Context`. Start it when
/// the run begins, attach it once the context exists, and finish it exactly once.
#[derive(Debug)]
pub struct Recording {
    recorder: Recorder,
    expectation: CaseExpectation,
}

impl Recording {
    /// Emit `RunStarted` now. An empty identity `id` stays empty.
    pub fn start(options: RecordOptions) -> Self {
        Self {
            recorder: Recorder::start(options.identity, options.limits),
            expectation: options.expectation,
        }
    }

    /// Emit the terminal event and return the folded record.
    pub fn finish(self, result: Result<(), &Diagnostic>) -> RunRecord {
        self.recorder.finish(result, &self.expectation)
    }

    pub(crate) fn recorder(&self) -> &Recorder {
        &self.recorder
    }
}

/// Live capture for one run, shared with worker and module contexts.
#[derive(Clone)]
pub(crate) struct Recorder {
    live: Arc<Mutex<Live>>,
}

impl fmt::Debug for Recorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Recorder")
    }
}

impl Recorder {
    pub(crate) fn start(identity: RunIdentity, limits: RecordLimits) -> Self {
        let started = SystemTime::now();
        let mut live = Live {
            record: RunRecord::new(identity.clone(), limits),
            start: Instant::now(),
            statement: None,
        };
        live.emit(Event::RunStarted {
            identity,
            started_at: timestamp(started),
        });
        Self {
            live: Arc::new(Mutex::new(live)),
        }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn statement_started(&self, kind: &'static str, span: &Span) {
        let mut live = self.live();
        let now = Instant::now();
        let index = live.record.next_statement;
        let offset_us = micros(now - live.start);
        live.statement = Some((index, now));
        live.emit(Event::StatementStarted {
            index,
            kind,
            location: SourceLocation::from_span(span),
            offset_us,
        });
    }

    pub(crate) fn statement_finished(&self, error: Option<&Diagnostic>) {
        let mut live = self.live();
        let Some((index, _)) = live.statement.take() else {
            return;
        };
        let offset_us = micros(live.start.elapsed());
        let (status, code) = match error {
            None => (CaseStatus::Succeeded, None),
            Some(error) => (
                CaseStatus::from_diagnostic_code(error.code()),
                Some(error.code().as_str()),
            ),
        };
        live.emit(Event::StatementFinished {
            index,
            status,
            code,
            offset_us,
        });
    }

    /// Capture a successfully written Log value as bounded text.
    pub(crate) fn log(&self, value: &dyn fmt::Display) {
        let mut live = self.live();
        let limits = &live.record.limits;
        let room = limits
            .log_bytes
            .saturating_sub(live.record.captured_log_bytes);
        let (text, truncated) = if live.record.logs.len() < limits.logs {
            bounded(value, limits.log_record_bytes.min(room))
        } else {
            (String::new(), true)
        };
        let log = LogRecord {
            statement: live.statement.map(|(index, _)| index),
            offset_us: micros(live.start.elapsed()),
            bytes: measure(value),
            text,
            truncated,
        };
        live.emit(Event::Log(log));
    }

    /// Emit the terminal event and return the folded record.
    pub(crate) fn finish(
        &self,
        result: Result<(), &Diagnostic>,
        expectation: &CaseExpectation,
    ) -> RunRecord {
        let mut live = self.live();
        if live.statement.is_some() {
            // The run stopped inside a statement: close it with the stop.
            drop(live);
            self.statement_finished(result.err());
            live = self.live();
        }
        let status = decide(result, expectation);
        let error = result
            .err()
            .map(|error| ErrorRecord::from_diagnostic(error, &live.record.limits));
        let finished = SystemTime::now();
        let duration_us = micros(live.start.elapsed());
        live.emit(Event::RunFinished {
            status,
            finished_at: timestamp(finished),
            duration_us,
            error,
        });
        live.record.clone()
    }
}
