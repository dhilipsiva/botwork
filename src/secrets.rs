//! Secret-marked inputs for the CLI: `--secret NAME` marks an input variable,
//! and `--secret-env NAME=VARIABLE` defines one from an environment variable
//! without placing its value on the command line. Every run's inputs register
//! their secret values in one shared registry that masks all CLI output.
use super::{BWErr, CliError, Diagnostic};
use botwork::core::{grammar::Literal, input::load_variables, secret::Secrets};
use std::{collections::BTreeMap, path::PathBuf, sync::OnceLock};

/// Which inputs are secret, fixed once from the command line.
#[derive(Default)]
struct Marking {
    names: Vec<String>,
    environment: Vec<(String, String)>,
}

static MARKING: OnceLock<Marking> = OnceLock::new();
static REGISTRY: OnceLock<Secrets> = OnceLock::new();

/// The registry that masks every output of this invocation.
pub(super) fn registry() -> &'static Secrets {
    REGISTRY.get_or_init(Secrets::default)
}

fn invalid(message: String) -> CliError {
    Diagnostic::new(BWErr::InputError(message)).into()
}

/// Record the marking and load the inputs once, so every secret is registered
/// before any run or output starts.
pub(super) fn prepare(
    names: &[String],
    environment: &[String],
    files: &[PathBuf],
    settings: &[String],
) -> Result<(), CliError> {
    let mut marking = Marking {
        names: names.to_vec(),
        environment: Vec::new(),
    };
    for (index, setting) in environment.iter().enumerate() {
        let (name, variable) = setting.split_once('=').ok_or_else(|| {
            invalid(format!(
                "--secret-env {}: expected NAME=VARIABLE",
                index + 1
            ))
        })?;
        marking
            .environment
            .push((name.to_owned(), variable.to_owned()));
    }
    let _ = MARKING.set(marking);
    load(files, settings).map(|_| ())
}

/// Load a run's input variables, add environment secrets, and register every
/// secret value's string and number leaves.
pub(super) fn load(
    files: &[PathBuf],
    settings: &[String],
) -> Result<BTreeMap<String, Literal>, CliError> {
    let mut variables = load_variables(files, settings)?;
    let Some(marking) = MARKING.get() else {
        return Ok(variables);
    };
    for (name, variable) in &marking.environment {
        let value = std::env::var(variable).map_err(|error| {
            invalid(format!(
                "--secret-env {name}: environment variable {variable:?} is {}",
                match error {
                    std::env::VarError::NotPresent => "not set",
                    std::env::VarError::NotUnicode(_) => "not valid Unicode",
                }
            ))
        })?;
        // Validate the name through the ordinary input path.
        let checked = load_variables(&[], &[format!("{name}=null")])?;
        debug_assert!(checked.contains_key(name));
        variables.insert(name.clone(), Literal::String(value));
    }
    for name in marking
        .names
        .iter()
        .chain(marking.environment.iter().map(|(name, _)| name))
    {
        let value = variables
            .get(name)
            .ok_or_else(|| invalid(format!("--secret {name}: no input variable has that name")))?;
        registry().add(value);
    }
    Ok(variables)
}
