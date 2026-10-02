use super::*;
use crate::core::eval::execution;
use std::future::Future;

// Wrap host-owned trees before constructing a future, including an unpolled one.
type Recording = Option<crate::core::report::Recording>;

pub(super) struct PendingRun {
    options: RunOptions,
    variables: Owned<BTreeMap<String, Literal>>,
    start: Instant,
    control_start: tokio::time::Instant,
    environment: Option<RunEnvironment>,
    recording: Recording,
}

impl PendingRun {
    /// Recording starts here, so failed preparation still yields a terminal record.
    pub(super) fn new(mut options: RunOptions, name: &str) -> Self {
        let variables = Owned::new(std::mem::take(&mut options.variables));
        let recording = options.record.take().map(|record| {
            let mut identity = record.identity;
            if identity.id.is_empty() {
                identity.id = name.to_owned();
            }
            if identity.name.is_empty() {
                identity.name = identity.id.clone();
            }
            crate::core::report::Recording::start(crate::core::report::RecordOptions {
                identity,
                secrets: options.secrets.clone(),
                ..record
            })
        });
        Self {
            options,
            variables,
            start: Instant::now(),
            control_start: tokio::time::Instant::now(),
            environment: None,
            recording,
        }
    }
}

pub(super) struct ActiveRun {
    pub(super) context: Context,
    result_limits: ResultLimits,
    start: Instant,
    recording: Recording,
}

impl ActiveRun {
    /// Wait for the workers the run left unresolved; see
    /// [`Context::settle_workers_blocking`].
    pub(super) fn settle_blocking(&self) -> Option<Diagnostic> {
        self.context.settle_workers_blocking()
    }

    /// As [`ActiveRun::settle_blocking`], waiting on a blocking thread.
    pub(super) async fn settle(&self) -> Option<Diagnostic> {
        self.context.settle_workers().await
    }

    pub(super) fn finish(
        self,
        result: EvaluationResult<Literal>,
        unsettled: Option<Diagnostic>,
    ) -> RunResult {
        let result = self
            .context
            .after_evaluation(result)
            .map_err(|error| self.context.runtime_diagnostic(error, None, false));
        let result = workers::with_unsettled(result, unsettled, self.context.budget.as_ref());
        let steps = self.context.budget.as_ref().map_or(0, RunBudget::used);
        let (result, variables, snapshot_error) =
            self.context.finish_result(result, &self.result_limits);
        let record = self
            .recording
            .map(|recording| recording.finish(result.as_ref().map(|_| ())));
        RunResult {
            result,
            variables,
            snapshot_error,
            steps,
            elapsed: self.start.elapsed(),
            record,
        }
    }
}

enum Input<'a> {
    Source(&'a str, &'a str),
    Program(&'a Program),
    File(PathBuf),
}

impl Engine {
    pub(super) fn prepare_run(
        &self,
        pending: PendingRun,
        asynchronous: bool,
    ) -> (ActiveRun, EvaluationResult<()>) {
        let PendingRun {
            options,
            variables,
            start,
            control_start,
            environment,
            recording,
        } = pending;
        let result_limits = options.limits.results.clone();
        // Successful setup installs its canonical snapshot below. Failed setup
        // never evaluates source and does not need another current_dir syscall.
        let mut context = Context::with_directory_snapshot(Ok(PathBuf::new()));
        context.asynchronous = asynchronous;
        let result = (|| {
            options.control.checkpoint()?;
            options.limits.validate()?;
            let mut environment = match environment {
                Some(environment) => environment,
                None => RunEnvironment::prepare(&options, control_start)?,
            };
            environment.recorder = recording
                .as_ref()
                .map(|recording| recording.recorder().clone());
            environment.compiled = Some(self.compiled.clone());
            let environment = Arc::new(environment);
            context.working_directory = Ok(environment.working_directory().to_owned());
            context.budget = Some(RunBudget::new(options.limits, environment.control.clone()));
            context.environment = Some(environment);
            context.copy_native_template(&self.template)?;
            context.set_input_variables_runtime(variables.into_inner())?;
            context.checkpoint()?;
            Ok(())
        })();
        (
            ActiveRun {
                context,
                result_limits,
                start,
                recording,
            },
            result,
        )
    }

    /// Run source without blocking on registered asynchronous operations.
    /// Dropping the future drops DSL state and requests operation cancellation.
    /// Cooperative blocking operations still require their callback to stop; use
    /// isolated operations for hard host-operation deadlines.
    pub fn run_source_async<'a>(
        &'a self,
        name: &'a str,
        source: &'a str,
        options: RunOptions,
    ) -> impl Future<Output = RunResult> + 'a {
        self.run_async(Input::Source(name, source), PendingRun::new(options, name))
    }

    /// Reuse immutable syntax while keeping all mutable execution state local to this future.
    pub fn run_program_async<'a>(
        &'a self,
        program: &'a Program,
        options: RunOptions,
    ) -> impl Future<Output = RunResult> + 'a {
        self.run_async(
            Input::Program(program),
            PendingRun::new(options, program.source.name()),
        )
    }

    /// Read and run a local file using bounded filesystem workers. Parsing remains
    /// bounded synchronous CPU work; native calls and filesystem waits yield the executor.
    pub fn run_file_async(
        &self,
        path: impl AsRef<Path>,
        options: RunOptions,
    ) -> impl Future<Output = RunResult> + '_ {
        self.run_async(
            Input::File(path.as_ref().to_owned()),
            PendingRun::new(options, &path.as_ref().display().to_string()),
        )
    }

    async fn run_async(&self, input: Input<'_>, pending: PendingRun) -> RunResult {
        let (mut active, prepared) = self.prepare_async(pending).await;
        let context = &mut active.context;
        let result = match prepared {
            Err(error) => Err(error),
            Ok(()) => {
                (async {
                    match input {
                        Input::Source(name, source) => {
                            context.check_source_size(source.len())?;
                            let program = context.parse_source(name, source)?;
                            execution::evaluate_program_runtime(&program, context).await
                        }
                        Input::Program(program) => {
                            context.check_source_size(program.source.text().len())?;
                            context.check_syntax(program.source.name(), program.source.text())?;
                            execution::evaluate_program_runtime(program, context).await
                        }
                        Input::File(path) => {
                            let path = context
                                .environment
                                .as_ref()
                                .expect("configured run")
                                .directory
                                .join(path);
                            let name = path.to_str().ok_or_else(|| {
                                context.formatted_error(
                                    BWErr::SourceRead,
                                    format_args!("Source paths must be valid UTF-8"),
                                    None,
                                    false,
                                )
                            })?;
                            let source = context.read_source_async(&path).await.map_err(
                                |error| match error {
                                    SourceFailure::Io(error) => context.formatted_error(
                                        BWErr::SourceRead,
                                        format_args!("{name}: {error}"),
                                        None,
                                        false,
                                    ),
                                    SourceFailure::Diagnostic(error) => error.into(),
                                },
                            )?;
                            let program = context.parse_source(name, &source)?;
                            execution::evaluate_program_runtime(&program, context).await
                        }
                    }
                })
                .await
            }
        };
        let unsettled = active.settle().await;
        active.finish(result, unsettled)
    }

    async fn prepare_async(&self, mut pending: PendingRun) -> (ActiveRun, EvaluationResult<()>) {
        let start = pending.start;
        // Keep the recorder outside the preparation worker so failures still finish it.
        let recording = pending.recording.take();
        let result_limits = pending.options.limits.results.clone();
        let prepared = async {
            pending.options.control.checkpoint()?;
            pending.options.limits.validate()?;
            let control = RunEnvironment::control_at(&pending.options, pending.control_start)?;
            let (mut pending, mut environment) =
                blocking_io::run(control.clone(), move |worker_control| {
                    let environment = RunEnvironment::prepare_with_control(
                        &pending.options,
                        worker_control.clone(),
                    )
                    .map_err(SourceFailure::Diagnostic)?;
                    Ok((pending, environment))
                })
                .await
                .map_err(|error| match error {
                    SourceFailure::Diagnostic(error) => error,
                    SourceFailure::Io(_) => {
                        unreachable!("environment preparation returns structured diagnostics")
                    }
                })?;
            // The worker's child is cancelled on handoff/drop; execution uses
            // the independently retained run child, with the same deadline.
            environment.control = control;
            pending.environment = Some(environment);
            Ok::<_, Diagnostic>(pending)
        }
        .await;
        match prepared {
            Ok(mut pending) => {
                pending.recording = recording;
                self.prepare_run(pending, true)
            }
            Err(error) => (
                ActiveRun {
                    context: Context::with_directory_snapshot(Ok(PathBuf::new())),
                    result_limits,
                    start,
                    recording,
                },
                Err(error.into()),
            ),
        }
    }
}
