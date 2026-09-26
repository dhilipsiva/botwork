use super::*;
use crate::core::diagnostic::DiagnosticCode;

#[test]
fn valid_names_never_format_errors_and_invalid_names_invoke_the_reporter_once() {
    validate_name_with("origin", "é", |_| panic!("valid identifier was formatted")).unwrap();
    let mut calls = 0;
    let error = validate_name_with("origin", "true", |message| {
        calls += 1;
        assert_eq!(
            message.to_string(),
            "origin: variable \"true\": expected an exact DSL identifier"
        );
        Diagnostic::new(BWErr::InputError("reported".into()))
    })
    .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(error.code(), DiagnosticCode::Input);
}

#[test]
fn input_identifier_validation_preserves_the_legacy_parser_byte_ceiling() {
    let mut name = "x".repeat(crate::core::syntax_limits::DEFAULT_SOURCE_BYTES);
    validate_name("origin", &name).unwrap();
    name.push('x');
    let error = validate_name("origin", &name).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Input);
    assert!(error.omissions.is_none());
}

#[test]
fn standalone_name_validation_admits_origin_and_name_under_default_diagnostic_limits() {
    let limits = DiagnosticLimits::default();
    let suffix = ": variable \"!\": expected an exact DSL identifier";
    let mut origin = "x".repeat(limits.text_bytes - "source".len() - suffix.len());
    let error = validate_name(&origin, "!").unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Input);
    assert_eq!(limits.check(&error).unwrap().text_bytes, limits.text_bytes);
    drop(error);
    origin.push('x');
    let error = validate_name(&origin, "!").unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Input);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
    assert!(error.causes[0].span.is_none());
}
