//! Admission for borrowed/formatted diagnostic details before their first owned copies.

use super::{CallFrame, Diagnostic, DiagnosticLimits, DiagnosticResult, DiagnosticSize};
use crate::core::{
    ast::{SourceFile, Span},
    grammar::BWErr,
};
use std::{
    fmt::{self, Write},
    sync::Arc,
};

#[cfg(test)]
mod tests;

/// Internal alternative evidence for fields whose full formatting requires work
/// that must not run after admission fails (for example source-coordinate scans).
pub(crate) struct FormattedDetail<'a> {
    pub full: fmt::Arguments<'a>,
    pub summary: Option<fmt::Arguments<'a>>,
}

impl<'a> FormattedDetail<'a> {
    pub(crate) fn exact(full: fmt::Arguments<'a>) -> Self {
        Self {
            full,
            summary: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct DiagnosticConstruction<'a> {
    pub location: Option<(&'a Span, bool)>,
    pub related: Option<(&'a str, &'a Span)>,
    pub stopped: Option<Diagnostic>,
}

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

pub(super) fn formatted_prefix(message: fmt::Arguments<'_>) -> (String, bool) {
    formatted_prefix_with_limit(message, super::SUMMARY_DETAIL_BYTES)
}

fn formatted_prefix_with_limit(message: fmt::Arguments<'_>, maximum: usize) -> (String, bool) {
    let mut output = BoundedText {
        text: String::with_capacity(maximum),
        maximum,
    };
    let shortened = output.write_fmt(message).is_err();
    if shortened {
        let marker = super::rejection::TRUNCATED;
        let mut end = output.text.len().min(maximum - marker.len());
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

impl Diagnostic {
    /// Setup and standalone helpers have no installed caller context. Use the
    /// default diagnostic budget before their first owned message allocation.
    pub(crate) fn formatted(category: fn(String) -> BWErr, message: fmt::Arguments<'_>) -> Self {
        DiagnosticLimits::default().formatted_detail(
            category,
            message,
            None,
            false,
            std::iter::empty(),
        )
    }
}

impl DiagnosticLimits {
    /// Admit the entire construction before owning message, location, or frame
    /// payloads. The caller keeps the returned reservation with the diagnostic.
    pub(crate) fn formatted_admitted<'a, const N: usize, R>(
        &self,
        category: impl Fn([String; N]) -> BWErr,
        messages: [FormattedDetail<'_>; N],
        context: DiagnosticConstruction<'_>,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
        mut admit: impl FnMut(DiagnosticSize, Vec<Arc<SourceFile>>, Option<R>) -> Result<R, BWErr>,
    ) -> (Diagnostic, Option<R>) {
        let mut skeleton = Diagnostic::new(category(std::array::from_fn(|_| String::new())));
        let admission = self
            .retained_construction_size(
                &skeleton,
                context.stopped.as_ref(),
                context.location,
                context.related,
                frames.clone(),
            )
            .and_then(|(mut size, sources)| {
                let initial = admit(size, sources.clone(), None)?;
                let sizes = self.measure_strings(&messages, size.text_bytes)?;
                for bytes in sizes {
                    size.text_bytes += bytes;
                } // Checked by the grouped counter.
                let reservation = admit(size, sources, Some(initial))?;
                let details = self.write_strings(&messages, sizes)?;
                Ok((details, reservation))
            });
        match admission {
            Ok((details, reservation)) => {
                skeleton.error = Arc::new(category(details));
                let mut error = skeleton.capture_context(context.location, frames.clone());
                if let Some((message, span)) = context.related {
                    error = error.with_related(message, span);
                }
                if let Some(stopped) = context.stopped {
                    error = stopped
                        .capture_context(context.location, frames)
                        .while_handling(error);
                }
                (error, Some(reservation))
            }
            Err(violation) => {
                let error = if let Some(stopped) = context.stopped {
                    // The deferred detail is one omitted cause. Its formatter need
                    // not run when even the observed primary cannot be admitted.
                    let preserve_control = matches!(
                        stopped.code(),
                        super::DiagnosticCode::Cancelled | super::DiagnosticCode::Timeout
                    );
                    let mut rejected =
                        stopped.rejected_context(violation, context.location, frames.len());
                    rejected = rejected.omit_handled_cause();
                    if preserve_control {
                        let mut primary = rejected.causes.pop().expect("bounded stop");
                        primary.causes.push(rejected);
                        primary
                    } else {
                        rejected
                    }
                } else {
                    let mut shortened = 0;
                    let details = std::array::from_fn(|index| {
                        let message = &messages[index];
                        let (detail, truncated) =
                            formatted_prefix(message.summary.unwrap_or(message.full));
                        shortened += usize::from(truncated || message.summary.is_some());
                        detail
                    });
                    skeleton.error = Arc::new(category(details));
                    let mut rejected =
                        skeleton.rejected_context(violation, context.location, frames.len());
                    rejected.causes[0]
                        .omissions
                        .as_mut()
                        .expect("bounded detail")
                        .detail_fields += shortened;
                    if let Some((message, span)) = context.related {
                        rejected = rejected.with_related(message, span);
                    }
                    rejected
                };
                (error, None)
            }
        }
    }

    /// Source/syntax guards report a checked prefix ending just after the rejected
    /// token. Admit this new source owner together with prospective calls first.
    pub(crate) fn source_prefix<'a>(
        &self,
        error: BWErr,
        name: &str,
        prefix: &str,
        start: usize,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Diagnostic {
        let skeleton = Diagnostic::new(error);
        let admission = self
            .check_with_stack(&skeleton, frames.clone())
            .and_then(|size| {
                size.source_bytes
                    .checked_add(name.len())
                    .and_then(|bytes| bytes.checked_add(prefix.len()))
                    .filter(|bytes| *bytes <= self.source_bytes)
                    .map(|_| ())
                    .ok_or(BWErr::ResourceLimit {
                        resource: "diagnostic source bytes",
                        limit: self.source_bytes as u64,
                    })
            });
        match admission {
            Ok(()) => skeleton
                .at(&Span::source_prefix(name, prefix, start))
                .capture_stack(frames),
            Err(violation) => super::rejection::reject_source_prefix(
                skeleton,
                name,
                start,
                prefix.len(),
                violation,
                frames.len(),
            ),
        }
    }

    /// Admit a payload-free input origin before formatting its borrowed name into
    /// a SourceFile. Internal origins stream strings, paths, or scalar flag indexes.
    pub(crate) fn input_origin(
        &self,
        error: BWErr,
        origin: &(impl fmt::Display + ?Sized),
    ) -> Diagnostic {
        let skeleton = Diagnostic::new(error);
        let violation = || BWErr::ResourceLimit {
            resource: "diagnostic source bytes",
            limit: self.source_bytes as u64,
        };
        let admission = self.check(&skeleton).and_then(|_| {
            let mut count = Counter {
                bytes: 0,
                maximum: self.source_bytes,
            };
            write!(&mut count, "{origin}").map_err(|_| violation())?;
            let mut output = BoundedText {
                text: String::with_capacity(count.bytes),
                maximum: count.bytes,
            };
            write!(&mut output, "{origin}").map_err(|_| violation())?;
            Ok(output.text)
        });
        match admission {
            Ok(origin) => skeleton.at(&Span::input_origin(origin)),
            Err(violation) => {
                let prefix = formatted_prefix_with_limit(
                    format_args!("{origin}"),
                    super::SUMMARY_SOURCE_NAME_BYTES,
                );
                super::rejection::reject_input_origin(skeleton, prefix, violation)
            }
        }
    }

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
        self.formatted_evidence(
            category,
            messages.map(FormattedDetail::exact),
            span,
            expression,
            frames,
        )
    }

    pub(crate) fn formatted_evidence<'a, const N: usize>(
        &self,
        category: impl Fn([String; N]) -> BWErr,
        details: [FormattedDetail<'_>; N],
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
        self.formatted_in(skeleton, category, details, frames)
    }

    /// The related label is fixed interpreter text. Its small owned record is
    /// measured with the complete prospective diagnostic before detail formatting.
    #[cfg(test)]
    pub(crate) fn formatted_related_detail<'a>(
        &self,
        category: fn(String) -> BWErr,
        message: fmt::Arguments<'_>,
        span: &Span,
        related: (&'static str, &Span),
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Diagnostic {
        self.formatted_related_fields(
            |[detail]| category(detail),
            [FormattedDetail::exact(message)],
            span,
            related,
            frames,
        )
    }

    pub(crate) fn formatted_related_fields<'a, const N: usize>(
        &self,
        category: impl Fn([String; N]) -> BWErr,
        details: [FormattedDetail<'_>; N],
        span: &Span,
        related: (&'static str, &Span),
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Diagnostic {
        let skeleton = Diagnostic::new(category(std::array::from_fn(|_| String::new())))
            .at(span)
            .with_related(related.0, related.1);
        self.formatted_in(skeleton, category, details, frames)
    }

    /// The caller already owns the primary error. Admit a newly constructed cause
    /// with that entire tree before formatting its detail; rejection summarizes
    /// the primary and explicitly counts the omitted cause.
    pub(crate) fn formatted_cause(
        &self,
        primary: Diagnostic,
        category: fn(String) -> BWErr,
        message: fmt::Arguments<'_>,
        span: &Span,
    ) -> DiagnosticResult<Diagnostic> {
        let mut primary = primary.at(span);
        primary.causes.push(skeleton(category, Some(span), false));
        match self.formatted_strings(
            &primary,
            &[FormattedDetail::exact(message)],
            std::iter::empty(),
        ) {
            Ok([detail]) => {
                primary.causes.last_mut().expect("new cause").error = Arc::new(category(detail));
                Ok(primary)
            }
            Err(violation) => Err(primary.rejected(violation)),
        }
    }

    fn formatted_strings<'a, const N: usize>(
        &self,
        skeleton: &Diagnostic,
        messages: &[FormattedDetail<'_>; N],
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Result<[String; N], BWErr> {
        self.check_with_stack(skeleton, frames).and_then(|size| {
            let sizes = self.measure_strings(messages, size.text_bytes)?;
            self.write_strings(messages, sizes)
        })
    }

    fn measure_strings<const N: usize>(
        &self,
        messages: &[FormattedDetail<'_>; N],
        context_bytes: usize,
    ) -> Result<[usize; N], BWErr> {
        let mut remaining = self.text_bytes - context_bytes;
        let mut sizes = [0; N];
        for (message, size) in messages.iter().zip(&mut sizes) {
            let mut count = Counter {
                bytes: 0,
                maximum: remaining,
            };
            count
                .write_fmt(message.full)
                .map_err(|_| text_limit(self.text_bytes))?;
            *size = count.bytes;
            remaining -= count.bytes;
        }
        Ok(sizes)
    }

    fn write_strings<const N: usize>(
        &self,
        messages: &[FormattedDetail<'_>; N],
        sizes: [usize; N],
    ) -> Result<[String; N], BWErr> {
        let mut details = std::array::from_fn(|_| String::new());
        for ((message, size), detail) in messages.iter().zip(sizes).zip(&mut details) {
            let mut output = BoundedText {
                text: String::with_capacity(size),
                maximum: size,
            };
            output
                .write_fmt(message.full)
                .map_err(|_| text_limit(self.text_bytes))?;
            *detail = output.text;
        }
        Ok(details)
    }

    fn formatted_in<'a, const N: usize>(
        &self,
        mut skeleton: Diagnostic,
        category: impl Fn([String; N]) -> BWErr,
        messages: [FormattedDetail<'_>; N],
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Diagnostic {
        let admission = self.formatted_strings(&skeleton, &messages, frames.clone());
        match admission {
            Ok(details) => {
                skeleton.error = Arc::new(category(details));
                skeleton.capture_stack(frames)
            }
            Err(violation) => {
                let mut shortened = 0;
                let details = std::array::from_fn(|index| {
                    let message = &messages[index];
                    let (detail, truncated) =
                        formatted_prefix(message.summary.unwrap_or(message.full));
                    shortened += usize::from(truncated || message.summary.is_some());
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
