//! Suite owners retain their context while cases borrow immutable inputs. Only
//! setup, case execution, and teardown occupy the bounded execution slots.
use super::*;
use crate::batch::{Identity, Message, Outcome, Stop, Tally};
use botwork::core::{
    acceptance::SkipReason,
    eval::{evaluate_suite_fixture_async, FixtureInputs},
};
use std::collections::{HashMap, VecDeque};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};

enum State {
    Plain,
    Pending,
    SettingUp,
    Ready {
        inputs: FixtureInputs,
        finish: oneshot::Sender<()>,
    },
    Finishing,
    Done,
}

struct Group {
    suite: Arc<Suite>,
    indices: Vec<usize>,
    pending: VecDeque<usize>,
    active: usize,
    state: State,
}

struct Ready {
    group: usize,
    inputs: FixtureInputs,
    finish: oneshot::Sender<()>,
}

#[derive(Clone, Copy)]
enum Task {
    Case { group: usize, index: usize },
    Owner(usize),
}

enum Action {
    Setup(usize),
    Case(usize),
    Finish(usize),
    Skip(usize),
}

fn action(groups: &[Group], stopping: bool) -> Option<Action> {
    // Release idle owners before acquiring more resources.
    for (index, group) in groups.iter().enumerate() {
        if matches!(group.state, State::Ready { .. })
            && group.pending.is_empty()
            && group.active == 0
        {
            return Some(Action::Finish(index));
        }
    }
    if stopping {
        return None;
    }
    for (index, group) in groups.iter().enumerate() {
        match &group.state {
            State::Pending => return Some(Action::Setup(index)),
            State::Ready { inputs, .. } if !group.pending.is_empty() => {
                return Some(if inputs.control().checkpoint().is_err() {
                    Action::Skip(index)
                } else {
                    Action::Case(index)
                });
            }
            State::Plain if !group.pending.is_empty() => return Some(Action::Case(index)),
            _ => (),
        }
    }
    None
}

fn identity(index: usize, cases: &[SelectedCase]) -> Identity {
    let case = &cases[index];
    Identity {
        number: index + 1,
        path: Arc::new(PathBuf::from(case.suite().source().name())),
        case: Some((case.id(), case.display_name())),
        dataset: case.row().map(|row| {
            (
                case.dataset().expect("row dataset").id().to_owned(),
                row.metadata().id().to_owned(),
            )
        }),
        artifacts: None,
    }
}

/// Report unless reporting already failed; return any exported artifacts.
async fn report(message: Message, error: &mut Option<CliError>) -> Vec<PathBuf> {
    if error.is_none() {
        match batch::report(message).await {
            Ok(exported) => return exported,
            Err(failure) => *error = Some(failure),
        }
    }
    Vec::new()
}

fn skip_message(reason: SkipReason) -> &'static str {
    reason.message()
}

async fn skip(
    group: &mut Group,
    cases: &[SelectedCase],
    failed: &mut [bool],
    tally: &mut Tally,
    configuration: &batch::Configuration,
    reporting_error: &mut Option<CliError>,
    reason: SkipReason,
) {
    for index in group.pending.drain(..) {
        if let Err(error) = tally.skipped() {
            reporting_error.get_or_insert(error);
        }
        failed[index] = true;
        configuration.skipped(index + 1, &cases[index], reason);
        report(
            Message::Skipped(identity(index, cases), skip_message(reason)),
            reporting_error,
        )
        .await;
    }
}

async fn owner(
    group: usize,
    suite: Arc<Suite>,
    configuration: Arc<batch::Configuration>,
    ready: mpsc::Sender<Ready>,
) -> Result<(), CliError> {
    let control = crate::run_control(configuration.suite_timeout_ms)?;
    let context = crate::prepared(
        &control.clone(),
        move || {
            let mut context =
                Context::with_host_environment(configuration.limits.clone(), control)?;
            let variables = crate::secrets::load(&configuration.files, &configuration.settings)?;
            context.init_statements();
            context.set_input_variables(variables)?;
            context.set_secrets(crate::secrets::registry())?;
            context.set_compiled_modules(crate::compiled_modules())?;
            context.checkpoint()?;
            context.set_statement_tracing(configuration.debug);
            Ok(context)
        },
        "CLI task failed before completing",
    )
    .await?;
    let result = evaluate_suite_fixture_async(&suite, context, move |inputs| async move {
        let (finish, finished) = oneshot::channel();
        if ready
            .send(Ready {
                group,
                inputs,
                finish,
            })
            .await
            .is_ok()
        {
            let _ = finished.await;
        }
    })
    .await;
    result.result.map_err(CliError::from)
}

pub(super) async fn run(
    cases: Vec<SelectedCase>,
    jobs: usize,
    configuration: batch::Configuration,
) -> Result<Outcome, CliError> {
    if !(1..=64).contains(&jobs) {
        return Err(suite::configuration("Parallel jobs must be between 1 and 64").into());
    }
    let mut groups: Vec<Group> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        if groups
            .last()
            .is_none_or(|group| group.suite.metadata().id() != case.suite().metadata().id())
        {
            groups.push(Group {
                suite: case.shared_suite(),
                indices: Vec::new(),
                pending: VecDeque::new(),
                active: 0,
                state: if case.suite().has_suite_fixture() {
                    State::Pending
                } else {
                    State::Plain
                },
            });
        }
        let group = groups.last_mut().unwrap();
        group.indices.push(index);
        group.pending.push_back(index);
    }
    if let Some(report) = &configuration.report {
        report.selected(cases.len());
    }
    let configuration = Arc::new(configuration);
    let (ready_tx, mut ready_rx) = mpsc::channel(suite::MAX_SUITES);
    let mut running = JoinSet::new();
    let mut identities = HashMap::new();
    let mut work = 0;
    let mut tally = Tally::default();
    let mut failed_ids = vec![false; cases.len()];
    let mut reporting_error = None;
    loop {
        // A reporting failure or signal stops admission; started work and
        // teardown of ready fixtures still finish.
        let stopping =
            |error: &Option<CliError>| error.is_some() || crate::interrupt::interrupted();
        if stopping(&reporting_error) {
            for group in &mut groups {
                group.pending.clear();
            }
        }
        while work < jobs {
            if stopping(&reporting_error) {
                for group in &mut groups {
                    group.pending.clear();
                }
            }
            let Some(action) = action(&groups, stopping(&reporting_error)) else {
                break;
            };
            match action {
                Action::Finish(group) => {
                    let State::Ready { inputs, finish } =
                        std::mem::replace(&mut groups[group].state, State::Finishing)
                    else {
                        unreachable!("ready owner")
                    };
                    // No borrowed snapshots remain in the coordinator during teardown.
                    drop(inputs);
                    let _ = finish.send(());
                    work += 1;
                }
                Action::Skip(group) => {
                    skip(
                        &mut groups[group],
                        &cases,
                        &mut failed_ids,
                        &mut tally,
                        &configuration,
                        &mut reporting_error,
                        SkipReason::SuiteStopped,
                    )
                    .await;
                }
                Action::Setup(group) => {
                    report(
                        Message::SuiteStarted(groups[group].suite.metadata().id().into()),
                        &mut reporting_error,
                    )
                    .await;
                    if reporting_error.is_some() {
                        continue;
                    }
                    groups[group].state = State::SettingUp;
                    let owner = owner(
                        group,
                        Arc::clone(&groups[group].suite),
                        Arc::clone(&configuration),
                        ready_tx.clone(),
                    );
                    let task = running.spawn(async move { (owner.await, None) });
                    identities.insert(task.id(), Task::Owner(group));
                    work += 1;
                }
                Action::Case(group) => {
                    let index = *groups[group].pending.front().unwrap();
                    report(
                        Message::Started(identity(index, &cases)),
                        &mut reporting_error,
                    )
                    .await;
                    if reporting_error.is_some() {
                        continue;
                    }
                    groups[group].pending.pop_front();
                    groups[group].active += 1;
                    let inputs = match &groups[group].state {
                        State::Ready { inputs, .. } => Some(inputs.clone()),
                        State::Plain => None,
                        _ => unreachable!("ready case"),
                    };
                    let case = cases[index].clone();
                    let configuration = Arc::clone(&configuration);
                    let recording = configuration.recording(index + 1, batch::case_identity(&case));
                    let task =
                        running.spawn(crate::run_case(case, configuration, inputs, recording));
                    identities.insert(task.id(), Task::Case { group, index });
                    work += 1;
                }
            }
        }
        if running.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            Some(ready) = ready_rx.recv() => {
                let group = &mut groups[ready.group];
                if matches!(group.state, State::SettingUp) {
                    group.state = State::Ready { inputs: ready.inputs, finish: ready.finish };
                    work -= 1;
                    report(Message::SuiteReady(group.suite.metadata().id().into()), &mut reporting_error).await;
                }
            }
            completed = running.join_next_with_id() => {
                // A lost task has no record, which keeps any report incomplete.
                let (id, (result, record)) = match completed.expect("owned tasks") {
                    Ok(value) => value,
                    Err(error) => (error.id(), (Err(batch::task_failure()), None)),
                };
                match identities.remove(&id).expect("admitted task") {
                    Task::Case { group, index } => {
                        work -= 1;
                        groups[group].active -= 1;
                        if result.is_err() { failed_ids[index] = true; }
                        let mut run_identity = identity(index, &cases);
                        run_identity.artifacts = configuration.artifacts.clone();
                        if let Err(error) = tally.finished(&run_identity, &result) {
                            reporting_error.get_or_insert(error);
                        }
                        let number = run_identity.number;
                        let exported = report(Message::Finished(run_identity, result), &mut reporting_error).await;
                        if let (Some(json), Some(record)) = (&configuration.report, record) {
                            json.run(number, record, &exported);
                        }
                    }
                    Task::Owner(group) => {
                        if matches!(groups[group].state, State::SettingUp | State::Finishing) { work -= 1; }
                        tally.fixture(groups[group].suite.metadata().id(), &result);
                        configuration.fixture(groups[group].suite.metadata().id(), &result);
                        if result.is_err() {
                            // A failed shared fixture affects every selected borrower,
                            // including passed cases that need the fixture on a rerun.
                            for &index in &groups[group].indices { failed_ids[index] = true; }
                        }
                        groups[group].state = State::Done;
                        report(Message::SuiteFinished(groups[group].suite.metadata().id().into(), result, configuration.artifacts.clone()), &mut reporting_error).await;
                        skip(&mut groups[group], &cases, &mut failed_ids, &mut tally, &configuration, &mut reporting_error, SkipReason::SuiteSetupFailed).await;
                    }
                }
            }
        }
    }
    let mut outcome = Outcome {
        totals: tally.totals,
        fixtures_failed: tally.fixtures_failed,
        failed_cases: cases
            .iter()
            .zip(failed_ids)
            .filter(|(_, failed)| *failed)
            .map(|(case, _)| case.id())
            .collect(),
        selected: cases.len(),
        stop: None,
    };
    if let Some(error) = reporting_error {
        outcome.stop = Some(Stop::Delivery(error));
        return Ok(outcome);
    }
    let rerun = batch::rerun_hint(&outcome.failed_cases, configuration.failures.as_deref());
    batch::report(tally.summary(true, rerun)).await?;
    if crate::interrupt::interrupted() {
        outcome.stop = Some(Stop::Interrupted);
    } else {
        debug_assert_eq!(outcome.totals.total(), cases.len());
    }
    Ok(outcome)
}
