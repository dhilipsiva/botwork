//! Admission for a single borrowed error-detail field before its first owned copy.

use super::{CallFrame, Diagnostic, DiagnosticLimits};
use crate::core::{ast::Span, grammar::BWErr};
use std::sync::Arc;

#[cfg(test)]
mod tests;

impl DiagnosticLimits {
    pub(crate) fn borrowed_detail<'a>(
        &self,
        category: fn(String) -> BWErr,
        detail: &str,
        span: Option<&Span>,
        expression: bool,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Diagnostic {
        // Empty detail has no payload allocation. Measure all prospective source
        // owners, labels and frames before admitting the requested string bytes.
        let mut skeleton = Diagnostic::new(category(String::new()));
        if let Some(span) = span {
            skeleton = if expression {
                skeleton.at_expression(span)
            } else {
                skeleton.at(span)
            };
        }
        let admission = self
            .check_with_stack(&skeleton, frames.clone())
            .and_then(|size| {
                size.text_bytes
                    .checked_add(detail.len())
                    .filter(|bytes| *bytes <= self.text_bytes)
                    .map(|_| ())
                    .ok_or(BWErr::ResourceLimit {
                        resource: "diagnostic text bytes",
                        limit: self.text_bytes as u64,
                    })
            });
        match admission {
            Ok(()) => {
                skeleton.error = Arc::new(category(detail.to_owned()));
                skeleton.capture_stack(frames)
            }
            Err(violation) => super::rejection::reject_borrowed_detail(
                skeleton,
                category,
                detail,
                violation,
                frames.len(),
            ),
        }
    }
}
