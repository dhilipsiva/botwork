//! Immutable collection operations. Complete output admission precedes copying.
use super::*;
use crate::core::{
    signature::ValueKinds,
    value_limits::{ValueLimits, ValueSize},
};

mod build;
use build::{array_size, map_size, produce};

#[derive(Clone, Copy)]
pub(in crate::core::eval) enum Collection {
    Array,
    Map,
    FromEntries,
    Repeat,
    Get,
    Set,
    Append,
    Remove,
    Contains,
    Length,
    Keys,
    Values,
    Entries,
    Enumerate,
    Equal,
    Slice,
}

impl Collection {
    pub(super) fn signature(self) -> StatementSignature {
        let both = ValueKinds::from(Kind::Array).union(Kind::Map.into());
        let array = ValueKinds::from(Kind::Array);
        let map = ValueKinds::from(Kind::Map);
        let int = ValueKinds::from(Kind::Int);
        let (header, description, parameters, returns) = match self {
            Self::Array => ("Create Array", "Return an empty array.", vec![], Kind::Array),
            Self::Map => ("Create Map", "Return an empty map.", vec![], Kind::Map),
            Self::FromEntries => ("Create Map From |entries|", "Copy [String key, value] pairs into a map; duplicate keys are invalid.", vec![("entries", array)], Kind::Map),
            Self::Repeat => ("Repeat |value| Times |count|", "Return count independent copies in an array; count is a nonnegative Int.", vec![("count", int)], Kind::Array),
            Self::Get => ("Get From |collection| At |key|", "Copy an existing element using an Int array index or exact String map key.", vec![("collection", both)], Kind::None),
            Self::Set => ("Set In |collection| At |key| To |value|", "Return a replacement collection; replace an existing array index or upsert a map key.", vec![("collection", both)], Kind::None),
            Self::Append => ("Append To |array| Value |value|", "Return a replacement array with one appended value.", vec![("array", array)], Kind::Array),
            Self::Remove => ("Remove From |collection| At |key|", "Return a replacement without the existing index/key; array elements shift left.", vec![("collection", both)], Kind::None),
            Self::Contains => ("Collection Contains |collection| Item |item|", "Return whether an array has a deeply equal value or a map has the exact String key.", vec![("collection", both)], Kind::Bool),
            Self::Length => ("Length Of |collection|", "Return the top-level entry count as an Int; fail if it cannot be represented.", vec![("collection", both)], Kind::Int),
            Self::Keys => ("Map Keys |map|", "Copy keys to an array in ascending Unicode order.", vec![("map", map)], Kind::Array),
            Self::Values => ("Map Values |map|", "Copy values to an array in ascending key order.", vec![("map", map)], Kind::Array),
            Self::Entries => ("Map Entries |map|", "Copy [key, value] pairs to an array in ascending key order.", vec![("map", map)], Kind::Array),
            Self::Enumerate => ("Enumerate |array|", "Copy [Int index, value] pairs to an array in original order.", vec![("array", array)], Kind::Array),
            Self::Equal => ("Collections Equal |left| And |right|", "Return deep equality using exact numeric comparisons, array order, and map keys.", vec![("left", both), ("right", both)], Kind::Bool),
            Self::Slice => ("Slice |array| From |start| To |end|", "Copy the half-open range start..end; require 0 <= start <= end <= length.", vec![("array", array), ("start", int), ("end", int)], Kind::Array),
        };
        let mut signature = StatementSignature::native_at("<collections>", header)
            .expect("fixed collection signature")
            .description(description);
        for (name, kind) in parameters {
            signature = signature.parameter(name, kind).expect("fixed parameter");
        }
        if !matches!(self, Self::Get | Self::Set | Self::Remove) {
            signature = signature.returns(returns);
        }
        if matches!(self, Self::Set | Self::Remove) {
            signature = signature.returns(both);
        }
        if matches!(
            self,
            Self::Get | Self::Set | Self::Remove | Self::Slice | Self::Contains
        ) {
            signature = signature
                .documents_error(
                    Code::CollectionAccess,
                    "The index/key kind, range, or presence is invalid.",
                )
                .expect("fixed error");
        }
        if matches!(self, Self::Repeat | Self::FromEntries) {
            signature = signature
                .documents_error(
                    Code::IncompatibleType,
                    "The count or key/value pairs are invalid.",
                )
                .expect("fixed error");
        }
        if matches!(self, Self::Length | Self::Enumerate) {
            signature = signature
                .documents_error(Code::Arithmetic, "A length or index cannot fit an Int.")
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
        let limits = context.limits().values;
        let limit_error = |error| context.retain_limit(Diagnostic::new(error));
        match self {
            Self::Array => context.temporary(Literal::Array(vec![])),
            Self::Map => context.temporary(Literal::Map(HashMap::new())),
            Self::Repeat => {
                let Literal::Int(count) = *arguments[1] else {
                    unreachable!("validated count")
                };
                let count = usize::try_from(count)
                    .map_err(|_| invalid(context, "Repeat count must be nonnegative"))?;
                let size =
                    build::repeat_size(&limits, &arguments[0], count).map_err(limit_error)?;
                produce(context, size, || {
                    let mut values = Vec::with_capacity(count);
                    for _ in 0..count {
                        context.checkpoint()?;
                        values.push((*arguments[0]).clone());
                    }
                    Ok(Literal::Array(values))
                })
            }
            Self::FromEntries => {
                let entries = array(&arguments[0]);
                // Validate shapes before output copies; duplicate detection occurs
                // during the admitted build, without an extra unbounded key table.
                for value in entries {
                    pair(context, value)?;
                }
                let pairs = || {
                    entries.iter().map(|value| {
                        let Literal::Array(pair) = value else {
                            unreachable!()
                        };
                        let Literal::String(key) = &pair[0] else {
                            unreachable!()
                        };
                        (key.as_str(), &pair[1])
                    })
                };
                let size = map_size(&limits, entries.len(), pairs()).map_err(limit_error)?;
                build::map(context, size, entries.len(), pairs())
            }
            Self::Get => {
                let selected = match &*arguments[0] {
                    Literal::Array(values) => {
                        &values[index(context, &arguments[1], values.len(), false)?]
                    }
                    Literal::Map(values) => values
                        .get(key(context, &arguments[1])?)
                        .ok_or_else(|| access(context, &arguments[1], "map key does not exist"))?,
                    _ => unreachable!("validated collection"),
                };
                context.copy_temporary(selected)
            }
            Self::Set | Self::Remove => match &*arguments[0] {
                Literal::Array(values) => {
                    let at = index(context, &arguments[1], values.len(), false)?;
                    let remove = matches!(self, Self::Remove);
                    let selected = || {
                        values.iter().enumerate().filter_map(|(i, value)| {
                            if i != at {
                                Some(value)
                            } else if remove {
                                None
                            } else {
                                Some(&*arguments[2])
                            }
                        })
                    };
                    let count = values.len() - usize::from(remove);
                    let size =
                        array_size(&limits, count, selected().map(|value| limits.check(value)))
                            .map_err(limit_error)?;
                    build::array(context, size, count, selected())
                }
                Literal::Map(values) => {
                    let key = key(context, &arguments[1])?;
                    let remove = matches!(self, Self::Remove);
                    if remove && !values.contains_key(key) {
                        return Err(access(context, &arguments[1], "map key does not exist"));
                    }
                    let replacement = (!remove).then(|| (key, &*arguments[2]));
                    let count =
                        values.len() - usize::from(values.contains_key(key)) + usize::from(!remove);
                    let selected = || {
                        values
                            .iter()
                            .filter(|(name, _)| name.as_str() != key)
                            .map(|(name, value)| (name.as_str(), value))
                            .chain(replacement)
                    };
                    let size = map_size(&limits, count, selected()).map_err(limit_error)?;
                    build::map(context, size, count, selected())
                }
                _ => unreachable!("validated collection"),
            },
            Self::Append => {
                let values = array(&arguments[0]);
                let count = values.len().saturating_add(1);
                let selected = || values.iter().chain(std::iter::once(&*arguments[1]));
                let size = array_size(&limits, count, selected().map(|value| limits.check(value)))
                    .map_err(limit_error)?;
                build::array(context, size, count, selected())
            }
            Self::Contains => {
                let found = match &*arguments[0] {
                    Literal::Map(values) => values.contains_key(key(context, &arguments[1])?),
                    Literal::Array(values) => {
                        let mut found = false;
                        for value in values {
                            context.checkpoint()?;
                            if equal(context, value, &arguments[1])? {
                                found = true;
                                break;
                            }
                        }
                        found
                    }
                    _ => unreachable!("validated collection"),
                };
                context.temporary(Literal::Bool(found))
            }
            Self::Length => {
                let length = match &*arguments[0] {
                    Literal::Array(values) => values.len(),
                    Literal::Map(values) => values.len(),
                    _ => unreachable!(),
                };
                context.temporary(Literal::Int(integer(context, length)?))
            }
            Self::Equal => {
                context.temporary(Literal::Bool(equal(context, &arguments[0], &arguments[1])?))
            }
            Self::Slice => {
                let values = array(&arguments[0]);
                let start = index(context, &arguments[1], values.len(), true)?;
                let end = index(context, &arguments[2], values.len(), true)?;
                if end < start {
                    return Err(access(context, &arguments[2], "slice end precedes start"));
                }
                let selected = &values[start..end];
                let size = array_size(
                    &limits,
                    selected.len(),
                    selected.iter().map(|value| limits.check(value)),
                )
                .map_err(limit_error)?;
                build::array(context, size, selected.len(), selected.iter())
            }
            Self::Keys | Self::Values | Self::Entries => {
                let Literal::Map(values) = &*arguments[0] else {
                    unreachable!("validated map")
                };
                let size = array_size(
                    &limits,
                    values.len(),
                    values.iter().map(|(key, value)| match self {
                        Self::Keys => limits.string_size(key.len()),
                        Self::Values => limits.check(value),
                        _ => array_size(
                            &limits,
                            2,
                            [limits.string_size(key.len()), limits.check(value)],
                        ),
                    }),
                )
                .map_err(limit_error)?;
                produce(context, size, || {
                    let mut entries: Vec<_> = values.iter().collect();
                    entries.sort_unstable_by_key(|(key, _)| *key);
                    let mut result = Vec::with_capacity(entries.len());
                    for (key, value) in entries {
                        context.checkpoint()?;
                        result.push(match self {
                            Self::Keys => Literal::String(key.clone()),
                            Self::Values => value.clone(),
                            _ => Literal::Array(vec![Literal::String(key.clone()), value.clone()]),
                        });
                    }
                    Ok(Literal::Array(result))
                })
            }
            Self::Enumerate => {
                let values = array(&arguments[0]);
                if !values.is_empty() {
                    integer(context, values.len() - 1)?;
                }
                let size = array_size(
                    &limits,
                    values.len(),
                    values.iter().map(|value| {
                        array_size(
                            &limits,
                            2,
                            [limits.check(&Literal::Int(0)), limits.check(value)],
                        )
                    }),
                )
                .map_err(limit_error)?;
                produce(context, size, || {
                    let mut result = Vec::with_capacity(values.len());
                    for (index, value) in values.iter().enumerate() {
                        context.checkpoint()?;
                        result.push(Literal::Array(vec![
                            Literal::Int(integer(context, index)?),
                            value.clone(),
                        ]));
                    }
                    Ok(Literal::Array(result))
                })
            }
        }
    }
}

fn array(value: &Literal) -> &[Literal] {
    let Literal::Array(values) = value else {
        unreachable!("validated array")
    };
    values
}
fn pair<'a>(context: &Context, value: &'a Literal) -> EvaluationResult<(&'a str, &'a Literal)> {
    if let Literal::Array(pair) = value {
        if pair.len() == 2 {
            if let Literal::String(key) = &pair[0] {
                return Ok((key, &pair[1]));
            }
        }
    }
    Err(invalid(
        context,
        "Map entries must be [String key, value] pairs",
    ))
}
fn invalid(context: &Context, reason: &str) -> RuntimeDiagnostic {
    context.detail_error(BWErr::OperationIncompatibleError, reason, None, false)
}
fn access(context: &Context, key: &Literal, reason: &str) -> RuntimeDiagnostic {
    context.collection_error("collection statement", key, reason)
}
fn key<'a>(context: &Context, value: &'a Literal) -> EvaluationResult<&'a str> {
    match value {
        Literal::String(key) => Ok(key),
        _ => Err(access(context, value, "map key must be a String")),
    }
}
fn index(context: &Context, value: &Literal, length: usize, end: bool) -> EvaluationResult<usize> {
    let index = match value {
        Literal::Int(index) => usize::try_from(*index).ok(),
        _ => None,
    };
    index
        .filter(|index| *index < length || (end && *index == length))
        .ok_or_else(|| {
            access(
                context,
                value,
                "array index must be a nonnegative Int within range",
            )
        })
}
fn integer(context: &Context, value: usize) -> EvaluationResult<i32> {
    i32::try_from(value).map_err(|_| {
        context.detail_error(
            BWErr::ArithmeticError,
            "Collection length/index exceeds the Int range",
            None,
            false,
        )
    })
}
fn equal(context: &Context, left: &Literal, right: &Literal) -> EvaluationResult<bool> {
    crate::core::grammar::values_equal(left, right).map_err(|error| {
        context.formatted_error(BWErr::ArithmeticError, format_args!("{error}"), None, false)
    })
}

lazy_static::lazy_static! {
    pub(super) static ref FIXED: Vec<(Builtin, Arc<StatementSignature>)> = [
        Collection::Array, Collection::Map, Collection::FromEntries, Collection::Repeat,
        Collection::Get, Collection::Set, Collection::Append, Collection::Remove,
        Collection::Contains, Collection::Length, Collection::Keys, Collection::Values,
        Collection::Entries, Collection::Enumerate, Collection::Equal, Collection::Slice,
    ].into_iter().map(|kind| (Builtin::Collection(kind), Arc::new(kind.signature()))).collect();
}

#[cfg(test)]
mod tests;
