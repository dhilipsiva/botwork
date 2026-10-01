use botwork::core::{
    ast::Program,
    diagnostic::Diagnostic,
    eval::{evaluate_program_async, Context, FixtureInputs},
    grammar::BWErr,
    listener::ListenerOptions,
    operation::{OperationControl, DEFAULT_STOP_GRACE},
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
mod atomic_file;
mod batch;
mod check;
mod formatting;
mod interrupt;
mod listener;
mod lsp;
mod report_json;
mod secrets;
mod suites;

/// Run Botwork automation scripts.
#[derive(Clap, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Botwork file to run (repeatable; each occurrence starts a fresh run)
    #[arg(short, long, required_unless_present_any = ["suite", "list_statements", "statement_help", "reconcile_report", "lsp", "fetch"])]
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
    /// Serve the Language Server Protocol on stdin and stdout
    #[arg(long, exclusive = true)]
    lsp: bool,
    /// Resolve the packages and URL files that DIRECTORY's botwork.toml names
    /// (by default the current directory's), fetch them into the cache, and
    /// write botwork.lock
    #[arg(long, value_name = "DIRECTORY", num_args = 0..=1, default_missing_value = ".", conflicts_with_all = ["file", "suite", "check", "format", "format_check", "list_cases", "list_statements", "statement_help", "reconcile_report"])]
    fetch: Option<PathBuf>,
    /// With --fetch: use only botwork.lock and the cache, and fetch nothing
    #[arg(long, requires = "fetch")]
    offline: bool,
    /// With --fetch: fail rather than change botwork.lock
    #[arg(long, requires = "fetch")]
    locked: bool,
    /// Check files or suites without running them: syntax, control placement, and lint rules
    #[arg(long, conflicts_with_all = ["list_cases", "list_statements", "statement_help", "reconcile_report", "report_json", "report_html", "listener", "failures", "rerun_failed", "assertion_artifacts"])]
    check: bool,
    /// Rewrite files or suites in canonical layout; `*.dataset.botwork` files are datasets
    #[arg(long, conflicts_with_all = ["check", "format_check", "list_cases", "list_statements", "statement_help", "reconcile_report", "report_json", "report_html", "listener", "failures", "rerun_failed", "assertion_artifacts"])]
    format: bool,
    /// Report files or suites whose layout is not canonical, without changing them
    #[arg(long, conflicts_with_all = ["check", "list_cases", "list_statements", "statement_help", "reconcile_report", "report_json", "report_html", "listener", "failures", "rerun_failed", "assertion_artifacts"])]
    format_check: bool,
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
    /// Write a self-contained HTML report; mark it incomplete before running
    #[arg(long, value_name = "PATH", conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    report_html: Option<PathBuf>,
    /// Finish a report whose invocation was forcibly terminated: started runs without an outcome become interrupted
    #[arg(long, value_name = "PATH", conflicts_with_all = ["file", "suite", "list_statements", "statement_help", "report_json", "report_html", "listener", "assertion_artifacts"])]
    reconcile_report: Option<PathBuf>,
    /// Stream execution events as JSON Lines to PROGRAM's stdin (run without a shell)
    #[arg(long, value_name = "PROGRAM", conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    listener: Option<PathBuf>,
    /// Argument for the listener program (repeatable)
    #[arg(
        long = "listener-arg",
        value_name = "ARG",
        requires = "listener",
        allow_hyphen_values = true
    )]
    listener_args: Vec<std::ffi::OsString>,
    /// Events queued for a listener before a slow listener is detached (1-1048576)
    #[arg(long, value_name = "EVENTS", requires = "listener", value_parser = clap::value_parser!(u32).range(1..=1_048_576))]
    listener_queue: Option<u32>,
    /// Milliseconds to wait for the listener after the last event (1-3600000)
    #[arg(long, value_name = "MS", requires = "listener", value_parser = clap::value_parser!(u64).range(1..=3_600_000))]
    listener_timeout_ms: Option<u64>,
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
    /// Mark an input variable secret: mask its values from all output (repeatable)
    #[arg(long = "secret", value_name = "NAME", conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    secrets: Vec<String>,
    /// Define a secret string input from an environment variable (repeatable)
    #[arg(long = "secret-env", value_name = "NAME=VARIABLE", conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    secret_environment: Vec<String>,
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
    /// After a stop, milliseconds to wait for started blocking work, such as a
    /// system call that cannot be interrupted, before abandoning it
    #[arg(long, default_value_t = DEFAULT_STOP_GRACE.as_millis() as u64, value_parser = clap::value_parser!(u64).range(0..=3_600_000), conflicts_with_all = ["list_cases", "list_statements", "statement_help"])]
    stop_grace_ms: u64,
}

/// A runtime that waits at most [`EXIT_JOIN`] for its threads when dropped, not
/// for abandoned blocking jobs, which may never return; see
/// [`OperationControl::stop_grace`].
struct Runtime(Option<tokio::runtime::Runtime>);

/// How long a dropped runtime waits for its threads to end. Threads that end in
/// time are joined, so none is still exiting when the process exits: a thread's
/// library destructors can crash racing exit-time cleanup, as OpenSSL's did on
/// macOS once Python's `asyncio` had loaded it into a blocking thread.
const EXIT_JOIN: Duration = Duration::from_millis(250);

impl std::ops::Deref for Runtime {
    type Target = tokio::runtime::Runtime;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("runtime")
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_timeout(EXIT_JOIN);
        }
    }
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
    /// A signal stopped the invocation; the summary has already been printed.
    #[error("{0}")]
    Interrupted(String),
    /// Reconciliation succeeded; its verdict is interrupted, so the status is 1.
    #[error("{0}")]
    Reconciled(String),
    #[error("{file}: {source}")]
    Read { file: PathBuf, source: io::Error },
    #[error(transparent)]
    Script(#[from] Diagnostic),
    #[error("{file}: {source}")]
    SourceLimit {
        file: PathBuf,
        source: Box<Diagnostic>,
    },
    /// `--check` found errors; its findings and summary are already printed.
    #[error("check failed")]
    Checked,
}

/// Parsed imported modules shared by every run, case, and fixture of this
/// invocation; each still builds its own module state.
fn compiled_modules() -> &'static botwork::core::eval::CompiledModules {
    static MODULES: std::sync::OnceLock<botwork::core::eval::CompiledModules> =
        std::sync::OnceLock::new();
    MODULES.get_or_init(botwork::core::eval::CompiledModules::default)
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
    let (program, context) = prepared(
        &control.clone(),
        move || prepare(&file, debug, &files, &settings, limits, control),
        "CLI preparation worker failed before completing",
    )
    .await?;
    let mut context = context;
    if let Some(recording) = recording {
        context.attach_recording(recording)?;
    }
    evaluate_program_async(&program, context).await?;
    Ok(())
}

/// Run preparation off the executor. A stop ends the wait at once; preparation
/// still running after the stop grace is abandoned, like any started blocking job.
async fn prepared<T: Send + 'static>(
    control: &OperationControl,
    work: impl FnOnce() -> Result<T, CliError> + Send + 'static,
    failure: &'static str,
) -> Result<T, CliError> {
    let mut job = tokio::task::spawn_blocking(work);
    let stop = tokio::select! {
        biased;
        result = &mut job => {
            return result.map_err(|_| Diagnostic::new(BWErr::AsyncRuntime(failure.into())))?;
        }
        stop = control.stopped() => stop,
    };
    job.abort();
    Err(match control.within_grace(&mut job).await {
        Some(_) => stop,
        None => control.abandoned(stop),
    }
    .into())
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
    let control = interrupt::root().child(deadline);
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
    // Every case descends from the root control, through its fixture when it has one.
    let control = run_control(configuration.timeout_ms)?;
    let control = match &fixture {
        Some(fixture) => fixture.control().child(control.deadline()),
        None => control,
    };
    control.checkpoint()?;
    let (program, context) = prepared(
        &control.clone(),
        move || {
            let mut context =
                Context::with_host_environment(configuration.limits.clone(), control)?;
            let variables = match &fixture {
                Some(_) => std::collections::BTreeMap::new(),
                None => secrets::load(&configuration.files, &configuration.settings)?,
            };
            let variables = case.bind_inputs(variables, &configuration.limits.values)?;
            let program = case.program();
            context.init_statements();
            context.set_input_variables(variables)?;
            context.set_secrets(secrets::registry())?;
            context.set_compiled_modules(compiled_modules())?;
            if let Some(fixture) = fixture {
                fixture.inherit_into(&mut context)?;
            }
            context.checkpoint()?;
            context.set_statement_tracing(configuration.debug);
            Ok((program, context))
        },
        "Case preparation worker failed before completing",
    )
    .await?;
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
    let variables = secrets::load(files, settings)?;
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
    context.set_secrets(secrets::registry())?;
    context.set_compiled_modules(compiled_modules())?;
    context.checkpoint()?;
    context.set_statement_tracing(debug);
    Ok((program, context))
}

/// The CLI's stack, the same on every platform, so programs reach the same
/// depth limits everywhere; Windows gives a main thread only 1 MiB.
const STACK_BYTES: usize = 8 * 1024 * 1024;

fn main() -> ExitCode {
    if let Some(status) = botwork::core::worker::guardian_main() {
        return ExitCode::from(status);
    }
    let thread = std::thread::Builder::new()
        .name("botwork".into())
        .stack_size(STACK_BYTES)
        .spawn(cli);
    match thread {
        Ok(thread) => thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        Err(error) => {
            eprintln!("Starting the CLI thread failed: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `--fetch`: resolve a project's packages and report what it locked.
fn fetch(directory: &Path, offline: bool, locked: bool) -> ExitCode {
    use botwork::core::packages;
    let root = match botwork::core::paths::canonicalize(directory) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("[fetch] error: {}: {error}", directory.display());
            return ExitCode::FAILURE;
        }
    };
    if !root.join(packages::MANIFEST).is_file() {
        eprintln!(
            "[fetch] error: {} has no {}",
            root.display(),
            packages::MANIFEST
        );
        return ExitCode::FAILURE;
    }
    let options = packages::FetchOptions {
        offline,
        locked,
        cache: None,
    };
    match packages::fetch(&root, &options) {
        Ok(report) => {
            for package in &report.lock.packages {
                println!(
                    "[fetch] {} {} from {}",
                    package.name, package.version, package.source
                );
            }
            for file in &report.lock.files {
                println!("[fetch] {} sha256 {}", file.url, file.sha256);
            }
            println!(
                "[fetch] {} {}",
                packages::LOCKFILE,
                if report.written {
                    "written"
                } else {
                    "up to date"
                }
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[fetch] error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn cli() -> ExitCode {
    let args = Args::parse();
    // Show imported modules, named by their full path, like the files given here.
    if let Ok(directory) = std::env::current_dir() {
        botwork::core::diagnostic::show_paths_relative_to(&directory);
    }
    if args.lsp {
        return ExitCode::from(lsp::run() as u8);
    }
    if let Some(directory) = &args.fetch {
        return fetch(directory, args.offline, args.locked);
    }
    let output_limits = OutputLimits {
        record_bytes: args.max_output_record_bytes,
        total_bytes: args.max_output_bytes,
    };
    let result = if args.list_statements || args.statement_help.is_some() {
        statement_help(args.statement_help.as_deref(), output_limits)
    } else if args.check {
        check::run(&args.file, &args.suite)
    } else if args.format || args.format_check {
        let files: Vec<_> = args.file.iter().chain(&args.suite).cloned().collect();
        formatting::run(&files, args.format_check)
    } else if let Some(path) = &args.reconcile_report {
        report_json::Report::reconcile(path).and_then(|found| {
            let selected = found
                .selected
                .map_or_else(|| "an unknown number of".to_owned(), |count| count.to_string());
            let mut message = format!(
                "Reconciled {}: {} of {selected} selected runs have records, {} of them interrupted",
                path.display(),
                found.recorded,
                found.interrupted
            );
            if found.torn != 0 {
                message.push_str(&format!(
                    "; ignored a {}-byte final journal line cut short by the termination",
                    found.torn
                ));
            }
            Err(CliError::Reconciled(message))
        })
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                CliError::Script(Diagnostic::new(BWErr::AsyncRuntime(error.to_string())))
            })
            .and_then(|runtime| {
                let runtime = Runtime(Some(runtime));
                interrupt::install(Duration::from_millis(args.stop_grace_ms)).map_err(|error| {
                    Diagnostic::new(BWErr::AsyncRuntime(format!(
                        "Signal handling setup failed: {error}"
                    )))
                })?;
                // Register every secret before any run or output starts.
                if !args.secrets.is_empty() || !args.secret_environment.is_empty() {
                    secrets::prepare(
                        &args.secrets,
                        &args.secret_environment,
                        &args.variable_files,
                        &args.variables,
                    )?;
                }
                // A batch selects at most as many runs as suites may, which bounds
                // every per-run structure a report or summary keeps.
                if args.file.len() > botwork::core::suite::MAX_SELECTED_CASES {
                    return Err(botwork::core::suite::resource(
                        "selected files",
                        botwork::core::suite::MAX_SELECTED_CASES,
                    )
                    .into());
                }
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
                // Mark the reports incomplete before discovery or any run effects.
                let report = (args.report_json.is_some() || args.report_html.is_some())
                    .then(|| {
                        report_json::Report::begin(
                            args.report_json.clone(),
                            args.report_html.clone(),
                            mode,
                        )
                    })
                    .transpose()?
                    .map(std::sync::Arc::new);
                // Start the listener before discovery, so its stream covers every run.
                let listener = args
                    .listener
                    .as_deref()
                    .map(|program| {
                        let defaults = ListenerOptions::default();
                        listener::Hub::begin(
                            program,
                            &args.listener_args,
                            ListenerOptions {
                                capacity: args
                                    .listener_queue
                                    .map_or(defaults.capacity, |events| events as usize),
                                close_timeout: args
                                    .listener_timeout_ms
                                    .map_or(defaults.close_timeout, Duration::from_millis),
                            },
                            mode,
                        )
                    })
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
                let result = if !args.suite.is_empty() {
                    runtime.block_on(suites::run(
                        suites::Request::from(&args),
                        usize::from(args.jobs),
                        batch::Configuration {
                            artifacts,
                            report,
                            listener: listener.clone(),
                            debug: args.debug,
                            files: args.variable_files,
                            settings: args.variables,
                            limits,
                            timeout_ms: args.timeout_ms,
                            suite_timeout_ms: args.suite_timeout_ms,
                            failures: args.failures.clone(),
                        },
                    ))
                } else if args.file.len() == 1 {
                    let path = args.file[0].display().to_string();
                    if let Some(report) = &report {
                        report.selected(1);
                    }
                    let recording = batch::recording(
                        report.as_deref(),
                        listener.as_deref(),
                        1,
                        RunIdentity::new(path.clone(), path),
                    );
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
                    if let (Some(report), Some(record)) = (&report, record) {
                        report.run(1, record, &exported);
                    }
                    let mut totals = botwork::core::acceptance::CaseTotals::default();
                    totals.record(status)?;
                    // A single run needs no admission; a signal still marks it interrupted.
                    let delivery = if interrupt::interrupted() {
                        botwork::core::acceptance::Delivery::Interrupted
                    } else {
                        botwork::core::acceptance::Delivery::Complete
                    };
                    let delivered = batch::deliver(
                        report.as_deref(),
                        listener.as_deref(),
                        totals,
                        0,
                        delivery,
                        1,
                    );
                    batch::publish_report(result, delivered)
                } else {
                    runtime.block_on(batch::run(
                        args.file,
                        usize::from(args.jobs),
                        batch::Configuration {
                            artifacts,
                            report,
                            listener: listener.clone(),
                            debug: args.debug,
                            files: args.variable_files,
                            settings: args.variables,
                            limits,
                            timeout_ms: args.timeout_ms,
                            suite_timeout_ms: args.suite_timeout_ms,
                            failures: None,
                        },
                    ))
                };
                // A stopped invocation still closes the listener, without a trailer.
                let closed = listener.map_or(Ok(()), |hub| hub.finish(None));
                batch::publish_report(result, closed)
            })
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Batch { .. } | CliError::Suites { .. } | CliError::Checked) => {
            ExitCode::FAILURE
        }
        Err(error) => {
            // Failure reporting has its own bounded allowance, so an exhausted
            // script output budget does not hide the reason for failure.
            let _ = Context::default().write_output(
                &mut secrets::registry().writer(io::stderr().lock()),
                format_args!("{error}\n"),
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn a_dropped_runtime_has_ended_its_threads() {
        static ENDED: AtomicBool = AtomicBool::new(false);
        /// Set once its thread's destructors have run, slowly, as a library's
        /// may: a thread still running them would race the process's exit.
        struct Ending;
        impl Drop for Ending {
            fn drop(&mut self) {
                std::thread::sleep(Duration::from_millis(100));
                ENDED.store(true, Ordering::SeqCst);
            }
        }
        thread_local!(static ENDING: Ending = const { Ending });
        let runtime = Runtime(Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        ));
        runtime
            .block_on(async { tokio::task::spawn_blocking(|| ENDING.with(|_| ())).await })
            .unwrap();
        drop(runtime);
        assert!(
            ENDED.load(Ordering::SeqCst),
            "a runtime thread was still ending"
        );
    }
}
