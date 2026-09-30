use super::*;
use crate::core::{
    ast::{SourceFile, Span},
    diagnostic::{CallFrame, DiagnosticOmissions, OmittedSource, RelatedLocation},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

mod errors;
use errors::WireError;

struct Tables<'a> {
    sources: Vec<Arc<SourceFile>>,
    source_ids: HashMap<*const SourceFile, usize>,
    errors: Vec<&'a Arc<BWErr>>,
    error_ids: HashMap<*const BWErr, usize>,
}

impl<'a> Tables<'a> {
    fn collect_errors(&mut self, diagnostic: &'a Diagnostic) {
        let pointer = Arc::as_ptr(&diagnostic.error);
        if !self.error_ids.contains_key(&pointer) {
            self.error_ids.insert(pointer, self.errors.len());
            self.errors.push(&diagnostic.error);
        }
        for cause in &diagnostic.causes {
            self.collect_errors(cause);
        }
    }
    fn span(&self, output: &mut Encoder<'_>, span: &Span) -> DiagnosticResult<()> {
        output.u64(self.source_ids[&Arc::as_ptr(span.source())] as u64)?;
        output.u64(span.start() as u64)?;
        output.u64(span.end() as u64)
    }
    fn optional_span(&self, output: &mut Encoder<'_>, span: Option<&Span>) -> DiagnosticResult<()> {
        output.byte(u8::from(span.is_some()))?;
        if let Some(span) = span {
            self.span(output, span)?;
        }
        Ok(())
    }
    fn node(
        &self,
        protocol: &WorkerProtocol,
        output: &mut Encoder<'_>,
        diagnostic: &Diagnostic,
    ) -> DiagnosticResult<()> {
        output.u64(self.error_ids[&Arc::as_ptr(&diagnostic.error)] as u64)?;
        protocol.label(diagnostic.label)?;
        output.string(diagnostic.label)?;
        self.optional_span(output, diagnostic.span.as_ref())?;
        output.u64(diagnostic.call_stack.len() as u64)?;
        for call in &diagnostic.call_stack {
            output.string(&call.signature)?;
            self.span(output, &call.call_site)?;
            self.optional_span(output, call.definition_site.as_ref())?;
        }
        output.u64(diagnostic.related.len() as u64)?;
        for related in &diagnostic.related {
            output.string(&related.message)?;
            self.span(output, &related.span)?;
        }
        output.byte(u8::from(diagnostic.omissions.is_some()))?;
        if let Some(omissions) = &diagnostic.omissions {
            for count in [
                omissions.detail_fields,
                omissions.call_frames,
                omissions.related_locations,
                omissions.direct_causes,
            ] {
                output.u64(count as u64)?;
            }
            output.byte(u8::from(omissions.label))?;
            output.byte(u8::from(omissions.prior_summary))?;
            output.byte(u8::from(omissions.source.is_some()))?;
            if let Some(source) = &omissions.source {
                output.string(&source.file)?;
                output.byte(u8::from(source.file_truncated))?;
                output.u64(source.start_byte as u64)?;
                output.u64(source.end_byte as u64)?;
            }
        }
        output.u64(diagnostic.causes.len() as u64)?;
        for cause in &diagnostic.causes {
            self.node(protocol, output, cause)?;
        }
        Ok(())
    }
    fn encode(
        &self,
        protocol: &WorkerProtocol,
        output: &mut Encoder<'_>,
        diagnostic: &Diagnostic,
    ) -> DiagnosticResult<()> {
        output.u64(self.sources.len() as u64)?;
        for source in &self.sources {
            output.string(source.name())?;
            output.string(source.text())?;
        }
        output.u64(self.errors.len() as u64)?;
        for error in &self.errors {
            errors::encode(protocol, output, error)?;
        }
        self.node(protocol, output, diagnostic)
    }
}

/// The sources of the spans the wire carries, as `Tables::node` writes them.
/// Call-frame headers stay behind, so sources only they use are not sent.
fn wire_sources(diagnostic: &Diagnostic, used: &mut HashSet<*const SourceFile>) {
    let spans =
        diagnostic
            .span
            .iter()
            .chain(diagnostic.call_stack.iter().flat_map(|call| {
                std::iter::once(&call.call_site).chain(call.definition_site.as_ref())
            }))
            .chain(diagnostic.related.iter().map(|related| &related.span));
    used.extend(spans.map(|span| Arc::as_ptr(span.source())));
    for cause in &diagnostic.causes {
        wire_sources(cause, used);
    }
}

pub(super) fn encode(
    protocol: &WorkerProtocol,
    diagnostic: &Diagnostic,
    control: &OperationControl,
) -> DiagnosticResult<Vec<u8>> {
    let (_, mut sources) = protocol.limits.diagnostics.retained_runtime_size(
        diagnostic,
        std::iter::empty(),
        None,
        None,
        None,
    )?;
    let mut used = HashSet::new();
    wire_sources(diagnostic, &mut used);
    sources.retain(|source| used.contains(&Arc::as_ptr(source)));
    if sources.len() > protocol.limits.sources {
        return Err(limit("worker protocol sources", protocol.limits.sources));
    }
    let source_ids = sources
        .iter()
        .enumerate()
        .map(|(index, source)| (Arc::as_ptr(source), index))
        .collect();
    let mut tables = Tables {
        sources,
        source_ids,
        errors: Vec::new(),
        error_ids: HashMap::new(),
    };
    tables.collect_errors(diagnostic);
    let mut output = Encoder::counter(protocol.limits.frame_bytes, control)?;
    tables.encode(protocol, &mut output, diagnostic)?;
    let mut output = output.buffer(2);
    tables.encode(protocol, &mut output, diagnostic)?;
    output.finish()
}

#[derive(Clone, Copy)]
struct WireSpan {
    source: usize,
    start: usize,
    end: usize,
}

struct WireSource<'a> {
    name: &'a str,
    text: &'a str,
}
#[derive(Clone, Copy)]
struct Shape {
    error: usize,
    code: u16,
    label: &'static str,
    span: Option<WireSpan>,
    frames: usize,
    related: usize,
    causes: usize,
    omissions: bool,
}

pub(crate) struct DiagnosticPlan<'a> {
    protocol: &'a WorkerProtocol,
    bytes: &'a [u8],
    node_offset: usize,
    sources: Vec<WireSource<'a>>,
    errors: Vec<WireError<'a>>,
    pub(crate) size: DiagnosticSize,
    root: Shape,
    first_cause: Option<Shape>,
}

impl DiagnosticPlan<'_> {
    fn emergency(&self) -> bool {
        let root = self.root;
        if root.span.is_some() || root.frames != 0 || root.related != 0 || root.causes != 1 {
            return false;
        }
        let Some(cause) = self.first_cause else {
            return false;
        };
        if cause.span.is_some() || cause.frames != 0 || cause.related != 0 || cause.causes != 0 {
            return false;
        }
        (matches!(root.code, 8001 | 7002 | 5001 | 5002) && !root.omissions && cause.omissions)
            || (matches!(root.code, 5001 | 5002)
                && root.omissions
                && cause.code == 8001
                && !cause.omissions)
    }
    pub(crate) fn needs_header(&self) -> bool {
        self.root.span.is_none() && !self.emergency()
    }
    pub(crate) fn size_with_header(
        &self,
        header: &Span,
        limits: &DiagnosticLimits,
    ) -> DiagnosticResult<DiagnosticSize> {
        let mut size = self.size;
        if self.needs_header() {
            size.text_bytes -= self.root.label.len();
            add(
                &mut size.text_bytes,
                "source".len(),
                limits.text_bytes,
                "diagnostic text bytes",
            )?;
            add(
                &mut size.source_bytes,
                header.source().name().len(),
                limits.source_bytes,
                "diagnostic source bytes",
            )?;
            add(
                &mut size.source_bytes,
                header.source().text().len(),
                limits.source_bytes,
                "diagnostic source bytes",
            )?;
        }
        for (actual, maximum, resource) in [
            (size.diagnostics, limits.diagnostics, "diagnostic nodes"),
            (size.depth, limits.depth, "diagnostic depth"),
            (
                size.call_frames,
                limits.call_frames,
                "diagnostic call frames",
            ),
            (
                size.related_locations,
                limits.related_locations,
                "diagnostic related locations",
            ),
            (size.text_bytes, limits.text_bytes, "diagnostic text bytes"),
            (
                size.source_bytes,
                limits.source_bytes,
                "diagnostic source bytes",
            ),
        ] {
            if actual > maximum {
                return Err(limit(resource, maximum));
            }
        }
        Ok(size)
    }
    /// Fixed-size evidence, without copying rejected source or detail payloads.
    pub(crate) fn rejected(&self, violation: Diagnostic, header: Option<&Span>) -> Diagnostic {
        let (error, detail_fields) = self.errors[self.root.error].summary();
        let mut summary = Diagnostic::new(error);
        let header = header.filter(|_| self.needs_header());
        let effective_label = if header.is_some() {
            "source"
        } else {
            self.root.label
        };
        let label = !matches!(effective_label, "source" | "expression");
        if !label {
            summary.label = effective_label;
        }
        summary.omissions = Some(Box::new(DiagnosticOmissions {
            detail_fields,
            label,
            call_frames: self.root.frames,
            related_locations: self.root.related,
            direct_causes: self.root.causes,
            prior_summary: self.root.omissions,
            source: self
                .root
                .span
                .map(|span| {
                    let file = self.sources[span.source].name;
                    OmittedSource {
                        file: prefix(file).into(),
                        file_truncated: file.len() > 256,
                        start_byte: span.start,
                        end_byte: span.end,
                    }
                })
                .or_else(|| {
                    header.map(|span| OmittedSource {
                        file: prefix(span.source().name()).into(),
                        file_truncated: span.source().name().len() > 256,
                        start_byte: span.start(),
                        end_byte: span.end(),
                    })
                }),
        }));
        violation.while_handling(summary)
    }
    pub(crate) fn build(self) -> Diagnostic {
        self.build_with_header(None)
    }
    pub(crate) fn build_with_header(self, header: Option<&Span>) -> Diagnostic {
        let attach = header.filter(|_| self.needs_header());
        let sources: Vec<_> = self
            .sources
            .iter()
            .map(|source| {
                Arc::new(SourceFile::from_owned_parts(
                    source.name.into(),
                    source.text.into(),
                ))
            })
            .collect();
        let errors: Vec<_> = self
            .errors
            .iter()
            .map(|error| Arc::new(error.build()))
            .collect();
        let control = OperationControl::default();
        let mut input = Decoder {
            bytes: self.bytes,
            offset: self.node_offset,
            control: &control,
        };
        let mut diagnostic = build_node(self.protocol, &mut input, &sources, &errors);
        if let Some(header) = attach {
            diagnostic.span = Some(header.clone());
            diagnostic.label = "source";
        }
        diagnostic
    }
}

pub(super) fn plan<'a>(
    protocol: &'a WorkerProtocol,
    mut input: Decoder<'a>,
) -> DiagnosticResult<DiagnosticPlan<'a>> {
    let mut size = DiagnosticSize::default();
    let count = input.count(protocol.limits.sources, "worker protocol sources")?;
    let mut sources = Vec::with_capacity(count);
    for _ in 0..count {
        let name = input.string()?;
        let text = input.string()?;
        add(
            &mut size.source_bytes,
            name.len(),
            protocol.limits.diagnostics.source_bytes,
            "diagnostic source bytes",
        )?;
        add(
            &mut size.source_bytes,
            text.len(),
            protocol.limits.diagnostics.source_bytes,
            "diagnostic source bytes",
        )?;
        sources.push(WireSource { name, text });
    }
    let count = input.count(protocol.limits.diagnostics.diagnostics, "diagnostic nodes")?;
    let mut errors = Vec::with_capacity(count);
    for _ in 0..count {
        errors.push(WireError::read(protocol, &mut input)?);
    }
    let node_offset = input.offset;
    let mut scanner = Scanner {
        protocol,
        sources: &sources,
        errors: &errors,
        source_used: vec![false; sources.len()],
        error_used: vec![false; errors.len()],
        size,
        first_cause: None,
    };
    let root = scanner.node(&mut input, 1)?;
    input.end()?;
    if scanner.source_used.contains(&false) || scanner.error_used.contains(&false) {
        return Err(invalid("Unused worker diagnostic table entry"));
    }
    let size = scanner.size;
    let first_cause = scanner.first_cause;
    Ok(DiagnosticPlan {
        protocol,
        bytes: input.bytes,
        node_offset,
        sources,
        errors,
        size,
        root,
        first_cause,
    })
}

struct Scanner<'a, 'b> {
    protocol: &'a WorkerProtocol,
    sources: &'b [WireSource<'a>],
    errors: &'b [WireError<'a>],
    source_used: Vec<bool>,
    error_used: Vec<bool>,
    size: DiagnosticSize,
    first_cause: Option<Shape>,
}

impl Scanner<'_, '_> {
    fn text(&mut self, text: &str) -> DiagnosticResult<()> {
        add(
            &mut self.size.text_bytes,
            text.len(),
            self.protocol.limits.diagnostics.text_bytes,
            "diagnostic text bytes",
        )
    }
    fn span(&mut self, input: &mut Decoder<'_>) -> DiagnosticResult<WireSpan> {
        let index = input.usize()?;
        let source = self
            .sources
            .get(index)
            .ok_or_else(|| invalid("Invalid worker source reference"))?;
        let start = input.usize()?;
        let end = input.usize()?;
        if start > end
            || end > source.text.len()
            || !source.text.is_char_boundary(start)
            || !source.text.is_char_boundary(end)
        {
            return Err(invalid("Invalid worker source byte range"));
        }
        self.source_used[index] = true;
        Ok(WireSpan {
            source: index,
            start,
            end,
        })
    }
    fn optional_span(&mut self, input: &mut Decoder<'_>) -> DiagnosticResult<Option<WireSpan>> {
        if input.boolean()? {
            Ok(Some(self.span(input)?))
        } else {
            Ok(None)
        }
    }
    fn node(&mut self, input: &mut Decoder<'_>, depth: usize) -> DiagnosticResult<Shape> {
        let limits = &self.protocol.limits.diagnostics;
        add(
            &mut self.size.diagnostics,
            1,
            limits.diagnostics,
            "diagnostic nodes",
        )?;
        if depth > limits.depth {
            return Err(limit("diagnostic depth", limits.depth));
        }
        self.size.depth = self.size.depth.max(depth);
        let index = input.usize()?;
        let error = self
            .errors
            .get(index)
            .ok_or_else(|| invalid("Invalid worker error reference"))?;
        let code = error.code;
        for text in error.fields {
            self.text(text)?;
        }
        self.error_used[index] = true;
        let label = self.protocol.label(input.string()?)?;
        self.text(label)?;
        let span = self.optional_span(input)?;
        let frames = input.count(
            limits.call_frames.saturating_sub(self.size.call_frames),
            "diagnostic call frames",
        )?;
        self.size.call_frames += frames;
        for _ in 0..frames {
            self.text(input.string()?)?;
            self.span(input)?;
            self.optional_span(input)?;
        }
        let related = input.count(
            limits
                .related_locations
                .saturating_sub(self.size.related_locations),
            "diagnostic related locations",
        )?;
        self.size.related_locations += related;
        for _ in 0..related {
            self.text(input.string()?)?;
            self.span(input)?;
        }
        let omissions = input.boolean()?;
        if omissions {
            for _ in 0..4 {
                input.usize()?;
            }
            input.boolean()?;
            input.boolean()?;
            if input.boolean()? {
                self.text(input.string()?)?;
                input.boolean()?;
                input.usize()?;
                input.usize()?;
            }
        }
        let causes = input.count(
            limits.diagnostics.saturating_sub(self.size.diagnostics),
            "diagnostic nodes",
        )?;
        let shape = Shape {
            error: index,
            code,
            label,
            span,
            frames,
            related,
            causes,
            omissions,
        };
        // Capture the root's first child before visiting its descendants.
        if depth == 2 && self.first_cause.is_none() {
            self.first_cause = Some(shape);
        }
        for _ in 0..causes {
            self.node(input, depth + 1)?;
        }
        Ok(shape)
    }
}

fn build_span(input: &mut Decoder<'_>, sources: &[Arc<SourceFile>]) -> Span {
    let index = input.usize().unwrap();
    let start = input.usize().unwrap();
    let end = input.usize().unwrap();
    Span::from_source_range(Arc::clone(&sources[index]), start, end).expect("admitted span")
}
fn build_optional_span(input: &mut Decoder<'_>, sources: &[Arc<SourceFile>]) -> Option<Span> {
    input.boolean().unwrap().then(|| build_span(input, sources))
}
fn build_node(
    protocol: &WorkerProtocol,
    input: &mut Decoder<'_>,
    sources: &[Arc<SourceFile>],
    errors: &[Arc<BWErr>],
) -> Diagnostic {
    let error = Arc::clone(&errors[input.usize().unwrap()]);
    let label = protocol.label(input.string().unwrap()).unwrap();
    let span = build_optional_span(input, sources);
    let count = input.usize().unwrap();
    let call_stack = (0..count)
        .map(|_| CallFrame {
            signature: input.string().unwrap().into(),
            // The wire carries the signature alone; `shown` falls back to it.
            statement: None,
            call_site: build_span(input, sources),
            definition_site: build_optional_span(input, sources),
        })
        .collect();
    let count = input.usize().unwrap();
    let related = (0..count)
        .map(|_| RelatedLocation {
            message: input.string().unwrap().into(),
            span: build_span(input, sources),
        })
        .collect();
    let omissions = input.boolean().unwrap().then(|| {
        let detail_fields = input.usize().unwrap();
        let call_frames = input.usize().unwrap();
        let related_locations = input.usize().unwrap();
        let direct_causes = input.usize().unwrap();
        let label = input.boolean().unwrap();
        let prior_summary = input.boolean().unwrap();
        let source = input.boolean().unwrap().then(|| OmittedSource {
            file: input.string().unwrap().into(),
            file_truncated: input.boolean().unwrap(),
            start_byte: input.usize().unwrap(),
            end_byte: input.usize().unwrap(),
        });
        Box::new(DiagnosticOmissions {
            detail_fields,
            call_frames,
            related_locations,
            direct_causes,
            label,
            prior_summary,
            source,
        })
    });
    let count = input.usize().unwrap();
    let causes = (0..count)
        .map(|_| build_node(protocol, input, sources, errors))
        .collect();
    Diagnostic {
        error,
        label,
        span,
        call_stack,
        related,
        causes,
        omissions,
    }
}

fn prefix(text: &str) -> &str {
    let mut end = text.len().min(256);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
