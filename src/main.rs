use botwork::core::{
    ast::Program,
    diagnostic::Diagnostic,
    eval::{execute_statement_detailed, Context},
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
    #[arg(short, long)]
    file: PathBuf,
    /// Trace top-level statement locations on stderr
    #[arg(long)]
    debug: bool,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{file}: {source}")]
    Read { file: PathBuf, source: io::Error },
    #[error(transparent)]
    Script(#[from] Diagnostic),
    #[error("Writing debug trace failed: {0}")]
    Trace(io::Error),
}

fn run(file: &Path, debug: bool) -> Result<(), CliError> {
    let source = read_to_string(file).map_err(|source| CliError::Read {
        file: file.to_owned(),
        source,
    })?;
    let program = Program::parse_detailed(&file.display().to_string(), &source)?;
    let mut context = Context::default();
    context.init_statements();
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
    match run(&args.file, args.debug) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}
