use super::*;

#[tokio::test]
async fn worker_join_construction_preserves_exact_details_and_every_context_boundary() {
    let join = tokio::spawn(async { std::panic::panic_any("é\njoin failure".to_owned()) })
        .await
        .unwrap_err();
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Read").unwrap(), |_, _| async {
            Ok(Literal::None)
        })
        .unwrap();
    let baseline = operation.worker_error(&join);
    let BWErr::AsyncRuntime(detail) = baseline.error.as_ref() else {
        panic!("worker error")
    };
    assert_eq!(
        detail,
        &format!("Blocking worker ended unexpectedly: {join}")
    );
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    for dimension in 0..5 {
        let mut limits = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        match dimension {
            0 => {}
            1 => limits.text_bytes -= 1,
            2 => limits.source_bytes -= 1,
            3 => limits.diagnostics = 0,
            _ => limits.depth = 0,
        }
        let limited = operation
            .clone()
            .with_diagnostic_limits(limits.clone())
            .unwrap();
        let error = limited.worker_error(&join);
        if dimension == 0 {
            assert_eq!(
                error.to_value().to_string(),
                baseline.to_value().to_string()
            );
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::AsyncRuntime);
            assert!(error.causes[0].span.is_none());
            let again = admit_error(&limits, &operation.signature, error, false, true);
            assert_eq!(again.causes[0].code(), DiagnosticCode::AsyncRuntime);
            assert!(!again.causes[0].omissions.as_ref().unwrap().prior_summary);
        }
    }
}

#[tokio::test]
async fn cleanup_join_construction_admits_primary_and_new_cause_together_and_preserves_stop() {
    let join = tokio::spawn(async { panic!("cleanup é") })
        .await
        .unwrap_err();
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Read").unwrap(), |_, _| async {
            Ok(Literal::None)
        })
        .unwrap();
    for stop in 0..3 {
        let primary = || {
            Diagnostic::new(match stop {
                0 => BWErr::Cancelled("cancelled".into()),
                1 => BWErr::Timeout("expired".into()),
                _ => BWErr::AsyncRuntime("time driver unavailable".into()),
            })
        };
        let baseline = operation.worker_cleanup_error(primary(), &join);
        let BWErr::AsyncRuntime(detail) = baseline.causes[0].error.as_ref() else {
            panic!("worker cause")
        };
        assert_eq!(
            detail,
            &format!("Blocking worker ended during cleanup: {join}")
        );
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        let child_size = DiagnosticLimits::default()
            .check(&baseline.causes[0])
            .unwrap();
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
                0 => {}
                1 => limits.text_bytes -= 1,
                2 => limits.source_bytes -= 1,
                3 => limits.diagnostics = 1,
                4 => limits.depth = 1,
                _ => limits.text_bytes = child_size.text_bytes,
            }
            let limited = operation
                .clone()
                .with_diagnostic_limits(limits.clone())
                .unwrap();
            let error = limited.worker_cleanup_error(primary(), &join);
            if dimension == 0 {
                assert_eq!(
                    error.to_value().to_string(),
                    baseline.to_value().to_string()
                );
            } else {
                let (summary, limit) = if stop == 2 {
                    (&error.causes[0], &error)
                } else {
                    (&error, &error.causes[0])
                };
                assert_eq!(summary.code(), primary().code());
                assert_eq!(limit.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(summary.omissions.as_ref().unwrap().direct_causes, 1);
                assert!(summary.span.is_none());
                let code = error.code();
                let again = admit_error(&limits, &operation.signature, error, stop != 2, true);
                assert_eq!(again.code(), code);
                let summary = if stop == 2 { &again.causes[0] } else { &again };
                assert!(!summary.omissions.as_ref().unwrap().prior_summary);
            }
        }
    }
}

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
