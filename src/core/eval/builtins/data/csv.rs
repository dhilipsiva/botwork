//! Header-row CSV: RFC 4180 fields and doubled quotes, LF or CRLF record ends.
//! A validating pass measures the exact result before the building pass allocates it.
use super::*;
use crate::core::csv::{Fields, Malformed};
use std::{borrow::Cow, collections::HashMap};

fn preview(name: &str) -> &str {
    name.char_indices()
        .nth(32)
        .map_or(name, |(index, _)| &name[..index])
}

/// The validated header and exact result size, from the measuring pass.
struct Table<'a> {
    header: Vec<Cow<'a, str>>,
    records: usize,
    size: ValueSize,
}

fn measure<'a>(text: &'a str, context: &Context) -> EvaluationResult<Table<'a>> {
    let invalid = |error: Malformed| {
        context.formatted_error(
            BWErr::OperationIncompatibleError,
            format_args!(
                "Parse CSV: line {}, column {}: {}",
                error.line, error.column, error.reason
            ),
            None,
            false,
        )
    };
    let limits = context.limits().values;
    let mut fields = Fields::new(text);
    let mut header = Vec::new();
    let mut header_bytes = 0usize;
    loop {
        let Some(field) = fields.next() else {
            return Err(context.detail_error(
                BWErr::OperationIncompatibleError,
                "Parse CSV requires a header row",
                None,
                false,
            ));
        };
        let field = field.map_err(invalid)?;
        limits
            .key_size(field.text.len())
            .and_then(|()| limits.container_header(header.len() + 1))
            .map_err(|error| {
                RuntimeDiagnostic::from(context.retain_limit(Diagnostic::new(error)))
            })?;
        header_bytes = header_bytes.saturating_add(field.text.len());
        header.push(field.text);
        if field.last {
            break;
        }
    }
    let mut columns = HashMap::with_capacity(header.len());
    for (index, name) in header.iter().enumerate() {
        if name.is_empty() {
            return Err(context.formatted_error(
                BWErr::OperationIncompatibleError,
                format_args!("Parse CSV: header column {} is empty", index + 1),
                None,
                false,
            ));
        }
        if let Some(first) = columns.insert(name.as_ref(), index) {
            return Err(context.formatted_error(
                BWErr::OperationIncompatibleError,
                format_args!(
                    "Parse CSV: header {:?} repeats columns {} and {}",
                    preview(name),
                    first + 1,
                    index + 1
                ),
                None,
                false,
            ));
        }
    }
    drop(columns);
    let limit =
        |error: BWErr| RuntimeDiagnostic::from(context.retain_limit(Diagnostic::new(error)));
    // Each record is a Map holding every header key plus one String per column.
    let mut row = limits.container_header(header.len()).map_err(limit)?;
    limits.add_bytes(&mut row, header_bytes).map_err(limit)?;
    let mut size = limits.container_header(0).map_err(limit)?;
    let (mut records, mut record, mut count, mut line) = (0, row, 0, fields.line);
    while let Some(field) = fields.next() {
        let field = field.map_err(invalid)?;
        count += 1;
        if count <= header.len() {
            let child = limits.string_size(field.text.len()).map_err(limit)?;
            limits.add_child(&mut record, child).map_err(limit)?;
        }
        if field.last {
            if count != header.len() {
                return Err(context.formatted_error(
                    BWErr::OperationIncompatibleError,
                    format_args!(
                        "Parse CSV: line {line}: record {} has {count} field{}; the header has {}",
                        records + 1,
                        if count == 1 { "" } else { "s" },
                        header.len()
                    ),
                    None,
                    false,
                ));
            }
            records += 1;
            context.checkpoint()?;
            limits.container_header(records).map_err(limit)?;
            limits.add_child(&mut size, record).map_err(limit)?;
            (record, count, line) = (row, 0, fields.line);
        }
    }
    Ok(Table {
        header,
        records,
        size,
    })
}

pub(super) fn parse(text: &str, context: &Context) -> TemporaryResult {
    let text = without_signature(text);
    let table = measure(text, context)?;
    let reservation = context.temporary_reservation(table.size)?;
    context.checkpoint()?;
    let mut fields = Fields::new(text);
    for _ in &table.header {
        fields.next();
    }
    let mut rows = Vec::with_capacity(table.records);
    for _ in 0..table.records {
        context.checkpoint()?;
        let mut row = std::collections::HashMap::with_capacity(table.header.len());
        for name in &table.header {
            let Some(Ok(field)) = fields.next() else {
                unreachable!("fields were validated by the measuring pass")
            };
            row.insert(
                name.clone().into_owned(),
                Literal::String(field.text.into_owned()),
            );
        }
        rows.push(Literal::Map(row));
    }
    Ok(TemporaryValue::new(Literal::Array(rows), reservation))
}
