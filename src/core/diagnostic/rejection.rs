//! Fixed emergency evidence for rejected owned diagnostics.

use super::{CallFrame, Diagnostic, DiagnosticLimits, DiagnosticResult};
use crate::core::grammar::BWErr;

#[cfg(test)]
mod tests;

pub const SUMMARY_DETAIL_BYTES: usize = 256;
pub const SUMMARY_SOURCE_NAME_BYTES: usize = 256;
pub(super) const TRUNCATED: &str = "…[truncated]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OmittedSource {
    /// A bounded filename prefix; no SourceFile owner is retained.
    pub file: String,
    pub file_truncated: bool,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
        BWErr::VariableNotDefined(value) => BWErr::VariableNotDefined(detail(value)),
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
    /// Fixed emergency evidence for a runtime budget/control failure retaining this error.
    pub(crate) fn rejected(self, violation: BWErr) -> Self {
        rejected(self, violation, 0)
    }
}

fn rejected(diagnostic: Diagnostic, violation: BWErr, pending_frames: usize) -> Diagnostic {
    let mut shortened = 0;
    let mut summary = Diagnostic::new(error_summary(&diagnostic.error, &mut shortened));
    let source = diagnostic.span.as_ref().map(|span| {
        let (file, file_truncated) = prefix(span.source().name(), SUMMARY_SOURCE_NAME_BYTES);
        OmittedSource {
            file,
            file_truncated,
            start_byte: span.start(),
            end_byte: span.end(),
        }
    });
    let label = !matches!(diagnostic.label, "source" | "expression");
    if !label {
        summary.label = diagnostic.label;
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
        label,
        prior_summary: diagnostic.omissions.is_some(),
        source,
    }));
    diagnostic.discard();
    Diagnostic::new(violation).while_handling(summary)
}

pub(super) fn reject_borrowed_detail(
    skeleton: Diagnostic,
    category: fn(String) -> BWErr,
    detail: &str,
    violation: BWErr,
    pending_frames: usize,
) -> Diagnostic {
    let (detail, shortened) = prefix(detail, SUMMARY_DETAIL_BYTES);
    reject_constructed_detail(
        skeleton,
        category,
        detail,
        shortened,
        violation,
        pending_frames,
    )
}

pub(super) fn reject_constructed_detail(
    mut skeleton: Diagnostic,
    category: fn(String) -> BWErr,
    detail: String,
    shortened: bool,
    violation: BWErr,
    pending_frames: usize,
) -> Diagnostic {
    skeleton.error = std::sync::Arc::new(category(detail));
    reject_constructed_error(skeleton, usize::from(shortened), violation, pending_frames)
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
