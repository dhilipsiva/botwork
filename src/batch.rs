//! CLI batch admission. Mutable interpreter state belongs to each admitted run.
use super::{BWErr, CliError, Context, Diagnostic, RunLimits};
use botwork::core::diagnostic::DiagnosticCode;
use std::{collections::HashMap, io, path::PathBuf, sync::Arc};
use tokio::task::JoinSet;

#[cfg(test)]
mod tests;

pub(super) struct Configuration {
    pub(super) debug: bool,
    pub(super) files: Vec<PathBuf>,
    pub(super) settings: Vec<String>,
    pub(super) limits: RunLimits,
    pub(super) timeout_ms: Option<u64>,
}

#[derive(Clone)]
struct Identity {
    // Command-line position stays stable even when completion order changes.
    number: usize,
    path: Arc<PathBuf>,
}

enum Message {
    Started(Identity),
    Finished(Identity, Result<(), CliError>),
    Summary {
        total: usize,
        succeeded: usize,
        failed: usize,
    },
}

fn task_failure() -> CliError {
    Diagnostic::new(BWErr::AsyncRuntime(
        "CLI task failed before completing".into(),
    ))
    .into()
}

fn outcome(error: &CliError) -> &'static str {
    let code = match error {
        CliError::Script(error) => error.code(),
        CliError::SourceLimit { source, .. } => source.code(),
        _ => return "failed",
    };
    match code {
        DiagnosticCode::Cancelled => "cancelled",
        DiagnosticCode::Timeout => "timed out",
        DiagnosticCode::ResourceLimit => "limit exceeded",
        _ => "failed",
    }
}

fn write_report(message: Message) -> Result<(), CliError> {
    let context = Context::default();
    let mut stderr = io::stderr().lock();
    match message {
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
            total,
            succeeded,
            failed,
        } => context.write_output(
            &mut stderr,
            format_args!("[batch] {total} runs: {succeeded} succeeded, {failed} failed\n"),
        ),
    }
    .map(|_| ())
    .map_err(CliError::from)
}

async fn report(message: Message) -> Result<(), CliError> {
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
    // The parser enforces this too; keep the scheduler's bound explicit.
    if !(1..=64).contains(&jobs) {
        return Err(Diagnostic::new(BWErr::RunConfiguration(
            "Parallel jobs must be between 1 and 64".into(),
        ))
        .into());
    }
    let total = files.len();
    let configuration = Arc::new(configuration);
    let mut pending = files.into_iter().enumerate();
    let mut running = JoinSet::new();
    let mut identities = HashMap::new();
    let mut failed = 0;
    let mut reporting_error = None;
    loop {
        while reporting_error.is_none() && running.len() < jobs {
            let Some((index, path)) = pending.next() else {
                break;
            };
            let identity = Identity {
                number: index + 1,
                path: Arc::new(path),
            };
            if let Err(error) = report(Message::Started(identity.clone())).await {
                reporting_error = Some(error);
                break;
            }
            let configuration = Arc::clone(&configuration);
            let path = Arc::clone(&identity.path);
            let task = running.spawn(async move {
                super::run(
                    &path,
                    configuration.debug,
                    &configuration.files,
                    &configuration.settings,
                    configuration.limits.clone(),
                    configuration.timeout_ms,
                )
                .await
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
    })
    .await?;
    if failed == 0 {
        Ok(())
    } else {
        Err(CliError::Batch { failed, total })
    }
}
