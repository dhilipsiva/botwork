use super::*;
use crate::core::{
    ast::Program,
    diagnostic::{CallFrame, DiagnosticCode, DiagnosticOmissions, RelatedLocation},
    grammar::BWErr,
};

// Frozen small-tree reference for the previously public text layout.
fn legacy(error: &Diagnostic, output: &mut String) {
    if let Some(span) = &error.span {
        write!(output, "{}", span.location()).unwrap();
        if span.start() != span.end() {
            let (line, column) = span.end_line_column();
            write!(output, "-{line}:{column}").unwrap();
        }
        output.push_str(": ");
    }
    write!(output, "[{}] {}", error.code(), error.error).unwrap();
    if let Some(span) = &error.span {
        if !span.text().trim().is_empty() {
            write!(output, "\n  {}: {}", error.label, span.text().trim()).unwrap();
        }
    }
    for location in &error.related {
        write!(
            output,
            "\n  {}: {}",
            location.message,
            location.span.location()
        )
        .unwrap();
    }
    for frame in &error.call_stack {
        write!(
            output,
            "\n  in `{}` called at {}",
            frame.signature,
            frame.call_site.location()
        )
        .unwrap();
        if let Some(definition) = &frame.definition_site {
            write!(output, " (defined at {})", definition.location()).unwrap();
        }
    }
    if let Some(omissions) = &error.omissions {
        write!(output, "\n  diagnostic metadata omitted: {} shortened detail fields, {} call frames, {} related locations, {} direct causes; label omitted: {}; prior summary omitted: {}", omissions.detail_fields, omissions.call_frames, omissions.related_locations, omissions.direct_causes, omissions.label, omissions.prior_summary).unwrap();
        if let Some(source) = &omissions.source {
            write!(
                output,
                "; source {} bytes {}..{} (filename shortened: {})",
                source.file, source.start_byte, source.end_byte, source.file_truncated
            )
            .unwrap();
        }
    }
    write!(output, "\n  help: {}", error.help()).unwrap();
    for cause in &error.causes {
        output.push_str("\nwhile handling: ");
        legacy(cause, output);
    }
}

fn sample() -> Diagnostic {
    let program = Program::parse("தமிழ்-é", "# 🙂\r\n\t|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let mut error =
        Diagnostic::new(BWErr::undefined_variable("cafe\u{301}".into())).at_expression(span);
    error.call_stack.push(CallFrame {
        signature: "read".into(),
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    error.related.push(RelatedLocation {
        message: "related é".into(),
        span: span.clone(),
    });
    let mut first = Diagnostic::new(BWErr::NativeError("first".into())).at(span);
    first.omissions = Some(Box::new(DiagnosticOmissions {
        detail_fields: 1,
        call_frames: 2,
        related_locations: 3,
        direct_causes: 4,
        label: false,
        prior_summary: true,
        source: None,
    }));
    error.causes.push(first);
    error
        .causes
        .push(Diagnostic::new(BWErr::OutputError("second".into())).at(span));
    error
}

#[test]
fn complete_rendering_preserves_legacy_text_at_every_exact_limit_and_rejects_one_less() {
    let error = sample();
    let mut expected = String::new();
    legacy(&error, &mut expected);
    let span = error.span.as_ref().unwrap();
    let source_scan_bytes =
        3 * (6 * (span.start() + span.end()) + 4 * span.text().len()) + 3 * 6 * span.start();
    let exact = DiagnosticRenderLimits {
        output_bytes: expected.len(),
        diagnostics: 3,
        depth: 2,
        call_frames: 1,
        related_locations: 1,
        source_scan_bytes,
    };
    let rendered = render(&error, &exact);
    assert_eq!(rendered.text, expected);
    assert!(rendered.truncation.is_none());
    assert_eq!(error.to_string(), expected);
    for dimension in 0..6 {
        let mut limits = exact.clone();
        let (field, resource) = match dimension {
            0 => (&mut limits.output_bytes, "diagnostic render output bytes"),
            1 => (&mut limits.diagnostics, "diagnostic render records"),
            2 => (&mut limits.depth, "diagnostic render depth"),
            3 => (&mut limits.call_frames, "diagnostic render call frames"),
            4 => (
                &mut limits.related_locations,
                "diagnostic render related locations",
            ),
            _ => (
                &mut limits.source_scan_bytes,
                "diagnostic render source scan bytes",
            ),
        };
        *field -= 1;
        let rendered = render(&error, &limits);
        assert_eq!(rendered.truncation.unwrap().resource, resource);
        assert!(rendered.text.len() <= RENDER_SUMMARY_BYTES);
        assert!(rendered.text.starts_with("[BW2001]"));
        assert!(rendered.text.contains("while handling: [BW4002]"));
        assert!(rendered.text.contains("1 additional direct causes"));
        assert_eq!(error.causes.len(), 2);
        assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    }
}

#[test]
fn zero_limits_return_explicit_bounded_evidence_without_source_scanning() {
    // Private sentinel offsets would panic if rendering scanned or sliced this source.
    let span = Span::source_prefix("é", "", usize::MAX);
    let error = Diagnostic::new(BWErr::NativeError("reason".into())).at(&span);
    let rendered = render(
        &error,
        &DiagnosticRenderLimits {
            source_scan_bytes: 0,
            ..DiagnosticRenderLimits::default()
        },
    );
    assert!(rendered.text.contains(&format!("bytes {}..0", usize::MAX)));
    assert!(rendered.text.contains("coordinates/excerpt omitted"));
    assert_eq!(
        rendered.truncation.unwrap().resource,
        "diagnostic render source scan bytes"
    );
    for limits in [
        DiagnosticRenderLimits {
            output_bytes: 0,
            ..DiagnosticRenderLimits::default()
        },
        DiagnosticRenderLimits {
            diagnostics: 0,
            ..DiagnosticRenderLimits::default()
        },
        DiagnosticRenderLimits {
            depth: 0,
            ..DiagnosticRenderLimits::default()
        },
    ] {
        let rendered = render(
            &Diagnostic::new(BWErr::ArithmeticError("divide by zero".into())),
            &limits,
        );
        assert!(rendered.truncation.is_some());
        assert!(rendered
            .text
            .starts_with("[BW3002] Arithmetic error: divide by zero"));
        assert!(rendered.text.contains("diagnostic rendering truncated"));
    }
}

#[test]
fn rendering_and_disposal_handle_deep_and_wide_host_trees_without_recursive_walks() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut error = Diagnostic::new(BWErr::NativeError("leaf".into()));
            for _ in 0..10_000 {
                let mut parent = Diagnostic::new(BWErr::NativeError("parent".into()));
                parent.causes.push(error);
                error = parent;
            }
            let bounded = render(&error, &DiagnosticRenderLimits::default());
            assert_eq!(
                bounded.truncation.unwrap().resource,
                "diagnostic render depth"
            );
            assert_eq!(bounded.text.matches("[BW4002]").count(), SUMMARY_RECORDS);
            let raised = DiagnosticRenderLimits {
                output_bytes: usize::MAX,
                diagnostics: usize::MAX,
                depth: usize::MAX,
                ..DiagnosticRenderLimits::default()
            };
            let complete = render(&error, &raised);
            assert!(complete.truncation.is_none());
            assert_eq!(complete.text.matches("[BW4002]").count(), 10_001);
            error.discard();
            let mut wide = Diagnostic::new(BWErr::NativeError("root".into()));
            for _ in 0..100_000 {
                wide.causes
                    .push(Diagnostic::new(BWErr::OutputError("cause".into())));
            }
            let bounded = render(
                &wide,
                &DiagnosticRenderLimits {
                    diagnostics: 1,
                    ..DiagnosticRenderLimits::default()
                },
            );
            assert!(bounded.text.contains("99999 additional direct causes"));
            assert_eq!(bounded.text.matches("while handling:").count(), 1);
            wide.discard();
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn counters_reject_overflow_atomically_and_summaries_bound_every_unicode_preview() {
    let mut counter = Counter {
        bytes: usize::MAX,
        maximum: usize::MAX,
    };
    assert!(counter.write_str("x").is_err());
    assert_eq!(counter.bytes, usize::MAX);
    let mut used = usize::MAX;
    assert!(charge(&mut used, 1, usize::MAX, "test").is_err());
    assert_eq!(used, usize::MAX);
    let span = Span::input_origin("🙂".repeat(100_000));
    let mut error = Diagnostic::new(BWErr::NativeError("é".repeat(100_000))).at(&span);
    for _ in 0..12 {
        let mut next = Diagnostic::new(BWErr::NativeError("🙂".repeat(100_000))).at(&span);
        next.causes.push(error);
        error = next;
    }
    let rendered = render(
        &error,
        &DiagnosticRenderLimits {
            output_bytes: 0,
            ..DiagnosticRenderLimits::default()
        },
    );
    assert!(rendered.text.len() <= RENDER_SUMMARY_BYTES);
    assert_eq!(
        rendered.text.matches("filename shortened: true").count(),
        SUMMARY_RECORDS
    );
    assert_eq!(
        rendered.text.matches("detail shortened: true").count(),
        SUMMARY_RECORDS
    );
    assert!(rendered.text.ends_with("prior diagnostic omissions: false"));
    error.discard();
}

#[test]
fn rendering_propagates_destination_failure_for_full_and_summary_output() {
    struct Reject;
    impl Write for Reject {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }
    assert!(display(&sample(), &mut Reject).is_err());
    let error = Diagnostic::new(BWErr::NativeError("x".repeat(100_000)));
    assert!(display(&error, &mut Reject).is_err());
}

#[test]
fn rendering_preserves_prior_filename_truncation_and_empty_or_whitespace_span_layout() {
    let error = Diagnostic::new(BWErr::NativeError("failed".into()))
        .at(&Span::input_origin("é".repeat(1000)));
    let error = crate::core::diagnostic::DiagnosticLimits {
        source_bytes: 0,
        ..Default::default()
    }
    .admit(error)
    .unwrap_err();
    let rendered = render(
        &error,
        &DiagnosticRenderLimits {
            output_bytes: 0,
            ..Default::default()
        },
    );
    assert!(rendered.text.starts_with("[BW8001]"));
    assert!(rendered.text.contains("while handling: [BW4002]"));
    assert!(rendered.text.contains("filename shortened: true"));
    assert!(rendered.text.contains("prior diagnostic omissions: true"));
    for span in [
        Span::input_origin("empty"),
        Span::source_prefix("whitespace", "\t\r\n\u{2003}", 0),
    ] {
        let error = Diagnostic::new(BWErr::NativeError("failed".into())).at(&span);
        let mut expected = String::new();
        legacy(&error, &mut expected);
        let rendered = render(
            &error,
            &DiagnosticRenderLimits {
                source_scan_bytes: 10 * span.end(),
                ..Default::default()
            },
        );
        assert_eq!(rendered.text, expected);
        assert!(rendered.truncation.is_none());
    }
}

#[test]
fn repair_guidance_obeys_exact_zero_and_default_output_bounds_without_changing_category() {
    let error = Diagnostic::new(BWErr::undefined_variable("café".into()));
    let expected = Help(&error.error).to_string();
    for output_bytes in [0, expected.len() - 1, expected.len()] {
        let rendered = error.help_with_limit(output_bytes);
        if output_bytes == expected.len() {
            assert_eq!(rendered.text, expected);
            assert!(rendered.truncation.is_none());
        } else {
            assert!(rendered
                .text
                .starts_with("[BW2001] repair guidance truncated:"));
            assert_eq!(rendered.truncation.unwrap().limit, output_bytes);
            assert!(rendered.text.len() <= RENDER_SUMMARY_BYTES);
        }
    }
    let error = BWErr::undefined_variable("🙂".repeat(100_000));
    assert!(error
        .help()
        .starts_with("[BW2001] repair guidance truncated:"));
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
}
