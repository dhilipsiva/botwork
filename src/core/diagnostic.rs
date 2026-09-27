//! Structured source diagnostics. Compatibility APIs can recover the original BWErr.

use std::{error::Error, fmt, sync::Arc};

use super::{
    ast::Span,
    grammar::{BWErr, Literal},
};

mod construction;
pub(crate) use construction::FormattedDetail;
mod rejection;
pub use rejection::{
    DiagnosticOmissions, OmittedSource, SUMMARY_DETAIL_BYTES, SUMMARY_SOURCE_NAME_BYTES,
};
mod ownership;
pub(crate) use ownership::OwnedDiagnostic;
pub use ownership::{DiagnosticLimits, DiagnosticSize, MAX_DIAGNOSTIC_DEPTH};
mod value;
pub use value::DiagnosticValueLimits;
mod render;
pub use render::{
    DiagnosticRenderLimits, DiagnosticRenderTruncation, RenderedDiagnostic, RENDER_SUMMARY_BYTES,
};

pub type DiagnosticResult<T> = Result<T, Diagnostic>;

/// Stable machine-readable categories. Existing string identifiers are never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticCode {
    Syntax,
    InvalidControl,
    DuplicateParameter,
    Signature,
    UndefinedVariable,
    UndefinedStatement,
    DuplicateStatement,
    ParameterCount,
    InvalidNumber,
    Arithmetic,
    IncompatibleType,
    CollectionAccess,
    Output,
    Native,
    NativePanic,
    Cancelled,
    Timeout,
    AsyncRuntime,
    ImportRead,
    ImportCycle,
    DuplicateNamespace,
    Input,
    RunConfiguration,
    SourceRead,
    ResourceLimit,
}

impl DiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Syntax => "BW1001",
            Self::InvalidControl => "BW1002",
            Self::DuplicateParameter => "BW1003",
            Self::Signature => "BW1004",
            Self::UndefinedVariable => "BW2001",
            Self::UndefinedStatement => "BW2002",
            Self::DuplicateStatement => "BW2003",
            Self::ParameterCount => "BW2004",
            Self::InvalidNumber => "BW3001",
            Self::Arithmetic => "BW3002",
            Self::IncompatibleType => "BW3003",
            Self::CollectionAccess => "BW3004",
            Self::Output => "BW4001",
            Self::Native => "BW4002",
            Self::NativePanic => "BW4003",
            Self::Cancelled => "BW5001",
            Self::Timeout => "BW5002",
            Self::AsyncRuntime => "BW5003",
            Self::ImportRead => "BW6001",
            Self::ImportCycle => "BW6002",
            Self::DuplicateNamespace => "BW6003",
            Self::Input => "BW7001",
            Self::RunConfiguration => "BW7002",
            Self::SourceRead => "BW7003",
            Self::ResourceLimit => "BW8001",
        }
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl BWErr {
    pub fn code(&self) -> DiagnosticCode {
        match self {
            Self::ParsingError(_) => DiagnosticCode::Syntax,
            Self::ControlFlowError(_) => DiagnosticCode::InvalidControl,
            Self::DuplicateParameter { .. } => DiagnosticCode::DuplicateParameter,
            Self::SignatureError(_) => DiagnosticCode::Signature,
            Self::VariableNotDefined(_) => DiagnosticCode::UndefinedVariable,
            Self::StatementNotDefined(_) => DiagnosticCode::UndefinedStatement,
            Self::DuplicateStatement { .. } => DiagnosticCode::DuplicateStatement,
            Self::ParameterMissingError(_) => DiagnosticCode::ParameterCount,
            Self::ParsingIntegerError(_) => DiagnosticCode::InvalidNumber,
            Self::ArithmeticError(_) => DiagnosticCode::Arithmetic,
            Self::OperationIncompatibleError(_) => DiagnosticCode::IncompatibleType,
            Self::CollectionAccessError { .. } => DiagnosticCode::CollectionAccess,
            Self::OutputError(_) => DiagnosticCode::Output,
            Self::NativeError(_) => DiagnosticCode::Native,
            Self::NativePanic(_) => DiagnosticCode::NativePanic,
            Self::Cancelled(_) => DiagnosticCode::Cancelled,
            Self::Timeout(_) => DiagnosticCode::Timeout,
            Self::AsyncRuntime(_) => DiagnosticCode::AsyncRuntime,
            Self::ImportRead(_) => DiagnosticCode::ImportRead,
            Self::ImportCycle(_) => DiagnosticCode::ImportCycle,
            Self::DuplicateNamespace { .. } => DiagnosticCode::DuplicateNamespace,
            Self::InputError(_) => DiagnosticCode::Input,
            Self::RunConfiguration(_) => DiagnosticCode::RunConfiguration,
            Self::SourceRead(_) => DiagnosticCode::SourceRead,
            Self::ResourceLimit { .. } => DiagnosticCode::ResourceLimit,
        }
    }

    pub fn help(&self) -> String {
        self.help_with_limit(DiagnosticRenderLimits::default().output_bytes)
            .text
    }

    /// Format repair guidance with a byte limit and explicit bounded truncation evidence.
    pub fn help_with_limit(&self, output_bytes: usize) -> RenderedDiagnostic {
        render::help(self, output_bytes)
    }
}

struct Help<'a>(&'a BWErr);
impl fmt::Display for Help<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guidance = match self.0 {
            BWErr::VariableNotDefined(name) => return write!(formatter, "Define `{name}` before reading it in this lexical scope; check spelling and case."),
            BWErr::ParsingError(_) => "Check the indicated token and close every pipe, bracket, brace, quote, and block comment.",
            BWErr::ControlFlowError(_) => "Return needs a custom body; Break/Continue need a loop and Rethrow needs a Catch in the same invocation.",
            BWErr::DuplicateParameter { .. } => "Give each parameter a distinct, case-sensitive name.",
            BWErr::SignatureError(_) => "Use declared parameter names and document each error code once with a nonempty description.",
            BWErr::StatementNotDefined(_) => "Define the statement before calling it; check sentence punctuation and parameter positions/count.",
            BWErr::DuplicateStatement { .. } => "Rename this declaration or remove the duplicate in this scope; the original remains registered.",
            BWErr::ParameterMissingError(_) => "Supply one argument for each parameter in the registered signature.",
            BWErr::ParsingIntegerError(_) => "Use an i32 integer (-2147483648..2147483647) or a finite f32 decimal; numbers use ASCII digits.",
            BWErr::ArithmeticError(_) => "Check zero divisors, intermediate i32 overflow, and whether floating-point operands/results are finite.",
            BWErr::OperationIncompatibleError(_) => "Use the documented operand kinds; conditions require booleans and For requires an array.",
            BWErr::CollectionAccessError { .. } => "Check each key/index and container kind; maps use exact string keys and arrays use in-bounds nonnegative indexes.",
            BWErr::OutputError(_) => "Check the output destination and account for bytes already written before retrying.",
            BWErr::NativeError(_) => "Check the registered operation's requirements and reason; account for completed effects before retrying.",
            BWErr::NativePanic(_) => "Fix the native callback; return an error for expected failures and inspect captured host state before reuse.",
            BWErr::Cancelled(_) => "Inspect completed effects and use a fresh operation control for an intentional retry.",
            BWErr::Timeout(_) => "Inspect completed effects and set an appropriate deadline before intentionally retrying.",
            BWErr::AsyncRuntime(_) => "Use a live Tokio runtime with time enabled and keep it running until operations finish.",
            BWErr::InputError(_) => "Use exact DSL variable names and JSON values with i32 integers, finite f32 decimals, and at most 128 nested containers.",
            BWErr::RunConfiguration(_) => "Use an existing working directory, valid environment names/values, a representable timeout, and syntax/AST/value/evaluation/import limits within documented ceilings.",
            BWErr::SourceRead(_) => "Use a readable UTF-8 source file relative to the run's working directory.",
            BWErr::ResourceLimit { .. } => "Reduce the workload or adjust configurable budgets within documented ceilings; completed effects are not rolled back.",
            BWErr::ImportRead(_) => "Use a readable local .botwork file, resolving relative paths from the importing source file.",
            BWErr::ImportCycle(_) => "Break the shown import cycle by moving shared definitions into a separate module.",
            BWErr::DuplicateNamespace { .. } => "Choose a distinct namespace or remove conflicting declarations in this scope; the original remains registered.",
        };
        formatter.write_str(guidance)
    }
}

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
    /// Shared immutable identity lets rethrow retain the original error without self-causes.
    pub error: Arc<BWErr>,
    pub span: Option<Span>,
    pub label: &'static str,
    /// Entered calls, innermost first. Failed argument binding adds no callee frame.
    pub call_stack: Vec<CallFrame>,
    pub related: Vec<RelatedLocation>,
    /// Errors being handled when this error occurred, with their original spans/stacks.
    pub causes: Vec<Diagnostic>,
    /// Explicit loss of metadata in an emergency bounded summary. None for full diagnostics.
    pub omissions: Option<Box<DiagnosticOmissions>>,
}

impl Diagnostic {
    pub(crate) fn is_emergency(&self) -> bool {
        if self.span.is_some()
            || !self.call_stack.is_empty()
            || !self.related.is_empty()
            || self.causes.len() != 1
        {
            return false;
        }
        let cause = &self.causes[0];
        if cause.span.is_some()
            || !cause.call_stack.is_empty()
            || !cause.related.is_empty()
            || !cause.causes.is_empty()
        {
            return false;
        }
        (matches!(
            self.code(),
            DiagnosticCode::ResourceLimit
                | DiagnosticCode::RunConfiguration
                | DiagnosticCode::Cancelled
                | DiagnosticCode::Timeout
        ) && self.omissions.is_none()
            && cause.omissions.is_some())
            || (matches!(
                self.code(),
                DiagnosticCode::Cancelled | DiagnosticCode::Timeout
            ) && self.omissions.is_some()
                && cause.code() == DiagnosticCode::ResourceLimit
                && cause.omissions.is_none())
    }

    fn emergency_omissions_mut(&mut self) -> &mut DiagnosticOmissions {
        if self.omissions.is_some() {
            self.omissions.as_deref_mut().expect("emergency omissions")
        } else {
            self.causes[0]
                .omissions
                .as_deref_mut()
                .expect("emergency omissions")
        }
    }

    pub fn code(&self) -> DiagnosticCode {
        self.error.code()
    }

    pub fn help(&self) -> String {
        self.error.help()
    }

    pub fn help_with_limit(&self, output_bytes: usize) -> RenderedDiagnostic {
        self.error.help_with_limit(output_bytes)
    }

    pub fn new(error: BWErr) -> Self {
        Self {
            error: Arc::new(error),
            span: None,
            label: "source",
            call_stack: vec![],
            related: vec![],
            causes: vec![],
            omissions: None,
        }
    }

    /// Keep the innermost location when an error crosses enclosing syntax nodes.
    pub fn at(mut self, span: &Span) -> Self {
        if self.span.is_none() && !self.is_emergency() {
            self.span = Some(span.clone());
        }
        self
    }

    pub(crate) fn at_expression(mut self, span: &Span) -> Self {
        if self.span.is_none() && !self.is_emergency() {
            self.span = Some(span.clone());
            self.label = "expression";
        }
        self
    }

    pub(crate) fn capture_stack<'a>(
        mut self,
        frames: impl DoubleEndedIterator<Item = &'a CallFrame>,
    ) -> Self {
        if self.call_stack.is_empty() && !self.is_emergency() {
            self.call_stack.extend(frames.rev().cloned());
        }
        self
    }

    pub(crate) fn prospective_location<'a>(
        &'a self,
        context: Option<(&'a Span, bool)>,
    ) -> (Option<&'a Span>, &'static str) {
        match (self.span.as_ref(), context) {
            (None, Some((span, expression))) => (
                Some(span),
                if expression { "expression" } else { self.label },
            ),
            (span, _) => (span, self.label),
        }
    }

    pub(crate) fn capture_context<'a>(
        self,
        context: Option<(&Span, bool)>,
        frames: impl DoubleEndedIterator<Item = &'a CallFrame>,
    ) -> Self {
        let error = match context {
            Some((span, true)) => self.at_expression(span),
            Some((span, false)) => self.at(span),
            None => self,
        };
        error.capture_stack(frames)
    }

    pub(crate) fn while_handling(mut self, original: Diagnostic) -> Self {
        if !Arc::ptr_eq(&self.error, &original.error) {
            if self.is_emergency() {
                let omitted = self.emergency_omissions_mut();
                omitted.direct_causes = omitted.direct_causes.saturating_add(1);
                original.discard();
            } else {
                self.causes.push(original);
            }
        }
        self
    }

    pub(crate) fn omit_handled_cause(mut self) -> Self {
        let omitted = self.emergency_omissions_mut();
        omitted.direct_causes = omitted.direct_causes.saturating_add(1);
        self
    }

    pub(crate) fn with_related(mut self, message: &str, span: &Span) -> Self {
        if self.is_emergency() {
            let omitted = self.emergency_omissions_mut();
            omitted.related_locations = omitted.related_locations.saturating_add(1);
            return self;
        }
        self.related.push(RelatedLocation {
            message: message.into(),
            span: span.clone(),
        });
        self
    }

    /// Discard source/stack information for the original error-category API.
    pub fn into_error(mut self) -> BWErr {
        ownership::discard_causes(std::mem::take(&mut self.causes));
        Arc::try_unwrap(self.error).unwrap_or_else(|error| (*error).clone())
    }

    /// Destroy an arbitrarily deep owned cause tree without recursive destruction.
    /// Host-built diagnostics retain ordinary field ownership; use this for unadmitted trees.
    pub fn discard(mut self) {
        ownership::discard_causes(std::mem::take(&mut self.causes));
    }

    /// Admit retained diagnostic shape and source owners before copying mutable metadata.
    pub fn try_clone_with_limits(&self, limits: &DiagnosticLimits) -> DiagnosticResult<Self> {
        limits.check(self)?;
        Ok(self.clone())
    }

    /// Full owned metadata for host-managed use, without resource admission.
    /// Prefer `to_value_with_limits` for bounded conversion. Coordinates are decimal strings.
    pub fn to_value(&self) -> Literal {
        value::build(self)
    }

    /// Measure metadata before copying text or constructing owned collections.
    pub fn value_size_with_limits(
        &self,
        limits: &DiagnosticValueLimits,
    ) -> DiagnosticResult<super::value_limits::ValueSize> {
        value::measure(self, limits).map_err(Diagnostic::new)
    }

    /// Convert only after all value and source-position work limits pass.
    pub fn to_value_with_limits(
        &self,
        limits: &DiagnosticValueLimits,
    ) -> DiagnosticResult<Literal> {
        self.value_size_with_limits(limits)?;
        Ok(self.to_value())
    }
}

impl From<BWErr> for Diagnostic {
    fn from(error: BWErr) -> Self {
        Self::new(error)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        render::display(self, formatter)
    }
}

impl Diagnostic {
    /// Render under local byte/work limits, or return explicit bounded truncation evidence.
    pub fn render_with_limits(&self, limits: &DiagnosticRenderLimits) -> RenderedDiagnostic {
        render::render(self, limits)
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
