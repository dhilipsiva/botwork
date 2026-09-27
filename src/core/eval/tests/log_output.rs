use super::super::*;
use crate::core::diagnostic::{DiagnosticCode, DiagnosticLimits};
use std::sync::Mutex;

struct PartialWriter {
    written: Vec<u8>,
    error: Option<io::Error>,
    cancel: Option<OperationControl>,
}

impl PartialWriter {
    fn new(reason: &str) -> Self {
        Self {
            written: Vec::new(),
            error: Some(io::Error::new(io::ErrorKind::BrokenPipe, reason.to_owned())),
            cancel: None,
        }
    }
}

impl Write for PartialWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.written.is_empty() && !buffer.is_empty() {
            self.written.push(buffer[0]);
            Ok(1)
        } else {
            if let Some(control) = &self.cancel {
                control.cancel();
            }
            Err(self.error.take().expect("one failed write"))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn install_writer(context: &mut Context, writer: Arc<Mutex<PartialWriter>>) {
    context.init_statements();
    let StmtType::Native { callback, .. } =
        context.frames[0].statements.get_mut("log|param|").unwrap()
    else {
        panic!("builtin Log")
    };
    *callback = Arc::new(move |values, context| {
        write_log(&values[0], &mut *writer.lock().unwrap(), context)?;
        Ok(values[0].clone())
    });
}

#[test]
fn output_errors_admit_complete_calls_at_exact_limits_and_reject_each_context_deficit() {
    let program = Program::parse(
        "é.botwork",
        "Write { Log |\"hello\"| }\n|before| = |1|\nWrite\n|after| = |1|",
    )
    .unwrap();
    let reason = "destination é closed\n";
    let mut baseline_context = Context::default();
    install_writer(
        &mut baseline_context,
        Arc::new(Mutex::new(PartialWriter::new(reason))),
    );
    let baseline = evaluate_program_detailed(&program, &mut baseline_context).unwrap_err();
    assert!(matches!(baseline.error.as_ref(), BWErr::OutputError(detail)
        if detail == &format!("{reason}; 1 of 6 bytes accepted by destination; output incomplete")));
    assert_eq!(baseline.span.as_ref().unwrap().text(), "Log |\"hello\"|");
    assert_eq!(baseline.call_stack.len(), 2);
    assert_eq!(baseline.call_stack[0].signature, "log|param|");
    assert_eq!(baseline.call_stack[1].signature, "write");
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    for dimension in 0..6 {
        let mut limits = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        match dimension {
            0 => (),
            1 => limits.text_bytes -= 1,
            2 => limits.source_bytes -= 1,
            3 => limits.call_frames -= 1,
            4 => limits.diagnostics = 0,
            _ => limits.depth = 0,
        }
        let mut context = Context::with_limits(RunLimits {
            diagnostics: limits,
            ..RunLimits::default()
        })
        .unwrap();
        let writer = Arc::new(Mutex::new(PartialWriter::new(reason)));
        install_writer(&mut context, writer.clone());
        let mut sibling = context.clone();
        let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
        assert_eq!(writer.lock().unwrap().written, b"h");
        assert_eq!(context.frames[0].variables["before"].value.to_string(), "1");
        assert!(!context.frames[0].variables.contains_key("after"));
        assert!(context.calls.is_empty() && context.handlers.is_empty());
        assert_eq!(context.frames.len(), 1);
        if dimension == 0 {
            assert_eq!(
                error.to_value().to_string(),
                baseline.to_value().to_string()
            );
            context.checkpoint().unwrap();
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            let original = &error.causes[0];
            assert_eq!(original.code(), DiagnosticCode::Output);
            assert!(original.span.is_none());
            let omitted = original.omissions.as_ref().unwrap();
            assert_eq!(omitted.call_frames, 2);
            assert!(!omitted.prior_summary);
            assert!(context.checkpoint().is_err());
        }
        let next = Program::parse("next", "|next| = |1|").unwrap();
        evaluate_program_detailed(&next, &mut sibling).unwrap();
    }
}

#[test]
fn output_failure_remains_catchable_but_construction_rejection_skips_the_handler() {
    let program = Program::parse(
        "catch",
        "Try { Log |\"hello\"| } Catch |error| { |code| = |error.code| }\n|after| = |1|",
    )
    .unwrap();
    for reject in [false, true] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: if reject { 0 } else { 1_048_576 },
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        context.set_variable("error", Literal::Int(7)).unwrap();
        let writer = Arc::new(Mutex::new(PartialWriter::new("closed")));
        install_writer(&mut context, writer.clone());
        let result = evaluate_program_detailed(&program, &mut context);
        assert_eq!(context.frames[0].variables["error"].value.to_string(), "7");
        assert!(context.calls.is_empty() && context.handlers.is_empty());
        assert_eq!(writer.lock().unwrap().written, b"h");
        if reject {
            assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit);
            assert!(!context.frames[0].variables.contains_key("code"));
            assert!(!context.frames[0].variables.contains_key("after"));
        } else {
            result.unwrap();
            assert_eq!(context.get_variable("code").unwrap().to_string(), "BW4001");
            assert_eq!(context.get_variable("after").unwrap().to_string(), "1");
        }
    }
}

#[test]
fn output_failure_preserves_cancellation_observed_during_the_partial_write() {
    let program = Program::parse("cancel", "Log |\"hello\"|\n|after| = |1|").unwrap();
    for reject in [false, true] {
        let control = OperationControl::default();
        let mut context = Context::with_control(
            RunLimits {
                diagnostics: DiagnosticLimits {
                    diagnostics: if reject { 0 } else { 16 },
                    ..DiagnosticLimits::default()
                },
                ..RunLimits::default()
            },
            control.clone(),
        )
        .unwrap();
        let mut writer = PartialWriter::new("closed");
        writer.cancel = Some(control);
        let writer = Arc::new(Mutex::new(writer));
        install_writer(&mut context, writer.clone());
        let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Cancelled);
        assert_eq!(
            error.causes[0].code(),
            if reject {
                DiagnosticCode::ResourceLimit
            } else {
                DiagnosticCode::Output
            }
        );
        assert_eq!(writer.lock().unwrap().written, b"h");
        assert!(!context.frames[0].variables.contains_key("after"));
        assert!(context.calls.is_empty() && context.handlers.is_empty());
    }
}

#[test]
fn rejected_output_details_keep_a_unicode_prefix_and_success_needs_no_error_allowance() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Debug)]
    struct CountedError(Arc<AtomicUsize>);
    impl std::fmt::Display for CountedError {
        fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for _ in 0..65_536 {
                self.0.fetch_add(1, Ordering::SeqCst);
                output.write_str("é")?;
            }
            Ok(())
        }
    }
    impl std::error::Error for CountedError {}
    let context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let mut output = Vec::new();
    write_log(&Literal::String("hello".into()), &mut output, &context).unwrap();
    assert_eq!(output, b"hello\n");
    context.checkpoint().unwrap();
    let visits = Arc::new(AtomicUsize::new(0));
    let mut writer = PartialWriter::new("unused");
    writer.error = Some(io::Error::new(
        io::ErrorKind::BrokenPipe,
        CountedError(visits.clone()),
    ));
    let error = write_log(&Literal::String("hello".into()), &mut writer, &context).unwrap_err();
    let original = &error.causes[0];
    assert_eq!(original.code(), DiagnosticCode::Output);
    let BWErr::OutputError(detail) = original.error.as_ref() else {
        panic!("output detail")
    };
    assert!(detail.len() <= 256 && detail.ends_with("…[truncated]"));
    assert_eq!(original.omissions.as_ref().unwrap().detail_fields, 1);
    assert_eq!(writer.written, b"h");
    // A zero text allowance rejects the skeleton before the full formatter runs;
    // only the bounded summary visits the first 256 UTF-8 bytes and next chunk.
    assert_eq!(visits.load(Ordering::SeqCst), 129);
}
