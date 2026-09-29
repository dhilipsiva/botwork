//! CLI batch admission. Mutable interpreter state belongs to each admitted run.
use super::{BWErr, CliError, Context, Diagnostic, RunLimits};
use botwork::core::acceptance::{CaseStatus, CaseTotals, Delivery, SkipReason};
use botwork::core::report::{ErrorRecord, Recording, RunIdentity};
use botwork::core::suite::SelectedCase;
use std::{collections::HashMap, io, path::PathBuf, sync::Arc};
use tokio::task::JoinSet;

#[cfg(test)]
mod tests;

pub(super) struct Configuration {
    pub(super) artifacts: Option<Arc<super::assertion_artifacts::Store>>,
    pub(super) report: Option<Arc<super::report_json::Report>>,
    pub(super) listener: Option<Arc<super::listener::Hub>>,
    pub(super) debug: bool,
    pub(super) files: Vec<PathBuf>,
    pub(super) settings: Vec<String>,
    pub(super) limits: RunLimits,
    pub(super) timeout_ms: Option<u64>,
    pub(super) suite_timeout_ms: Option<u64>,
}

#[derive(Clone)]
pub(super) struct Identity {
    // Command-line position stays stable even when completion order changes.
    pub(super) number: usize,
    pub(super) path: Arc<PathBuf>,
    pub(super) case: Option<(String, String)>,
    pub(super) dataset: Option<(String, String)>,
    pub(super) artifacts: Option<Arc<super::assertion_artifacts::Store>>,
}

struct DatasetLabel<'a>(&'a Option<(String, String)>);
impl std::fmt::Display for DatasetLabel<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some((dataset, row)) = self.0 {
            write!(f, " [dataset {dataset:?}, row {row:?}]")
        } else {
            Ok(())
        }
    }
}

enum Input {
    File(PathBuf),
    Case(SelectedCase),
}

pub(super) struct Outcome {
    pub(super) totals: CaseTotals,
    pub(super) fixtures_failed: usize,
    pub(super) failed_cases: Vec<String>,
}

impl Outcome {
    /// The exit decision comes from the same verdict as the printed summary.
    pub(super) fn result(self) -> Result<(), CliError> {
        let skipped = self.totals.count(CaseStatus::Skipped);
        let total = self.totals.total();
        let unsuccessful = total
            - self.totals.count(CaseStatus::Succeeded)
            - self.totals.count(CaseStatus::ExpectedFailure)
            - skipped;
        // The scheduler returns Outcome only after owners and reports drain.
        if !self
            .totals
            .finish(self.fixtures_failed, Delivery::Complete)
            .failed()
        {
            Ok(())
        } else if skipped != 0 || self.fixtures_failed != 0 {
            Err(CliError::Suites {
                failed: unsuccessful,
                skipped,
                fixtures_failed: self.fixtures_failed,
            })
        } else {
            Err(CliError::Batch {
                failed: unsuccessful,
                total,
            })
        }
    }
}

/// Combine a run result with report publication. A publication failure is
/// printed at once so an earlier failure keeps its place as the result.
pub(super) fn publish_report(
    result: Result<(), CliError>,
    published: Result<(), CliError>,
) -> Result<(), CliError> {
    match (result, published) {
        (result, Ok(())) => result,
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(publication)) => {
            let _ = Context::default()
                .write_output(&mut io::stderr().lock(), format_args!("{publication}\n"));
            Err(error)
        }
    }
}

/// Finish the listener, then publish the JSON report. The listener hears the
/// execution outcome; its delivery then decides the verdict the report records,
/// which is also the one that decides the exit status.
pub(super) fn deliver(
    report: Option<&super::report_json::Report>,
    listener: Option<&super::listener::Hub>,
    totals: CaseTotals,
    fixtures_failed: usize,
) -> Result<(), CliError> {
    let listened = listener.map_or(Ok(()), |hub| {
        hub.finish(Some(&totals.finish(fixtures_failed, Delivery::Complete)))
    });
    let delivery = if listened.is_ok() {
        Delivery::Complete
    } else {
        Delivery::Failed
    };
    let reported = report.map_or(Ok(()), |report| {
        report.finish(&totals.finish(fixtures_failed, delivery), totals.total())
    });
    publish_report(listened, reported)
}

/// [`deliver`] off the executor, since a listener may take its close timeout.
pub(super) async fn finish_outputs(
    report: Option<Arc<super::report_json::Report>>,
    listener: Option<Arc<super::listener::Hub>>,
    outcome: &Outcome,
) -> Result<(), CliError> {
    if report.is_none() && listener.is_none() {
        return Ok(());
    }
    let (totals, fixtures_failed) = (outcome.totals, outcome.fixtures_failed);
    tokio::task::spawn_blocking(move || {
        deliver(
            report.as_deref(),
            listener.as_deref(),
            totals,
            fixtures_failed,
        )
    })
    .await
    .map_err(|_| task_failure())?
}

/// Start recording run `number` when a report or listener consumes records.
pub(super) fn recording(
    report: bool,
    listener: Option<&super::listener::Hub>,
    number: usize,
    identity: RunIdentity,
) -> Option<Recording> {
    (report || listener.is_some())
        .then(|| super::report_json::recording(identity, listener.map(|hub| hub.observer(number))))
}

impl Configuration {
    pub(super) fn recording(&self, number: usize, identity: RunIdentity) -> Option<Recording> {
        recording(
            self.report.is_some(),
            self.listener.as_deref(),
            number,
            identity,
        )
    }

    /// A selected case that never started: one skipped record and event.
    pub(super) fn skipped(&self, number: usize, case: &SelectedCase, reason: SkipReason) {
        if self.report.is_none() && self.listener.is_none() {
            return;
        }
        let event = super::report_json::skipped_event(case_identity(case), reason);
        if let Some(hub) = &self.listener {
            hub.event(number, event.clone());
        }
        if let Some(report) = &self.report {
            report.skipped(number, &event);
        }
    }

    /// A finished shared suite fixture.
    pub(super) fn fixture(&self, suite: &str, result: &Result<(), CliError>) {
        if self.report.is_none() && self.listener.is_none() {
            return;
        }
        let error = result.as_ref().err().map(|error| {
            ErrorRecord::from_diagnostic(
                &super::report_json::diagnostic(error),
                &super::report_json::limits(),
            )
        });
        let status = error
            .as_ref()
            .map_or(CaseStatus::Succeeded, |error| error.status);
        if let Some(hub) = &self.listener {
            hub.fixture(suite, status, error.clone());
        }
        if let Some(report) = &self.report {
            report.fixture(suite, status, error);
        }
    }
}

/// Failure recap lines kept for the final console summary.
const RECAP_LINES: usize = 50;

/// Terminal observations for one CLI invocation, in completion order.
#[derive(Default)]
pub(super) struct Tally {
    pub(super) totals: CaseTotals,
    pub(super) fixtures_failed: usize,
    recap: Vec<String>,
    omitted: usize,
}

/// The console status of a finished run, from its diagnostic code alone.
pub(super) fn status(result: &Result<(), CliError>) -> CaseStatus {
    result
        .as_ref()
        .err()
        .map_or(CaseStatus::Succeeded, error_status)
}

fn error_status(error: &CliError) -> CaseStatus {
    match error {
        CliError::Script(error) => CaseStatus::from_diagnostic_code(error.code()),
        CliError::SourceLimit { source, .. } => CaseStatus::from_diagnostic_code(source.code()),
        _ => CaseStatus::Failed,
    }
}

/// ` (BWnnnn at file:line:column)` when the failure has a diagnostic. Recap lines
/// never repeat a progress line's `label:` form, so progress lines stay unique.
struct Cause<'a>(&'a CliError);
impl std::fmt::Display for Cause<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let diagnostic = match self.0 {
            CliError::Script(error) => error,
            CliError::SourceLimit { source, .. } => source,
            _ => return Ok(()),
        };
        write!(f, " ({}", diagnostic.code())?;
        // Stops inside built-ins point at synthetic sources such as `<builtin>`;
        // report the innermost script call site instead.
        let real = |span: &&botwork::core::ast::Span| !span.source().name().starts_with('<');
        let span = diagnostic.span.as_ref().filter(real).or_else(|| {
            diagnostic
                .call_stack
                .iter()
                .rev()
                .map(|frame| &frame.call_site)
                .find(real)
        });
        if let Some(span) = span {
            write!(f, " at {}", span.location())?;
        }
        f.write_str(")")
    }
}

impl Tally {
    pub(super) fn finished(
        &mut self,
        identity: &Identity,
        result: &Result<(), CliError>,
    ) -> Result<(), CliError> {
        let status = status(result);
        self.totals.record(status)?;
        if let Err(error) = result {
            let line = match &identity.case {
                Some((id, _)) => format!("[case {id}] {}{}", status.label(), Cause(error)),
                None => format!(
                    "[run {}] {} {:?}{}",
                    identity.number,
                    status.label(),
                    identity.path,
                    Cause(error)
                ),
            };
            self.remember(line);
        }
        Ok(())
    }

    pub(super) fn skipped(&mut self) -> Result<(), CliError> {
        Ok(self.totals.record(CaseStatus::Skipped)?)
    }

    pub(super) fn fixture(&mut self, suite: &str, result: &Result<(), CliError>) {
        if let Err(error) = result {
            self.fixtures_failed += 1;
            let line = format!(
                "[suite {suite}] fixture {}{}",
                status(result).label(),
                Cause(error)
            );
            self.remember(line);
        }
    }

    fn remember(&mut self, line: String) {
        if self.recap.len() < RECAP_LINES {
            self.recap.push(line);
        } else {
            self.omitted += 1;
        }
    }

    pub(super) fn summary(&mut self, cases: bool) -> Message {
        Message::Summary {
            cases,
            totals: self.totals,
            fixtures_failed: self.fixtures_failed,
            recap: std::mem::take(&mut self.recap),
            omitted: self.omitted,
        }
    }
}

/// `[batch] N runs: …` or `[cases] N selected: …`, listing stop categories only when present.
fn summary(cases: bool, totals: &CaseTotals, fixtures_failed: usize) -> String {
    use std::fmt::Write;
    let mut line = String::new();
    let _ = if cases {
        write!(line, "[cases] {} selected: ", totals.total())
    } else {
        write!(line, "[batch] {} runs: ", totals.total())
    };
    let _ = write!(
        line,
        "{} succeeded, {} failed",
        totals.count(CaseStatus::Succeeded),
        totals.count(CaseStatus::Failed)
    );
    for status in [
        CaseStatus::ExpectedFailure,
        CaseStatus::UnexpectedPass,
        CaseStatus::Cancelled,
        CaseStatus::TimedOut,
        CaseStatus::LimitExceeded,
        CaseStatus::Interrupted,
    ] {
        let count = totals.count(status);
        if count != 0 {
            let _ = write!(line, ", {count} {}", status.label());
        }
    }
    let skipped = totals.count(CaseStatus::Skipped);
    if cases && (skipped != 0 || fixtures_failed != 0) {
        let _ = write!(
            line,
            ", {skipped} skipped; {fixtures_failed} suite fixtures failed"
        );
    }
    line
}

pub(super) enum Message {
    Started(Identity),
    Finished(Identity, Result<(), CliError>),
    Skipped(Identity, &'static str),
    SuiteStarted(String),
    SuiteReady(String),
    SuiteFinished(
        String,
        Result<(), CliError>,
        Option<Arc<super::assertion_artifacts::Store>>,
    ),
    Summary {
        cases: bool,
        totals: CaseTotals,
        fixtures_failed: usize,
        recap: Vec<String>,
        omitted: usize,
    },
}

pub(super) fn task_failure() -> CliError {
    Diagnostic::new(BWErr::AsyncRuntime(
        "CLI task failed before completing".into(),
    ))
    .into()
}

fn outcome(error: &CliError) -> &'static str {
    error_status(error).label()
}

/// Write one console record; return the assertion artifacts it exported.
fn write_report(message: Message) -> Result<Vec<PathBuf>, CliError> {
    let context = Context::default();
    let mut stderr = io::stderr().lock();
    let exported = match &message {
        Message::Finished(run, Err(error)) => run.artifacts.as_ref().map(|store| {
            store.write_error(
                error,
                &super::assertion_artifacts::Identity {
                    run: run.number,
                    file: &run.path,
                    case: run.case.as_ref().map(|(id, _)| id.as_str()),
                    name: run.case.as_ref().map(|(_, name)| name.as_str()),
                    dataset: run.dataset.as_ref().map(|(id, _)| id.as_str()),
                    row: run.dataset.as_ref().map(|(_, row)| row.as_str()),
                    suite_fixture: None,
                },
            )
        }),
        Message::SuiteFinished(id, Err(error), artifacts) => artifacts.as_ref().map(|store| {
            store.write_error(
                error,
                &super::assertion_artifacts::Identity {
                    run: 0,
                    file: std::path::Path::new(""),
                    case: None,
                    name: None,
                    dataset: None,
                    row: None,
                    suite_fixture: Some(id),
                },
            )
        }),
        _ => None,
    }
    .transpose()
    .map_err(|source| {
        let original = match &message {
            Message::Finished(run, Err(error)) => format!(
                "[run {}] {:?}{}\n{error}",
                run.number,
                run.case,
                DatasetLabel(&run.dataset)
            ),
            Message::SuiteFinished(id, Err(error), _) => format!("[suite {id}]\n{error}"),
            _ => unreachable!("only failed runs export assertion evidence"),
        };
        CliError::Artifact { source, original }
    })?
    .unwrap_or_default();
    let result = match message {
        Message::SuiteStarted(id) => {
            context.write_output(&mut stderr, format_args!("[suite {id}] setup started\n"))
        }
        Message::SuiteReady(id) => {
            context.write_output(&mut stderr, format_args!("[suite {id}] setup succeeded\n"))
        }
        Message::SuiteFinished(id, Ok(()), _) => context.write_output(
            &mut stderr,
            format_args!("[suite {id}] teardown succeeded\n"),
        ),
        Message::SuiteFinished(id, Err(error), _) => context.write_output(
            &mut stderr,
            format_args!("[suite {id}] fixture {}:\n{error}\n", outcome(&error)),
        ),
        Message::Skipped(
            Identity {
                case: Some((id, name)),
                ..
            },
            reason,
        ) => context.write_output(
            &mut stderr,
            format_args!("[case {id}] skipped: {name:?}: {reason}\n"),
        ),
        Message::Skipped(_, _) => unreachable!("only cases are skipped by suite fixtures"),
        Message::Started(Identity {
            case: Some((id, name)),
            ..
        }) => context.write_output(&mut stderr, format_args!("[case {id}] started: {name:?}\n")),
        Message::Finished(
            Identity {
                case: Some((id, name)),
                dataset,
                ..
            },
            result,
        ) => match result {
            Ok(()) => context.write_output(
                &mut stderr,
                format_args!("[case {id}] succeeded: {name:?}\n"),
            ),
            Err(error) => context.write_output(
                &mut stderr,
                format_args!(
                    "[case {id}] {}: {name:?}{}\n{error}\n",
                    outcome(&error),
                    DatasetLabel(&dataset)
                ),
            ),
        },
        Message::Started(run) => context.write_output(
            &mut stderr,
            format_args!("[run {}] started: {:?}\n", run.number, run.path),
        ),
        Message::Finished(run, Ok(())) => context.write_output(
            &mut stderr,
            format_args!("[run {}] succeeded: {:?}\n", run.number, run.path),
        ),
        Message::Finished(run, Err(error)) => context.write_output(
            &mut stderr,
            format_args!(
                "[run {}] {}: {:?}\n{error}\n",
                run.number,
                outcome(&error),
                run.path
            ),
        ),
        Message::Summary {
            cases,
            totals,
            fixtures_failed,
            recap,
            omitted,
        } => {
            // Failures scroll past with their full diagnostics; recap them before
            // the summary so the final lines explain the exit status.
            if !recap.is_empty() {
                context.write_output(
                    &mut stderr,
                    format_args!("[failures] {}:\n", recap.len() + omitted),
                )?;
                for line in &recap {
                    context.write_output(&mut stderr, format_args!("  {line}\n"))?;
                }
                if omitted != 0 {
                    context.write_output(&mut stderr, format_args!("  …and {omitted} more\n"))?;
                }
            }
            context.write_output(
                &mut stderr,
                format_args!("{}\n", summary(cases, &totals, fixtures_failed)),
            )
        }
    }
    .map(|_| ())
    .map_err(CliError::from);
    result?;
    for path in &exported {
        context.write_output(&mut stderr, format_args!("[assertion artifact] {path:?}\n"))?;
    }
    Ok(exported)
}

pub(super) async fn report(message: Message) -> Result<Vec<PathBuf>, CliError> {
    // One bounded record at a time, off the executor. A blocked reporter stops
    // admission while already-admitted runs continue to receive executor time.
    tokio::task::spawn_blocking(move || write_report(message))
        .await
        .map_err(|_| task_failure())?
}

pub(super) async fn run(
    files: Vec<PathBuf>,
    jobs: usize,
    configuration: Configuration,
) -> Result<(), CliError> {
    let (report, listener) = (configuration.report.clone(), configuration.listener.clone());
    let outcome = run_inputs(
        files.into_iter().map(Input::File).collect(),
        jobs,
        configuration,
        false,
    )
    .await?;
    let published = finish_outputs(report, listener, &outcome).await;
    publish_report(outcome.result(), published)
}

/// The report identity of a selected case, with its dataset row when present.
pub(super) fn case_identity(case: &SelectedCase) -> RunIdentity {
    let identity = RunIdentity::new(case.id(), case.display_name());
    match case.row() {
        Some(row) => identity.with_row(
            case.dataset().expect("row dataset").id(),
            row.metadata().id(),
        ),
        None => identity,
    }
}

pub(super) async fn run_cases(
    cases: Vec<SelectedCase>,
    jobs: usize,
    configuration: Configuration,
) -> Result<Outcome, CliError> {
    run_inputs(
        cases.into_iter().map(Input::Case).collect(),
        jobs,
        configuration,
        true,
    )
    .await
}

async fn run_inputs(
    inputs: Vec<Input>,
    jobs: usize,
    configuration: Configuration,
    cases: bool,
) -> Result<Outcome, CliError> {
    // The parser enforces this too; keep the scheduler's bound explicit.
    if !(1..=64).contains(&jobs) {
        return Err(Diagnostic::new(BWErr::RunConfiguration(
            "Parallel jobs must be between 1 and 64".into(),
        ))
        .into());
    }
    let total = inputs.len();
    let configuration = Arc::new(configuration);
    let mut pending = inputs.into_iter().enumerate();
    let mut running = JoinSet::new();
    let mut identities = HashMap::new();
    let mut tally = Tally::default();
    let mut failed_cases = Vec::new();
    let mut reporting_error = None;
    loop {
        while reporting_error.is_none() && running.len() < jobs {
            let Some((index, input)) = pending.next() else {
                break;
            };
            let (path, case) = match &input {
                Input::File(path) => (path.clone(), None),
                Input::Case(case) => (
                    PathBuf::from(case.suite().source().name()),
                    Some((case.id(), case.display_name())),
                ),
            };
            let identity = Identity {
                number: index + 1,
                path: Arc::new(path),
                case,
                dataset: match &input {
                    Input::Case(case) => case.row().map(|row| {
                        (
                            case.dataset().expect("row dataset").id().to_owned(),
                            row.metadata().id().to_owned(),
                        )
                    }),
                    Input::File(_) => None,
                },
                artifacts: configuration.artifacts.clone(),
            };
            if let Err(error) = report(Message::Started(identity.clone())).await {
                reporting_error = Some(error);
                break;
            }
            let recording = configuration.recording(
                identity.number,
                match &input {
                    Input::File(path) => {
                        let path = path.display().to_string();
                        RunIdentity::new(path.clone(), path)
                    }
                    Input::Case(case) => case_identity(case),
                },
            );
            let configuration = Arc::clone(&configuration);
            let task = running.spawn(async move {
                match input {
                    Input::Case(case) => {
                        super::run_case(case, configuration, None, recording).await
                    }
                    Input::File(path) => {
                        super::run(
                            &path,
                            configuration.debug,
                            &configuration.files,
                            &configuration.settings,
                            configuration.limits.clone(),
                            configuration.timeout_ms,
                            recording,
                        )
                        .await
                    }
                }
            });
            identities.insert(task.id(), identity);
        }
        let Some(completed) = running.join_next_with_id().await else {
            break;
        };
        // A lost task has no record, which keeps any report incomplete.
        let (task, (result, record)) = match completed {
            Ok(completed) => completed,
            Err(error) => (error.id(), (Err(task_failure()), None)),
        };
        let identity = identities.remove(&task).expect("admitted run identity");
        let number = identity.number;
        tally.finished(&identity, &result)?;
        if result.is_err() {
            if let Some((id, _)) = &identity.case {
                failed_cases.push((identity.number, id.clone()));
            }
        }
        if reporting_error.is_none() {
            match report(Message::Finished(identity, result)).await {
                Ok(exported) => {
                    if let (Some(report), Some(record)) = (&configuration.report, record) {
                        report.run(number, record, &exported);
                    }
                }
                Err(error) => reporting_error = Some(error),
            }
        }
        // After a reporting failure, drain every admitted run without admitting
        // queued paths or detaching callbacks. The original reporting error wins.
    }
    if let Some(error) = reporting_error {
        return Err(error);
    }
    debug_assert_eq!(tally.totals.total(), total);
    report(tally.summary(cases)).await?;
    failed_cases.sort_unstable_by_key(|(number, _)| *number);
    Ok(Outcome {
        totals: tally.totals,
        fixtures_failed: 0,
        failed_cases: failed_cases.into_iter().map(|(_, id)| id).collect(),
    })
}
