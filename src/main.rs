use botwork::core::{
    eval::{botwork, Context},
    grammar::{BWParser, Rule},
};
use clap::Parser as Clap;
use pest::Parser;
use std::{
    error::Error,
    fs::read_to_string,
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
}

fn run(file: &Path) -> Result<(), Box<dyn Error>> {
    let source = read_to_string(file)?;
    let tree = BWParser::parse(Rule::botwork, &source)?;
    let mut context = Context::default();
    context.init_statements();
    for pair in tree {
        botwork(pair, &mut context)?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args.file) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {error}", args.file.display());
            ExitCode::FAILURE
        }
    }
}
