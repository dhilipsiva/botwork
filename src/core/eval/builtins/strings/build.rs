use super::*;
use std::fmt;

pub(super) enum RenderError<'a> {
    Invalid(&'static str),
    Field(&'a str),
    Write,
}
impl From<fmt::Error> for RenderError<'_> {
    fn from(_: fmt::Error) -> Self {
        Self::Write
    }
}
impl RenderError<'_> {
    fn diagnostic(self, context: &Context) -> RuntimeDiagnostic {
        match self {
            Self::Invalid(reason) => invalid(context, reason),
            Self::Field(name) => context.formatted_error(
                BWErr::OperationIncompatibleError,
                format_args!("Missing or invalid format field {name:?}"),
                None,
                false,
            ),
            Self::Write => invalid(context, "String rendering failed"),
        }
    }
}

struct Counter<'a> {
    context: &'a Context,
    limits: crate::core::value_limits::ValueLimits,
    bytes: usize,
    failure: Option<RuntimeDiagnostic>,
}
impl fmt::Write for Counter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.failure.is_some() {
            return Err(fmt::Error);
        }
        let checked = self
            .context
            .checkpoint()
            .map_err(RuntimeDiagnostic::from)
            .and_then(|()| {
                let limits = &self.limits;
                let bytes = self.bytes.checked_add(text.len()).ok_or_else(|| {
                    limit(self.context, "value string bytes", limits.string_bytes)
                })?;
                limits.string_size(bytes).map_err(|e| {
                    RuntimeDiagnostic::from(self.context.retain_limit(Diagnostic::new(e)))
                })?;
                self.bytes = bytes;
                Ok(())
            });
        if let Err(error) = checked {
            self.failure = Some(error);
            return Err(fmt::Error);
        }
        Ok(())
    }
}

pub(super) fn produce(
    context: &Context,
    size: ValueSize,
    build: impl FnOnce() -> EvaluationResult<Literal>,
) -> TemporaryResult {
    let reservation = context.temporary_reservation(size)?;
    context.checkpoint()?;
    Ok(TemporaryValue::new(build()?, reservation))
}

pub(super) fn render<'a>(
    context: &Context,
    render: impl Fn(&mut dyn fmt::Write, bool) -> Result<(), RenderError<'a>>,
) -> TemporaryResult {
    let mut counter = Counter {
        context,
        limits: context.limits().values,
        bytes: 0,
        failure: None,
    };
    let measured = render(&mut counter, false);
    if let Some(error) = counter.failure {
        return Err(error);
    }
    measured.map_err(|e| e.diagnostic(context))?;
    let size = context
        .limits()
        .values
        .string_size(counter.bytes)
        .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
    produce(context, size, || {
        let mut output = Sink {
            context,
            text: String::with_capacity(size.payload_bytes),
            expected: size.payload_bytes,
            failure: None,
        };
        let rendered = render(&mut output, true);
        if let Some(error) = output.failure {
            return Err(error);
        }
        rendered.map_err(|e| e.diagnostic(context))?;
        if output.text.len() != size.payload_bytes {
            return Err(invalid(
                context,
                "String rendering size changed after admission",
            ));
        }
        Ok(Literal::String(output.text))
    })
}

pub(super) fn array<'a, I>(context: &Context, parts: impl Fn() -> I) -> TemporaryResult
where
    I: Iterator<Item = &'a str>,
{
    let limits = context.limits().values;
    let mut size = limits
        .container_header(0)
        .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
    let mut count = 0usize;
    for part in parts() {
        context.checkpoint()?;
        count = count
            .checked_add(1)
            .ok_or_else(|| limit(context, "value container entries", limits.entries))?;
        limits
            .container_header(count)
            .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
        let child = limits
            .string_size(part.len())
            .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
        limits
            .add_child(&mut size, child)
            .map_err(|e| context.retain_limit(Diagnostic::new(e)))?;
    }
    produce(context, size, || {
        let mut output = Vec::with_capacity(count);
        for part in parts() {
            context.checkpoint()?;
            output.push(Literal::String(part.into()));
        }
        Ok(Literal::Array(output))
    })
}

pub(super) fn case_size(context: &Context, text: &str, upper: bool) -> EvaluationResult<ValueSize> {
    let mut counter = Counter {
        context,
        limits: context.limits().values,
        bytes: 0,
        failure: None,
    };
    use fmt::Write;
    let result = if upper {
        text.chars()
            .flat_map(char::to_uppercase)
            .try_for_each(|ch| counter.write_char(ch))
    } else {
        // The contextual lowercasing exception is sigma: σ and ς both occupy
        // two UTF-8 bytes. Render with str::to_lowercase for correct word context.
        text.chars()
            .flat_map(char::to_lowercase)
            .try_for_each(|ch| counter.write_char(ch))
    };
    if let Some(error) = counter.failure {
        return Err(error);
    }
    result.map_err(|_| invalid(context, "Case mapping measurement failed"))?;
    context
        .limits()
        .values
        .string_size(counter.bytes)
        .map_err(|e| context.retain_limit(Diagnostic::new(e)).into())
}

struct Sink<'a> {
    context: &'a Context,
    text: String,
    expected: usize,
    failure: Option<RuntimeDiagnostic>,
}
impl fmt::Write for Sink<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.failure.is_some() {
            return Err(fmt::Error);
        }
        if let Err(error) = self.context.checkpoint() {
            self.failure = Some(error.into());
            return Err(fmt::Error);
        }
        if text.len() > self.expected - self.text.len() {
            self.failure = Some(invalid(
                self.context,
                "String rendering exceeded admitted size",
            ));
            return Err(fmt::Error);
        }
        self.text.push_str(text);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
