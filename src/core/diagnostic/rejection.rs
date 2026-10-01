//! Fixed emergency evidence for rejected owned diagnostics.

use super::{CallFrame, Diagnostic, DiagnosticLimits, DiagnosticResult};
use crate::core::ast::Span;
use crate::core::grammar::BWErr;

#[cfg(test)]
mod tests;

pub const SUMMARY_DETAIL_BYTES: usize = 256;
pub const SUMMARY_SOURCE_NAME_BYTES: usize = 256;
pub(super) const TRUNCATED: &str = "…[truncated]";

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OmittedSource {
    /// A bounded filename prefix; no SourceFile owner is retained.
    pub file: String,
    pub file_truncated: bool,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiagnosticOmissions {
    pub detail_fields: usize,
    pub call_frames: usize,
    pub related_locations: usize,
    /// Direct root causes; discarded subtrees are not traversed just to count them.
    pub direct_causes: usize,
    pub label: bool,
    pub prior_summary: bool,
    pub source: Option<OmittedSource>,
}

fn prefix(value: &str, maximum: usize) -> (String, bool) {
    if value.len() <= maximum {
        return (value.to_owned(), false);
    }
    let mut end = maximum - TRUNCATED.len();
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = String::with_capacity(maximum);
    result.push_str(&value[..end]);
    result.push_str(TRUNCATED);
    (result, true)
}

fn error_summary(error: &BWErr, shortened: &mut usize) -> BWErr {
    let mut detail = |value: &str| {
        let (value, truncated) = prefix(value, SUMMARY_DETAIL_BYTES);
        *shortened += usize::from(truncated);
        value
    };
    match error {
        BWErr::AssertionMismatch {
            reason,
            actual,
            expected,
        } => BWErr::AssertionMismatch {
            reason: detail(reason),
            actual: detail(actual),
            expected: detail(expected),
        },
        BWErr::ConditionNotMet {
            reason,
            attempts,
            history,
        } => BWErr::ConditionNotMet {
            reason: detail(reason),
            attempts: detail(attempts),
            history: detail(history),
        },
        BWErr::RetriesExhausted {
            reason,
            attempts,
            history,
        } => BWErr::RetriesExhausted {
            reason: detail(reason),
            attempts: detail(attempts),
            history: detail(history),
        },
        BWErr::VariableNotDefined { name, suggestion } => BWErr::VariableNotDefined {
            name: detail(name),
            suggestion: suggestion.as_deref().map(&mut detail),
        },
        BWErr::StatementNotDefined(value) => BWErr::StatementNotDefined(detail(value)),
        BWErr::DuplicateStatement {
            signature,
            original,
            duplicate,
        } => BWErr::DuplicateStatement {
            signature: detail(signature),
            original: detail(original),
            duplicate: detail(duplicate),
        },
        BWErr::DuplicateParameter {
            name,
            original,
            duplicate,
        } => BWErr::DuplicateParameter {
            name: detail(name),
            original: detail(original),
            duplicate: detail(duplicate),
        },
        BWErr::CollectionAccessError {
            path,
            segment,
            reason,
        } => BWErr::CollectionAccessError {
            path: detail(path),
            segment: detail(segment),
            reason: detail(reason),
        },
        BWErr::DuplicateNamespace {
            namespace,
            original,
            duplicate,
        } => BWErr::DuplicateNamespace {
            namespace: detail(namespace),
            original: detail(original),
            duplicate: detail(duplicate),
        },
        BWErr::ResourceLimit { resource, limit } => {
            let resource = if resource.len() > SUMMARY_DETAIL_BYTES {
                *shortened += 1;
                "<resource identifier truncated>"
            } else {
                resource
            };
            BWErr::ResourceLimit {
                resource,
                limit: *limit,
            }
        }
        BWErr::ParameterMissingError(value) => BWErr::ParameterMissingError(detail(value)),
        BWErr::ParsingError(value) => BWErr::ParsingError(detail(value)),
        BWErr::SignatureError(value) => BWErr::SignatureError(detail(value)),
        BWErr::ParsingIntegerError(value) => BWErr::ParsingIntegerError(detail(value)),
        BWErr::OperationIncompatibleError(value) => {
            BWErr::OperationIncompatibleError(detail(value))
        }
        BWErr::ControlFlowError(value) => BWErr::ControlFlowError(detail(value)),
        BWErr::ArithmeticError(value) => BWErr::ArithmeticError(detail(value)),
        BWErr::OutputError(value) => BWErr::OutputError(detail(value)),
        BWErr::NativeError(value) => BWErr::NativeError(detail(value)),
        BWErr::Cancelled(value) => BWErr::Cancelled(detail(value)),
        BWErr::Timeout(value) => BWErr::Timeout(detail(value)),
        BWErr::AsyncRuntime(value) => BWErr::AsyncRuntime(detail(value)),
        BWErr::ImportRead(value) => BWErr::ImportRead(detail(value)),
        BWErr::ImportCycle(value) => BWErr::ImportCycle(detail(value)),
        BWErr::NativePanic(value) => BWErr::NativePanic(detail(value)),
        BWErr::InputError(value) => BWErr::InputError(detail(value)),
        BWErr::RunConfiguration(value) => BWErr::RunConfiguration(detail(value)),
        BWErr::AssertionFailed(value) => BWErr::AssertionFailed(detail(value)),
        BWErr::ExplicitFailure(value) => BWErr::ExplicitFailure(detail(value)),
        BWErr::SourceRead(value) => BWErr::SourceRead(detail(value)),
    }
}

impl DiagnosticLimits {
    /// Take ownership only if the complete tree fits. On rejection, dispose it
    /// iteratively and return the violation with a bounded original-category summary.
    /// Emergency evidence has fixed independent caps and can exceed a zero input quota.
    pub fn admit(&self, diagnostic: Diagnostic) -> DiagnosticResult<Diagnostic> {
        self.admit_with_stack(diagnostic, std::iter::empty())
    }

    pub(crate) fn admit_with_stack<'a>(
        &self,
        diagnostic: Diagnostic,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> DiagnosticResult<Diagnostic> {
        if let Err(violation) = self.check_with_stack(&diagnostic, frames.clone()) {
            Err(rejected(diagnostic, violation, frames.len()))
        } else {
            Ok(diagnostic.capture_stack(frames))
        }
    }
}

impl Diagnostic {
    pub(crate) fn rejected_context(
        self,
        violation: BWErr,
        context: Option<(&Span, bool)>,
        pending_frames: usize,
    ) -> Self {
        let summary = borrowed_context_summary(&self, pending_frames, context);
        self.discard();
        Diagnostic::new(violation).while_handling(summary)
    }

    /// Preserve bounded evidence without first copying a retained original tree.
    pub(crate) fn rejected_copy(&self, violation: BWErr, pending_related: usize) -> Self {
        let mut summary = borrowed_summary(self, 0);
        let omissions = summary.omissions.as_mut().expect("bounded original");
        omissions.related_locations = omissions.related_locations.saturating_add(pending_related);
        Diagnostic::new(violation).while_handling(summary)
    }
}

fn borrowed_summary(diagnostic: &Diagnostic, pending_frames: usize) -> Diagnostic {
    borrowed_context_summary(diagnostic, pending_frames, None)
}

fn borrowed_context_summary(
    diagnostic: &Diagnostic,
    pending_frames: usize,
    context: Option<(&Span, bool)>,
) -> Diagnostic {
    let mut shortened = 0;
    let mut summary = Diagnostic::new(error_summary(&diagnostic.error, &mut shortened));
    let (span, label) = diagnostic.prospective_location(context);
    let source = span.map(|span| {
        let (file, file_truncated) = prefix(span.source().name(), SUMMARY_SOURCE_NAME_BYTES);
        OmittedSource {
            file,
            file_truncated,
            start_byte: span.start(),
            end_byte: span.end(),
        }
    });
    let omitted_label = !matches!(label, "source" | "expression");
    if !omitted_label {
        summary.label = label;
    }
    summary.omissions = Some(Box::new(DiagnosticOmissions {
        detail_fields: shortened,
        call_frames: if diagnostic.call_stack.is_empty() {
            pending_frames
        } else {
            diagnostic.call_stack.len()
        },
        related_locations: diagnostic.related.len(),
        direct_causes: diagnostic.causes.len(),
        label: omitted_label,
        prior_summary: diagnostic.omissions.is_some(),
        source,
    }));
    summary
}

fn rejected(diagnostic: Diagnostic, violation: BWErr, pending_frames: usize) -> Diagnostic {
    let summary = borrowed_summary(&diagnostic, pending_frames);
    diagnostic.discard();
    Diagnostic::new(violation).while_handling(summary)
}

pub(super) fn reject_input_origin(
    skeleton: Diagnostic,
    origin: (String, bool),
    violation: BWErr,
) -> Diagnostic {
    let mut error = rejected(skeleton, violation, 0);
    error.causes[0]
        .omissions
        .as_mut()
        .expect("bounded original")
        .source = Some(OmittedSource {
        file: origin.0,
        file_truncated: origin.1,
        start_byte: 0,
        end_byte: 0,
    });
    error
}

pub(super) fn reject_source_prefix(
    skeleton: Diagnostic,
    origin: &str,
    start_byte: usize,
    end_byte: usize,
    violation: BWErr,
    pending_frames: usize,
) -> Diagnostic {
    let mut error = rejected(skeleton, violation, pending_frames);
    let (file, file_truncated) = prefix(origin, SUMMARY_SOURCE_NAME_BYTES);
    error.causes[0]
        .omissions
        .as_mut()
        .expect("bounded original")
        .source = Some(OmittedSource {
        file,
        file_truncated,
        start_byte,
        end_byte,
    });
    error
}

pub(super) fn reject_constructed_error(
    skeleton: Diagnostic,
    shortened: usize,
    violation: BWErr,
    pending_frames: usize,
) -> Diagnostic {
    let mut error = rejected(skeleton, violation, pending_frames);
    error.causes[0]
        .omissions
        .as_mut()
        .expect("bounded original")
        .detail_fields += shortened;
    error
}
