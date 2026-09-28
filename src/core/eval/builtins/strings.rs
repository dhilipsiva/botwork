//! String transformations with explicit Unicode semantics and output admission.
use super::*;
use crate::core::{signature::ValueKinds, value_limits::ValueSize};

mod build;
mod patterns;
mod template;

// Share the two-pass bounded writer with other standard statement catalogues.
pub(super) fn formatted(
    context: &Context,
    render: impl Fn(&mut dyn std::fmt::Write) -> std::fmt::Result,
) -> TemporaryResult {
    build::render(context, |output, _| render(output).map_err(Into::into))
}

#[derive(Clone, Copy)]
pub(in crate::core::eval) enum StringOp {
    Format,
    Join,
    Split,
    Lines,
    Replace,
    Contains,
    Starts,
    Ends,
    Length,
    Slice,
    Trim,
    Upper,
    Lower,
    Matches,
    FindAll,
    Captures,
}

impl StringOp {
    pub(super) fn is_regex(self) -> bool {
        matches!(self, Self::Matches | Self::FindAll | Self::Captures)
    }

    pub(super) fn signature(self) -> StatementSignature {
        let string = ValueKinds::from(Kind::String);
        let array = ValueKinds::from(Kind::Array);
        let int = ValueKinds::from(Kind::Int);
        let (header, description, parameters, returns) = match self {
            Self::Format => ("Format String |template| With |values|", "Substitute {index} from an Array or {key} from a Map; {{ and }} escape braces. Values use readable Botwork display; no code is evaluated.", vec![("template", string), ("values", array.union(Kind::Map.into()))], Kind::String),
            Self::Join => ("Join Strings |strings| With |separator|", "Join an Array of Strings with an exact separator; an empty array returns an empty String.", vec![("strings", array), ("separator", string)], Kind::String),
            Self::Split => ("Split String |text| On |separator|", "Split on a nonempty literal separator, preserving empty fields including the final field.", vec![("text", string), ("separator", string)], Kind::Array),
            Self::Lines => ("Split Lines |text|", "Split LF/CRLF lines, excluding terminators; a final terminator adds no extra line and an empty input gives an empty Array.", vec![("text", string)], Kind::Array),
            Self::Replace => ("Replace String |text| Find |needle| With |replacement|", "Replace every non-overlapping occurrence of a nonempty literal needle; replacement is literal text.", vec![("text", string), ("needle", string), ("replacement", string)], Kind::String),
            Self::Contains => ("String Contains |text| Text |needle|", "Test exact literal substring membership, without case folding or Unicode normalization.", vec![("text", string), ("needle", string)], Kind::Bool),
            Self::Starts => ("String Starts With |text| Prefix |prefix|", "Test an exact literal prefix; the empty prefix matches.", vec![("text", string), ("prefix", string)], Kind::Bool),
            Self::Ends => ("String Ends With |text| Suffix |suffix|", "Test an exact literal suffix; the empty suffix matches.", vec![("text", string), ("suffix", string)], Kind::Bool),
            Self::Length => ("String Length |text|", "Count Unicode scalar values as an Int, not UTF-8 bytes or grapheme clusters.", vec![("text", string)], Kind::Int),
            Self::Slice => ("Slice String |text| From |start| To |end|", "Copy a half-open Unicode scalar range; require 0 <= start <= end <= scalar length.", vec![("text", string), ("start", int), ("end", int)], Kind::String),
            Self::Trim => ("Trim String |text|", "Remove Unicode whitespace at both ends; preserve internal whitespace.", vec![("text", string)], Kind::String),
            Self::Upper => ("Uppercase String |text|", "Apply Unicode default uppercase mapping without locale tailoring or normalization.", vec![("text", string)], Kind::String),
            Self::Lower => ("Lowercase String |text|", "Apply Unicode default lowercase mapping, including contextual Greek sigma, without locale tailoring or normalization.", vec![("text", string)], Kind::String),
            Self::Matches => ("String Matches |text| Regex |pattern|", "Search for a Unicode regular expression; use anchors for a whole-string match. Compilation and search budgets apply.", vec![("text", string), ("pattern", string)], Kind::Bool),
            Self::FindAll => ("Find Matches In |text| Regex |pattern|", "Return all non-overlapping full regex matches as Strings, in source order, under bounded search/output budgets.", vec![("text", string), ("pattern", string)], Kind::Array),
            Self::Captures => ("Capture From |text| Regex |pattern|", "Return the first full match and numbered capture groups; unmatched groups are None, no match returns an empty Array.", vec![("text", string), ("pattern", string)], Kind::Array),
        };
        let mut signature = StatementSignature::native_at("<strings>", header)
            .expect("fixed string signature")
            .description(description)
            .returns(returns);
        for (name, kinds) in parameters {
            signature = signature
                .parameter(name, kinds)
                .expect("fixed string parameter");
        }
        if matches!(
            self,
            Self::Format | Self::Join | Self::Split | Self::Replace | Self::Slice
        ) || self.is_regex()
        {
            signature = signature
                .documents_error(
                    Code::IncompatibleType,
                    "Invalid template, field, string element, separator, range, or regex syntax.",
                )
                .expect("fixed error");
        }
        if matches!(self, Self::Length) {
            signature = signature
                .documents_error(Code::Arithmetic, "The scalar count cannot fit an Int.")
                .expect("fixed error");
        }
        if self.is_regex() {
            signature = signature
                .documents_error(
                    Code::ResourceLimit,
                    "The pattern, compiled regex, search work, or output exceeds its budget.",
                )
                .expect("fixed error");
        }
        signature
    }

    pub(super) fn invoke(
        self,
        arguments: Vec<TemporaryValue>,
        context: &Context,
    ) -> TemporaryResult {
        context.checkpoint()?;
        if self.is_regex() {
            return patterns::invoke(self, text(&arguments[0]), text(&arguments[1]), context);
        }
        match self {
            Self::Format => build::render(context, |output, sorted| {
                template::render(text(&arguments[0]), &arguments[1], output, sorted)
            }),
            Self::Join => {
                let Literal::Array(values) = &*arguments[0] else {
                    unreachable!("validated Array")
                };
                for value in values {
                    context.checkpoint()?;
                    if !matches!(value, Literal::String(_)) {
                        return Err(invalid(
                            context,
                            "Join Strings requires every element to be a String",
                        ));
                    }
                }
                let separator = text(&arguments[1]);
                build::render(context, |output, _| {
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            output.write_str(separator)?;
                        }
                        output.write_str(text(value))?;
                    }
                    Ok(())
                })
            }
            Self::Split => {
                let separator = nonempty(context, text(&arguments[1]))?;
                build::array(context, || text(&arguments[0]).split(separator))
            }
            Self::Lines => build::array(context, || text(&arguments[0]).lines()),
            Self::Replace => {
                let value = text(&arguments[0]);
                let needle = nonempty(context, text(&arguments[1]))?;
                let replacement = text(&arguments[2]);
                build::render(context, |output, _| {
                    for (index, part) in value.split(needle).enumerate() {
                        if index > 0 {
                            output.write_str(replacement)?;
                        }
                        output.write_str(part)?;
                    }
                    Ok(())
                })
            }
            Self::Contains | Self::Starts | Self::Ends => {
                let value = text(&arguments[0]);
                let needle = text(&arguments[1]);
                context.temporary(Literal::Bool(match self {
                    Self::Contains => value.contains(needle),
                    Self::Starts => value.starts_with(needle),
                    _ => value.ends_with(needle),
                }))
            }
            Self::Length => {
                let count = text(&arguments[0]).chars().count();
                let count = length_int(context, count)?;
                context.temporary(Literal::Int(count))
            }
            Self::Slice => {
                let value = text(&arguments[0]);
                let start = nonnegative(context, &arguments[1])?;
                let end = nonnegative(context, &arguments[2])?;
                if end < start {
                    return Err(invalid(context, "String slice end precedes start"));
                }
                let mut start_byte = None;
                let mut end_byte = None;
                for (index, byte) in value
                    .char_indices()
                    .map(|(byte, _)| byte)
                    .chain(std::iter::once(value.len()))
                    .enumerate()
                {
                    context.checkpoint()?;
                    if index == start {
                        start_byte = Some(byte);
                    }
                    if index == end {
                        end_byte = Some(byte);
                        break;
                    }
                }
                let (Some(start), Some(end)) = (start_byte, end_byte) else {
                    return Err(invalid(
                        context,
                        "String slice range exceeds the scalar length",
                    ));
                };
                context.temporary_string(&value[start..end])
            }
            Self::Trim => context.temporary_string(text(&arguments[0]).trim()),
            Self::Upper | Self::Lower => {
                let value = text(&arguments[0]);
                let upper = matches!(self, Self::Upper);
                let size = build::case_size(context, value, upper)?;
                build::produce(context, size, || {
                    Ok(Literal::String(if upper {
                        value.to_uppercase()
                    } else {
                        value.to_lowercase()
                    }))
                })
            }
            _ => unreachable!("regex handled above"),
        }
    }
}

fn text(value: &Literal) -> &str {
    let Literal::String(text) = value else {
        unreachable!("validated String")
    };
    text
}
fn invalid(context: &Context, reason: &str) -> RuntimeDiagnostic {
    context.detail_error(BWErr::OperationIncompatibleError, reason, None, false)
}
fn length_int(context: &Context, count: usize) -> EvaluationResult<i32> {
    i32::try_from(count).map_err(|_| {
        context.detail_error(
            BWErr::ArithmeticError,
            "String scalar length exceeds the Int range",
            None,
            false,
        )
    })
}
fn nonempty<'a>(context: &Context, text: &'a str) -> EvaluationResult<&'a str> {
    if text.is_empty() {
        Err(invalid(
            context,
            "String separator/needle must not be empty",
        ))
    } else {
        Ok(text)
    }
}
fn nonnegative(context: &Context, value: &Literal) -> EvaluationResult<usize> {
    let Literal::Int(value) = value else {
        unreachable!("validated Int")
    };
    usize::try_from(*value).map_err(|_| invalid(context, "String indexes must be nonnegative"))
}
fn limit(context: &Context, resource: &'static str, limit: usize) -> RuntimeDiagnostic {
    context
        .retain_limit(Diagnostic::new(BWErr::ResourceLimit {
            resource,
            limit: limit as u64,
        }))
        .into()
}

lazy_static::lazy_static! {
    pub(super) static ref FIXED: Vec<(Builtin, Arc<StatementSignature>)> = [
        StringOp::Format, StringOp::Join, StringOp::Split, StringOp::Lines,
        StringOp::Replace, StringOp::Contains, StringOp::Starts, StringOp::Ends,
        StringOp::Length, StringOp::Slice, StringOp::Trim, StringOp::Upper,
        StringOp::Lower, StringOp::Matches, StringOp::FindAll, StringOp::Captures,
    ].into_iter().map(|kind| (Builtin::String(kind), Arc::new(kind.signature()))).collect();
}

#[cfg(test)]
mod tests;
