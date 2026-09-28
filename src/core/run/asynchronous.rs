use super::*;
use crate::core::eval::execution;
use std::future::Future;

// Wrap host-owned trees before constructing a future, including an unpolled one.
pub(super) struct PendingRun {
    options: RunOptions,
    variables: Owned<BTreeMap<String, Literal>>,
    start: Instant,
    control_start: tokio::time::Instant,
}

impl PendingRun {
    pub(super) fn new(mut options: RunOptions) -> Self {
        let variables = Owned::new(std::mem::take(&mut options.variables));
        Self {
            options,
            variables,
            start: Instant::now(),
            control_start: tokio::time::Instant::now(),
        }
    }
}

pub(super) struct ActiveRun {
    pub(super) context: Context,
    result_limits: ResultLimits,
    start: Instant,
}

impl ActiveRun {
    pub(super) fn finish(self, result: EvaluationResult<Literal>) -> RunResult {
        let result = self
            .context
            .after_evaluation(result)
            .map_err(|error| self.context.runtime_diagnostic(error, None, false));
        let steps = self.context.budget.as_ref().map_or(0, RunBudget::used);
        let (result, variables, snapshot_error) =
            self.context.finish_result(result, &self.result_limits);
        RunResult {
            result,
            variables,
            snapshot_error,
            steps,
            elapsed: self.start.elapsed(),
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
        } = pending;
        let result_limits = options.limits.results.clone();
        let mut context = Context::default();
        context.asynchronous = asynchronous;
        let result = (|| {
            options.control.checkpoint()?;
            options.limits.validate()?;
            let environment = Arc::new(RunEnvironment::prepare(&options, control_start)?);
            context.working_directory = Ok(environment.directory.clone());
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
        self.run_async(Input::Source(name, source), PendingRun::new(options))
    }

    /// Reuse immutable syntax while keeping all mutable execution state local to this future.
    pub fn run_program_async<'a>(
        &'a self,
        program: &'a Program,
        options: RunOptions,
    ) -> impl Future<Output = RunResult> + 'a {
        self.run_async(Input::Program(program), PendingRun::new(options))
    }

    /// Read and run a local file. File loading remains synchronous and bounded;
    /// waiting NativeOperation calls and CPU-loop yields are asynchronous.
    pub fn run_file_async(
        &self,
        path: impl AsRef<Path>,
        options: RunOptions,
    ) -> impl Future<Output = RunResult> + '_ {
        self.run_async(
            Input::File(path.as_ref().to_owned()),
            PendingRun::new(options),
        )
    }

    async fn run_async(&self, input: Input<'_>, pending: PendingRun) -> RunResult {
        let (mut active, prepared) = self.prepare_run(pending, true);
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
                            let source =
                                context.read_source(&path).map_err(|error| match error {
                                    SourceFailure::Io(error) => context.formatted_error(
                                        BWErr::SourceRead,
                                        format_args!("{name}: {error}"),
                                        None,
                                        false,
                                    ),
                                    SourceFailure::Diagnostic(error) => error.into(),
                                })?;
                            let program = context.parse_source(name, &source)?;
                            execution::evaluate_program_runtime(&program, context).await
                        }
                    }
                })
                .await
            }
        };
        active.finish(result)
    }
}
