use super::*;
use crate::core::{diagnostic::DiagnosticCode, run::OutputLimits};
use std::cell::Cell;

fn context(record_bytes: usize, total_bytes: usize) -> Context {
    Context::with_limits(RunLimits {
        output: OutputLimits {
            record_bytes,
            total_bytes,
        },
        ..Default::default()
    })
    .unwrap()
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    calls: usize,
    flushes: usize,
    max: Option<usize>,
    fail_after: Option<usize>,
    interrupts: usize,
    flush_interrupts: usize,
    flush_failure: bool,
    cancel_write: Option<OperationControl>,
    cancel_flush: Option<OperationControl>,
}

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        assert!(bytes.len() <= CHUNK_BYTES);
        if let Some(control) = self.cancel_write.take() {
            control.cancel();
        }
        if self.interrupts > 0 {
            self.interrupts -= 1;
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self
            .fail_after
            .is_some_and(|limit| self.bytes.len() >= limit)
        {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
        }
        let count = bytes.len().min(self.max.unwrap_or(usize::MAX)).min(
            self.fail_after
                .map_or(usize::MAX, |limit| limit - self.bytes.len()),
        );
        self.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        if let Some(control) = self.cancel_flush.take() {
            control.cancel();
        }
        if self.flush_interrupts > 0 {
            self.flush_interrupts -= 1;
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.flush_failure {
            Err(io::Error::other("flush failed"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn readable_values_admit_exact_utf8_escapes_keys_punctuation_and_newlines() {
    let values = [
        Literal::None,
        Literal::Bool(false),
        Literal::Int(i32::MIN),
        Literal::Float(f32::MAX),
        Literal::String("தமிழ் é\n\0".into()),
        Literal::Array(vec![
            Literal::String("'\"\\\n\0தமிழ்".into()),
            Literal::Array(vec![]),
        ]),
        Literal::Map(HashMap::from([
            ("z\"".into(), Literal::String("é".into())),
            ("a\0".into(), Literal::Map(HashMap::new())),
        ])),
    ];
    for value in values {
        for newline in [false, true] {
            let expected = format!("{value}{}", if newline { "\n" } else { "" });
            for (record, total, success) in [
                (expected.len(), expected.len(), true),
                (expected.len() - 1, usize::MAX, false),
                (usize::MAX, expected.len() - 1, false),
            ] {
                let context = context(record, total);
                let sibling = context.clone();
                let mut writer = Writer::default();
                let result = context.write_value_record(&value, &mut writer, newline);
                if success {
                    assert_eq!(result.unwrap(), expected.len());
                    assert_eq!(writer.bytes, expected.as_bytes());
                    assert_eq!(writer.flushes, 1);
                } else {
                    assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit);
                    assert_eq!((writer.calls, writer.flushes), (0, 0));
                    assert!(context.checkpoint().is_err());
                }
                sibling.checkpoint().unwrap();
            }
        }
    }
}

#[test]
fn repeated_output_shares_the_quota_and_clones_copy_usage_without_stop_leakage() {
    let context = context(8, 8);
    let mut bytes = Vec::new();
    context
        .write_output(&mut bytes, format_args!("1234"))
        .unwrap();
    let sibling = context.clone();
    context
        .write_value(&Literal::Int(5678), &mut bytes)
        .unwrap();
    assert!(context.write_output(&mut bytes, format_args!("x")).is_err());
    assert_eq!(bytes, b"12345678");
    let mut sibling_bytes = Vec::new();
    sibling
        .write_output(&mut sibling_bytes, format_args!("abcd"))
        .unwrap();
    assert!(sibling
        .write_output(&mut sibling_bytes, format_args!("x"))
        .is_err());
    assert_eq!(sibling_bytes, b"abcd");
}

#[test]
fn empty_output_with_zero_limits_still_requires_a_successful_flush() {
    let context = context(0, 0);
    let mut writer = Writer::default();
    assert_eq!(
        context
            .write_value(&Literal::String(String::new()), &mut writer)
            .unwrap(),
        0
    );
    assert_eq!((writer.calls, writer.flushes), (0, 1));
    writer.flush_failure = true;
    assert_eq!(
        context
            .write_output(&mut writer, format_args!(""))
            .unwrap_err()
            .code(),
        DiagnosticCode::Output
    );
    context.checkpoint().unwrap();
}

#[test]
fn short_and_interrupted_writes_complete_with_fixed_size_chunks_and_flush() {
    let expected = "é".repeat(CHUNK_BYTES * 2);
    let mut writer = Writer {
        max: Some(3),
        interrupts: 3,
        flush_interrupts: 2,
        ..Default::default()
    };
    let bytes = context(expected.len(), expected.len())
        .write_output(&mut writer, format_args!("{expected}"))
        .unwrap();
    assert_eq!(bytes, expected.len());
    assert_eq!(writer.bytes, expected.as_bytes());
    assert_eq!(writer.flushes, 3);
}

#[test]
fn partial_zero_and_flush_failures_report_incomplete_output_and_keep_the_charge() {
    for (max, fail_after, flush_failure, accepted) in [
        (None, Some(2), false, 2),
        (Some(0), None, false, 0),
        (None, None, true, 5),
    ] {
        let context = context(5, 5);
        let mut writer = Writer {
            max,
            fail_after,
            flush_failure,
            ..Default::default()
        };
        let error = context
            .write_output(&mut writer, format_args!("hello"))
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Output);
        assert!(error
            .error
            .to_string()
            .contains(&format!("{accepted} of 5 bytes accepted")));
        assert!(error.error.to_string().contains("output incomplete"));
        assert_eq!(writer.bytes.len(), accepted);
        context.checkpoint().unwrap(); // IO errors alone remain catchable.
        let calls = writer.calls;
        assert_eq!(
            context
                .write_output(&mut writer, format_args!("x"))
                .unwrap_err()
                .code(),
            DiagnosticCode::ResourceLimit
        );
        assert_eq!(writer.calls, calls);
    }
}

#[test]
fn cancellation_before_during_and_after_output_never_returns_success() {
    for stage in 0..5 {
        let control = OperationControl::default();
        let context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
        let mut writer = Writer::default();
        match stage {
            0 => control.cancel(),
            1 => {
                writer.cancel_write = Some(control);
                writer.max = Some(1);
            }
            2 => writer.cancel_write = Some(control),
            3 => writer.cancel_flush = Some(control),
            _ => {
                writer.cancel_write = Some(control);
                writer.interrupts = 1;
            }
        }
        let error = context
            .write_output(&mut writer, format_args!("hello"))
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Cancelled);
        assert_eq!(writer.bytes.len(), [0, 1, 5, 5, 0][stage]);
        assert_eq!(writer.flushes, usize::from(stage == 3));
    }
}

#[test]
fn cancellation_and_flush_failure_keep_both_causes() {
    let control = OperationControl::default();
    let context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    let mut writer = Writer {
        cancel_flush: Some(control),
        flush_failure: true,
        ..Default::default()
    };
    let error = context
        .write_output(&mut writer, format_args!("hello"))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Output);
}

#[test]
fn admission_stops_formatting_early_and_cancellation_skips_the_destination() {
    struct Counted<'a>(&'a Cell<usize>, Option<&'a OperationControl>);
    impl fmt::Display for Counted<'_> {
        fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
            for _ in 0..100_000 {
                self.0.set(self.0.get() + 1);
                output.write_str("é")?;
                if let Some(control) = self.1 {
                    control.cancel();
                }
            }
            Ok(())
        }
    }
    let visits = Cell::new(0);
    let mut writer = Writer::default();
    assert!(context(3, 10)
        .write_output(&mut writer, format_args!("{}", Counted(&visits, None)))
        .is_err());
    assert_eq!(visits.get(), 2);
    let control = OperationControl::default();
    let context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    visits.set(0);
    let error = context
        .write_output(
            &mut writer,
            format_args!("{}", Counted(&visits, Some(&control))),
        )
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(visits.get(), 2);
    assert_eq!((writer.calls, writer.flushes), (0, 0));
}

#[test]
fn unstable_or_failed_formatters_cannot_overrun_admission_or_report_success() {
    struct Changing(Cell<usize>, &'static str, &'static str, bool);
    impl fmt::Display for Changing {
        fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
            let pass = self.0.replace(self.0.get() + 1);
            output.write_str(if pass == 0 { self.1 } else { self.2 })?;
            if self.3 {
                Err(fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    for (first, second, fail) in [("a", "bb", false), ("bb", "a", false), ("a", "a", true)] {
        let mut writer = Writer::default();
        let error = context(16, 16)
            .write_output(
                &mut writer,
                format_args!("{}", Changing(Cell::new(0), first, second, fail)),
            )
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Output);
        assert_eq!((writer.calls, writer.flushes), (0, 0));
    }
}

#[test]
fn invalid_host_depth_is_rejected_before_recursive_formatting() {
    let mut value = Literal::None;
    for _ in 0..1000 {
        value = Literal::Array(vec![value]);
    }
    let value = Owned::new(value);
    let mut writer = Writer::default();
    let error = Context::default()
        .write_value(&value, &mut writer)
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!((writer.calls, writer.flushes), (0, 0));
}

#[test]
fn accepted_maximum_depth_can_be_formatted_with_exact_byte_limits() {
    let mut value = Literal::None;
    for _ in 1..crate::core::value_limits::MAX_VALUE_DEPTH {
        value = Literal::Array(vec![value]);
    }
    let expected = value.to_string();
    let mut bytes = Vec::new();
    context(expected.len(), expected.len())
        .write_value(&value, &mut bytes)
        .unwrap();
    assert_eq!(bytes, expected.as_bytes());
}

#[test]
fn a_deadline_expiring_inside_a_write_is_observed_before_success_or_flush() {
    struct SlowWriter {
        deadline: tokio::time::Instant,
        written: usize,
    }
    impl Write for SlowWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            std::thread::sleep(
                self.deadline
                    .saturating_duration_since(tokio::time::Instant::now()),
            );
            self.written += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("deadline prevents flush")
        }
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    let context = Context::with_control(
        RunLimits::default(),
        OperationControl::default().child(Some(deadline)),
    )
    .unwrap();
    let mut writer = SlowWriter {
        deadline,
        written: 0,
    };
    let error = context
        .write_output(&mut writer, format_args!("hello"))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert_eq!(writer.written, 5);
}

#[test]
fn invalid_writer_counts_are_errors_instead_of_panics_or_success() {
    struct Invalid;
    impl Write for Invalid {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len() + 1)
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("invalid write prevents flush")
        }
    }
    let error = Context::default()
        .write_output(&mut Invalid, format_args!("hello"))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Output);
    assert!(error.error.to_string().contains("invalid byte count"));
}
