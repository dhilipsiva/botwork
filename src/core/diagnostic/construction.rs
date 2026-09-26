//! Admission for a single borrowed error-detail field before its first owned copy.

use super::{CallFrame, Diagnostic, DiagnosticLimits};
use crate::core::{ast::Span, grammar::BWErr};
use std::{
    fmt::{self, Write},
    sync::Arc,
};

#[cfg(test)]
mod tests;

struct Counter {
    bytes: usize,
    maximum: usize,
}
impl Write for Counter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.bytes = self
            .bytes
            .checked_add(text.len())
            .filter(|bytes| *bytes <= self.maximum)
            .ok_or(fmt::Error)?;
        Ok(())
    }
}

struct BoundedText {
    text: String,
    maximum: usize,
}
impl Write for BoundedText {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let available = self.maximum - self.text.len();
        if text.len() <= available {
            self.text.push_str(text);
            Ok(())
        } else {
            let mut end = available;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.push_str(&text[..end]);
            Err(fmt::Error)
        }
    }
}

fn formatted_prefix(message: fmt::Arguments<'_>) -> (String, bool) {
    let mut output = BoundedText {
        text: String::with_capacity(super::SUMMARY_DETAIL_BYTES),
        maximum: super::SUMMARY_DETAIL_BYTES,
    };
    let shortened = output.write_fmt(message).is_err();
    if shortened {
        let marker = super::rejection::TRUNCATED;
        let mut end = output
            .text
            .len()
            .min(super::SUMMARY_DETAIL_BYTES - marker.len());
        while !output.text.is_char_boundary(end) {
            end -= 1;
        }
        output.text.truncate(end);
        output.text.push_str(marker);
    }
    (output.text, shortened)
}

fn skeleton(category: fn(String) -> BWErr, span: Option<&Span>, expression: bool) -> Diagnostic {
    let diagnostic = Diagnostic::new(category(String::new()));
    match span {
        Some(span) if expression => diagnostic.at_expression(span),
        Some(span) => diagnostic.at(span),
        None => diagnostic,
    }
}

fn text_limit(maximum: usize) -> BWErr {
    BWErr::ResourceLimit {
        resource: "diagnostic text bytes",
        limit: maximum as u64,
    }
}

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
        let mut skeleton = skeleton(category, span, expression);
        let admission = self
            .check_with_stack(&skeleton, frames.clone())
            .and_then(|size| {
                size.text_bytes
                    .checked_add(detail.len())
                    .filter(|bytes| *bytes <= self.text_bytes)
                    .map(|_| ())
                    .ok_or_else(|| text_limit(self.text_bytes))
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

    /// Internal formatters must stream deterministic scalar/text fragments without
    /// constructing intermediate owned strings. This is not an arbitrary host Display API.
    pub(crate) fn formatted_detail<'a>(
        &self,
        category: fn(String) -> BWErr,
        message: fmt::Arguments<'_>,
        span: Option<&Span>,
        expression: bool,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Diagnostic {
        self.formatted_fields(
            |[detail]| category(detail),
            [message],
            span,
            expression,
            frames,
        )
    }

    /// Internal constructors map each measured message to exactly one BWErr detail field.
    /// Current categories have one or three fields; admit the complete group before allocation.
    pub(crate) fn formatted_fields<'a, const N: usize>(
        &self,
        category: impl Fn([String; N]) -> BWErr,
        messages: [fmt::Arguments<'_>; N],
        span: Option<&Span>,
        expression: bool,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Diagnostic {
        let mut skeleton = Diagnostic::new(category(std::array::from_fn(|_| String::new())));
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
                let mut remaining = self.text_bytes - size.text_bytes;
                let mut sizes = [0; N];
                for (message, size) in messages.iter().zip(&mut sizes) {
                    let mut count = Counter {
                        bytes: 0,
                        maximum: remaining,
                    };
                    count
                        .write_fmt(*message)
                        .map_err(|_| text_limit(self.text_bytes))?;
                    *size = count.bytes;
                    remaining -= count.bytes;
                }
                let mut details = std::array::from_fn(|_| String::new());
                for ((message, size), detail) in messages.iter().zip(sizes).zip(&mut details) {
                    let mut output = BoundedText {
                        text: String::with_capacity(size),
                        maximum: size,
                    };
                    output
                        .write_fmt(*message)
                        .map_err(|_| text_limit(self.text_bytes))?;
                    *detail = output.text;
                }
                Ok(details)
            });
        match admission {
            Ok(details) => {
                skeleton.error = Arc::new(category(details));
                skeleton.capture_stack(frames)
            }
            Err(violation) => {
                let mut shortened = 0;
                let details = std::array::from_fn(|index| {
                    let (detail, truncated) = formatted_prefix(messages[index]);
                    shortened += usize::from(truncated);
                    detail
                });
                skeleton.error = Arc::new(category(details));
                super::rejection::reject_constructed_error(
                    skeleton,
                    shortened,
                    violation,
                    frames.len(),
                )
            }
        }
    }
}
