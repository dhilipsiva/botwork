use botwork::core::{
    ast::Program,
    diagnostic::{
        Diagnostic, DiagnosticCode, DiagnosticLimits, DiagnosticValueLimits, SUMMARY_DETAIL_BYTES,
        SUMMARY_SOURCE_NAME_BYTES,
    },
    grammar::{BWErr, Literal},
};
use std::sync::Arc;

fn zero() -> DiagnosticLimits {
    DiagnosticLimits {
        diagnostics: 0,
        ..DiagnosticLimits::default()
    }
}
fn map(value: &Literal) -> &std::collections::HashMap<String, Literal> {
    let Literal::Map(map) = value else {
        panic!("metadata map")
    };
    map
}

#[test]
fn rejection_preserves_original_category_and_exact_byte_coordinates_without_source_ownership() {
    let name = "é".repeat(512);
    let program = Program::parse(&name, "# தமிழ்\r\n|x| = |1|").unwrap();
    let owner = Arc::downgrade(&program.source);
    let span = &program.statements[0].span;
    let coordinates = (span.start().to_string(), span.end().to_string());
    let error = Diagnostic::new(BWErr::undefined_variable("🙂".repeat(1024))).at(span);
    drop(program);
    let error = zero().admit(error).unwrap_err();
    assert!(owner.upgrade().is_none());
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    let summary = &error.causes[0];
    assert_eq!(summary.code(), DiagnosticCode::UndefinedVariable);
    let value = summary
        .to_value_with_limits(&DiagnosticValueLimits::default())
        .unwrap();
    let fields = map(&value);
    assert_eq!(fields.len(), 9);
    assert!(matches!(fields["source"], Literal::None));
    let omissions = map(&fields["omissions"]);
    assert_eq!(omissions["detail_fields"].to_string(), "1");
    let source = map(&omissions["source"]);
    assert_eq!(source["start_byte"].to_string(), coordinates.0);
    assert_eq!(source["end_byte"].to_string(), coordinates.1);
    assert!(matches!(source["file_truncated"], Literal::Bool(true)));
    assert!(source["file"].to_string().len() <= SUMMARY_SOURCE_NAME_BYTES);
    assert!(map(&fields["details"])["name"].to_string().len() <= SUMMARY_DETAIL_BYTES);
    assert!(error.to_string().contains("diagnostic metadata omitted"));
}

#[test]
fn accepted_diagnostics_keep_the_original_schema_and_allocation_identity() {
    let error = Diagnostic::new(BWErr::NativeError("offline".into()));
    let identity = Arc::downgrade(&error.error);
    let original = identity.as_ptr();
    let error = DiagnosticLimits::default().admit(error).unwrap();
    assert_eq!(Arc::as_ptr(&error.error), original);
    assert_eq!(map(&error.to_value()).len(), 8);
    assert!(error.omissions.is_none());
    error.discard();
    assert!(identity.upgrade().is_none());
}

#[test]
fn emergency_evidence_is_independent_of_zero_quotas_and_never_retains_deep_host_trees() {
    let mut error = Diagnostic::new(BWErr::NativeError("leaf".into()));
    let leaf = Arc::downgrade(&error.error);
    for _ in 0..100_000 {
        let mut outer = Diagnostic::new(BWErr::Cancelled("cancelled".into()));
        outer.causes.push(error);
        error = outer;
    }
    let limits = DiagnosticLimits {
        diagnostics: 0,
        depth: 0,
        call_frames: 0,
        related_locations: 0,
        text_bytes: 0,
        source_bytes: 0,
    };
    let rejected = limits.admit(error).unwrap_err();
    assert!(leaf.upgrade().is_none());
    assert_eq!(rejected.causes[0].code(), DiagnosticCode::Cancelled);
    assert_eq!(
        rejected.causes[0].omissions.as_ref().unwrap().direct_causes,
        1
    );
    let measured = DiagnosticLimits::default().check(&rejected).unwrap();
    assert_eq!(measured.diagnostics, 2);
    assert_eq!(measured.depth, 2);
    assert_eq!(measured.source_bytes, 0);
    assert!(measured.text_bytes < 128);
}
