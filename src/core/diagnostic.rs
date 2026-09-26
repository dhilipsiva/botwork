//! Structured source diagnostics. Compatibility APIs can recover the original BWErr.

use std::{error::Error, fmt, sync::Arc};

use super::{
    ast::Span,
    grammar::{BWErr, Literal},
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
        }
    }

    pub fn help(&self) -> String {
        match self {
            Self::VariableNotDefined(name) => format!("Define `{name}` before reading it in this lexical scope; check spelling and case."),
            Self::ParsingError(_) => "Check the indicated token and close every pipe, bracket, brace, quote, and block comment.".into(),
            Self::ControlFlowError(_) => "Return needs a custom body; Break/Continue need a loop and Rethrow needs a Catch in the same invocation.".into(),
            Self::DuplicateParameter { .. } => "Give each parameter a distinct, case-sensitive name.".into(),
            Self::SignatureError(_) => "Use declared parameter names and document each error code once with a nonempty description.".into(),
            Self::StatementNotDefined(_) => "Define the statement before calling it; check sentence punctuation and parameter positions/count.".into(),
            Self::DuplicateStatement { .. } => "Rename this declaration or remove the duplicate in this scope; the original remains registered.".into(),
            Self::ParameterMissingError(_) => "Supply one argument for each parameter in the registered signature.".into(),
            Self::ParsingIntegerError(_) => "Use an i32 integer (-2147483648..2147483647) or a finite f32 decimal; numbers use ASCII digits.".into(),
            Self::ArithmeticError(_) => "Check zero divisors, intermediate i32 overflow, and whether floating-point operands/results are finite.".into(),
            Self::OperationIncompatibleError(_) => "Use the documented operand kinds; conditions require booleans and For requires an array.".into(),
            Self::CollectionAccessError { .. } => "Check each key/index and container kind; maps use exact string keys and arrays use in-bounds nonnegative indexes.".into(),
            Self::OutputError(_) => "Check the output destination and account for bytes already written before retrying.".into(),
            Self::NativeError(_) => "Check the registered operation's requirements and reason; account for completed effects before retrying.".into(),
            Self::NativePanic(_) => "Fix the native callback; return an error for expected failures and inspect captured host state before reuse.".into(),
            Self::Cancelled(_) => "Inspect completed effects and use a fresh operation control for an intentional retry.".into(),
            Self::Timeout(_) => "Inspect completed effects and set an appropriate deadline before intentionally retrying.".into(),
            Self::AsyncRuntime(_) => "Use a live Tokio runtime with time enabled and keep it running until operations finish.".into(),
            Self::ImportRead(_) => "Use a readable local .botwork file, resolving relative paths from the importing source file.".into(),
            Self::ImportCycle(_) => "Break the shown import cycle by moving shared definitions into a separate module.".into(),
            Self::DuplicateNamespace { .. } => "Choose a distinct namespace or remove conflicting declarations in this scope; the original remains registered.".into(),
        }
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

#[derive(Clone, Debug)]
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
}

impl Diagnostic {
    pub fn code(&self) -> DiagnosticCode {
        self.error.code()
    }

    pub fn help(&self) -> String {
        self.error.help()
    }

    pub fn new(error: BWErr) -> Self {
        Self {
            error: Arc::new(error),
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
        if !Arc::ptr_eq(&self.error, &original.error) {
            self.causes.push(original);
        }
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
        Arc::try_unwrap(self.error).unwrap_or_else(|error| (*error).clone())
    }

    /// Owned DSL metadata. Coordinates are decimal strings to avoid i32 truncation.
    pub fn to_value(&self) -> Literal {
        let details = match self.error.as_ref() {
            BWErr::VariableNotDefined(name) => value_map([("name", text(name))]),
            BWErr::StatementNotDefined(call) => value_map([("call", text(call))]),
            BWErr::DuplicateStatement {
                signature,
                original,
                duplicate,
            } => value_map([
                ("signature", text(signature)),
                ("original", text(original)),
                ("duplicate", text(duplicate)),
            ]),
            BWErr::DuplicateParameter {
                name,
                original,
                duplicate,
            } => value_map([
                ("name", text(name)),
                ("original", text(original)),
                ("duplicate", text(duplicate)),
            ]),
            BWErr::CollectionAccessError {
                path,
                segment,
                reason,
            } => value_map([
                ("path", text(path)),
                ("segment", text(segment)),
                ("reason", text(reason)),
            ]),
            BWErr::DuplicateNamespace {
                namespace,
                original,
                duplicate,
            } => value_map([
                ("namespace", text(namespace)),
                ("original", text(original)),
                ("duplicate", text(duplicate)),
            ]),
            BWErr::ParameterMissingError(reason)
            | BWErr::ParsingError(reason)
            | BWErr::SignatureError(reason)
            | BWErr::ParsingIntegerError(reason)
            | BWErr::OperationIncompatibleError(reason)
            | BWErr::ControlFlowError(reason)
            | BWErr::ArithmeticError(reason)
            | BWErr::OutputError(reason)
            | BWErr::NativeError(reason)
            | BWErr::Cancelled(reason)
            | BWErr::Timeout(reason)
            | BWErr::AsyncRuntime(reason)
            | BWErr::ImportRead(reason)
            | BWErr::ImportCycle(reason)
            | BWErr::NativePanic(reason) => value_map([("reason", text(reason))]),
        };
        value_map([
            ("code", text(self.code().as_str())),
            ("message", text(&self.error.to_string())),
            ("help", text(&self.help())),
            ("details", details),
            ("source", span_value(self.span.as_ref())),
            (
                "call_stack",
                Literal::Array(
                    self.call_stack
                        .iter()
                        .map(|frame| {
                            value_map([
                                ("signature", text(&frame.signature)),
                                ("call_site", span_value(Some(&frame.call_site))),
                                (
                                    "definition_site",
                                    span_value(frame.definition_site.as_ref()),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "related",
                Literal::Array(
                    self.related
                        .iter()
                        .map(|location| {
                            value_map([
                                ("message", text(&location.message)),
                                ("source", span_value(Some(&location.span))),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "causes",
                Literal::Array(self.causes.iter().map(Self::to_value).collect()),
            ),
        ])
    }
}

fn text(value: &str) -> Literal {
    Literal::String(value.to_owned())
}

fn value_map(entries: impl IntoIterator<Item = (&'static str, Literal)>) -> Literal {
    Literal::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn span_value(span: Option<&Span>) -> Literal {
    let Some(span) = span else {
        return Literal::None;
    };
    let (line, column) = span.line_column();
    let (end_line, end_column) = span.end_line_column();
    value_map([
        ("file", text(span.source().name())),
        ("text", text(span.text())),
        ("start_byte", text(&span.start().to_string())),
        ("end_byte", text(&span.end().to_string())),
        ("line", text(&line.to_string())),
        ("column", text(&column.to_string())),
        ("end_line", text(&end_line.to_string())),
        ("end_column", text(&end_column.to_string())),
    ])
}

impl From<BWErr> for Diagnostic {
    fn from(error: BWErr) -> Self {
        Self::new(error)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = &self.span {
            write!(formatter, "{}", span.location())?;
            if span.start() != span.end() {
                let (line, column) = span.end_line_column();
                write!(formatter, "-{line}:{column}")?;
            }
            formatter.write_str(": ")?;
        }
        write!(formatter, "[{}] {}", self.code(), self.error)?;
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
        write!(formatter, "\n  help: {}", self.help())?;
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
