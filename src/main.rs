use botwork::core::{
    ast::Program,
    diagnostic::Diagnostic,
    eval::{evaluate_program_async, Context, FixtureInputs},
    grammar::BWErr,
    input::load_variables,
    operation::OperationControl,
    report::{Recording, RunIdentity, RunRecord},
    run::{
        CleanupLimits, OutputLimits, RunLimits, DEFAULT_OUTPUT_BYTES, DEFAULT_OUTPUT_RECORD_BYTES,
        DEFAULT_STEPS, MAX_EVALUATION_DEPTH,
    },
    syntax_limits::DEFAULT_SOURCE_BYTES,
};
use clap::Parser as Clap;
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

mod assertion_artifacts;
mod atomic_json;
mod batch;
mod report_json;
mod suites;

/// Run Botwork automation scripts.
#[derive(Clap, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Botwork file to run (repeatable; each occurrence starts a fresh run)
    #[arg(short, long, required_unless_present_any = ["suite", "list_statements", "statement_help"])]
    file: Vec<PathBuf>,
    /// Discover cases from an explicit suite file (repeatable; paths keep their order)
    #[arg(long, conflicts_with_all = ["file", "list_statements", "statement_help"])]
    suite: Vec<PathBuf>,
    /// Select a stable suite/case or suite/case/row ID (repeatable)
    #[arg(long, requires = "suite")]
    case: Vec<String>,
    /// Include cases with any of these inherited or local tags (repeatable)
    #[arg(long, requires = "suite")]
    tag: Vec<String>,
    /// Exclude cases with any of these inherited or local tags (repeatable)
    #[arg(long, requires = "suite")]
    exclude_tag: Vec<String>,
    /// List selected case metadata as JSON lines without executing libraries or cases
    #[arg(long, requires = "suite", conflicts_with_all = ["jobs", "failures", "debug", "variables", "variable_files", "max_steps", "max_call_depth", "max_evaluation_depth", "timeout_ms"])]
    list_cases: bool,
    /// Restrict selection to IDs from a completed failed-case record
    #[arg(long, requires = "suite", value_name = "PATH")]
    rerun_failed: Option<PathBuf>,
    /// Write a failed-case record; invalidate it before discovery/execution
    #[arg(long, requires = "suite", value_name = "PATH")]
    failures: Option<PathBuf>,
    /// Save full permitted assertion operands in a fresh directory beneath PATH
    #[arg(long, value_name = "PATH", conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    assertion_artifacts: Option<PathBuf>,
    /// Write a versioned botwork-report JSON document; mark it incomplete before running
    #[arg(long, value_name = "PATH", conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    report_json: Option<PathBuf>,
    /// Maximum simultaneous runs, including file/input preparation (1-64)
    #[arg(short = 'j', long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=64))]
    jobs: u8,
    /// List built-in statement headers without executing a file
    #[arg(long, conflicts_with_all = ["file", "jobs", "statement_help", "debug", "variables", "variable_files", "max_steps", "max_call_depth", "max_evaluation_depth", "timeout_ms"])]
    list_statements: bool,
    /// Show a built-in's parameters, return kinds, and documented errors
    #[arg(long, value_name = "HEADER", conflicts_with_all = ["file", "jobs", "list_statements", "debug", "variables", "variable_files", "max_steps", "max_call_depth", "max_evaluation_depth", "timeout_ms"])]
    statement_help: Option<String>,
    /// Trace top-level statement locations on stderr
    #[arg(long)]
    debug: bool,
    /// Set a root variable from JSON (repeatable; overrides all variable files)
    #[arg(long = "var", value_name = "NAME=JSON")]
    variables: Vec<String>,
    /// Read root variables from a JSON object (repeatable; later files override earlier)
    #[arg(long = "vars-file", value_name = "PATH")]
    variable_files: Vec<PathBuf>,
    /// Maximum evaluation steps before terminating each run
    #[arg(long, default_value_t = DEFAULT_STEPS)]
    max_steps: u64,
    /// Maximum entered native or custom calls
    #[arg(long, default_value_t = 32)]
    max_call_depth: usize,
    /// Maximum combined evaluation depth (ceiling 96)
    #[arg(long, default_value_t = MAX_EVALUATION_DEPTH)]
    max_evaluation_depth: usize,
    /// Maximum bytes in one Log, debug trace, or statement-help record
    #[arg(long, default_value_t = DEFAULT_OUTPUT_RECORD_BYTES)]
    max_output_record_bytes: usize,
    /// Maximum output bytes per run or help request (failed writes also count)
    #[arg(long, default_value_t = DEFAULT_OUTPUT_BYTES)]
    max_output_bytes: usize,
    /// Per-run cooperative timeout in milliseconds, including loading/parsing after admission
    #[arg(long)]
    timeout_ms: Option<u64>,
    /// Whole lifetime timeout for each suite owning SuiteSetup/SuiteTeardown
    #[arg(long, requires = "suite", conflicts_with = "list_cases")]
    suite_timeout_ms: Option<u64>,
    /// Evaluation steps available to each independent Finally cleanup
    #[arg(long, default_value_t = 10_000, conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    max_cleanup_steps: u64,
    /// Cooperative timeout for each independent Finally cleanup, in milliseconds
    #[arg(long, default_value_t = 5_000, conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    cleanup_timeout_ms: u64,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("Writing assertion artifacts failed: {source}\n{original}")]
    Artifact { source: io::Error, original: String },
    #[error("Suite execution failed: {failed} cases failed, {skipped} skipped, {fixtures_failed} suite fixtures failed")]
    Suites {
        failed: usize,
        skipped: usize,
        fixtures_failed: usize,
    },
    #[error("Batch failed: {failed} of {total} runs failed")]
    Batch { failed: usize, total: usize },
    #[error("{file}: {source}")]
    Read { file: PathBuf, source: io::Error },
    #[error(transparent)]
    Script(#[from] Diagnostic),
    #[error("{file}: {source}")]
    SourceLimit {
        file: PathBuf,
        source: Box<Diagnostic>,
    },
}

/// Run a file, recording it when a report requested a recording.
async fn run(
    file: &Path,
    debug: bool,
    files: &[PathBuf],
    settings: &[String],
    limits: RunLimits,
    timeout_ms: Option<u64>,
    recording: Option<Recording>,
) -> (Result<(), CliError>, Option<RunRecord>) {
    let result = run_recorded(
        file,
        debug,
        files,
        settings,
        limits,
        timeout_ms,
        recording.as_ref(),
    )
    .await;
    let record = finish_recording(recording, &result);
    (result, record)
}

fn finish_recording(
    recording: Option<Recording>,
    result: &Result<(), CliError>,
) -> Option<RunRecord> {
    let error = result.as_ref().err().map(report_json::diagnostic);
    recording.map(|recording| recording.finish(error.as_ref().map_or(Ok(()), Err)))
}

async fn run_recorded(
    file: &Path,
    debug: bool,
    files: &[PathBuf],
    settings: &[String],
    limits: RunLimits,
    timeout_ms: Option<u64>,
    recording: Option<&Recording>,
) -> Result<(), CliError> {
    let control = run_control(timeout_ms)?;
    let file = file.to_owned();
    let files = files.to_vec();
    let settings = settings.to_vec();
    // Each admitted run owns one preparation job. File/variable loading and parsing
    // finish off the executor before the owned program/context are handed back.
    let (program, context) = tokio::task::spawn_blocking(move || {
        prepare(&file, debug, &files, &settings, limits, control)
    })
    .await
    .map_err(|_| {
        Diagnostic::new(BWErr::AsyncRuntime(
            "CLI preparation worker failed before completing".into(),
        ))
    })??;
    let mut context = context;
    if let Some(recording) = recording {
        context.attach_recording(recording)?;
    }
    evaluate_program_async(&program, context).await?;
    Ok(())
}

fn run_control(timeout_ms: Option<u64>) -> Result<OperationControl, CliError> {
    let deadline = timeout_ms
        .map(|milliseconds| {
            tokio::time::Instant::now()
                .checked_add(Duration::from_millis(milliseconds))
                .ok_or_else(|| {
                    Diagnostic::new(BWErr::RunConfiguration(
                        "Timeout exceeds the monotonic clock range".into(),
                    ))
                })
        })
        .transpose()?;
    let control = OperationControl::default().child(deadline);
    control.checkpoint()?;
    Ok(control)
}

/// Run one selected case, recording it when a report requested a recording.
async fn run_case(
    case: botwork::core::suite::SelectedCase,
    configuration: std::sync::Arc<batch::Configuration>,
    fixture: Option<FixtureInputs>,
    recording: Option<Recording>,
) -> (Result<(), CliError>, Option<RunRecord>) {
    let result = run_case_recorded(case, configuration, fixture, recording.as_ref()).await;
    let record = finish_recording(recording, &result);
    (result, record)
}

async fn run_case_recorded(
    case: botwork::core::suite::SelectedCase,
    configuration: std::sync::Arc<batch::Configuration>,
    fixture: Option<FixtureInputs>,
    recording: Option<&Recording>,
) -> Result<(), CliError> {
    let deadline = run_control(configuration.timeout_ms)?.deadline();
    let control = fixture
        .as_ref()
        .map_or_else(OperationControl::default, |fixture| {
            fixture.control().clone()
        })
        .child(deadline);
    control.checkpoint()?;
    let (program, context) = tokio::task::spawn_blocking(move || {
        let mut context = Context::with_host_environment(configuration.limits.clone(), control)?;
        let variables = match &fixture {
            Some(_) => std::collections::BTreeMap::new(),
            None => load_variables(&configuration.files, &configuration.settings)?,
        };
        let variables = case.bind_inputs(variables, &configuration.limits.values)?;
        let program = case.program();
        context.init_statements();
        context.set_input_variables(variables)?;
        if let Some(fixture) = fixture {
            fixture.inherit_into(&mut context)?;
        }
        context.checkpoint()?;
        context.set_statement_tracing(configuration.debug);
        Ok::<_, CliError>((program, context))
    })
    .await
    .map_err(|_| {
        Diagnostic::new(BWErr::AsyncRuntime(
            "Case preparation worker failed before completing".into(),
        ))
    })??;
    let mut context = context;
    if let Some(recording) = recording {
        context.attach_recording(recording)?;
    }
    evaluate_program_async(&program, context).await?;
    Ok(())
}

fn prepare(
    file: &Path,
    debug: bool,
    files: &[PathBuf],
    settings: &[String],
    limits: RunLimits,
    control: OperationControl,
) -> Result<(Program, Context), CliError> {
    let mut context = Context::with_host_environment(limits, control)?;
    let variables = load_variables(files, settings)?;
    let mut bytes = Vec::new();
    let read_error = |source| CliError::Read {
        file: file.to_owned(),
        source,
    };
    File::open(file)
        .map_err(read_error)?
        .take(DEFAULT_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(read_error)?;
    if bytes.len() > DEFAULT_SOURCE_BYTES {
        return Err(CliError::SourceLimit {
            file: file.to_owned(),
            source: Box::new(Diagnostic::new(BWErr::ResourceLimit {
                resource: "source bytes",
                limit: DEFAULT_SOURCE_BYTES as u64,
            })),
        });
    }
    let source = String::from_utf8(bytes)
        .map_err(|error| read_error(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    let program = Program::parse_detailed(&file.display().to_string(), &source)?;
    context.init_statements();
    context.set_input_variables(variables)?;
    context.checkpoint()?;
    context.set_statement_tracing(debug);
    Ok((program, context))
}

fn main() -> ExitCode {
    if let Some(status) = botwork::core::worker::guardian_main() {
        return ExitCode::from(status);
    }
    let args = Args::parse();
    let output_limits = OutputLimits {
        record_bytes: args.max_output_record_bytes,
        total_bytes: args.max_output_bytes,
    };
    let result = if args.list_statements || args.statement_help.is_some() {
        statement_help(args.statement_help.as_deref(), output_limits)
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                CliError::Script(Diagnostic::new(BWErr::AsyncRuntime(error.to_string())))
            })
            .and_then(|runtime| {
                let artifacts = args
                    .assertion_artifacts
                    .as_deref()
                    .map(assertion_artifacts::Store::new)
                    .transpose()
                    .map_err(|source| CliError::Read {
                        file: args.assertion_artifacts.clone().expect("artifact path"),
                        source,
                    })?
                    .map(std::sync::Arc::new);
                let mode = match (args.suite.is_empty(), args.file.len()) {
                    (false, _) => "suites",
                    (true, 1) => "file",
                    _ => "batch",
                };
                // Mark the report incomplete before discovery or any run effects.
                let report = args
                    .report_json
                    .clone()
                    .map(|path| report_json::Report::begin(path, mode))
                    .transpose()?
                    .map(std::sync::Arc::new);
                let limits = RunLimits {
                    output: output_limits,
                    steps: args.max_steps,
                    call_depth: args.max_call_depth,
                    evaluation_depth: args.max_evaluation_depth,
                    cleanup: CleanupLimits {
                        steps: args.max_cleanup_steps,
                        timeout: Duration::from_millis(args.cleanup_timeout_ms),
                    },
                    ..RunLimits::default()
                };
                if !args.suite.is_empty() {
                    runtime.block_on(suites::run(
                        suites::Request::from(&args),
                        usize::from(args.jobs),
                        batch::Configuration {
                            artifacts,
                            report,
                            debug: args.debug,
                            files: args.variable_files,
                            settings: args.variables,
                            limits,
                            timeout_ms: args.timeout_ms,
                            suite_timeout_ms: args.suite_timeout_ms,
                        },
                    ))
                } else if args.file.len() == 1 {
                    let path = args.file[0].display().to_string();
                    let recording = report
                        .as_ref()
                        .map(|_| report_json::recording(RunIdentity::new(path.clone(), path)));
                    let (result, record) = runtime.block_on(run(
                        &args.file[0],
                        args.debug,
                        &args.variable_files,
                        &args.variables,
                        limits,
                        args.timeout_ms,
                        recording,
                    ));
                    let status = batch::status(&result);
                    let mut exported = Vec::new();
                    let result = assertion_artifacts::write_single(
                        result,
                        artifacts.as_deref(),
                        &args.file[0],
                        &mut exported,
                    );
                    match (report, record) {
                        (Some(report), Some(record)) => {
                            report.run(1, record, &exported);
                            let mut totals = botwork::core::acceptance::CaseTotals::default();
                            totals.record(status)?;
                            let verdict =
                                totals.finish(0, botwork::core::acceptance::Delivery::Complete);
                            batch::publish_report(result, report.finish(&verdict, 1))
                        }
                        _ => result,
                    }
                } else {
                    runtime.block_on(batch::run(
                        args.file,
                        usize::from(args.jobs),
                        batch::Configuration {
                            artifacts,
                            report,
                            debug: args.debug,
                            files: args.variable_files,
                            settings: args.variables,
                            limits,
                            timeout_ms: args.timeout_ms,
                            suite_timeout_ms: args.suite_timeout_ms,
                        },
                    ))
                }
            })
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Batch { .. } | CliError::Suites { .. }) => ExitCode::FAILURE,
        Err(error) => {
            // Failure reporting has its own bounded allowance, so an exhausted
            // script output budget does not hide the reason for failure.
            let _ = Context::default()
                .write_output(&mut io::stderr().lock(), format_args!("{error}\n"));
            ExitCode::FAILURE
        }
    }
}

fn statement_help(header: Option<&str>, output: OutputLimits) -> Result<(), CliError> {
    let mut context = Context::with_limits(RunLimits {
        output,
        ..RunLimits::default()
    })?;
    context.init_statements();
    let mut stdout = io::stdout().lock();
    match header {
        Some(header) => {
            let signature = context
                .statement_signature(header)?
                .ok_or_else(|| Diagnostic::new(BWErr::StatementNotDefined(header.into())))?;
            context.write_output(&mut stdout, format_args!("{}\n", signature.display_help()))?;
        }
        None => {
            for signature in context.statement_signatures() {
                context.write_output(
                    &mut stdout,
                    format_args!("{}\n", signature.header().text().trim()),
                )?;
            }
        }
    }
    Ok(())
}
