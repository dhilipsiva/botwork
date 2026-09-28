use super::*;
use crate::core::{
    diagnostic::DiagnosticCode, operation::OperationControl, run::TemporaryLimits,
    value_limits::ValueLimits,
};

#[test]
fn an_excessive_opened_length_is_rejected_before_reading_any_payload() {
    struct Unread;
    impl Read for Unread {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("rejected metadata must precede reads")
        }
    }
    let context = Context::with_limits(RunLimits {
        values: ValueLimits {
            string_bytes: 3,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        from_reader(&context, &mut Unread, Path::new("test"), 4, false)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn opened_length_hints_allow_growth_and_release_unused_credit_after_shrinkage() {
    for binary in [false, true] {
        let scale = if binary { 4 } else { 1 };
        for hint in [0, 1, 6] {
            let context = Context::with_limits(RunLimits {
                temporaries: TemporaryLimits {
                    payload_bytes: 6 * scale,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
            let value = from_reader(
                &context,
                &mut b"abc".as_slice(),
                Path::new("test"),
                hint,
                binary,
            )
            .unwrap();
            assert!(
                context.temporary_string(&"x".repeat(3 * scale)).is_ok(),
                "unused file-size hint must be released"
            );
            if binary {
                assert_eq!(value.to_string(), "[97, 98, 99]");
            } else {
                assert_eq!(value.to_string(), "abc");
            }
        }
    }
}

#[test]
fn growing_files_cannot_bypass_value_or_live_temporary_limits() {
    for binary in [false, true] {
        for temporary in [false, true] {
            let context = Context::with_limits(RunLimits {
                values: ValueLimits {
                    string_bytes: if temporary { 10 } else { 2 },
                    entries: if temporary { 10 } else { 2 },
                    ..Default::default()
                },
                temporaries: TemporaryLimits {
                    payload_bytes: if temporary {
                        if binary {
                            8
                        } else {
                            2
                        }
                    } else {
                        100
                    },
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
            let error = from_reader(
                &context,
                &mut b"abc".as_slice(),
                Path::new("test"),
                0,
                binary,
            )
            .unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        }
    }
}

struct Reader {
    control: OperationControl,
    entered: bool,
}
impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        assert!(!self.entered, "cancelled reader must not be read again");
        self.entered = true;
        output[0] = b'x';
        self.control.cancel();
        Ok(1)
    }
}

#[test]
fn cancelled_reads_discard_the_last_chunk_and_utf8_can_cross_chunk_boundaries() {
    let control = OperationControl::default();
    let context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    let error = from_reader(
        &context,
        &mut Reader {
            control,
            entered: false,
        },
        Path::new("test"),
        0,
        false,
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    let source = format!("{}🙂", "x".repeat(CHUNK - 1));
    let value = from_reader(
        &Context::default(),
        &mut source.as_bytes(),
        Path::new("test"),
        0,
        false,
    )
    .unwrap();
    assert_eq!(text(&value), source);
}
