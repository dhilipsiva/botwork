use super::*;
use crate::core::diagnostic::DiagnosticCode;

impl Context {
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
            .admit_with_stack(error, &self.calls)
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
