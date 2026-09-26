//! Structured source diagnostics. Compatibility APIs can recover the original BWErr.

use std::{error::Error, fmt};

use super::{ast::Span, grammar::BWErr};

pub type DiagnosticResult<T> = Result<T, Diagnostic>;

#[derive(Clone, Debug)]
pub struct CallFrame {
    /// Normalized statement signature, independent of parameter values.
    pub signature: String,
    pub call_site: Span,
    /// None for native statements.
    pub definition_site: Option<Span>,
}

#[derive(Clone, Debug)]
pub struct RelatedLocation {
    pub message: String,
    pub span: Span,
}

#[derive(Debug)]
pub struct Diagnostic {
    pub error: Box<BWErr>,
    pub span: Option<Span>,
    pub label: &'static str,
    /// Entered calls, innermost first. Failed argument binding adds no DSL frame.
    pub call_stack: Vec<CallFrame>,
    pub related: Vec<RelatedLocation>,
    /// Errors being handled when this error occurred, with their original spans/stacks.
    pub causes: Vec<Diagnostic>,
}

impl Diagnostic {
    pub fn new(error: BWErr) -> Self {
        Self {
            error: Box::new(error),
            span: None,
            label: "source",
            call_stack: vec![],
            related: vec![],
            causes: vec![],
        }
    }

    /// Keep the innermost location when an error crosses enclosing syntax nodes.
    pub fn at(mut self, span: &Span) -> Self {
        if self.span.is_none() {
            self.span = Some(span.clone());
        }
        self
    }

    pub(crate) fn at_expression(mut self, span: &Span) -> Self {
        if self.span.is_none() {
            self.span = Some(span.clone());
            self.label = "expression";
        }
        self
    }

    pub(crate) fn capture_stack(mut self, frames: &[CallFrame]) -> Self {
        if self.call_stack.is_empty() {
            self.call_stack.extend(frames.iter().rev().cloned());
        }
        self
    }

    pub(crate) fn while_handling(mut self, original: Diagnostic) -> Self {
        self.causes.push(original);
        self
    }

    pub(crate) fn with_related(mut self, message: &str, span: &Span) -> Self {
        self.related.push(RelatedLocation {
            message: message.into(),
            span: span.clone(),
        });
        self
    }

    /// Discard source/stack information for the original error-category API.
    pub fn into_error(self) -> BWErr {
        *self.error
    }
}

impl From<BWErr> for Diagnostic {
    fn from(error: BWErr) -> Self {
        Self::new(error)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = &self.span {
            write!(formatter, "{}: ", span.location())?;
        }
        write!(formatter, "{}", self.error)?;
        if let Some(span) = &self.span {
            if !span.text().trim().is_empty() {
                write!(formatter, "\n  {}: {}", self.label, span.text().trim())?;
            }
        }
        for location in &self.related {
            write!(
                formatter,
                "\n  {}: {}",
                location.message,
                location.span.location()
            )?;
        }
        for frame in &self.call_stack {
            write!(
                formatter,
                "\n  in `{}` called at {}",
                frame.signature,
                frame.call_site.location()
            )?;
            if let Some(definition) = &frame.definition_site {
                write!(formatter, " (defined at {})", definition.location())?;
            }
        }
        for cause in &self.causes {
            write!(formatter, "\nwhile handling: {cause}")?;
        }
        Ok(())
    }
}

impl Error for Diagnostic {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.causes.first() {
            Some(cause) => Some(cause),
            None => Some(self.error.as_ref()),
        }
    }
}
