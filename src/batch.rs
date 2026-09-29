//! CLI batch admission. Mutable interpreter state belongs to each admitted run.
use super::{BWErr, CliError, Context, Diagnostic, RunLimits};
use botwork::core::acceptance::{CaseStatus, CaseTotals, Delivery};
use botwork::core::suite::SelectedCase;
use std::{collections::HashMap, io, path::PathBuf, sync::Arc};
use tokio::task::JoinSet;

#[cfg(test)]
mod tests;

pub(super) struct Configuration {
    pub(super) artifacts: Option<Arc<super::assertion_artifacts::Store>>,
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
    pub(super) total: usize,
    pub(super) failed: usize,
    pub(super) skipped: usize,
    pub(super) fixtures_failed: usize,
    pub(super) failed_cases: Vec<String>,
}

impl Outcome {
    pub(super) fn result(self) -> Result<(), CliError> {
        let succeeded = self
            .total
            .checked_sub(self.failed)
            .and_then(|remaining| remaining.checked_sub(self.skipped))
            .ok_or_else(|| {
                Diagnostic::new(BWErr::RunConfiguration(
                    "Inconsistent acceptance result counts".into(),
                ))
            })?;
        let mut totals = CaseTotals::default();
        totals.record_many(CaseStatus::Succeeded, succeeded)?;
        totals.record_many(CaseStatus::Failed, self.failed)?;
        totals.record_many(CaseStatus::Skipped, self.skipped)?;
        // The scheduler returns Outcome only after owners and reports drain.
        if !totals
            .finish(self.fixtures_failed, Delivery::Complete)
            .failed()
        {
            Ok(())
        } else if self.skipped != 0 || self.fixtures_failed != 0 {
            Err(CliError::Suites {
                failed: self.failed,
                skipped: self.skipped,
                fixtures_failed: self.fixtures_failed,
            })
        } else {
            Err(CliError::Batch {
                failed: self.failed,
                total: self.total,
            })
        }
    }
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
        total: usize,
        succeeded: usize,
        failed: usize,
        cases: bool,
        skipped: usize,
        fixtures_failed: usize,
    },
}

pub(super) fn task_failure() -> CliError {
    Diagnostic::new(BWErr::AsyncRuntime(
        "CLI task failed before completing".into(),
    ))
    .into()
}

fn outcome(error: &CliError) -> &'static str {
    let code = match error {
        CliError::Script(error) => error.code(),
        CliError::SourceLimit { source, .. } => source.code(),
        _ => return CaseStatus::Failed.label(),
    };
    CaseStatus::from_diagnostic_code(code).label()
}

fn write_report(message: Message) -> Result<(), CliError> {
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
        Message::SuiteStarted(id) => context.write_output(&mut stderr, format_args!("[suite {id}] setup started\n")),
        Message::SuiteReady(id) => context.write_output(&mut stderr, format_args!("[suite {id}] setup succeeded\n")),
        Message::SuiteFinished(id, Ok(()), _) => context.write_output(&mut stderr, format_args!("[suite {id}] teardown succeeded\n")),
        Message::SuiteFinished(id, Err(error), _) => context.write_output(&mut stderr, format_args!("[suite {id}] fixture {}:\n{error}\n", outcome(&error))),
        Message::Skipped(Identity { case: Some((id, name)), .. }, reason) => context.write_output(&mut stderr, format_args!("[case {id}] skipped: {name:?}: {reason}\n")),
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
                format_args!("[case {id}] {}: {name:?}{}\n{error}\n", outcome(&error), DatasetLabel(&dataset)),
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
        Message::Summary { total, succeeded, failed, cases: true, skipped, fixtures_failed }
            if skipped != 0 || fixtures_failed != 0 => context.write_output(&mut stderr,
                format_args!("[cases] {total} selected: {succeeded} succeeded, {failed} failed, {skipped} skipped; {fixtures_failed} suite fixtures failed\n")),
        Message::Summary {
            total,
            succeeded,
            failed,
            cases: true,
            ..
        } => context.write_output(
            &mut stderr,
            format_args!("[cases] {total} selected: {succeeded} succeeded, {failed} failed\n"),
        ),
        Message::Summary {
            total,
            succeeded,
            failed,
            cases: false,
            ..
        } => context.write_output(
            &mut stderr,
            format_args!("[batch] {total} runs: {succeeded} succeeded, {failed} failed\n"),
        ),
    }
    .map(|_| ())
    .map_err(CliError::from);
    result?;
    for path in exported {
        context.write_output(&mut stderr, format_args!("[assertion artifact] {path:?}\n"))?;
    }
    Ok(())
}

pub(super) async fn report(message: Message) -> Result<(), CliError> {
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
    run_inputs(
        files.into_iter().map(Input::File).collect(),
        jobs,
        configuration,
        false,
    )
    .await?
    .result()
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
    let mut failed = 0;
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
            let configuration = Arc::clone(&configuration);
            let task = running.spawn(async move {
                match input {
                    Input::Case(case) => super::run_case(case, configuration, None).await,
                    Input::File(path) => {
                        super::run(
                            &path,
                            configuration.debug,
                            &configuration.files,
                            &configuration.settings,
                            configuration.limits.clone(),
                            configuration.timeout_ms,
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
        let (task, result) = match completed {
            Ok((task, result)) => (task, result),
            Err(error) => (error.id(), Err(task_failure())),
        };
        let identity = identities.remove(&task).expect("admitted run identity");
        if result.is_err() {
            failed += 1;
            if let Some((id, _)) = &identity.case {
                failed_cases.push((identity.number, id.clone()));
            }
        }
        if reporting_error.is_none() {
            if let Err(error) = report(Message::Finished(identity, result)).await {
                reporting_error = Some(error);
            }
        }
        // After a reporting failure, drain every admitted run without admitting
        // queued paths or detaching callbacks. The original reporting error wins.
    }
    if let Some(error) = reporting_error {
        return Err(error);
    }
    report(Message::Summary {
        total,
        succeeded: total - failed,
        failed,
        cases,
        skipped: 0,
        fixtures_failed: 0,
    })
    .await?;
    failed_cases.sort_unstable_by_key(|(number, _)| *number);
    Ok(Outcome {
        total,
        failed,
        skipped: 0,
        fixtures_failed: 0,
        failed_cases: failed_cases.into_iter().map(|(_, id)| id).collect(),
    })
}
