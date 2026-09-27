use super::*;
use crate::core::grammar::DisplayValue;
use std::fmt;

#[cfg(test)]
mod tests;

impl Context {
    /// Serialize a value's readable representation under this run's limits.
    /// Returns bytes accepted only after a successful flush and stop check.
    /// Errors can leave a prefix in the destination; no output is rolled back.
    /// Unlike Log this does not append a newline. See docs/output-limits.md.
    ///
    /// ```
    /// use botwork::core::{eval::Context, grammar::Literal, run::{RunLimits, OutputLimits}};
    /// let context = Context::with_limits(RunLimits {
    ///     output: OutputLimits { record_bytes: 3, total_bytes: 3 },
    ///     ..Default::default()
    /// }).unwrap();
    /// let mut bytes = Vec::new();
    /// assert_eq!(context.write_value(&Literal::String("é".into()), &mut bytes).unwrap(), 2);
    /// context.write_output(&mut bytes, format_args!("\n")).unwrap();
    /// assert!(context.write_value(&Literal::Int(7), &mut bytes).is_err());
    /// assert_eq!(bytes, "é\n".as_bytes());
    /// ```
    pub fn write_value(&self, value: &Literal, output: &mut impl Write) -> DiagnosticResult<usize> {
        self.write_value_record(value, output, false)
            .map_err(|error| {
                self.runtime_diagnostic(error, None, false)
                    .into_diagnostic()
            })
    }

    /// Admit formatted bytes before buffering/writing, sharing Log's run quota.
    /// Display is called twice and must be deterministic. Arbitrary host Display
    /// implementations and blocking writers must cooperate with cancellation.
    pub fn write_output(
        &self,
        output: &mut impl Write,
        arguments: fmt::Arguments<'_>,
    ) -> DiagnosticResult<usize> {
        self.write_record(output, arguments, arguments)
            .map_err(|error| {
                self.runtime_diagnostic(error, None, false)
                    .into_diagnostic()
            })
    }

    pub(super) fn write_value_record(
        &self,
        value: &Literal,
        output: &mut impl Write,
        newline: bool,
    ) -> EvaluationResult<usize> {
        self.check_value(value)?;
        let suffix = if newline { "\n" } else { "" };
        self.write_record(
            output,
            format_args!("{}{suffix}", DisplayValue(value, false)),
            format_args!("{value}{suffix}"),
        )
    }

    fn write_record(
        &self,
        output: &mut impl Write,
        measure: fmt::Arguments<'_>,
        render: fmt::Arguments<'_>,
    ) -> EvaluationResult<usize> {
        self.checkpoint()?;
        let (remaining, resource, limit) = self.budget.as_ref().map_or_else(
            || {
                let limit = self.limits().output.record_bytes;
                (limit, "output record bytes", limit)
            },
            RunBudget::output_allowance,
        );
        let mut counter = Counter {
            context: self,
            remaining,
            resource,
            limit,
            bytes: 0,
            failure: None,
        };
        let measured = fmt::write(&mut counter, measure);
        self.checkpoint()?;
        if let Some(error) = counter.failure {
            return Err(error.into());
        }
        if measured.is_err() {
            return Err(self.output_error(format_args!("Formatting output failed before writing")));
        }
        let expected = counter.bytes;
        if let Some(budget) = &self.budget {
            budget.charge_output(expected)?;
        }
        let mut stream = Stream {
            context: self,
            output,
            buffer: [0; CHUNK_BYTES],
            buffered: 0,
            formatted: 0,
            written: 0,
            expected,
            failure: None,
        };
        let rendered = fmt::write(&mut stream, render);
        let result = match stream.failure.take() {
            Some(error) => Err(error),
            None if rendered.is_err() => {
                Err(Failure::Io(io::Error::other("Formatting output failed")))
            }
            None => stream.finish(),
        };
        // A writer/formatter can cancel while failing, including on the final flush.
        self.after_evaluation(match result {
            Ok(()) => Ok(expected),
            Err(Failure::Stop(error)) => Err(error.into()),
            Err(Failure::Io(error)) => Err(self.output_error(format_args!(
                "{error}; {} of {expected} bytes accepted by destination; output incomplete",
                stream.written
            ))),
        })
    }

    fn output_error(&self, message: fmt::Arguments<'_>) -> RuntimeDiagnostic {
        self.formatted_error(
            BWErr::OutputError,
            message,
            self.calls.last().map(|record| &record.frame.call_site),
            false,
        )
    }
}

struct Counter<'a> {
    context: &'a Context,
    remaining: usize,
    resource: &'static str,
    limit: usize,
    bytes: usize,
    failure: Option<Diagnostic>,
}

impl fmt::Write for Counter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.failure.is_some() {
            return Err(fmt::Error);
        }
        if let Err(error) = self.context.checkpoint() {
            self.failure = Some(error);
            return Err(fmt::Error);
        }
        if text.len() > self.remaining {
            self.failure = Some(
                self.context
                    .retain_limit(Diagnostic::new(BWErr::ResourceLimit {
                        resource: self.resource,
                        limit: self.limit as u64,
                    })),
            );
            return Err(fmt::Error);
        }
        self.remaining -= text.len();
        self.bytes += text.len(); // Bounded by the original allowance, even at usize::MAX.
        Ok(())
    }
}

const CHUNK_BYTES: usize = 4096;

enum Failure {
    Stop(Diagnostic),
    Io(io::Error),
}

struct Stream<'a, W> {
    context: &'a Context,
    output: &'a mut W,
    buffer: [u8; CHUNK_BYTES],
    buffered: usize,
    formatted: usize,
    written: usize,
    expected: usize,
    failure: Option<Failure>,
}

impl<W: Write> Stream<'_, W> {
    fn checkpoint(&self) -> Result<(), Failure> {
        self.context.checkpoint().map_err(Failure::Stop)
    }

    fn drain(&mut self) -> Result<(), Failure> {
        let mut offset = 0;
        while offset < self.buffered {
            self.checkpoint()?;
            let result = self.output.write(&self.buffer[offset..self.buffered]);
            if let Ok(bytes) = result {
                if bytes > self.buffered - offset {
                    return Err(Failure::Io(io::Error::other(
                        "Writer returned an invalid byte count",
                    )));
                }
                offset += bytes;
                self.written += bytes;
            }
            match result {
                Ok(0) => return Err(Failure::Io(io::ErrorKind::WriteZero.into())),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(Failure::Io(error)),
            }
            self.checkpoint()?;
        }
        self.buffered = 0;
        Ok(())
    }

    fn append(&mut self, text: &str) -> Result<(), Failure> {
        self.checkpoint()?;
        if text.len() > self.expected - self.formatted {
            return Err(Failure::Io(io::Error::other(
                "Output size changed after admission",
            )));
        }
        self.formatted += text.len();
        let mut bytes = text.as_bytes();
        while !bytes.is_empty() {
            self.checkpoint()?;
            let count = bytes.len().min(CHUNK_BYTES - self.buffered);
            self.buffer[self.buffered..self.buffered + count].copy_from_slice(&bytes[..count]);
            self.buffered += count;
            bytes = &bytes[count..];
            if self.buffered == CHUNK_BYTES {
                self.drain()?;
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Failure> {
        if self.formatted != self.expected {
            return Err(Failure::Io(io::Error::other(
                "Output size changed after admission",
            )));
        }
        self.drain()?;
        loop {
            self.checkpoint()?;
            let result = self.output.flush();
            match result {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => self.checkpoint()?,
                result => return result.map_err(Failure::Io),
            }
        }
    }
}

impl<W: Write> fmt::Write for Stream<'_, W> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.failure.is_some() {
            return Err(fmt::Error);
        }
        self.append(text).map_err(|error| {
            self.failure = Some(error);
            fmt::Error
        })
    }
}
