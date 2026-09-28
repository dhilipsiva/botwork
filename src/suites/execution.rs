//! Suite owners retain their context while cases borrow immutable inputs. Only
//! setup, case execution, and teardown occupy the bounded execution slots.
use super::*;
use crate::batch::{Identity, Message, Outcome};
use botwork::core::eval::{evaluate_suite_fixture_async, FixtureInputs};
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
    }
}

async fn report(message: Message, error: &mut Option<CliError>) {
    if error.is_none() {
        if let Err(failure) = batch::report(message).await {
            *error = Some(failure);
        }
    }
}

async fn skip(
    group: &mut Group,
    cases: &[SelectedCase],
    failed: &mut [bool],
    count: &mut usize,
    reporting_error: &mut Option<CliError>,
    reason: &'static str,
) {
    for index in group.pending.drain(..) {
        *count += 1;
        failed[index] = true;
        report(
            Message::Skipped(identity(index, cases), reason),
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
    let context = tokio::task::spawn_blocking(move || {
        let mut context = Context::with_control(configuration.limits.clone(), control)?;
        let variables =
            botwork::core::input::load_variables(&configuration.files, &configuration.settings)?;
        context.init_statements();
        context.set_input_variables(variables)?;
        context.checkpoint()?;
        context.set_statement_tracing(configuration.debug);
        Ok::<_, CliError>(context)
    })
    .await
    .map_err(|_| batch::task_failure())??;
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
    let configuration = Arc::new(configuration);
    let (ready_tx, mut ready_rx) = mpsc::channel(suite::MAX_SUITES);
    let mut running = JoinSet::new();
    let mut identities = HashMap::new();
    let mut work = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut fixtures_failed = 0;
    let mut failed_ids = vec![false; cases.len()];
    let mut reporting_error = None;
    loop {
        if reporting_error.is_some() {
            for group in &mut groups {
                group.pending.clear();
            }
        }
        while work < jobs {
            if reporting_error.is_some() {
                for group in &mut groups {
                    group.pending.clear();
                }
            }
            let Some(action) = action(&groups, reporting_error.is_some()) else {
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
                        &mut skipped,
                        &mut reporting_error,
                        "suite control stopped",
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
                    let task = running.spawn(owner(
                        group,
                        Arc::clone(&groups[group].suite),
                        Arc::clone(&configuration),
                        ready_tx.clone(),
                    ));
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
                    let task = running.spawn(crate::run_case(case, configuration, inputs));
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
                let (id, result) = match completed.expect("owned tasks") {
                    Ok(value) => value,
                    Err(error) => (error.id(), Err(batch::task_failure())),
                };
                match identities.remove(&id).expect("admitted task") {
                    Task::Case { group, index } => {
                        work -= 1;
                        groups[group].active -= 1;
                        if result.is_err() { failed += 1; failed_ids[index] = true; }
                        report(Message::Finished(identity(index, &cases), result), &mut reporting_error).await;
                    }
                    Task::Owner(group) => {
                        if matches!(groups[group].state, State::SettingUp | State::Finishing) { work -= 1; }
                        if result.is_err() {
                            fixtures_failed += 1;
                            // A failed shared fixture affects every selected borrower,
                            // including passed cases that need the fixture on a rerun.
                            for &index in &groups[group].indices { failed_ids[index] = true; }
                        }
                        groups[group].state = State::Done;
                        report(Message::SuiteFinished(groups[group].suite.metadata().id().into(), result), &mut reporting_error).await;
                        skip(&mut groups[group], &cases, &mut failed_ids, &mut skipped, &mut reporting_error, "suite setup did not complete").await;
                    }
                }
            }
        }
    }
    if let Some(error) = reporting_error {
        return Err(error);
    }
    batch::report(Message::Summary {
        total: cases.len(),
        succeeded: cases.len() - failed - skipped,
        failed,
        skipped,
        fixtures_failed,
        cases: true,
    })
    .await?;
    Ok(Outcome {
        total: cases.len(),
        failed,
        skipped,
        fixtures_failed,
        failed_cases: cases
            .iter()
            .zip(failed_ids)
            .filter(|(_, failed)| *failed)
            .map(|(case, _)| case.id())
            .collect(),
    })
}
