//! JSON input conversion without evaluating DSL expressions or changing host state.

use std::{collections::BTreeMap, fs, path::PathBuf};

use pest::Parser;
use serde_json::value::RawValue;

use super::{
    diagnostic::{Diagnostic, DiagnosticResult},
    grammar::{BWErr, BWParser, Literal, Rule},
};

/// Maximum container nesting in one JSON document, including its root object.
pub const MAX_JSON_DEPTH: usize = 128;

fn invalid(origin: &str, path: &str, reason: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::new(BWErr::InputError(format!("{origin}: {path}: {reason}")))
}

pub(crate) fn validate_name(origin: &str, name: &str) -> DiagnosticResult<()> {
    let valid = BWParser::parse(Rule::ident, name)
        .ok()
        .and_then(|mut pairs| pairs.next())
        .is_some_and(|pair| pair.as_span().start() == 0 && pair.as_span().end() == name.len());
    if valid {
        Ok(())
    } else {
        Err(invalid(
            origin,
            &format!("variable {name:?}"),
            "expected an exact DSL identifier",
        ))
    }
}

// RawValue scans iteratively and does not apply serde_json's usual depth guard.
// Bound all containers before recursive conversion, including overwritten keys.
fn check_depth(origin: &str, text: &str) -> DiagnosticResult<()> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for byte in text.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > MAX_JSON_DEPTH {
                        return Err(invalid(origin, "$", "JSON exceeds 128 nested containers"));
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}

fn decode<'a, T: serde::Deserialize<'a>>(
    origin: &str,
    path: &str,
    text: &'a str,
) -> DiagnosticResult<T> {
    serde_json::from_str(text).map_err(|error| invalid(origin, path, error))
}

fn convert(origin: &str, path: &str, raw: &RawValue) -> DiagnosticResult<Literal> {
    let text = raw.get();
    match text.as_bytes()[0] {
        b'n' => Ok(Literal::None),
        b't' | b'f' => decode(origin, path, text).map(Literal::Bool),
        b'"' => decode(origin, path, text).map(Literal::String),
        b'[' => {
            let values: Vec<&RawValue> = decode(origin, path, text)?;
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| convert(origin, &format!("{path}[{index}]"), value))
                .collect::<DiagnosticResult<_>>()
                .map(Literal::Array)
        }
        b'{' => {
            let values: BTreeMap<String, &RawValue> = decode(origin, path, text)?;
            values
                .into_iter()
                .map(|(key, value)| {
                    let value = convert(origin, &format!("{path}[{key:?}]"), value)?;
                    Ok((key, value))
                })
                .collect::<DiagnosticResult<_>>()
                .map(Literal::Map)
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
            Ok(Literal::Float(value))
        }
        _ => text
            .parse::<i32>()
            .map(Literal::Int)
            .map_err(|_| invalid(origin, path, "integer is outside -2147483648..2147483647")),
    }
}

/// Parse a JSON object into root variables. Duplicate object keys use the last value.
/// Decimal/exponent tokens round directly to f32; integer tokens must fit i32.
pub fn parse_variables(origin: &str, text: &str) -> DiagnosticResult<BTreeMap<String, Literal>> {
    check_depth(origin, text)?;
    let root: &RawValue = decode(origin, "$", text)?;
    if !root.get().starts_with('{') {
        return Err(invalid(
            origin,
            "$",
            "expected a JSON object of variable names and values",
        ));
    }
    let values: BTreeMap<String, &RawValue> = decode(origin, "$", root.get())?;
    values
        .into_iter()
        .map(|(name, value)| {
            validate_name(origin, &name)?;
            let value = convert(origin, &format!("$[{name:?}]"), value)?;
            Ok((name, value))
        })
        .collect()
}

/// Parse one NAME=JSON setting. Split only at the first equals sign; do not trim names.
pub fn parse_variable(origin: &str, setting: &str) -> DiagnosticResult<(String, Literal)> {
    let (name, text) = setting
        .split_once('=')
        .ok_or_else(|| invalid(origin, "$", "expected NAME=JSON"))?;
    validate_name(origin, name)?;
    check_depth(origin, text)?;
    let raw: &RawValue = decode(origin, "$", text)?;
    Ok((name.into(), convert(origin, &format!("$[{name:?}]"), raw)?))
}

/// Read files in order, then explicit settings in order. Later values replace whole
/// earlier bindings. Relative file paths use the caller's cwd; no process state changes.
/// Every supplied file/setting is validated before a result is returned.
pub fn load_variables(
    files: &[PathBuf],
    settings: &[String],
) -> DiagnosticResult<BTreeMap<String, Literal>> {
    let mut variables = BTreeMap::new();
    for file in files {
        let origin = file.display().to_string();
        let text = fs::read_to_string(file).map_err(|error| invalid(&origin, "$", error))?;
        variables.extend(parse_variables(&origin, &text)?);
    }
    for (index, setting) in settings.iter().enumerate() {
        let (name, value) = parse_variable(&format!("--var #{}", index + 1), setting)?;
        variables.insert(name, value);
    }
    Ok(variables)
}
