//! Structured test data: JSON shared with input-variable conversion, canonical
//! JSON text, and header-row CSV tables. Results are admitted before retention.
use super::*;
use crate::core::{
    input::{self, InputLimits},
    value_limits::ValueSize,
};
use std::fmt::{self, Write};

mod csv;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
pub(in crate::core::eval) enum DataOp {
    ParseJson,
    FormatJson,
    ParseCsv,
}

impl DataOp {
    pub(super) fn signature(self) -> StatementSignature {
        let (header, description, returns, invalid) = match self {
            Self::ParseJson => (
                "Parse JSON |text|",
                "Convert one JSON document with the input-variable rules: integer tokens to Int, decimal/exponent tokens to finite Float, null to None. Duplicate keys keep the last value; one leading byte order mark is ignored.",
                None,
                "Invalid JSON syntax, trailing text, an integer outside Int, a decimal outside finite Float, or more than 128 nested containers.",
            ),
            Self::FormatJson => (
                "Format JSON |value|",
                "Write compact canonical JSON: map keys in Unicode scalar order, Floats with a decimal point or exponent, None as null. Parse JSON restores an equal value of the same kinds.",
                Some(Kind::String),
                "",
            ),
            Self::ParseCsv => (
                "Parse CSV |text|",
                "Convert comma-separated records with a header row into an Array of Maps from header names to String fields. Quoted fields use RFC 4180 doubled quotes; records end with LF or CRLF.",
                Some(Kind::Array),
                "A missing, empty, or duplicate header name, a record with a different field count, a stray quote, an unterminated quoted field, or a bare carriage return.",
            ),
        };
        let mut signature = StatementSignature::native_at("<data>", header)
            .expect("fixed data signature")
            .description(description);
        if !matches!(self, Self::FormatJson) {
            signature = signature
                .parameter("text", Kind::String)
                .expect("fixed data parameter");
        }
        if let Some(kind) = returns {
            signature = signature.returns(kind);
        }
        if !invalid.is_empty() {
            signature = signature
                .documents_error(Code::IncompatibleType, invalid)
                .expect("fixed error");
        }
        signature
            .documents_error(
                Code::ResourceLimit,
                "The result exceeds value, temporary, or output budgets.",
            )
            .expect("fixed error")
    }

    pub(super) fn invoke(
        self,
        arguments: Vec<TemporaryValue>,
        context: &Context,
    ) -> TemporaryResult {
        context.checkpoint()?;
        match self {
            Self::ParseJson => parse_json(text(&arguments[0]), context),
            Self::FormatJson => super::strings::formatted(context, |output| {
                write!(output, "{}", Canonical(&arguments[0]))
            }),
            Self::ParseCsv => csv::parse(text(&arguments[0]), context),
        }
    }
}

fn text(value: &Literal) -> &str {
    match value {
        Literal::String(text) => text,
        _ => unreachable!("validated String"),
    }
}

/// One leading U+FEFF is an encoding signature, not document content.
fn without_signature(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn parse_json(text: &str, context: &Context) -> TemporaryResult {
    let text = without_signature(text);
    let defaults = InputLimits::default();
    let values = context.limits().values;
    // The argument is already an admitted value; only conversion budgets apply.
    let limits = InputLimits {
        source_bytes: text.len(),
        total_bytes: text.len(),
        raw_nodes: defaults.raw_nodes.max(values.nodes),
        values,
        ..defaults
    };
    let mut reservation = context.temporary_reservation(ValueSize::default())?;
    let value = input::parse_document("Parse JSON", text, &limits, &mut |nodes, bytes| {
        reservation
            .as_mut()
            .map_or(Ok(()), |reservation| reservation.grow(nodes, bytes))
    })
    .map_err(|error| match &*error.error {
        BWErr::ResourceLimit { resource, limit } => context
            .retain_limit(Diagnostic::new(BWErr::ResourceLimit {
                resource,
                limit: *limit,
            }))
            .into(),
        detail => context.formatted_error(
            BWErr::OperationIncompatibleError,
            format_args!("{}", Reason(detail)),
            None,
            false,
        ),
    })?;
    context.checkpoint()?;
    Ok(TemporaryValue::new(value, reservation))
}

/// The conversion reason without its category prefix; input errors already
/// name "Parse JSON", the JSON path, and serde's line and column.
struct Reason<'a>(&'a BWErr);
impl fmt::Display for Reason<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            BWErr::InputError(reason) => f.write_str(reason),
            other => write!(f, "{other}"),
        }
    }
}

/// A JSON string literal with short escapes for common controls.
pub(super) struct JsonString<'a>(pub &'a str);
impl fmt::Display for JsonString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for ch in self.0.chars() {
            match ch {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                '\n' => f.write_str("\\n")?,
                '\r' => f.write_str("\\r")?,
                '\t' => f.write_str("\\t")?,
                '\u{8}' => f.write_str("\\b")?,
                '\u{c}' => f.write_str("\\f")?,
                '\0'..='\u{1f}' => write!(f, "\\u{:04x}", ch as u32)?,
                _ => f.write_char(ch)?,
            }
        }
        f.write_char('"')
    }
}

/// Canonical compact JSON. Map keys are sorted per map, borrowing the keys.
struct Canonical<'a>(&'a Literal);
impl fmt::Display for Canonical<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Literal::None => f.write_str("null"),
            Literal::Bool(value) => write!(f, "{value}"),
            Literal::Int(value) => write!(f, "{value}"),
            // Shortest round-trip form; always includes `.` or an exponent.
            Literal::Float(value) => write!(f, "{value:?}"),
            Literal::String(value) => write!(f, "{}", JsonString(value)),
            Literal::Array(values) => {
                f.write_char('[')?;
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{}", Canonical(value))?;
                }
                f.write_char(']')
            }
            Literal::Map(values) => {
                let mut keys: Vec<&String> = values.keys().collect();
                keys.sort_unstable();
                f.write_char('{')?;
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{}:{}", JsonString(key), Canonical(&values[key]))?;
                }
                f.write_char('}')
            }
        }
    }
}

lazy_static::lazy_static! {
    pub(super) static ref FIXED: Vec<(Builtin, Arc<StatementSignature>)> = [
        DataOp::ParseJson, DataOp::FormatJson, DataOp::ParseCsv,
    ].into_iter().map(|kind| (Builtin::Data(kind), Arc::new(kind.signature()))).collect();
}
