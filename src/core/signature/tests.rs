use super::*;
use std::sync::Arc;

#[test]
fn builder_unknown_parameter_messages_fit_exact_default_bytes_and_reject_one_more() {
    let header = "Read |value|";
    let limits = DiagnosticLimits::default();
    let overhead = "source".len() + format!("Unknown parameter `` in `{header}`").len();
    let mut name = "x".repeat(limits.text_bytes - overhead);
    let error = StatementSignature::native(header)
        .unwrap()
        .parameter(&name, ValueKind::Int)
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Signature);
    assert_eq!(limits.check(&error).unwrap().text_bytes, limits.text_bytes);
    let BWErr::SignatureError(message) = error.error.as_ref() else {
        panic!("signature")
    };
    assert!(
        message.starts_with("Unknown parameter `")
            && message.ends_with(&format!("` in `{header}`"))
    );
    assert!(error.omissions.is_none());
    assert_eq!(error.span.as_ref().unwrap().text(), header);
    drop(error);
    name.push('x');
    let error = StatementSignature::native(header)
        .unwrap()
        .parameter(&name, ValueKind::Int)
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Signature);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
    assert!(error.causes[0].span.is_none());
}

#[test]
fn builder_rejection_admits_complete_header_source_and_releases_unadmitted_owners() {
    let header = "Read |value|";
    let maximum = DiagnosticLimits::default().source_bytes;
    for extra in [0, 1] {
        let origin = "x".repeat(maximum - header.len() + extra);
        for branch in 0..3 {
            let signature = StatementSignature::native_at(&origin, header).unwrap();
            let source = Arc::downgrade(signature.header.source());
            let error = match branch {
                0 => signature.parameter("missing", ValueKind::Int),
                1 => signature.documents_error(DiagnosticCode::Native, " "),
                _ => signature
                    .documents_error(DiagnosticCode::Native, "first")
                    .unwrap()
                    .documents_error(DiagnosticCode::Native, "second"),
            }
            .unwrap_err();
            if extra == 0 {
                assert_eq!(error.code(), DiagnosticCode::Signature);
                assert!(source.upgrade().is_some());
                assert_eq!(
                    DiagnosticLimits::default()
                        .check(&error)
                        .unwrap()
                        .source_bytes,
                    maximum
                );
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert!(source.upgrade().is_none());
                let original = &error.causes[0];
                assert_eq!(original.code(), DiagnosticCode::Signature);
                let omitted = original.omissions.as_ref().unwrap();
                assert_eq!(omitted.detail_fields, 0);
                let location = omitted.source.as_ref().unwrap();
                assert!(location.file_truncated && location.file.len() <= 256);
                assert_eq!((location.start_byte, location.end_byte), (0, header.len()));
                assert!(original.span.is_none());
            }
            drop(error);
            assert!(source.upgrade().is_none());
        }
    }
}

#[test]
fn unknown_parameter_errors_do_not_evaluate_host_kind_conversions() {
    use std::cell::Cell;
    struct Kinds<'a>(&'a Cell<usize>);
    impl From<Kinds<'_>> for ValueKinds {
        fn from(value: Kinds<'_>) -> Self {
            value.0.set(value.0.get() + 1);
            ValueKind::Int.into()
        }
    }
    let conversions = Cell::new(0);
    let error = StatementSignature::native("Read |value|")
        .unwrap()
        .parameter("Value", Kinds(&conversions))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Signature);
    assert_eq!(conversions.get(), 0);
    let signature = StatementSignature::native("Read |value|")
        .unwrap()
        .parameter("value", Kinds(&conversions))
        .unwrap();
    assert_eq!(conversions.get(), 1);
    assert_eq!(signature.parameters()[0].accepted, ValueKind::Int.into());
}
