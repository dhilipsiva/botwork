use super::*;
use crate::core::{diagnostic::DiagnosticCode, operation::OperationControl};

struct Writer {
    output: Vec<u8>,
    interrupt: bool,
    stop: Option<OperationControl>,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.interrupt {
            self.interrupt = false;
            return Err(io::ErrorKind::Interrupted.into());
        }
        let count = bytes.len().min(3);
        self.output.extend_from_slice(&bytes[..count]);
        if let Some(control) = &self.stop {
            control.cancel();
        }
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn result_admission_precedes_file_creation_and_truncation() {
    use crate::core::value_limits::ValueLimits;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("kept");
    fs::write(&path, b"old").unwrap();
    for op in [OsOp::Write, OsOp::Append, OsOp::Create, OsOp::WriteBinary] {
        let context = Context::with_limits(RunLimits {
            values: ValueLimits {
                nodes: 0,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        let arguments = vec![
            TemporaryValue::new(Literal::String(path.to_str().unwrap().into()), None),
            TemporaryValue::new(
                if matches!(op, OsOp::WriteBinary) {
                    Literal::Array(vec![])
                } else {
                    Literal::String("new".into())
                },
                None,
            ),
        ];
        let error = invoke(op, &arguments, &context).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(fs::read(&path).unwrap(), b"old");
    }
}

#[test]
fn interrupted_and_partial_writes_complete_without_losing_or_repeating_bytes() {
    let context = Context::default();
    let mut writer = Writer {
        output: vec![],
        interrupt: true,
        stop: None,
    };
    let source = vec![37; CHUNK + 7];
    write_all(&context, &mut writer, Path::new("test"), &source).unwrap();
    assert_eq!(writer.output, source);
}

#[test]
fn writes_observe_cancellation_between_partial_effects_and_reject_zero_progress() {
    let control = OperationControl::default();
    let context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    let mut writer = Writer {
        output: vec![],
        interrupt: false,
        stop: Some(control),
    };
    assert_eq!(
        write_all(&context, &mut writer, Path::new("test"), b"abcdef")
            .unwrap_err()
            .code(),
        DiagnosticCode::Cancelled
    );
    assert_eq!(writer.output, b"abc");
    let mut empty = &mut [0u8; 0][..];
    assert_eq!(
        write_all(&Context::default(), &mut empty, Path::new("test"), b"a")
            .unwrap_err()
            .code(),
        DiagnosticCode::Native
    );
}
