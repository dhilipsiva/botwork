use super::*;
use crate::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, SUMMARY_DETAIL_BYTES, SUMMARY_SOURCE_NAME_BYTES},
};

#[test]
fn exact_borrowed_details_match_full_diagnostics_and_preserve_their_call_order() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let frames = [
        CallFrame {
            signature: "outer".into(),
            call_site: span.clone(),
            definition_site: None,
        },
        CallFrame {
            signature: "inner".into(),
            call_site: span.clone(),
            definition_site: Some(span.clone()),
        },
    ];
    for category in [
        BWErr::VariableNotDefined,
        BWErr::StatementNotDefined,
        BWErr::NativePanic,
    ] {
        for expression in [false, true] {
            let full = Diagnostic::new(category("é".into()));
            let full = if expression {
                full.at_expression(span)
            } else {
                full.at(span)
            }
            .capture_stack(frames.iter());
            let size = DiagnosticLimits::default().check(&full).unwrap();
            let limits = DiagnosticLimits {
                diagnostics: size.diagnostics,
                depth: size.depth,
                call_frames: size.call_frames,
                related_locations: 0,
                text_bytes: size.text_bytes,
                source_bytes: size.source_bytes,
            };
            let built =
                limits.borrowed_detail(category, "é", Some(span), expression, frames.iter());
            assert_eq!(built.code(), full.code());
            assert_eq!(built.to_value().to_string(), full.to_value().to_string());
            assert_eq!(limits.check(&built).unwrap(), size);
            assert_eq!(built.call_stack[0].signature, "inner");
        }
    }
}

#[test]
fn every_prospective_quota_rejects_before_context_capture_with_exact_omission_counts() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let frames = [CallFrame {
        signature: "read".into(),
        call_site: span.clone(),
        definition_site: None,
    }];
    for field in 0..5 {
        let mut limits = DiagnosticLimits::default();
        match field {
            0 => limits.diagnostics = 0,
            1 => limits.depth = 0,
            2 => limits.call_frames = 0,
            3 => limits.text_bytes = "source".len() + "read".len(),
            _ => limits.source_bytes = 0,
        }
        let error = limits.borrowed_detail(
            BWErr::NativePanic,
            "panic",
            Some(span),
            false,
            frames.iter(),
        );
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert!(error.is_emergency());
        let summary = &error.causes[0];
        assert_eq!(summary.code(), DiagnosticCode::NativePanic);
        let omitted = summary.omissions.as_ref().unwrap();
        assert_eq!(omitted.detail_fields, 0);
        assert_eq!(omitted.call_frames, 1);
        assert_eq!(omitted.source.as_ref().unwrap().start_byte, 0);
        assert!(summary.span.is_none());
    }
}

#[test]
fn rejected_large_unicode_details_and_source_names_have_explicit_bounded_evidence() {
    let program = Program::parse(&"é".repeat(4096), "|x| = |1|").unwrap();
    let source = Arc::downgrade(&program.source);
    let error = DiagnosticLimits {
        text_bytes: 0,
        ..DiagnosticLimits::default()
    }
    .borrowed_detail(
        BWErr::VariableNotDefined,
        &"🦀".repeat(4096),
        Some(&program.statements[0].span),
        true,
        std::iter::empty(),
    );
    drop(program);
    assert!(source.upgrade().is_none());
    let summary = &error.causes[0];
    let BWErr::VariableNotDefined(detail) = summary.error.as_ref() else {
        panic!("category")
    };
    assert!(detail.len() <= SUMMARY_DETAIL_BYTES && detail.ends_with("…[truncated]"));
    let omitted = summary.omissions.as_ref().unwrap();
    assert_eq!(omitted.detail_fields, 1);
    assert_eq!(summary.label, "expression");
    assert!(omitted.source.as_ref().unwrap().file.len() <= SUMMARY_SOURCE_NAME_BYTES);
    assert!(omitted.source.as_ref().unwrap().file_truncated);
}

#[test]
fn empty_details_and_invalid_configuration_preserve_category_without_source_owners() {
    let exact = DiagnosticLimits {
        diagnostics: 1,
        depth: 1,
        call_frames: 0,
        related_locations: 0,
        text_bytes: 6,
        source_bytes: 0,
    };
    assert_eq!(
        exact
            .borrowed_detail(BWErr::NativePanic, "", None, false, std::iter::empty())
            .code(),
        DiagnosticCode::NativePanic
    );
    let error = DiagnosticLimits { depth: 65, ..exact }.borrowed_detail(
        BWErr::NativePanic,
        "panic",
        None,
        false,
        std::iter::empty(),
    );
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    assert_eq!(error.causes[0].code(), DiagnosticCode::NativePanic);
    assert!(error.causes[0].omissions.as_ref().unwrap().source.is_none());
}
