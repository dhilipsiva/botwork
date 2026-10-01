//! `--check`: parse, validate, and lint files or suites without running them.
use super::{CliError, Context};
use botwork::core::{
    analysis::{Analyzer, External, Severity},
    ast::Program,
    diagnostic::Diagnostic,
    grammar::BWErr,
    suite::Suite,
    syntax_limits::DEFAULT_SOURCE_BYTES,
};
use std::{
    collections::{BTreeMap, BTreeSet},
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

/// Check each file and the local modules it imports, printing findings, what
/// could not be checked, and a summary on stderr. Errors, including syntax errors
/// and unreadable files or modules, fail the command; warnings do not.
pub(super) fn run(files: &[PathBuf], suites: &[PathBuf]) -> Result<(), CliError> {
    let directory = std::env::current_dir().map_err(|source| CliError::Read {
        file: PathBuf::from("."),
        source,
    })?;
    let analyzer = Analyzer::default().with_modules(directory);
    let context = Context::default();
    let mut stderr = io::stderr().lock();
    let (mut errors, mut warnings) = (0usize, 0usize);
    let mut modules = BTreeSet::new();
    let mut variables = BTreeSet::new();
    let mut external: BTreeMap<External, usize> = BTreeMap::new();
    let mut unchecked_imports = 0;
    let inputs = files
        .iter()
        .map(|path| (path, false))
        .chain(suites.iter().map(|path| (path, true)));
    for (path, suite) in inputs {
        let name = path.display().to_string();
        let checked = read(path).and_then(|text| {
            if suite {
                Ok(analyzer.report_suite(&Suite::parse(&name, &text)?))
            } else {
                Ok(analyzer.report_program(&Program::parse_detailed(&name, &text)?))
            }
        });
        match checked {
            Ok(report) => {
                for diagnostic in &report.diagnostics {
                    errors += 1;
                    context.write_output(&mut stderr, format_args!("{diagnostic}\n"))?;
                }
                for finding in &report.findings {
                    if finding.severity() == Severity::Error {
                        errors += 1;
                    } else {
                        warnings += 1;
                    }
                    context.write_output(&mut stderr, format_args!("{finding}\n"))?;
                }
                modules.extend(report.modules);
                variables.extend(report.inputs);
                for (kind, calls) in report.external {
                    *external.entry(kind).or_default() += calls;
                }
                unchecked_imports += report.unchecked_imports;
            }
            Err(error) => {
                errors += 1;
                context.write_output(&mut stderr, format_args!("{error}\n"))?;
            }
        }
    }
    let plural =
        |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    // Say what a passing check does not establish.
    if !variables.is_empty() {
        let names: Vec<_> = variables.into_iter().collect();
        context.write_output(
            &mut stderr,
            format_args!(
                "[check] not checked: input variables {}\n",
                names.join(", ")
            ),
        )?;
    }
    if !external.is_empty() {
        let uses: Vec<_> = external
            .iter()
            .map(|(kind, calls)| format!("{} ({calls})", kind.as_str()))
            .collect();
        context.write_output(
            &mut stderr,
            format_args!(
                "[check] not checked: results of calls that use {}\n",
                uses.join(", ")
            ),
        )?;
    }
    if unchecked_imports != 0 {
        context.write_output(
            &mut stderr,
            format_args!(
                "[check] not checked: {} into Python, JavaScript, or WebAssembly modules, or modules that could not be read\n",
                plural(unchecked_imports, "call")
            ),
        )?;
    }
    let mut checked = plural(files.len() + suites.len(), "file");
    if !modules.is_empty() {
        checked = format!("{checked}, {}", plural(modules.len(), "module"));
    }
    context.write_output(
        &mut stderr,
        format_args!(
            "[check] {checked}: {}, {}\n",
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
