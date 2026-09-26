use botwork::core::{
    eval::{botwork, Context},
    grammar::{BWParser, Rule},
};
use clap::Parser as Clap;
use pest::Parser;
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
    let tree = BWParser::parse(Rule::botwork, &source)?;
    let mut context = Context::default();
    context.init_statements();
    for pair in tree {
        if debug && pair.as_rule() != Rule::EOI {
            let (line, column) = pair.as_span().start_pos().line_col();
            let kind = match pair.as_rule() {
                Rule::stmt_assign => "assignment",
                Rule::stmt_define => "definition",
                Rule::stmt_invoke => "call",
                Rule::stmt_if => "if",
                Rule::stmt_for => "for",
                Rule::stmt_while => "while",
                Rule::stmt_try => "try",
                Rule::stmt_return => "return",
                Rule::stmt_break => "break",
                Rule::stmt_continue => "continue",
                _ => "statement",
            };
            writeln!(
                io::stderr().lock(),
                "debug: {}:{line}:{column}: {kind}",
                file.display()
            )?;
        }
        botwork(pair, &mut context)?;
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
