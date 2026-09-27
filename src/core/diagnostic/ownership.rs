//! Checked diagnostic ownership and iterative host-tree lifecycle helpers.

use super::{CallFrame, Diagnostic};
use crate::core::{
    ast::{SourceFile, Span},
    grammar::BWErr,
};
use std::{collections::HashSet, sync::Arc};

#[cfg(test)]
mod tests;

pub const MAX_DIAGNOSTIC_DEPTH: usize = 64;

/// Logical limits for a single borrowed diagnostic tree, including retained sources.
#[derive(Clone, Debug)]
pub struct DiagnosticLimits {
    pub diagnostics: usize,
    pub depth: usize,
    pub call_frames: usize,
    pub related_locations: usize,
    pub text_bytes: usize,
    pub source_bytes: usize,
}

impl Default for DiagnosticLimits {
    fn default() -> Self {
        Self {
            diagnostics: 1024,
            depth: 32,
            call_frames: 4096,
            related_locations: 4096,
            text_bytes: 8 * 1024 * 1024,
            source_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticSize {
    pub diagnostics: usize,
    pub depth: usize,
    pub call_frames: usize,
    pub related_locations: usize,
    pub text_bytes: usize,
    pub source_bytes: usize,
}

fn add(
    current: &mut usize,
    amount: usize,
    maximum: usize,
    resource: &'static str,
) -> Result<(), BWErr> {
    *current = current
        .checked_add(amount)
        .filter(|value| *value <= maximum)
        .ok_or(BWErr::ResourceLimit {
            resource,
            limit: maximum as u64,
        })?;
    Ok(())
}

fn error_text(error: &BWErr) -> [&str; 3] {
    match error {
        BWErr::DuplicateStatement {
            signature,
            original,
            duplicate,
        } => [signature, original, duplicate],
        BWErr::DuplicateParameter {
            name,
            original,
            duplicate,
        } => [name, original, duplicate],
        BWErr::CollectionAccessError {
            path,
            segment,
            reason,
        } => [path, segment, reason],
        BWErr::DuplicateNamespace {
            namespace,
            original,
            duplicate,
        } => [namespace, original, duplicate],
        BWErr::ResourceLimit { resource, .. } => [resource, "", ""],
        BWErr::VariableNotDefined(text)
        | BWErr::StatementNotDefined(text)
        | BWErr::ParameterMissingError(text)
        | BWErr::ParsingError(text)
        | BWErr::SignatureError(text)
        | BWErr::ParsingIntegerError(text)
        | BWErr::OperationIncompatibleError(text)
        | BWErr::ControlFlowError(text)
        | BWErr::ArithmeticError(text)
        | BWErr::OutputError(text)
        | BWErr::NativeError(text)
        | BWErr::Cancelled(text)
        | BWErr::Timeout(text)
        | BWErr::AsyncRuntime(text)
        | BWErr::ImportRead(text)
        | BWErr::ImportCycle(text)
        | BWErr::NativePanic(text)
        | BWErr::InputError(text)
        | BWErr::RunConfiguration(text)
        | BWErr::SourceRead(text) => [text, "", ""],
    }
}

struct Measurement<'a> {
    limits: &'a DiagnosticLimits,
    size: DiagnosticSize,
    sources: HashSet<*const SourceFile>,
    owners: Option<&'a mut Vec<Arc<SourceFile>>>,
}

impl Measurement<'_> {
    fn text(&mut self, text: &str) -> Result<(), BWErr> {
        add(
            &mut self.size.text_bytes,
            text.len(),
            self.limits.text_bytes,
            "diagnostic text bytes",
        )
    }

    fn source(&mut self, span: &Span) -> Result<(), BWErr> {
        let source = span.source();
        let pointer = Arc::as_ptr(source);
        if !self.sources.contains(&pointer) {
            add(
                &mut self.size.source_bytes,
                source.name().len(),
                self.limits.source_bytes,
                "diagnostic source bytes",
            )?;
            add(
                &mut self.size.source_bytes,
                source.text().len(),
                self.limits.source_bytes,
                "diagnostic source bytes",
            )?;
            self.sources.insert(pointer);
            if let Some(owners) = &mut self.owners {
                owners.push(Arc::clone(source));
            }
        }
        Ok(())
    }

    fn node(
        &mut self,
        diagnostic: &Diagnostic,
        depth: usize,
        context: Option<(&Span, bool)>,
    ) -> Result<(), BWErr> {
        add(
            &mut self.size.diagnostics,
            1,
            self.limits.diagnostics,
            "diagnostic nodes",
        )?;
        if depth > self.limits.depth {
            return Err(BWErr::ResourceLimit {
                resource: "diagnostic depth",
                limit: self.limits.depth as u64,
            });
        }
        self.size.depth = self.size.depth.max(depth);
        // Reject known width before walking frame/location payloads.
        add(
            &mut self.size.call_frames,
            diagnostic.call_stack.len(),
            self.limits.call_frames,
            "diagnostic call frames",
        )?;
        add(
            &mut self.size.related_locations,
            diagnostic.related.len(),
            self.limits.related_locations,
            "diagnostic related locations",
        )?;
        if diagnostic.causes.len() > self.limits.diagnostics - self.size.diagnostics {
            return Err(BWErr::ResourceLimit {
                resource: "diagnostic nodes",
                limit: self.limits.diagnostics as u64,
            });
        }
        let (span, label) = diagnostic.prospective_location(context);
        self.text(label)?;
        if let Some(source) = diagnostic
            .omissions
            .as_ref()
            .and_then(|omissions| omissions.source.as_ref())
        {
            self.text(&source.file)?;
        }
        for text in error_text(&diagnostic.error) {
            self.text(text)?;
        }
        if let Some(span) = span {
            self.source(span)?;
        }
        for frame in &diagnostic.call_stack {
            self.text(&frame.signature)?;
            self.source(&frame.call_site)?;
            if let Some(span) = &frame.definition_site {
                self.source(span)?;
            }
        }
        for related in &diagnostic.related {
            self.text(&related.message)?;
            self.source(&related.span)?;
        }
        Ok(())
    }
}

impl DiagnosticLimits {
    pub(crate) fn validate(&self) -> Result<(), BWErr> {
        if self.depth > MAX_DIAGNOSTIC_DEPTH {
            return Err(Diagnostic::formatted(
                BWErr::RunConfiguration,
                format_args!("Diagnostic depth cannot exceed {MAX_DIAGNOSTIC_DEPTH}"),
            )
            .into_error());
        }
        Ok(())
    }

    /// Inspect without recursion, formatting strings, scanning source positions, or copying payloads.
    /// Source allocations are deduplicated by identity; other metrics count occurrences.
    pub fn check(&self, diagnostic: &Diagnostic) -> Result<DiagnosticSize, BWErr> {
        self.check_with_stack(diagnostic, std::iter::empty())
    }

    pub(crate) fn check_with_stack<'a>(
        &self,
        diagnostic: &Diagnostic,
        frames: impl ExactSizeIterator<Item = &'a CallFrame>,
    ) -> Result<DiagnosticSize, BWErr> {
        self.inspect(diagnostic, frames, None, None, None, None)
    }

    /// Inspect a copied tree and its new related site before copying either payload.
    pub(crate) fn retained_copy_size(
        &self,
        diagnostic: &Diagnostic,
        related: Option<(&str, &Span)>,
    ) -> Result<(DiagnosticSize, Vec<Arc<SourceFile>>), BWErr> {
        self.retained_mutation_size(diagnostic, related, None)
    }

    /// Inspect a prospective appended site/cause without modifying either owned tree.
    pub(crate) fn retained_mutation_size(
        &self,
        diagnostic: &Diagnostic,
        related: Option<(&str, &Span)>,
        cause: Option<&Diagnostic>,
    ) -> Result<(DiagnosticSize, Vec<Arc<SourceFile>>), BWErr> {
        self.retained_runtime_size(diagnostic, std::iter::empty(), None, related, cause)
    }

    pub(crate) fn retained_runtime_size<'a>(
        &self,
        diagnostic: &Diagnostic,
        frames: impl ExactSizeIterator<Item = &'a CallFrame>,
        context: Option<(&Span, bool)>,
        related: Option<(&str, &Span)>,
        cause: Option<&Diagnostic>,
    ) -> Result<(DiagnosticSize, Vec<Arc<SourceFile>>), BWErr> {
        let mut sources = Vec::new();
        let size = self.inspect(
            diagnostic,
            frames,
            Some(&mut sources),
            related,
            cause,
            context,
        )?;
        Ok((size, sources))
    }

    fn inspect<'a>(
        &self,
        diagnostic: &Diagnostic,
        frames: impl ExactSizeIterator<Item = &'a CallFrame>,
        owners: Option<&mut Vec<Arc<SourceFile>>>,
        related: Option<(&str, &Span)>,
        cause: Option<&Diagnostic>,
        context: Option<(&Span, bool)>,
    ) -> Result<DiagnosticSize, BWErr> {
        self.validate()?;
        let mut measurement = Measurement {
            limits: self,
            size: DiagnosticSize::default(),
            sources: HashSet::new(),
            owners,
        };
        let mut pending = Vec::new();
        if let Some(cause) = cause {
            pending.push((std::slice::from_ref(cause).iter(), 2));
        }
        pending.push((std::slice::from_ref(diagnostic).iter(), 1));
        while let Some((nodes, depth)) = pending.last_mut() {
            if let Some(node) = nodes.next() {
                let depth = *depth;
                measurement.node(node, depth, if depth == 1 { context } else { None })?;
                if !node.causes.is_empty() {
                    pending.push((node.causes.iter(), depth + 1));
                }
            } else {
                pending.pop();
            }
        }
        if diagnostic.call_stack.is_empty() {
            add(
                &mut measurement.size.call_frames,
                frames.len(),
                self.call_frames,
                "diagnostic call frames",
            )?;
            for frame in frames {
                measurement.text(&frame.signature)?;
                measurement.source(&frame.call_site)?;
                if let Some(span) = &frame.definition_site {
                    measurement.source(span)?;
                }
            }
        }
        if let Some((message, span)) = related {
            add(
                &mut measurement.size.related_locations,
                1,
                self.related_locations,
                "diagnostic related locations",
            )?;
            measurement.text(message)?;
            measurement.source(span)?;
        }
        Ok(measurement.size)
    }
}

fn shallow_clone(value: &Diagnostic) -> Diagnostic {
    Diagnostic {
        error: Arc::clone(&value.error),
        span: value.span.clone(),
        label: value.label,
        call_stack: value.call_stack.clone(),
        related: value.related.clone(),
        causes: Vec::with_capacity(value.causes.len()),
        omissions: value.omissions.clone(),
    }
}

/// Infallible host-owned copying has no admission; checked callers preflight first.
impl Clone for Diagnostic {
    fn clone(&self) -> Self {
        let mut pending = vec![(self.causes.iter(), shallow_clone(self))];
        loop {
            let (children, _) = pending.last_mut().expect("diagnostic root");
            if let Some(child) = children.next() {
                pending.push((child.causes.iter(), shallow_clone(child)));
            } else {
                let (_, finished) = pending.pop().expect("completed diagnostic");
                match pending.last_mut() {
                    Some((_, parent)) => parent.causes.push(finished),
                    None => return finished,
                }
            }
        }
    }
}

pub(super) fn discard_causes(causes: Vec<Diagnostic>) {
    if causes.is_empty() {
        return;
    }
    let mut pending = vec![causes.into_iter()];
    while let Some(children) = pending.last_mut() {
        if let Some(mut child) = children.next() {
            let causes = std::mem::take(&mut child.causes);
            // A finished parent needs no pending slot while descending its last child.
            if children.len() == 0 {
                pending.pop();
            }
            if !causes.is_empty() {
                pending.push(causes.into_iter());
            }
        } else {
            pending.pop();
        }
    }
}

/// Results abandoned inside runtime-owned worker handles must not recursively drop host trees.
pub(crate) struct OwnedDiagnostic(Option<Diagnostic>);
impl OwnedDiagnostic {
    pub(crate) fn as_ref(&self) -> &Diagnostic {
        self.0.as_ref().expect("owned diagnostic")
    }
    pub(crate) fn new(value: Diagnostic) -> Self {
        Self(Some(value))
    }
    pub(crate) fn into_inner(mut self) -> Diagnostic {
        self.0.take().expect("owned diagnostic")
    }
    pub(crate) fn map(&mut self, transform: impl FnOnce(Diagnostic) -> Diagnostic) {
        let value = self.0.take().expect("owned diagnostic");
        self.0 = Some(transform(value));
    }
}
impl Drop for OwnedDiagnostic {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            value.discard();
        }
    }
}
