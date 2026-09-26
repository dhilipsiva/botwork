//! JSON input conversion without evaluating DSL expressions or changing host state.

use std::{collections::BTreeMap, fs, io::Read, path::PathBuf};

use serde_json::value::RawValue;

use super::{
    diagnostic::{Diagnostic, DiagnosticLimits, DiagnosticResult},
    grammar::{BWErr, Literal},
};

mod limits;
mod raw;
#[cfg(test)]
mod tests;
use super::value_limits::ValueSize;
pub use limits::InputLimits;
use limits::{resource, Budget};

/// Maximum container nesting in one JSON document, including its root object.
pub const MAX_JSON_DEPTH: usize = 128;

fn invalid(origin: &str, path: &str, reason: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::new(BWErr::InputError(format!("{origin}: {path}: {reason}")))
}

pub(crate) fn validate_name(origin: &str, name: &str) -> DiagnosticResult<()> {
    validate_name_with(origin, name, |message| {
        DiagnosticLimits::default().formatted_detail(
            BWErr::InputError,
            message,
            None,
            false,
            std::iter::empty(),
        )
    })
}

pub(crate) fn validate_name_with(
    origin: &str,
    name: &str,
    error: impl FnOnce(std::fmt::Arguments<'_>) -> Diagnostic,
) -> DiagnosticResult<()> {
    // Preserve the former parser facade's fixed name ceiling without building
    // a Pest error. Configured name quotas can reject earlier.
    if name.len() <= super::syntax_limits::DEFAULT_SOURCE_BYTES
        && super::grammar::is_identifier(name)
    {
        Ok(())
    } else {
        Err(error(format_args!(
            "{origin}: variable {name:?}: expected an exact DSL identifier"
        )))
    }
}

fn decode<'a, T: serde::Deserialize<'a>>(
    origin: &str,
    path: &str,
    text: &'a str,
) -> DiagnosticResult<T> {
    serde_json::from_str(text).map_err(|error| invalid(origin, path, error))
}

fn path_key(path: &str, key: &str) -> String {
    let end = key
        .char_indices()
        .nth(32)
        .map_or(key.len(), |(index, _)| index);
    let preview = &key[..end];
    let parent_end = path
        .char_indices()
        .nth(128)
        .map_or(path.len(), |(index, _)| index);
    format!(
        "{}{}[{preview:?}{}]",
        &path[..parent_end],
        if parent_end < path.len() { "…" } else { "" },
        if end < key.len() { "…" } else { "" }
    )
}

fn convert(
    origin: &str,
    path: &str,
    raw: &RawValue,
    limits: &InputLimits,
    depth: usize,
) -> DiagnosticResult<(Literal, ValueSize)> {
    limits
        .values
        .check_size(ValueSize {
            nodes: 1,
            depth,
            payload_bytes: 0,
        })
        .map_err(|error| resource(origin, error))?;
    let text = raw.get();
    let value = match text.as_bytes()[0] {
        b'n' => Literal::None,
        b't' | b'f' => Literal::Bool(decode(origin, path, text)?),
        b'"' => {
            let (_, bytes) = limits::string_extent(text.as_bytes(), 0);
            limits
                .values
                .string_size(bytes)
                .map_err(|error| resource(origin, error))?;
            Literal::String(decode(origin, path, text)?)
        }
        b'[' => {
            let raw = raw::array(origin, path, text, limits)?;
            let mut size = limits
                .values
                .container_header(raw.len())
                .map_err(|error| resource(origin, error))?;
            let mut values = Vec::with_capacity(raw.len());
            for (index, raw) in raw.into_iter().enumerate() {
                let (value, child) =
                    convert(origin, &format!("{path}[{index}]"), raw, limits, depth + 1)?;
                limits
                    .values
                    .add_child(&mut size, child)
                    .map_err(|error| resource(origin, error))?;
                values.push(value);
            }
            return Ok((Literal::Array(values), size));
        }
        b'{' => {
            let raw = raw::map(origin, path, text, limits, false)?;
            let mut size = limits
                .values
                .container_header(raw.len())
                .map_err(|error| resource(origin, error))?;
            for key in raw.keys() {
                limits
                    .values
                    .add_bytes(&mut size, key.len())
                    .map_err(|error| resource(origin, error))?;
            }
            let mut values = std::collections::HashMap::new();
            for (key, raw) in raw {
                let (value, child) =
                    convert(origin, &path_key(path, &key), raw, limits, depth + 1)?;
                limits
                    .values
                    .add_child(&mut size, child)
                    .map_err(|error| resource(origin, error))?;
                values.insert(key, value);
            }
            return Ok((Literal::Map(values), size));
        }
        _ if text.contains(['.', 'e', 'E']) => {
            let value: f32 = text
                .parse()
                .map_err(|_| invalid(origin, path, "expected a finite f32 decimal"))?;
            if !value.is_finite() {
                return Err(invalid(
                    origin,
                    path,
                    "decimal exceeds the finite f32 range",
                ));
            }
            Literal::Float(value)
        }
        _ => Literal::Int(
            text.parse::<i32>()
                .map_err(|_| invalid(origin, path, "integer is outside -2147483648..2147483647"))?,
        ),
    };
    let size = limits
        .values
        .check(&value)
        .map_err(|error| resource(origin, error))?;
    Ok((value, size))
}

fn variables_from_text(
    origin: &str,
    text: &str,
    budget: &mut Budget<'_>,
) -> DiagnosticResult<BTreeMap<String, Literal>> {
    budget.preflight(origin, text)?;
    let root: &RawValue = decode(origin, "$", text)?;
    if !root.get().starts_with('{') {
        return Err(invalid(
            origin,
            "$",
            "expected a JSON object of variable names and values",
        ));
    }
    let values = raw::map(origin, "$", root.get(), budget.limits, true)?;
    values
        .into_iter()
        .map(|(name, raw)| {
            validate_name(origin, &name)?;
            let (value, _) = convert(origin, &path_key("$", &name), raw, budget.limits, 1)?;
            Ok((name, value))
        })
        .collect()
}

fn variable_from_setting(
    origin: &str,
    setting: &str,
    budget: &mut Budget<'_>,
) -> DiagnosticResult<(String, Literal)> {
    let (name, text) = setting
        .split_once('=')
        .ok_or_else(|| invalid(origin, "$", "expected NAME=JSON"))?;
    budget
        .limits
        .values
        .key_size(name.len())
        .map_err(|error| resource(origin, error))?;
    validate_name(origin, name)?;
    budget.preflight(origin, text)?;
    let raw: &RawValue = decode(origin, "$", text)?;
    budget.variable_count(origin, 1)?;
    let (value, _) = convert(origin, &path_key("$", name), raw, budget.limits, 1)?;
    Ok((name.into(), value))
}

/// Parse a JSON object into root variables. Duplicate object keys use the last value.
/// Decimal/exponent tokens round directly to f32; integer tokens must fit i32.
pub fn parse_variables(origin: &str, text: &str) -> DiagnosticResult<BTreeMap<String, Literal>> {
    parse_variables_with_limits(origin, text, &InputLimits::default())
}

/// Parse a root object with fresh source, token, name, and decoded-value budgets.
pub fn parse_variables_with_limits(
    origin: &str,
    text: &str,
    limits: &InputLimits,
) -> DiagnosticResult<BTreeMap<String, Literal>> {
    let mut budget = Budget::new(limits)?;
    budget.source(origin)?;
    budget.bytes(origin, text.len())?;
    variables_from_text(origin, text, &mut budget)
}

/// Parse one NAME=JSON setting. Split only at the first equals sign; do not trim names.
pub fn parse_variable(origin: &str, setting: &str) -> DiagnosticResult<(String, Literal)> {
    parse_variable_with_limits(origin, setting, &InputLimits::default())
}

/// Parse one setting; source-byte accounting includes its name and equals sign.
pub fn parse_variable_with_limits(
    origin: &str,
    setting: &str,
    limits: &InputLimits,
) -> DiagnosticResult<(String, Literal)> {
    let mut budget = Budget::new(limits)?;
    budget.source(origin)?;
    budget.bytes(origin, setting.len())?;
    variable_from_setting(origin, setting, &mut budget)
}

/// Read files in order, then explicit settings in order. Later values replace whole
/// earlier bindings. Relative file paths use the caller's cwd; no process state changes.
/// Every supplied file/setting is validated before a result is returned.
pub fn load_variables(
    files: &[PathBuf],
    settings: &[String],
) -> DiagnosticResult<BTreeMap<String, Literal>> {
    load_variables_with_limits(files, settings, &InputLimits::default())
}

/// Bound reads and share cumulative budgets across all files, then settings.
pub fn load_variables_with_limits(
    files: &[PathBuf],
    settings: &[String],
    limits: &InputLimits,
) -> DiagnosticResult<BTreeMap<String, Literal>> {
    let mut budget = Budget::new(limits)?;
    let mut variables = BTreeMap::new();
    for file in files {
        let origin = file.display().to_string();
        budget.source(&origin)?;
        let file = fs::File::open(file).map_err(|error| invalid(&origin, "$", error))?;
        let mut bytes = Vec::new();
        file.take((budget.remaining() as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| invalid(&origin, "$", error))?;
        budget.bytes(&origin, bytes.len())?;
        let text = String::from_utf8(bytes).map_err(|error| invalid(&origin, "$", error))?;
        for (name, value) in variables_from_text(&origin, &text, &mut budget)? {
            if !variables.contains_key(&name) {
                budget.variable_count(&origin, variables.len() + 1)?;
            }
            variables.insert(name, value);
        }
    }
    for (index, setting) in settings.iter().enumerate() {
        let origin = format!("--var #{}", index + 1);
        budget.source(&origin)?;
        budget.bytes(&origin, setting.len())?;
        let (name, value) = variable_from_setting(&origin, setting, &mut budget)?;
        if !variables.contains_key(&name) {
            budget.variable_count(&origin, variables.len() + 1)?;
        }
        variables.insert(name, value);
    }
    Ok(variables)
}
