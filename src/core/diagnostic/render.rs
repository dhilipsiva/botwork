//! Bounded, nonrecursive rendering of borrowed diagnostics.
use super::{construction::formatted_prefix, Diagnostic, Help};
use crate::core::ast::Span;
use std::fmt::{self, Write};

#[cfg(test)]
mod tests;

/// Fixed emergency output allowance when ordinary rendering cannot fit.
pub const RENDER_SUMMARY_BYTES: usize = 16 * 1024;
const SUMMARY_RECORDS: usize = 8;

/// Limits for one complete rendered diagnostic, including all handled causes.
#[derive(Clone, Debug)]
pub struct DiagnosticRenderLimits {
    pub output_bytes: usize,
    pub diagnostics: usize,
    pub depth: usize,
    pub call_frames: usize,
    pub related_locations: usize,
    /// Conservative source scanning across both measurement and output passes.
    pub source_scan_bytes: usize,
}

impl Default for DiagnosticRenderLimits {
    fn default() -> Self {
        Self {
            output_bytes: 64 * 1024,
            diagnostics: 128,
            depth: 64,
            call_frames: 256,
            related_locations: 256,
            source_scan_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiagnosticRenderTruncation {
    pub resource: &'static str,
    pub limit: usize,
}

/// Rendering never changes the borrowed diagnostic or its original error category.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RenderedDiagnostic {
    pub text: String,
    pub truncation: Option<DiagnosticRenderTruncation>,
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

enum Failure {
    Limit(DiagnosticRenderTruncation),
    Write(fmt::Error),
}

impl From<fmt::Error> for Failure {
    fn from(error: fmt::Error) -> Self {
        Self::Write(error)
    }
}

fn exceeded(resource: &'static str, limit: usize) -> Failure {
    Failure::Limit(DiagnosticRenderTruncation { resource, limit })
}

fn charge(
    used: &mut usize,
    amount: usize,
    maximum: usize,
    resource: &'static str,
) -> Result<(), Failure> {
    *used = used
        .checked_add(amount)
        .filter(|next| *next <= maximum)
        .ok_or_else(|| exceeded(resource, maximum))?;
    Ok(())
}

#[derive(Default)]
struct Work {
    diagnostics: usize,
    call_frames: usize,
    related: usize,
    source: usize,
}

impl Work {
    fn source(
        &mut self,
        span: &Span,
        primary: bool,
        limits: &DiagnosticRenderLimits,
    ) -> Result<(), Failure> {
        // A coordinate scans its prefix at most three times per pass. Trimming
        // an excerpt scans at most twice its length per pass. Count both passes.
        let end = if primary && span.start() != span.end() {
            span.end()
        } else {
            0
        };
        let excerpt = if primary {
            span.end().checked_sub(span.start())
        } else {
            Some(0)
        };
        let amount = span
            .start()
            .checked_add(end)
            .and_then(|sum| sum.checked_mul(6))
            .and_then(|positions| {
                excerpt
                    .and_then(|bytes| bytes.checked_mul(4))
                    .and_then(|bytes| positions.checked_add(bytes))
            })
            .ok_or_else(|| {
                exceeded(
                    "diagnostic render source scan bytes",
                    limits.source_scan_bytes,
                )
            })?;
        charge(
            &mut self.source,
            amount,
            limits.source_scan_bytes,
            "diagnostic render source scan bytes",
        )
    }
}

fn location(output: &mut impl Write, span: &Span) -> fmt::Result {
    let (line, column) = span.line_column();
    write!(
        output,
        "{}:{line}:{column}",
        super::shown_path(span.source().name())
    )
}

fn full(
    diagnostic: &Diagnostic,
    limits: &DiagnosticRenderLimits,
    output: &mut impl Write,
) -> Result<(), Failure> {
    let mut work = Work::default();
    // Slice cursors keep storage proportional to entered depth, never fanout.
    let mut pending = vec![(std::slice::from_ref(diagnostic), 1usize)];
    while let Some((records, depth)) = pending.pop() {
        let Some((diagnostic, rest)) = records.split_first() else {
            continue;
        };
        if depth > limits.depth {
            return Err(exceeded("diagnostic render depth", limits.depth));
        }
        charge(
            &mut work.diagnostics,
            1,
            limits.diagnostics,
            "diagnostic render records",
        )?;
        if work.diagnostics > 1 {
            output.write_str("\nwhile handling: ")?;
        }
        if let Some(span) = &diagnostic.span {
            work.source(span, true, limits)?;
            location(output, span)?;
            if span.start() != span.end() {
                let (line, column) = span.end_line_column();
                write!(output, "-{line}:{column}")?;
            }
            output.write_str(": ")?;
        }
        write!(output, "[{}] {}", diagnostic.code(), diagnostic.error)?;
        if let Some(span) = &diagnostic.span {
            let text = span.text().trim();
            if !text.is_empty() {
                write!(output, "\n  {}: {text}", diagnostic.label)?;
            }
        }
        for related in &diagnostic.related {
            charge(
                &mut work.related,
                1,
                limits.related_locations,
                "diagnostic render related locations",
            )?;
            work.source(&related.span, false, limits)?;
            write!(output, "\n  {}: ", related.message)?;
            location(output, &related.span)?;
        }
        for frame in &diagnostic.call_stack {
            charge(
                &mut work.call_frames,
                1,
                limits.call_frames,
                "diagnostic render call frames",
            )?;
            work.source(&frame.call_site, false, limits)?;
            if let Some(header) = &frame.statement {
                // Showing the header as written scans it once.
                charge(
                    &mut work.source,
                    header.end() - header.start(),
                    limits.source_scan_bytes,
                    "diagnostic render source scan bytes",
                )?;
            }
            write!(output, "\n  in `{}` called at ", frame.shown())?;
            location(output, &frame.call_site)?;
            if let Some(definition) = &frame.definition_site {
                work.source(definition, false, limits)?;
                output.write_str(" (defined at ")?;
                location(output, definition)?;
                output.write_char(')')?;
            }
        }
        if let Some(omissions) = &diagnostic.omissions {
            write!(output, "\n  diagnostic metadata omitted: {} shortened detail fields, {} call frames, {} related locations, {} direct causes; label omitted: {}; prior summary omitted: {}", omissions.detail_fields, omissions.call_frames, omissions.related_locations, omissions.direct_causes, omissions.label, omissions.prior_summary)?;
            if let Some(source) = &omissions.source {
                write!(
                    output,
                    "; source {} bytes {}..{} (filename shortened: {})",
                    source.file, source.start_byte, source.end_byte, source.file_truncated
                )?;
            }
        }
        write!(output, "\n  help: {}", Help(&diagnostic.error))?;
        if !rest.is_empty() {
            pending.push((rest, depth));
        }
        if !diagnostic.causes.is_empty() {
            let next = depth
                .checked_add(1)
                .ok_or_else(|| exceeded("diagnostic render depth", limits.depth))?;
            pending.push((&diagnostic.causes, next));
        }
    }
    Ok(())
}

fn measure(
    diagnostic: &Diagnostic,
    limits: &DiagnosticRenderLimits,
) -> Result<usize, DiagnosticRenderTruncation> {
    let mut counter = Counter {
        bytes: 0,
        maximum: limits.output_bytes,
    };
    match full(diagnostic, limits, &mut counter) {
        Ok(()) => Ok(counter.bytes),
        Err(Failure::Limit(limit)) => Err(limit),
        Err(Failure::Write(_)) => Err(DiagnosticRenderTruncation {
            resource: "diagnostic render output bytes",
            limit: limits.output_bytes,
        }),
    }
}

fn summary(
    diagnostic: &Diagnostic,
    limit: &DiagnosticRenderTruncation,
    output: &mut impl Write,
) -> fmt::Result {
    // Eight records, two 256-byte previews and only bounded scalar/static fields
    // per record fit within RENDER_SUMMARY_BYTES, independently of host tree size.
    let mut current = Some(diagnostic);
    for index in 0..SUMMARY_RECORDS {
        let Some(diagnostic) = current else { break };
        if index > 0 {
            output.write_str("\nwhile handling: ")?;
        }
        let (detail, shortened) = formatted_prefix(format_args!("{}", diagnostic.error));
        write!(output, "[{}] {detail}", diagnostic.code())?;
        if index == 0 {
            write!(
                output,
                "\n  diagnostic rendering truncated: {} (limit {})",
                limit.resource, limit.limit
            )?;
        }
        let source = diagnostic
            .span
            .as_ref()
            .map(|span| {
                (
                    super::shown_path(span.source().name()),
                    span.start(),
                    span.end(),
                    false,
                )
            })
            .or_else(|| {
                diagnostic
                    .omissions
                    .as_ref()?
                    .source
                    .as_ref()
                    .map(|source| {
                        (
                            source.file.as_str(),
                            source.start_byte,
                            source.end_byte,
                            source.file_truncated,
                        )
                    })
            });
        if let Some((file, start, end, prior_shortened)) = source {
            let (file, shortened) = formatted_prefix(format_args!("{file}"));
            let shortened = shortened || prior_shortened;
            write!(output, "\n  source: {file} bytes {start}..{end} (coordinates/excerpt omitted; filename shortened: {shortened})")?;
        }
        current = if index + 1 < SUMMARY_RECORDS {
            diagnostic.causes.first()
        } else {
            None
        };
        let omitted = diagnostic.causes.len() - usize::from(current.is_some());
        write!(output, "\n  rendering omitted: help, source excerpts, {} call frames, {} related locations, {omitted} additional direct causes; detail shortened: {shortened}; prior diagnostic omissions: {}", diagnostic.call_stack.len(), diagnostic.related.len(), diagnostic.omissions.is_some())?;
        if let Some(prior) = &diagnostic.omissions {
            write!(output, "\n  prior omissions: {} detail fields, {} call frames, {} related locations, {} direct causes; label: {}; prior summary: {}", prior.detail_fields, prior.call_frames, prior.related_locations, prior.direct_causes, prior.label, prior.prior_summary)?;
        }
    }
    Ok(())
}

pub(super) fn display(diagnostic: &Diagnostic, output: &mut impl Write) -> fmt::Result {
    let limits = DiagnosticRenderLimits::default();
    match measure(diagnostic, &limits) {
        Ok(_) => full(diagnostic, &limits, output).map_err(|_| fmt::Error),
        Err(limit) => summary(diagnostic, &limit, output),
    }
}

pub(super) fn render(
    diagnostic: &Diagnostic,
    limits: &DiagnosticRenderLimits,
) -> RenderedDiagnostic {
    match measure(diagnostic, limits) {
        Ok(bytes) => {
            let mut text = String::with_capacity(bytes);
            assert!(
                full(diagnostic, limits, &mut text).is_ok(),
                "deterministic admitted rendering"
            );
            RenderedDiagnostic {
                text,
                truncation: None,
            }
        }
        Err(limit) => {
            let mut text = String::new();
            summary(diagnostic, &limit, &mut text).expect("String writer");
            debug_assert!(text.len() <= RENDER_SUMMARY_BYTES);
            RenderedDiagnostic {
                text,
                truncation: Some(limit),
            }
        }
    }
}

pub(super) fn help(error: &crate::core::grammar::BWErr, maximum: usize) -> RenderedDiagnostic {
    let mut counter = Counter { bytes: 0, maximum };
    if write!(&mut counter, "{}", Help(error)).is_ok() {
        let mut text = String::with_capacity(counter.bytes);
        write!(&mut text, "{}", Help(error)).expect("String writer");
        RenderedDiagnostic {
            text,
            truncation: None,
        }
    } else {
        let limit = DiagnosticRenderTruncation {
            resource: "diagnostic render output bytes",
            limit: maximum,
        };
        let (preview, _) = formatted_prefix(format_args!("{}", Help(error)));
        let text = format!(
            "[{}] repair guidance truncated: {} (limit {})\n  {preview}",
            error.code(),
            limit.resource,
            limit.limit
        );
        RenderedDiagnostic {
            text,
            truncation: Some(limit),
        }
    }
}
