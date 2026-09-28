use botwork::core::{
    ast::Program,
    diagnostic::Diagnostic,
    eval::{evaluate_program_async, Context},
    grammar::BWErr,
    input::load_variables,
    operation::OperationControl,
    run::{
        OutputLimits, RunLimits, DEFAULT_OUTPUT_BYTES, DEFAULT_OUTPUT_RECORD_BYTES, DEFAULT_STEPS,
        MAX_EVALUATION_DEPTH,
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

mod batch;

/// Run Botwork automation scripts.
#[derive(Clap, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Botwork file to run (repeatable; each occurrence starts a fresh run)
    #[arg(short, long, required_unless_present_any = ["list_statements", "statement_help"])]
    file: Vec<PathBuf>,
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
}

#[derive(Debug, thiserror::Error)]
enum CliError {
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

async fn run(
    file: &Path,
    debug: bool,
    files: &[PathBuf],
    settings: &[String],
    limits: RunLimits,
    timeout_ms: Option<u64>,
) -> Result<(), CliError> {
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
    let mut context = Context::with_control(limits, control)?;
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
            .enable_time()
            .build()
            .map_err(|error| {
                CliError::Script(Diagnostic::new(BWErr::AsyncRuntime(error.to_string())))
            })
            .and_then(|runtime| {
                let limits = RunLimits {
                    output: output_limits,
                    steps: args.max_steps,
                    call_depth: args.max_call_depth,
                    evaluation_depth: args.max_evaluation_depth,
                    ..RunLimits::default()
                };
                if args.file.len() == 1 {
                    runtime.block_on(run(
                        &args.file[0],
                        args.debug,
                        &args.variable_files,
                        &args.variables,
                        limits,
                        args.timeout_ms,
                    ))
                } else {
                    runtime.block_on(batch::run(
                        args.file,
                        usize::from(args.jobs),
                        batch::Configuration {
                            debug: args.debug,
                            files: args.variable_files,
                            settings: args.variables,
                            limits,
                            timeout_ms: args.timeout_ms,
                        },
                    ))
                }
            })
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Batch { .. }) => ExitCode::FAILURE,
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
