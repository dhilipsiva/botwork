//! Shared statement contracts for invocation, help, completion, and hover consumers.

use std::fmt;

use super::{
    ast::{self, Definition, Name, Span},
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLimits, DiagnosticResult},
    grammar::{BWErr, Literal},
};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueKind {
    None,
    Int,
    Float,
    Bool,
    String,
    Array,
    Map,
}

impl ValueKind {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Int,
        Self::Float,
        Self::Bool,
        Self::String,
        Self::Array,
        Self::Map,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Int => "Int",
            Self::Float => "Float",
            Self::Bool => "Bool",
            Self::String => "String",
            Self::Array => "Array",
            Self::Map => "Map",
        }
    }
}

impl Literal {
    pub fn kind(&self) -> ValueKind {
        match self {
            Self::None => ValueKind::None,
            Self::Int(_) => ValueKind::Int,
            Self::Float(_) => ValueKind::Float,
            Self::Bool(_) => ValueKind::Bool,
            Self::String(_) => ValueKind::String,
            Self::Array(_) => ValueKind::Array,
            Self::Map(_) => ValueKind::Map,
        }
    }
}

/// A nonempty set of accepted top-level kinds. Nested values must still be finite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueKinds(u8);

impl ValueKinds {
    pub const ANY: Self = Self(0b111_1111);
    pub const fn one(kind: ValueKind) -> Self {
        Self(1 << kind as u8)
    }
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub const fn contains(self, kind: ValueKind) -> bool {
        self.0 & (1 << kind as u8) != 0
    }
    pub fn iter(self) -> impl Iterator<Item = ValueKind> {
        ValueKind::ALL
            .into_iter()
            .filter(move |kind| self.contains(*kind))
    }
}

impl From<ValueKind> for ValueKinds {
    fn from(kind: ValueKind) -> Self {
        Self::one(kind)
    }
}

impl fmt::Display for ValueKinds {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::ANY {
            return formatter.write_str("Any");
        }
        for (index, kind) in self.iter().enumerate() {
            if index != 0 {
                formatter.write_str(" | ")?;
            }
            formatter.write_str(kind.as_str())?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatementOrigin {
    Native,
    Dsl,
}

#[derive(Clone, Debug)]
pub struct ParameterSignature {
    pub name: String,
    pub accepted: ValueKinds,
    pub span: Span,
}

/// Documented operation errors; additional interpreter or dynamic callee errors can occur.
#[derive(Clone, Debug)]
pub struct StatementError {
    pub code: DiagnosticCode,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct StatementSignature {
    normalized: String,
    header: Span,
    origin: StatementOrigin,
    parameters: Vec<ParameterSignature>,
    returns: ValueKinds,
    description: String,
    errors: Vec<StatementError>,
    namespace: Option<String>,
}

impl StatementSignature {
    /// Parse a native header. Kinds default to Any until constrained by the builder.
    pub fn native(header: &str) -> DiagnosticResult<Self> {
        Self::native_at("<native>", header)
    }

    pub(crate) fn native_at(name: &str, header: &str) -> DiagnosticResult<Self> {
        let parsed = ast::native_signature(name, header)?;
        Ok(Self::from_parts(
            parsed.signature,
            parsed.span,
            &parsed.parameters,
            StatementOrigin::Native,
        ))
    }

    pub(crate) fn from_definition(definition: &Definition) -> Self {
        Self::from_parts(
            definition.signature.clone(),
            definition.header.clone(),
            &definition.parameters,
            StatementOrigin::Dsl,
        )
    }

    fn from_parts(
        normalized: String,
        header: Span,
        parameters: &[Name],
        origin: StatementOrigin,
    ) -> Self {
        Self {
            normalized,
            header,
            origin,
            parameters: parameters
                .iter()
                .map(|name| ParameterSignature {
                    name: name.text.clone(),
                    accepted: ValueKinds::ANY,
                    span: name.span.clone(),
                })
                .collect(),
            returns: ValueKinds::ANY,
            description: String::new(),
            errors: vec![],
            namespace: None,
        }
    }

    pub(crate) fn builder_error(&self, message: fmt::Arguments<'_>) -> Diagnostic {
        DiagnosticLimits::default().formatted_detail(
            BWErr::SignatureError,
            message,
            Some(&self.header),
            false,
            std::iter::empty(),
        )
    }

    /// Constrain an exact case-sensitive parameter name; unknown names are errors.
    pub fn parameter(
        mut self,
        name: &str,
        accepted: impl Into<ValueKinds>,
    ) -> DiagnosticResult<Self> {
        let Some(parameter) = self
            .parameters
            .iter_mut()
            .find(|parameter| parameter.name == name)
        else {
            return Err(self.builder_error(format_args!(
                "Unknown parameter `{name}` in `{}`",
                self.header.text()
            )));
        };
        parameter.accepted = accepted.into();
        Ok(self)
    }

    pub fn returns(mut self, kinds: impl Into<ValueKinds>) -> Self {
        self.returns = kinds.into();
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn documents_error(
        mut self,
        code: DiagnosticCode,
        description: impl Into<String>,
    ) -> DiagnosticResult<Self> {
        let description = description.into();
        if description.trim().is_empty() || self.errors.iter().any(|error| error.code == code) {
            return Err(self.builder_error(format_args!(
                "Document each error code once with a nonempty description"
            )));
        }
        self.errors.push(StatementError { code, description });
        self.errors.sort_by_key(|error| error.code.as_str());
        Ok(self)
    }

    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    pub fn header(&self) -> &Span {
        &self.header
    }

    /// Qualified display name while retaining the original definition's header span.
    pub fn display_header(&self) -> String {
        match &self.namespace {
            Some(namespace) => format!("{namespace}::{}", self.header.text().trim()),
            None => self.header.text().trim().into(),
        }
    }

    /// Owned string payload copied into qualified metadata and its namespace map entry.
    pub(crate) fn qualified_bytes(&self, namespace: &str, normalized: &str) -> Option<usize> {
        let qualified = normalized
            .len()
            .checked_add(2)?
            .checked_add(self.normalized.len())?;
        let display = namespace.len().checked_add(
            self.namespace
                .as_ref()
                .map_or(0, |name| name.len().saturating_add(2)),
        )?;
        let mut bytes = qualified
            .checked_mul(2)?
            .checked_add(display)?
            .checked_add(self.normalized.len())?
            .checked_add(self.description.len())?;
        for parameter in &self.parameters {
            bytes = bytes.checked_add(parameter.name.len())?;
        }
        for error in &self.errors {
            bytes = bytes.checked_add(error.description.len())?;
        }
        Some(bytes)
    }

    /// Signature strings plus its independently owned registry lookup key.
    pub(crate) fn retained_bytes(&self) -> Option<usize> {
        let mut bytes = self
            .normalized
            .len()
            .checked_mul(2)?
            .checked_add(self.namespace.as_ref().map_or(0, String::len))?
            .checked_add(self.description.len())?;
        for parameter in &self.parameters {
            bytes = bytes.checked_add(parameter.name.len())?;
        }
        for error in &self.errors {
            bytes = bytes.checked_add(error.description.len())?;
        }
        Some(bytes)
    }

    pub(crate) fn qualified(&self, namespace: &str, normalized: &str) -> Self {
        Self {
            normalized: format!("{normalized}::{}", self.normalized),
            namespace: Some(match &self.namespace {
                Some(nested) => format!("{namespace}::{nested}"),
                None => namespace.into(),
            }),
            header: self.header.clone(),
            origin: self.origin,
            parameters: self.parameters.clone(),
            returns: self.returns,
            description: self.description.clone(),
            errors: self.errors.clone(),
        }
    }
    pub fn origin(&self) -> StatementOrigin {
        self.origin
    }
    pub fn parameters(&self) -> &[ParameterSignature] {
        &self.parameters
    }
    pub fn return_kinds(&self) -> ValueKinds {
        self.returns
    }
    pub fn documentation(&self) -> &str {
        &self.description
    }
    pub fn documented_errors(&self) -> &[StatementError] {
        &self.errors
    }

    pub fn help(&self) -> String {
        let mut lines = vec![self.display_header()];
        if !self.description.is_empty() {
            lines.push(self.description.clone());
        }
        for parameter in &self.parameters {
            lines.push(format!("  {}: {}", parameter.name, parameter.accepted));
        }
        lines.push(format!("  returns: {}", self.returns));
        for error in &self.errors {
            lines.push(format!("  {}: {}", error.code, error.description));
        }
        if self.origin == StatementOrigin::Dsl {
            lines.push("  Dynamic DSL body; return kinds and errors are not inferred.".into());
        }
        lines.join("\n")
    }

    pub(crate) fn validate_argument<E>(
        &self,
        index: usize,
        value: &Literal,
        error: impl FnOnce(fmt::Arguments<'_>) -> E,
    ) -> Result<(), E> {
        self.validate_argument_kind(index, value.kind(), error)
    }

    pub(crate) fn validate_argument_kind<E>(
        &self,
        index: usize,
        kind: ValueKind,
        error: impl FnOnce(fmt::Arguments<'_>) -> E,
    ) -> Result<(), E> {
        let parameter = &self.parameters[index];
        if parameter.accepted.contains(kind) {
            return Ok(());
        }
        Err(error(format_args!(
            "Parameter `{}` (argument {}) of `{}` requires {}; got {}",
            parameter.name,
            index + 1,
            self.normalized,
            parameter.accepted,
            kind.as_str(),
        )))
    }

    pub(crate) fn validate_return(
        &self,
        value: &Literal,
        error: impl FnOnce(fmt::Arguments<'_>) -> Diagnostic,
    ) -> DiagnosticResult<()> {
        if self.returns.contains(value.kind()) {
            return Ok(());
        }
        Err(error(format_args!(
            "Return value of `{}` requires {}; got {}",
            self.normalized,
            self.returns,
            value.kind().as_str(),
        )))
    }
}

impl Definition {
    /// Untyped DSL parameters/results are Any; dynamic errors are not inferred.
    pub fn signature_metadata(&self) -> StatementSignature {
        StatementSignature::from_definition(self)
    }
}
