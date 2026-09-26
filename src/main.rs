use botwork::core::{
    ast::Program,
    diagnostic::Diagnostic,
    eval::{execute_statement_detailed, Context},
    grammar::BWErr,
    input::load_variables,
    operation::OperationControl,
    run::{RunLimits, DEFAULT_STEPS, MAX_EVALUATION_DEPTH},
    syntax_limits::DEFAULT_SOURCE_BYTES,
};
use clap::Parser as Clap;
use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

/// Run a Botwork automation script.
#[derive(Clap, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Name of the botwork file to run
    #[arg(short, long, required_unless_present_any = ["list_statements", "statement_help"])]
    file: Option<PathBuf>,
    /// List built-in statement headers without executing a file
    #[arg(long, conflicts_with_all = ["file", "statement_help", "debug", "variables", "variable_files", "max_steps", "max_call_depth", "max_evaluation_depth", "timeout_ms"])]
    list_statements: bool,
    /// Show a built-in's parameters, return kinds, and documented errors
    #[arg(long, value_name = "HEADER", conflicts_with_all = ["file", "list_statements", "debug", "variables", "variable_files", "max_steps", "max_call_depth", "max_evaluation_depth", "timeout_ms"])]
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
    /// Maximum evaluation steps before terminating the run
    #[arg(long, default_value_t = DEFAULT_STEPS)]
    max_steps: u64,
    /// Maximum entered native or custom calls
    #[arg(long, default_value_t = 32)]
    max_call_depth: usize,
    /// Maximum combined evaluation depth (ceiling 96)
    #[arg(long, default_value_t = MAX_EVALUATION_DEPTH)]
    max_evaluation_depth: usize,
    /// Cooperative timeout in milliseconds, including loading and parsing
    #[arg(long)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{file}: {source}")]
    Read { file: PathBuf, source: io::Error },
    #[error(transparent)]
    Script(#[from] Diagnostic),
    #[error("Writing debug trace failed: {0}")]
    Trace(io::Error),
    #[error("Writing statement help failed: {0}")]
    Help(io::Error),
    #[error("{file}: {source}")]
    SourceLimit {
        file: PathBuf,
        source: Box<Diagnostic>,
    },
}

fn run(
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
    let mut context = Context::with_control(limits, OperationControl::default().child(deadline))?;
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
    for statement in &program.statements {
        context.checkpoint()?;
        if debug {
            let (line, column) = statement.span.line_column();
            let kind = statement.kind_name();
            writeln!(
                io::stderr().lock(),
                "debug: {}:{line}:{column}: {kind}",
                file.display()
            )
            .map_err(CliError::Trace)?;
        }
        execute_statement_detailed(statement, &mut context)?;
    }
    context.checkpoint()?;
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    let result = if args.list_statements || args.statement_help.is_some() {
        statement_help(args.statement_help.as_deref())
    } else {
        run(
            args.file
                .as_deref()
                .expect("clap requires a file for execution"),
            args.debug,
            &args.variable_files,
            &args.variables,
            RunLimits {
                steps: args.max_steps,
                call_depth: args.max_call_depth,
                evaluation_depth: args.max_evaluation_depth,
                ..RunLimits::default()
            },
            args.timeout_ms,
        )
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

fn statement_help(header: Option<&str>) -> Result<(), CliError> {
    let mut context = Context::default();
    context.init_statements();
    let text = match header {
        Some(header) => context
            .statement_signature(header)?
            .ok_or_else(|| Diagnostic::new(BWErr::StatementNotDefined(header.into())))?
            .help(),
        None => context
            .statement_signatures()
            .iter()
            .map(|signature| signature.header().text().trim())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    writeln!(io::stdout().lock(), "{text}").map_err(CliError::Help)
}
