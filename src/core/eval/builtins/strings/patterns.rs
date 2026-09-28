use super::*;
use regex::{Regex, RegexBuilder};

const PATTERN_BYTES: usize = 16 * 1024;
const COMPILED_BYTES: usize = 2 * 1024 * 1024;
const CACHE_BYTES: usize = 2 * 1024 * 1024;
const NESTING: u32 = 64;
const SEARCH_BYTES: usize = 64 * 1024 * 1024;

fn compile(context: &Context, pattern: &str) -> EvaluationResult<Regex> {
    context.checkpoint()?;
    if pattern.len() > PATTERN_BYTES {
        return Err(limit(context, "regex pattern bytes", PATTERN_BYTES));
    }
    RegexBuilder::new(pattern)
        .size_limit(COMPILED_BYTES)
        .dfa_size_limit(CACHE_BYTES)
        .nest_limit(NESTING)
        .build()
        .map_err(|error| match error {
            regex::Error::CompiledTooBig(_) => {
                limit(context, "regex compiled bytes", COMPILED_BYTES)
            }
            _ => context.formatted_error(
                BWErr::OperationIncompatibleError,
                format_args!("Invalid regex: {error}"),
                None,
                false,
            ),
        })
}

fn charge(
    context: &Context,
    used: &mut usize,
    bytes: usize,
    multiplier: usize,
) -> EvaluationResult<()> {
    context.checkpoint()?;
    let next = bytes
        .checked_add(1)
        .and_then(|n| n.checked_mul(multiplier))
        .and_then(|n| used.checked_add(n));
    match next {
        Some(next) if next <= SEARCH_BYTES => {
            *used = next;
            Ok(())
        }
        _ => Err(limit(context, "regex search bytes", SEARCH_BYTES)),
    }
}

pub(super) fn invoke(
    kind: StringOp,
    text: &str,
    pattern: &str,
    context: &Context,
) -> TemporaryResult {
    let regex = compile(context, pattern)?;
    let mut work = 0;
    let limits = context.limits().values;
    match kind {
        StringOp::Matches => {
            charge(context, &mut work, text.len(), 1)?;
            context.temporary(Literal::Bool(regex.is_match(text)))
        }
        StringOp::Captures => {
            charge(context, &mut work, text.len(), 1)?;
            let captures = regex.captures(text);
            let count = captures.as_ref().map_or(0, regex::Captures::len);
            let mut size = limits
                .container_header(count)
                .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
            if let Some(captures) = &captures {
                for value in captures.iter() {
                    context.checkpoint()?;
                    let child = match value {
                        Some(value) => limits.string_size(value.len()),
                        None => limits.check(&Literal::None),
                    }
                    .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
                    limits
                        .add_child(&mut size, child)
                        .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
                }
            }
            build::produce(context, size, || {
                let mut output = Vec::with_capacity(count);
                if let Some(captures) = captures {
                    for value in captures.iter() {
                        context.checkpoint()?;
                        output.push(value.map_or(Literal::None, |value| {
                            Literal::String(value.as_str().into())
                        }));
                    }
                }
                Ok(Literal::Array(output))
            })
        }
        StringOp::FindAll => {
            let mut size = limits
                .container_header(0)
                .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
            let mut count = 0usize;
            let mut previous = 0;
            let mut matches = regex.find_iter(text);
            loop {
                // Bound repeated suffix scans before each next(). Factor four
                // covers both passes and the iterator's empty-match retry.
                charge(context, &mut work, text.len() - previous, 4)?;
                let Some(value) = matches.next() else { break };
                previous = value.end();
                count = count
                    .checked_add(1)
                    .ok_or_else(|| limit(context, "value container entries", limits.entries))?;
                limits
                    .container_header(count)
                    .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
                let child = limits
                    .string_size(value.len())
                    .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
                limits
                    .add_child(&mut size, child)
                    .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
            }
            build::produce(context, size, || {
                let mut output = Vec::with_capacity(count);
                for value in regex.find_iter(text) {
                    context.checkpoint()?;
                    output.push(Literal::String(value.as_str().into()));
                }
                Ok(Literal::Array(output))
            })
        }
        _ => unreachable!("regex statement"),
    }
}

#[cfg(test)]
mod tests;
