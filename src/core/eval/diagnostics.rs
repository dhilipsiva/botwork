use super::*;

#[cfg(test)]
mod tests;
use crate::core::diagnostic::{DiagnosticCode, FormattedDetail};

struct OriginalLocation<'a> {
    span: &'a Span,
    native: bool,
}

impl std::fmt::Display for OriginalLocation<'_> {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.native {
            output.write_str(self.span.source().name())
        } else {
            std::fmt::Display::fmt(&self.span.location_display(), output)
        }
    }
}

struct AccessPath<'a> {
    base: &'a Expr,
    segments: &'a [AccessSegment],
}
impl std::fmt::Display for AccessPath<'_> {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(self.base.span.text().trim())?;
        for part in self.segments {
            match part {
                AccessSegment::Literal(name) => {
                    output.write_str(".")?;
                    output.write_str(&name.text)?;
                }
                AccessSegment::Computed { span, .. } => output.write_str(span.text().trim())?,
            }
        }
        Ok(())
    }
}

impl Context {
    pub(super) fn rethrow_handler(&self, original: &StoredDiagnostic, span: &Span) -> Diagnostic {
        match original.copy(self.budget.as_ref(), Some(("rethrow", span))) {
            Ok(copy) => copy.into_diagnostic(),
            Err(violation) => original.value.rejected_copy(violation.into_error(), 1),
        }
    }

    pub(super) fn finish_handler_error(
        &self,
        error: Diagnostic,
        original: Arc<StoredDiagnostic>,
    ) -> Diagnostic {
        // Rethrow already owns this category and its original cause tree. Do not
        // copy the handler merely to discard the duplicate identity afterward.
        if Arc::ptr_eq(&error.error, &original.value.error) {
            return error;
        }
        if error.is_emergency() {
            return error.omit_handled_cause();
        }
        match StoredDiagnostic::take(original, self.budget.as_ref()) {
            Ok(original) => error.while_handling(original.into_diagnostic()),
            Err(violation) => error.rejected(violation.into_error()).omit_handled_cause(),
        }
    }

    pub(super) fn ast_error(&self, failure: ast::AstFailure<'_>) -> Diagnostic {
        let stopped = self.checkpoint().err();
        let error = failure.diagnostic(
            &self.limits().diagnostics,
            self.calls.iter().map(|record| &record.frame),
        );
        self.finish_constructed_error(error, stopped, failure.span(), false)
    }

    pub(super) fn duplicate_error(
        &self,
        category: fn([String; 3]) -> BWErr,
        name: &str,
        original: (&Span, bool),
        duplicate: &Span,
        related_label: &'static str,
    ) -> Diagnostic {
        let stopped = self.checkpoint().err();
        let original_display = OriginalLocation {
            span: original.0,
            native: original.1,
        };
        let duplicate_display = duplicate.location_display();
        let original_full = format_args!("{original_display}");
        let original_summary = format_args!("{original_display:#}");
        let error = self.limits().diagnostics.formatted_related_fields(
            category,
            [
                FormattedDetail {
                    full: format_args!("{name}"),
                    summary: None,
                },
                FormattedDetail {
                    full: original_full,
                    summary: (!original.1).then_some(original_summary),
                },
                FormattedDetail {
                    full: format_args!("{duplicate_display}"),
                    summary: Some(format_args!("{duplicate_display:#}")),
                },
            ],
            duplicate,
            (related_label, original.0),
            self.calls.iter().map(|record| &record.frame),
        );
        self.finish_constructed_error(error, stopped, Some(duplicate), false)
    }

    pub(super) fn import_error(
        &self,
        category: fn(String) -> BWErr,
        message: std::fmt::Arguments<'_>,
        span: &Span,
        import_site: &Span,
    ) -> Diagnostic {
        let stopped = self.checkpoint().err();
        let error = self.limits().diagnostics.formatted_related_detail(
            category,
            message,
            span,
            ("imported here", import_site),
            self.calls.iter().map(|record| &record.frame),
        );
        self.finish_constructed_error(error, stopped, Some(span), false)
    }

    pub(super) fn access_error(
        &self,
        base: &Expr,
        segments: &[AccessSegment],
        segment: &AccessSegment,
        reason: std::fmt::Arguments<'_>,
    ) -> Diagnostic {
        let path = AccessPath { base, segments };
        let (span, text) = match segment {
            AccessSegment::Literal(name) => (&name.span, name.text.as_str()),
            AccessSegment::Computed { span, .. } => (span, span.text().trim()),
        };
        let stopped = self.checkpoint().err();
        let error = self.limits().diagnostics.formatted_fields(
            |[path, segment, reason]| BWErr::CollectionAccessError {
                path,
                segment,
                reason,
            },
            [format_args!("{path}"), format_args!("{text}"), reason],
            Some(span),
            true,
            self.calls.iter().map(|record| &record.frame),
        );
        self.finish_constructed_error(error, stopped, Some(span), true)
    }

    pub(crate) fn formatted_error(
        &self,
        category: fn(String) -> BWErr,
        message: std::fmt::Arguments<'_>,
        span: Option<&Span>,
        expression: bool,
    ) -> Diagnostic {
        let stopped = self.checkpoint().err();
        let error = self.limits().diagnostics.formatted_detail(
            category,
            message,
            span,
            expression,
            self.calls.iter().map(|record| &record.frame),
        );
        self.finish_constructed_error(error, stopped, span, expression)
    }

    pub(super) fn detail_error(
        &self,
        category: fn(String) -> BWErr,
        detail: &str,
        span: Option<&Span>,
        expression: bool,
    ) -> Diagnostic {
        let stopped = self.checkpoint().err();
        let error = self.limits().diagnostics.borrowed_detail(
            category,
            detail,
            span,
            expression,
            self.calls.iter().map(|record| &record.frame),
        );
        self.finish_constructed_error(error, stopped, span, expression)
    }

    fn finish_constructed_error(
        &self,
        error: Diagnostic,
        stopped: Option<Diagnostic>,
        span: Option<&Span>,
        expression: bool,
    ) -> Diagnostic {
        if let Some(stopped) = stopped {
            // Preserve a prior observed stop before a construction quota can latch.
            self.diagnostic(stopped.while_handling(error), span, expression)
        } else {
            self.diagnostic(self.retain_limit(error), None, false)
        }
    }

    /// Admit a complete error and its prospective call snapshot before metadata copies.
    /// Emergency evidence uses fixed independent bounds and must survive unwinding intact.
    pub(crate) fn diagnostic(
        &self,
        error: Diagnostic,
        span: Option<&Span>,
        expression: bool,
    ) -> Diagnostic {
        if error.is_emergency() {
            return error;
        }
        // Observe stops before a new diagnostic limit can latch. Preserve the native
        // cause's own entered-call snapshot before wrapping it in a control failure.
        let stopped = self.checkpoint().err();
        let preserve_control = stopped.as_ref().is_some_and(|stopped| {
            stopped.code() == error.code()
                && matches!(
                    stopped.code(),
                    DiagnosticCode::Cancelled | DiagnosticCode::Timeout
                )
        });
        let error = self.admit_diagnostic(error, span, expression, preserve_control);
        match stopped {
            Some(stopped) if stopped.code() != error.code() => {
                let preserve_control = matches!(
                    stopped.code(),
                    DiagnosticCode::Cancelled | DiagnosticCode::Timeout
                );
                self.admit_diagnostic(
                    stopped.while_handling(error),
                    span,
                    expression,
                    preserve_control,
                )
            }
            _ => error,
        }
    }

    pub(super) fn retain_handler(
        &self,
        original: Diagnostic,
    ) -> DiagnosticResult<Arc<StoredDiagnostic>> {
        let reservation = self
            .budget
            .as_ref()
            .map(|budget| budget.reserve_diagnostic(&original))
            .transpose();
        match reservation {
            Ok(reservation) => Ok(Arc::new(StoredDiagnostic {
                value: original,
                _reservation: reservation,
            })),
            Err(error) => Err(original.rejected(error.into_error())),
        }
    }

    pub(super) fn retain_call(
        &self,
        signature: &str,
        call_site: &Span,
        definition_site: Option<&Span>,
    ) -> DiagnosticResult<Arc<StoredCallFrame>> {
        let reservation = self
            .budget
            .as_ref()
            .map(|budget| budget.reserve_call_frame(signature, call_site, definition_site))
            .transpose()?;
        Ok(Arc::new(StoredCallFrame {
            frame: CallFrame {
                signature: signature.into(),
                call_site: call_site.clone(),
                definition_site: definition_site.cloned(),
            },
            _reservation: reservation,
        }))
    }

    fn admit_diagnostic(
        &self,
        error: Diagnostic,
        span: Option<&Span>,
        expression: bool,
        preserve_control: bool,
    ) -> Diagnostic {
        let error = match span {
            Some(span) if expression => error.at_expression(span),
            Some(span) => error.at(span),
            None => error,
        };
        match self
            .limits()
            .diagnostics
            .admit_with_stack(error, self.calls.iter().map(|record| &record.frame))
        {
            Ok(error) => error,
            Err(error) => {
                let mut error = self.retain_limit(error);
                if preserve_control {
                    let mut original = error.causes.pop().expect("bounded original");
                    original.causes.push(error);
                    original
                } else {
                    error
                }
            }
        }
    }
}
