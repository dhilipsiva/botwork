use super::*;
use crate::core::{
    ast::Program,
    diagnostic::{CallFrame, DiagnosticCode, DiagnosticValueLimits, RelatedLocation},
};
use std::sync::Arc;

fn zero() -> DiagnosticLimits {
    DiagnosticLimits {
        diagnostics: 0,
        ..DiagnosticLimits::default()
    }
}

#[test]
fn successful_admission_moves_full_identity_context_and_sources_without_truncation() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let diagnostic =
        Diagnostic::new(BWErr::NativeError("reason".into())).at(&program.statements[0].span);
    let identity = diagnostic.error.clone();
    let admitted = DiagnosticLimits::default().admit(diagnostic).unwrap();
    assert!(Arc::ptr_eq(&identity, &admitted.error));
    assert!(Arc::ptr_eq(
        &program.source,
        admitted.span.unwrap().source()
    ));
    assert!(admitted.omissions.is_none());
}

#[test]
fn rejected_source_and_context_are_released_and_root_byte_evidence_is_explicit() {
    let program = Program::parse(&"é".repeat(512), "#🙂\n|x| = |1|").unwrap();
    let owner = Arc::downgrade(&program.source);
    let span = program.statements[0].span.clone();
    let mut original = Diagnostic::new(BWErr::NativeError("λ".repeat(512))).at(&span);
    original.label = "host label";
    original.call_stack.push(CallFrame {
        signature: "call".into(),
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    original.related.push(RelatedLocation {
        message: "prior".into(),
        span: span.clone(),
    });
    original
        .causes
        .push(Diagnostic::new(BWErr::Timeout("first".into())));
    let expected = (span.start(), span.end());
    drop(span);
    drop(program);
    let rejected = zero().admit(original).unwrap_err();
    assert!(owner.upgrade().is_none());
    assert_eq!(rejected.code(), DiagnosticCode::ResourceLimit);
    let summary = &rejected.causes[0];
    assert_eq!(summary.code(), DiagnosticCode::Native);
    assert!(
        summary.span.is_none()
            && summary.call_stack.is_empty()
            && summary.related.is_empty()
            && summary.causes.is_empty()
    );
    let omitted = summary.omissions.as_ref().unwrap();
    assert_eq!(
        (
            omitted.detail_fields,
            omitted.call_frames,
            omitted.related_locations,
            omitted.direct_causes
        ),
        (1, 1, 1, 1)
    );
    assert!(omitted.label);
    assert!(!omitted.prior_summary);
    let source = omitted.source.as_ref().unwrap();
    assert!(source.file_truncated);
    assert!(source.file.len() <= SUMMARY_SOURCE_NAME_BYTES);
    assert_eq!((source.start_byte, source.end_byte), expected);
    let rendered = rejected.to_string();
    assert!(rendered.contains("diagnostic metadata omitted"));
    assert!(rendered.contains("filename shortened: true"));
    assert!(rendered.len() < 2048);
}

#[test]
fn prefix_cap_preserves_utf8_and_marks_every_shortened_detail_field() {
    for text in ["a".repeat(256), "é".repeat(129), "🙂".repeat(100)] {
        let (shortened, truncated) = prefix(&text, SUMMARY_DETAIL_BYTES);
        assert!(shortened.len() <= SUMMARY_DETAIL_BYTES);
        assert_eq!(truncated, text.len() > SUMMARY_DETAIL_BYTES);
        if truncated {
            assert!(shortened.ends_with(TRUNCATED));
        } else {
            assert_eq!(shortened, text);
        }
    }
    let error = BWErr::DuplicateParameter {
        name: "a".repeat(512),
        original: "b".repeat(512),
        duplicate: "c".repeat(512),
    };
    let rejected = zero().admit(error.into()).unwrap_err();
    assert_eq!(
        rejected.causes[0].omissions.as_ref().unwrap().detail_fields,
        3
    );
    let size = DiagnosticLimits::default().check(&rejected).unwrap();
    assert!(size.text_bytes < 1024);
    assert_eq!(size.source_bytes, 0);
}

#[test]
fn rejection_of_existing_summary_records_loss_and_resource_identifiers_are_bounded() {
    const LARGE_RESOURCE: &str = concat!(
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        "x"
    );
    let first = zero()
        .admit(
            BWErr::ResourceLimit {
                resource: LARGE_RESOURCE,
                limit: u64::MAX,
            }
            .into(),
        )
        .unwrap_err();
    assert_eq!(first.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert_eq!(first.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
    let mut first = first;
    let summary = first.causes.pop().unwrap();
    let repeated = zero().admit(summary).unwrap_err();
    assert!(repeated.causes[0].omissions.as_ref().unwrap().prior_summary);
    assert!(repeated.causes[0]
        .to_string()
        .contains("prior summary omitted: true"));
}

#[test]
fn omission_metadata_has_exact_value_metrics_and_checked_clone_independence() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let original =
        Diagnostic::new(BWErr::ArithmeticError("zero".into())).at(&program.statements[0].span);
    let rejected = zero().admit(original).unwrap_err();
    let limits = DiagnosticValueLimits::default();
    let value = rejected.to_value_with_limits(&limits).unwrap();
    assert_eq!(
        limits.values.check(&value).unwrap(),
        rejected.value_size_with_limits(&limits).unwrap()
    );
    let mut copy = rejected
        .try_clone_with_limits(&DiagnosticLimits::default())
        .unwrap();
    copy.causes[0]
        .omissions
        .as_mut()
        .unwrap()
        .source
        .as_mut()
        .unwrap()
        .file
        .clear();
    assert_eq!(
        rejected.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .file,
        "source"
    );
    let own = DiagnosticLimits::default().check(&rejected).unwrap();
    let copied = DiagnosticLimits::default().check(&copy).unwrap();
    assert_eq!(own.text_bytes - copied.text_bytes, "source".len());
}

#[test]
fn owned_deep_rejection_and_invalid_configuration_dispose_original_iteratively() {
    let mut diagnostic = Diagnostic::new(BWErr::NativeError("leaf".into()));
    let leaf = Arc::downgrade(&diagnostic.error);
    for _ in 0..100_000 {
        let mut parent = Diagnostic::new(BWErr::Cancelled("parent".into()));
        parent.causes.push(diagnostic);
        diagnostic = parent;
    }
    let error = DiagnosticLimits {
        depth: 65,
        ..DiagnosticLimits::default()
    }
    .admit(diagnostic)
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Cancelled);
    assert!(leaf.upgrade().is_none());
}

#[test]
fn emergency_evidence_stays_bounded_when_unwinding_adds_context_and_causes() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let frame = CallFrame {
        signature: "x".repeat(4096),
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    };
    let rejected = zero()
        .admit(BWErr::NativeError("reason".into()).into())
        .unwrap_err();
    let identity = rejected.error.clone();
    let rejected = rejected
        .at(span)
        .at_expression(span)
        .capture_stack(&[frame])
        .with_related(&"x".repeat(4096), span)
        .while_handling(BWErr::ArithmeticError("prior".into()).into());
    assert!(rejected.is_emergency());
    assert!(Arc::ptr_eq(&identity, &rejected.error));
    assert!(
        rejected.span.is_none() && rejected.call_stack.is_empty() && rejected.related.is_empty()
    );
    let omitted = rejected.causes[0].omissions.as_ref().unwrap();
    assert_eq!(omitted.related_locations, 1);
    assert_eq!(omitted.direct_causes, 1);
    assert_eq!(omitted.call_frames, 0); // The original snapshot count is stable during unwind.
}

#[test]
fn control_primary_emergency_evidence_uses_its_own_omission_record() {
    let mut rejected = zero()
        .admit(BWErr::Cancelled("stopped".into()).into())
        .unwrap_err();
    let mut original = rejected.causes.pop().unwrap();
    original.causes.push(rejected);
    assert!(original.is_emergency());
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let original = original
        .with_related("imported", &program.statements[0].span)
        .while_handling(BWErr::NativeError("prior".into()).into());
    assert!(original.is_emergency());
    assert_eq!(original.code(), DiagnosticCode::Cancelled);
    assert_eq!(original.omissions.as_ref().unwrap().related_locations, 1);
    assert_eq!(original.omissions.as_ref().unwrap().direct_causes, 1);
}
