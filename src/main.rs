use botwork::core::{
    ast::Program,
    eval::{execute_statement, Context},
};
use clap::Parser as Clap;
use std::{
    error::Error,
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

fn run(file: &Path, debug: bool) -> Result<(), Box<dyn Error>> {
    let source = read_to_string(file)?;
    let program = Program::parse(&file.display().to_string(), &source)?;
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
            )?;
        }
        execute_statement(statement, &mut context)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args.file, args.debug) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{}: {error}", args.file.display());
            ExitCode::FAILURE
        }
    }
}
