use botwork::core::{
    ast::Program,
    diagnostic::{
        CallFrame, Diagnostic, DiagnosticCode, DiagnosticLimits, DiagnosticRenderLimits,
        RENDER_SUMMARY_BYTES,
    },
    eval::{evaluate_program_detailed, Context},
    grammar::BWErr,
};
use std::sync::Arc;

#[path = "support/cli_harness.rs"]
mod cli_harness;

#[test]
fn rendering_preserves_original_categories_identity_sources_and_context_reuse() {
    let program = Program::parse(
        "handlers-é",
        "Outer { Try { |x| = |1 / 0| } Catch { |x| = |missing| } }\nOuter",
    )
    .unwrap();
    let source = Arc::downgrade(&program.source);
    let mut context = Context::default();
    let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
    let identity = Arc::clone(&error.error);
    let size = DiagnosticLimits::default().check(&error).unwrap();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
    let result = error.render_with_limits(&DiagnosticRenderLimits {
        output_bytes: 0,
        ..Default::default()
    });
    assert!(result.text.starts_with("[BW2001]"));
    assert!(result.text.contains("while handling: [BW3002]"));
    assert!(result.text.contains("1 call frames"));
    assert_eq!(DiagnosticLimits::default().check(&error).unwrap(), size);
    assert!(Arc::ptr_eq(&identity, &error.error));
    context.checkpoint().unwrap();
    evaluate_program_detailed(&Program::parse("next", "|ok| = |7|").unwrap(), &mut context)
        .unwrap();
    drop((context, program, error));
    assert!(source.upgrade().is_none()); // Rendered strings retain no source owners.
    assert!(result.truncation.is_some());
}

#[test]
fn default_display_bounds_large_details_help_filenames_and_repeated_call_locations() {
    for branch in 0..3 {
        let name = if branch == 1 {
            "é".repeat(100_000)
        } else {
            "source".into()
        };
        let program = Program::parse(&name, "|x| = |1|").unwrap();
        let span = &program.statements[0].span;
        let mut error = Diagnostic::new(if branch == 0 {
            BWErr::VariableNotDefined("🙂".repeat(100_000))
        } else {
            BWErr::NativeError("reason".into())
        })
        .at(span);
        if branch == 2 {
            for _ in 0..1000 {
                error.call_stack.push(CallFrame {
                    signature: "read".into(),
                    call_site: span.clone(),
                    definition_site: Some(span.clone()),
                });
            }
        }
        let rendered = error.to_string();
        assert!(
            rendered.len()
                <= DiagnosticRenderLimits::default()
                    .output_bytes
                    .max(RENDER_SUMMARY_BYTES)
        );
        assert!(rendered.starts_with(&format!("[{}]", error.code())));
        assert!(rendered.contains("diagnostic rendering truncated"));
        assert!(rendered.contains("coordinates/excerpt omitted"));
        assert_eq!(error.span.as_ref().unwrap(), span);
    }
}

#[test]
fn every_utf8_output_boundary_is_exact_or_explicitly_summarized() {
    let error = Diagnostic::new(BWErr::NativeError("cafe\u{301} é 🙂 தமிழ்".into()));
    let full = error.to_string();
    for output_bytes in 0..=full.len() + 1 {
        let rendered = error.render_with_limits(&DiagnosticRenderLimits {
            output_bytes,
            ..Default::default()
        });
        if output_bytes >= full.len() {
            assert_eq!(rendered.text, full);
            assert!(rendered.truncation.is_none());
        } else {
            assert_eq!(rendered.truncation.unwrap().limit, output_bytes);
            assert!(rendered
                .text
                .starts_with("[BW4002] Native operation failed:"));
            assert!(rendered.text.len() <= RENDER_SUMMARY_BYTES);
        }
    }
}

#[test]
fn source_work_is_counted_per_occurrence_before_coordinate_and_excerpt_scans() {
    let program = Program::parse(
        "large-offset",
        &format!("# {}\n|x| = |1|", "é".repeat(100_000)),
    )
    .unwrap();
    let span = &program.statements[0].span;
    let primary_work = 6 * (span.start() + span.end()) + 4 * span.text().len();
    let mut error = Diagnostic::new(BWErr::NativeError("failed".into())).at(span);
    error.call_stack.push(CallFrame {
        signature: "read".into(),
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    let total = primary_work + 12 * span.start();
    let exact = error.render_with_limits(&DiagnosticRenderLimits {
        source_scan_bytes: total,
        ..Default::default()
    });
    assert!(exact.truncation.is_none());
    assert!(exact.text.starts_with("large-offset:2:1"));
    let rejected = error.render_with_limits(&DiagnosticRenderLimits {
        source_scan_bytes: total - 1,
        ..Default::default()
    });
    assert_eq!(
        rejected.truncation.unwrap().resource,
        "diagnostic render source scan bytes"
    );
    assert!(rejected
        .text
        .contains(&format!("bytes {}..{}", span.start(), span.end())));
    assert!(rejected.text.contains("coordinates/excerpt omitted"));
}

#[test]
fn cli_bounds_oversized_excerpts_while_preserving_failure_status_prior_output_and_cause_codes() {
    let harness = cli_harness::Harness::new();
    let source = format!(
        "Log |\"before\"|\nTry {{ |x| = |1 / 0| }} Catch {{ If |\"{}\"| {{}} }}",
        "é".repeat(64 * 1024)
    );
    let output = harness
        .run(
            "bounded-rendering",
            &source,
            std::time::Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.len() <= RENDER_SUMMARY_BYTES + 1);
    assert!(diagnostic.starts_with("[BW3003]"));
    assert!(diagnostic.contains("while handling: [BW3002]"));
    assert!(diagnostic.contains("diagnostic rendering truncated"));
}
