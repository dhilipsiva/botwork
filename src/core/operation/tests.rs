use super::*;

#[test]
fn callback_rejection_preserves_category_and_internal_readmission_preserves_identity() {
    let signature = StatementSignature::native("Fail").unwrap();
    let limits = DiagnosticLimits {
        diagnostics: 0,
        ..DiagnosticLimits::default()
    };
    let error = admit_error(
        &limits,
        &signature,
        BWErr::NativeError("reason".into()).into(),
        false,
        false,
    );
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    let identity = error.error.clone();
    let next = admit_error(&limits, &signature, error, false, true);
    assert!(Arc::ptr_eq(&identity, &next.error));
    assert!(next.is_emergency());
}

#[test]
fn observed_stops_keep_priority_but_callback_reported_stop_codes_do_not_bypass_quotas() {
    let signature = StatementSignature::native("Stop").unwrap();
    let limits = DiagnosticLimits {
        source_bytes: 0,
        ..DiagnosticLimits::default()
    };
    for timeout in [false, true] {
        for observed in [false, true] {
            let original = if timeout {
                BWErr::Timeout("reason".into())
            } else {
                BWErr::Cancelled("reason".into())
            };
            let code = original.code();
            let error = admit_error(&limits, &signature, original.into(), observed, false);
            assert_eq!(
                error.code(),
                if observed {
                    code
                } else {
                    DiagnosticCode::ResourceLimit
                }
            );
            assert_eq!(
                error.causes[0].code(),
                if observed {
                    DiagnosticCode::ResourceLimit
                } else {
                    code
                }
            );
            assert!(error.is_emergency());
        }
    }
}

#[test]
fn accepted_operation_errors_retain_full_metadata_and_share_original_identity() {
    let signature = StatementSignature::native("Fail").unwrap();
    let original = Diagnostic::new(BWErr::NativeError("reason".into()));
    let identity = original.error.clone();
    let accepted = admit_error(
        &DiagnosticLimits::default(),
        &signature,
        original,
        false,
        false,
    );
    assert!(Arc::ptr_eq(&identity, &accepted.error));
    assert_eq!(accepted.span.as_ref().unwrap().text(), "Fail");
    assert!(accepted.omissions.is_none());
}

#[test]
fn panic_construction_keeps_full_admitted_details_or_bounded_original_evidence() {
    let signature = StatementSignature::native("Fail").unwrap();
    for text_bytes in [9, 10] {
        let limits = DiagnosticLimits {
            text_bytes,
            ..DiagnosticLimits::default()
        };
        let error = panic_error(&limits, &signature);
        if text_bytes == 10 {
            assert_eq!(error.code(), DiagnosticCode::NativePanic);
            assert_eq!(error.span.as_ref().unwrap().text(), "Fail");
            assert!(error.omissions.is_none());
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::NativePanic);
            assert!(error.is_emergency());
            let error = admit_error(&limits, &signature, error, false, true);
            assert_eq!(error.causes[0].code(), DiagnosticCode::NativePanic);
            assert!(!error.causes[0].omissions.as_ref().unwrap().prior_summary);
        }
    }
}
