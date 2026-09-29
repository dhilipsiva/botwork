//! `--check`: parse, validate, and lint files or suites without running them.
use super::{CliError, Context};
use botwork::core::{
    analysis::{Analyzer, Severity},
    ast::{suite::Suite, Program},
    diagnostic::Diagnostic,
    grammar::BWErr,
    syntax_limits::DEFAULT_SOURCE_BYTES,
};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
};

fn read(path: &PathBuf) -> Result<String, CliError> {
    let failure = |source| CliError::Read {
        file: path.clone(),
        source,
    };
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take(DEFAULT_SOURCE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(failure)?;
    if bytes.len() > DEFAULT_SOURCE_BYTES {
        return Err(CliError::SourceLimit {
            file: path.clone(),
            source: Box::new(Diagnostic::new(BWErr::ResourceLimit {
                resource: "source bytes",
                limit: DEFAULT_SOURCE_BYTES as u64,
            })),
        });
    }
    String::from_utf8(bytes)
        .map_err(|error| failure(io::Error::new(io::ErrorKind::InvalidData, error)))
}

/// Check each file, printing findings and a summary on stderr. Errors, including
/// syntax errors and unreadable files, fail the command; warnings do not.
pub(super) fn run(files: &[PathBuf], suites: &[PathBuf]) -> Result<(), CliError> {
    let analyzer = Analyzer::default();
    let context = Context::default();
    let mut stderr = io::stderr().lock();
    let (mut errors, mut warnings) = (0usize, 0usize);
    let inputs = files
        .iter()
        .map(|path| (path, false))
        .chain(suites.iter().map(|path| (path, true)));
    for (path, suite) in inputs {
        let name = path.display().to_string();
        let checked = read(path).and_then(|text| {
            if suite {
                Ok(analyzer.check_suite(&Suite::parse(&name, &text)?))
            } else {
                Ok(analyzer.check_program(&Program::parse_detailed(&name, &text)?))
            }
        });
        match checked {
            Ok(findings) => {
                for finding in findings {
                    match finding.severity() {
                        Severity::Error => errors += 1,
                        Severity::Warning => warnings += 1,
                    }
                    context.write_output(&mut stderr, format_args!("{finding}\n"))?;
                }
            }
            Err(error) => {
                errors += 1;
                context.write_output(&mut stderr, format_args!("{error}\n"))?;
            }
        }
    }
    let count = files.len() + suites.len();
    let plural =
        |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    context.write_output(
        &mut stderr,
        format_args!(
            "[check] {}: {}, {}\n",
            plural(count, "file"),
            plural(errors, "error"),
            plural(warnings, "warning")
        ),
    )?;
    stderr.flush().ok();
    if errors == 0 {
        Ok(())
    } else {
        Err(CliError::Checked)
    }
}
