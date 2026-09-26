use botwork::core::{
    ast::Program,
    diagnostic::Diagnostic,
    eval::{execute_statement_detailed, Context},
    grammar::BWErr,
    input::load_variables,
};
use clap::Parser as Clap;
use std::{
    fs::read_to_string,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

/// Run a Botwork automation script.
#[derive(Clap, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Name of the botwork file to run
    #[arg(short, long, required_unless_present_any = ["list_statements", "statement_help"])]
    file: Option<PathBuf>,
    /// List built-in statement headers without executing a file
    #[arg(long, conflicts_with_all = ["file", "statement_help", "debug", "variables", "variable_files"])]
    list_statements: bool,
    /// Show a built-in's parameters, return kinds, and documented errors
    #[arg(long, value_name = "HEADER", conflicts_with_all = ["file", "list_statements", "debug", "variables", "variable_files"])]
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
}

fn run(file: &Path, debug: bool, files: &[PathBuf], settings: &[String]) -> Result<(), CliError> {
    let variables = load_variables(files, settings)?;
    let source = read_to_string(file).map_err(|source| CliError::Read {
        file: file.to_owned(),
        source,
    })?;
    let program = Program::parse_detailed(&file.display().to_string(), &source)?;
    let mut context = Context::default();
    context.init_statements();
    context.set_input_variables(variables)?;
    for statement in &program.statements {
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
