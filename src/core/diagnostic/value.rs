//! Borrowed metadata traversal, admission, and iterative owned construction.

use super::{CallFrame, Diagnostic, DiagnosticOmissions, Help, OmittedSource, RelatedLocation};
use crate::core::{
    ast::Span,
    grammar::{BWErr, Literal},
    value_limits::{ValueLimits, ValueSize},
};
use std::fmt;

#[cfg(test)]
mod tests;

/// Limits for one diagnostic metadata conversion. No original diagnostic is mutated.
#[derive(Clone, Debug)]
pub struct DiagnosticValueLimits {
    pub values: ValueLimits,
    /// Conservative scan charge: six times start + end offsets per source occurrence.
    pub position_bytes: usize,
}

impl Default for DiagnosticValueLimits {
    fn default() -> Self {
        Self {
            values: ValueLimits::default(),
            position_bytes: 64 * 1024 * 1024,
        }
    }
}

impl DiagnosticValueLimits {
    pub(crate) fn intersect(mut self, other: &ValueLimits) -> Self {
        self.values.nodes = self.values.nodes.min(other.nodes);
        self.values.depth = self.values.depth.min(other.depth);
        self.values.string_bytes = self.values.string_bytes.min(other.string_bytes);
        self.values.key_bytes = self.values.key_bytes.min(other.key_bytes);
        self.values.entries = self.values.entries.min(other.entries);
        self.values.payload_bytes = self.values.payload_bytes.min(other.payload_bytes);
        self
    }
}

#[derive(Clone, Copy)]
enum Text<'a> {
    Borrowed(&'a str),
    Error(&'a BWErr),
    Help(&'a BWErr),
    Offset(usize),
    Limit(u64),
}

impl fmt::Display for Text<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Borrowed(value) => formatter.write_str(value),
            Self::Error(value) => write!(formatter, "{value}"),
            Self::Help(value) => write!(formatter, "{}", Help(value)),
            Self::Offset(value) => write!(formatter, "{value}"),
            Self::Limit(value) => write!(formatter, "{value}"),
        }
    }
}

#[derive(Clone, Copy)]
enum Node<'a> {
    Diagnostic(&'a Diagnostic),
    Details(&'a BWErr),
    Source(Option<&'a Span>),
    Call(&'a CallFrame),
    Related(&'a RelatedLocation),
    Array(Items<'a>),
    Text(Text<'a>),
    Bool(bool),
    Omissions(&'a DiagnosticOmissions),
    OmittedSource(&'a OmittedSource),
}

#[derive(Clone, Copy)]
enum Items<'a> {
    Calls(&'a [CallFrame]),
    Related(&'a [RelatedLocation]),
    Causes(&'a [Diagnostic]),
}

impl<'a> Items<'a> {
    fn len(self) -> usize {
        match self {
            Self::Calls(values) => values.len(),
            Self::Related(values) => values.len(),
            Self::Causes(values) => values.len(),
        }
    }

    fn get(self, index: usize) -> Node<'a> {
        match self {
            Self::Calls(values) => Node::Call(&values[index]),
            Self::Related(values) => Node::Related(&values[index]),
            Self::Causes(values) => Node::Diagnostic(&values[index]),
        }
    }
}

enum Shape<'a> {
    None,
    Text(Text<'a>),
    Bool(bool),
    // Schema maps have at most nine entries, regardless of input width.
    Map(Vec<(&'static str, Node<'a>)>),
    Array(Items<'a>),
}

fn text(value: &str) -> Node<'_> {
    Node::Text(Text::Borrowed(value))
}
fn offset(value: usize) -> Node<'static> {
    Node::Text(Text::Offset(value))
}

impl<'a> Node<'a> {
    fn shape(self) -> Shape<'a> {
        match self {
            Self::Text(value) => Shape::Text(value),
            Self::Bool(value) => Shape::Bool(value),
            Self::Omissions(value) => Shape::Map(vec![
                ("detail_fields", offset(value.detail_fields)),
                ("call_frames", offset(value.call_frames)),
                ("related_locations", offset(value.related_locations)),
                ("direct_causes", offset(value.direct_causes)),
                ("label", Self::Bool(value.label)),
                ("prior_summary", Self::Bool(value.prior_summary)),
                (
                    "source",
                    value
                        .source
                        .as_ref()
                        .map_or(Self::Source(None), Self::OmittedSource),
                ),
            ]),
            Self::OmittedSource(value) => Shape::Map(vec![
                ("file", text(&value.file)),
                ("file_truncated", Self::Bool(value.file_truncated)),
                ("start_byte", offset(value.start_byte)),
                ("end_byte", offset(value.end_byte)),
            ]),
            Self::Array(values) => Shape::Array(values),
            Self::Source(None) => Shape::None,
            Self::Source(Some(span)) => {
                let (line, column) = span.line_column();
                let (end_line, end_column) = span.end_line_column();
                Shape::Map(vec![
                    ("file", text(span.source().name())),
                    ("text", text(span.text())),
                    ("start_byte", offset(span.start())),
                    ("end_byte", offset(span.end())),
                    ("line", offset(line)),
                    ("column", offset(column)),
                    ("end_line", offset(end_line)),
                    ("end_column", offset(end_column)),
                ])
            }
            Self::Diagnostic(value) => {
                let mut fields = vec![
                    ("code", text(value.code().as_str())),
                    ("message", Self::Text(Text::Error(&value.error))),
                    ("help", Self::Text(Text::Help(&value.error))),
                    ("details", Self::Details(&value.error)),
                    ("source", Self::Source(value.span.as_ref())),
                    ("call_stack", Self::Array(Items::Calls(&value.call_stack))),
                    ("related", Self::Array(Items::Related(&value.related))),
                    ("causes", Self::Array(Items::Causes(&value.causes))),
                ];
                if let Some(omissions) = &value.omissions {
                    fields.push(("omissions", Self::Omissions(omissions)));
                }
                Shape::Map(fields)
            }
            Self::Call(value) => Shape::Map(vec![
                ("signature", text(&value.signature)),
                ("call_site", Self::Source(Some(&value.call_site))),
                (
                    "definition_site",
                    Self::Source(value.definition_site.as_ref()),
                ),
            ]),
            Self::Related(value) => Shape::Map(vec![
                ("message", text(&value.message)),
                ("source", Self::Source(Some(&value.span))),
            ]),
            Self::Details(error) => Shape::Map(match error {
                BWErr::VariableNotDefined(name) => vec![("name", text(name))],
                BWErr::StatementNotDefined(call) => vec![("call", text(call))],
                BWErr::DuplicateStatement {
                    signature,
                    original,
                    duplicate,
                } => vec![
                    ("signature", text(signature)),
                    ("original", text(original)),
                    ("duplicate", text(duplicate)),
                ],
                BWErr::DuplicateParameter {
                    name,
                    original,
                    duplicate,
                } => vec![
                    ("name", text(name)),
                    ("original", text(original)),
                    ("duplicate", text(duplicate)),
                ],
                BWErr::CollectionAccessError {
                    path,
                    segment,
                    reason,
                } => vec![
                    ("path", text(path)),
                    ("segment", text(segment)),
                    ("reason", text(reason)),
                ],
                BWErr::DuplicateNamespace {
                    namespace,
                    original,
                    duplicate,
                } => vec![
                    ("namespace", text(namespace)),
                    ("original", text(original)),
                    ("duplicate", text(duplicate)),
                ],
                BWErr::ResourceLimit { resource, limit } => vec![
                    ("resource", text(resource)),
                    ("limit", Self::Text(Text::Limit(*limit))),
                ],
                BWErr::ParameterMissingError(reason)
                | BWErr::ParsingError(reason)
                | BWErr::SignatureError(reason)
                | BWErr::ParsingIntegerError(reason)
                | BWErr::OperationIncompatibleError(reason)
                | BWErr::ControlFlowError(reason)
                | BWErr::ArithmeticError(reason)
                | BWErr::OutputError(reason)
                | BWErr::NativeError(reason)
                | BWErr::Cancelled(reason)
                | BWErr::Timeout(reason)
                | BWErr::AsyncRuntime(reason)
                | BWErr::ImportRead(reason)
                | BWErr::ImportCycle(reason)
                | BWErr::NativePanic(reason)
                | BWErr::InputError(reason)
                | BWErr::RunConfiguration(reason)
                | BWErr::SourceRead(reason) => vec![("reason", text(reason))],
            }),
        }
    }
}

struct Counter {
    bytes: usize,
    limit: usize,
}
impl fmt::Write for Counter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.bytes = self
            .bytes
            .checked_add(value.len())
            .filter(|bytes| *bytes <= self.limit)
            .ok_or(fmt::Error)?;
        Ok(())
    }
}

fn exceeded(resource: &'static str, limit: usize) -> BWErr {
    BWErr::ResourceLimit {
        resource,
        limit: limit as u64,
    }
}

fn charge_position(total: &mut usize, start: usize, end: usize, limit: usize) -> Result<(), BWErr> {
    // Each coordinate scans its prefix at most three times; two conversion passes.
    *total = start
        .checked_add(end)
        .and_then(|bytes| bytes.checked_mul(6))
        .and_then(|bytes| total.checked_add(bytes))
        .filter(|bytes| *bytes <= limit)
        .ok_or_else(|| exceeded("diagnostic position bytes", limit))?;
    Ok(())
}

enum Cursor<'a> {
    Visit(Node<'a>, usize),
    Map(std::vec::IntoIter<(&'static str, Node<'a>)>, usize),
    Array(Items<'a>, usize, usize),
}

pub(super) fn measure(
    diagnostic: &Diagnostic,
    limits: &DiagnosticValueLimits,
) -> Result<ValueSize, BWErr> {
    let values = &limits.values;
    values.validate()?;
    let mut size = ValueSize::default();
    let mut positions = 0;
    let mut pending = vec![Cursor::Visit(Node::Diagnostic(diagnostic), 1)];
    while let Some(cursor) = pending.pop() {
        match cursor {
            Cursor::Map(mut items, depth) => {
                if let Some((key, child)) = items.next() {
                    values.key_size(key.len())?;
                    values.add_bytes(&mut size, key.len())?;
                    pending.push(Cursor::Map(items, depth));
                    pending.push(Cursor::Visit(child, depth));
                }
            }
            Cursor::Array(items, index, depth) => {
                if index < items.len() {
                    pending.push(Cursor::Array(items, index + 1, depth));
                    pending.push(Cursor::Visit(items.get(index), depth));
                }
            }
            Cursor::Visit(node, depth) => {
                if size.nodes >= values.nodes {
                    return Err(exceeded("value nodes", values.nodes));
                }
                size.nodes += 1;
                size.depth = size.depth.max(depth);
                values.check_size(size)?;
                if let Node::Source(Some(span)) = node {
                    charge_position(
                        &mut positions,
                        span.start(),
                        span.end(),
                        limits.position_bytes,
                    )?;
                }
                match node.shape() {
                    Shape::None => (),
                    Shape::Bool(_) => values.add_bytes(&mut size, 1)?,
                    Shape::Text(value) => {
                        let mut counter = Counter {
                            bytes: 0,
                            limit: values.string_bytes,
                        };
                        fmt::write(&mut counter, format_args!("{value}"))
                            .map_err(|_| exceeded("value string bytes", values.string_bytes))?;
                        values.add_bytes(&mut size, counter.bytes)?;
                    }
                    Shape::Map(items) => {
                        values.container_header(items.len())?;
                        pending.push(Cursor::Map(items.into_iter(), depth + 1));
                    }
                    Shape::Array(items) => {
                        values.container_header(items.len())?;
                        pending.push(Cursor::Array(items, 0, depth + 1));
                    }
                }
            }
        }
    }
    Ok(size)
}

pub(super) fn build(diagnostic: &Diagnostic) -> Literal {
    enum Work<'a> {
        Visit(Node<'a>),
        Map(Vec<&'static str>),
        Array(usize),
    }
    let mut pending = vec![Work::Visit(Node::Diagnostic(diagnostic))];
    let mut ready = Vec::new();
    while let Some(work) = pending.pop() {
        match work {
            Work::Visit(node) => match node.shape() {
                Shape::None => ready.push(Literal::None),
                Shape::Bool(value) => ready.push(Literal::Bool(value)),
                Shape::Text(value) => ready.push(Literal::String(value.to_string())),
                Shape::Map(items) => {
                    pending.push(Work::Map(items.iter().map(|(key, _)| *key).collect()));
                    pending.extend(items.into_iter().rev().map(|(_, child)| Work::Visit(child)));
                }
                Shape::Array(items) => {
                    pending.push(Work::Array(items.len()));
                    pending.extend(
                        (0..items.len())
                            .rev()
                            .map(|index| Work::Visit(items.get(index))),
                    );
                }
            },
            Work::Map(keys) => {
                let values = ready.split_off(ready.len() - keys.len());
                ready.push(Literal::Map(
                    keys.into_iter().map(str::to_owned).zip(values).collect(),
                ));
            }
            Work::Array(length) => {
                let values = ready.split_off(ready.len() - length);
                ready.push(Literal::Array(values));
            }
        }
    }
    ready.pop().expect("diagnostic root")
}
