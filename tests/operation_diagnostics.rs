use botwork::core::{
    ast::Program,
    diagnostic::{
        CallFrame, Diagnostic, DiagnosticCode, DiagnosticLimits, DiagnosticOmissions,
        RelatedLocation,
    },
    grammar::{BWErr, Literal},
    operation::{NativeOperation, OperationControl},
    signature::StatementSignature,
    value_limits::ValueLimits,
};
use std::{
    future::Future,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    task::{Context, Waker},
    time::Duration,
};

fn operation(error: Diagnostic, blocking: bool) -> NativeOperation {
    let error = Mutex::new(Some(error));
    let signature = StatementSignature::native("Fail").unwrap();
    if blocking {
        NativeOperation::blocking(signature, NonZeroUsize::new(1).unwrap(), move |_, _| {
            Err(error.lock().unwrap().take().unwrap())
        })
        .unwrap()
    } else {
        NativeOperation::asynchronous(signature, move |_, _| {
            let error = error.lock().unwrap().take().unwrap();
            async move { Err(error) }
        })
        .unwrap()
    }
}

fn detailed() -> Diagnostic {
    let program = Program::parse("é", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let mut error = Diagnostic::new(BWErr::NativeError("reason".into())).at(span);
    error
        .causes
        .push(BWErr::ArithmeticError("cause".into()).into());
    error.related.push(RelatedLocation {
        message: "related".into(),
        span: span.clone(),
    });
    error.call_stack.push(CallFrame {
        signature: "read".into(),
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    error
}

fn panicking_operation(stage: usize, header: &str, cancel: bool) -> NativeOperation {
    let signature = StatementSignature::native(header).unwrap();
    if stage == 2 {
        NativeOperation::blocking(
            signature,
            NonZeroUsize::new(1).unwrap(),
            move |_, control| {
                if cancel {
                    control.cancel();
                }
                panic!("blocking panic")
            },
        )
        .unwrap()
    } else {
        NativeOperation::asynchronous(signature, move |_, control| {
            if stage == 0 {
                if cancel {
                    control.cancel();
                }
                panic!("factory panic");
            }
            async move {
                if cancel {
                    control.cancel();
                }
                panic!("poll panic")
            }
        })
        .unwrap()
    }
}

#[tokio::test]
async fn all_panic_stages_keep_one_bounded_signature_summary_and_remain_reusable() {
    let header = "x".repeat(65_536);
    for stage in 0..3 {
        let operation = panicking_operation(stage, &header, false)
            .with_diagnostic_limits(DiagnosticLimits {
                text_bytes: 32,
                ..DiagnosticLimits::default()
            })
            .unwrap();
        let parent = OperationControl::default();
        for _ in 0..2 {
            let error = operation.invoke(vec![], parent.clone()).await.unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes.len(), 1);
            let summary = &error.causes[0];
            assert_eq!(summary.code(), DiagnosticCode::NativePanic);
            let omitted = summary.omissions.as_ref().unwrap();
            assert_eq!(omitted.detail_fields, 1);
            assert!(!omitted.prior_summary);
            assert_eq!(omitted.source.as_ref().unwrap().end_byte, header.len());
            assert!(summary.causes.is_empty() && summary.span.is_none());
            assert!(!parent.is_cancelled());
        }
    }
}

#[tokio::test]
async fn every_panic_stage_admits_exact_signature_text_and_header_source_quotas() {
    for stage in 0..3 {
        for fits in [false, true] {
            let limits = DiagnosticLimits {
                text_bytes: 10,
                source_bytes: if fits { 12 } else { 11 },
                ..DiagnosticLimits::default()
            };
            let error = panicking_operation(stage, "Fail", false)
                .with_diagnostic_limits(limits)
                .unwrap()
                .invoke(vec![], OperationControl::default())
                .await
                .unwrap_err();
            if fits {
                assert_eq!(error.code(), DiagnosticCode::NativePanic);
                let BWErr::NativePanic(detail) = error.error.as_ref() else {
                    panic!("category")
                };
                assert_eq!(detail, "fail");
                assert_eq!(error.span.as_ref().unwrap().text(), "Fail");
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::NativePanic);
                assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 0);
            }
        }
    }
}

#[tokio::test]
async fn cancellation_then_panic_keeps_stop_priority_at_every_operation_stage() {
    for stage in 0..3 {
        let parent = OperationControl::default();
        let error = panicking_operation(stage, "Stop", true)
            .with_diagnostic_limits(DiagnosticLimits {
                diagnostics: 0,
                ..DiagnosticLimits::default()
            })
            .unwrap()
            .invoke(vec![], parent.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Cancelled);
        assert!(error.omissions.is_some());
        assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
        assert!(!parent.is_cancelled());
    }
}

#[tokio::test]
async fn formatted_operation_argument_and_return_errors_obey_exact_and_rejected_boundaries() {
    use botwork::core::signature::ValueKind;
    use std::sync::atomic::{AtomicUsize, Ordering};
    for blocking in [false, true] {
        for returning in [false, true] {
            let entries = Arc::new(AtomicUsize::new(0));
            let entered = entries.clone();
            let signature = StatementSignature::native("Read |value|").unwrap();
            let signature = if returning {
                signature.returns(ValueKind::String)
            } else {
                signature.parameter("value", ValueKind::Int).unwrap()
            };
            let original = if blocking {
                NativeOperation::blocking(signature, NonZeroUsize::new(1).unwrap(), move |_, _| {
                    entered.fetch_add(1, Ordering::SeqCst);
                    Ok(Literal::Bool(true))
                })
                .unwrap()
            } else {
                NativeOperation::asynchronous(signature, move |_, _| {
                    entered.fetch_add(1, Ordering::SeqCst);
                    async { Ok(Literal::Bool(true)) }
                })
                .unwrap()
            };
            let baseline = original
                .invoke(vec![Literal::Bool(true)], OperationControl::default())
                .await
                .unwrap_err();
            let size = DiagnosticLimits::default().check(&baseline).unwrap();
            for fits in [false, true] {
                let operation = original
                    .clone()
                    .with_diagnostic_limits(DiagnosticLimits {
                        text_bytes: size.text_bytes - usize::from(!fits),
                        source_bytes: size.source_bytes,
                        ..DiagnosticLimits::default()
                    })
                    .unwrap();
                let parent = OperationControl::default();
                let error = operation
                    .invoke(vec![Literal::Bool(true)], parent.clone())
                    .await
                    .unwrap_err();
                if fits {
                    assert_eq!(
                        error.to_value().to_string(),
                        baseline.to_value().to_string()
                    );
                } else {
                    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                    assert_eq!(error.causes[0].code(), DiagnosticCode::IncompatibleType);
                    assert!(!error.causes[0].omissions.as_ref().unwrap().prior_summary);
                }
                assert!(!parent.is_cancelled());
            }
            assert_eq!(
                entries.load(Ordering::SeqCst),
                if returning { 3 } else { 0 }
            );
        }
    }
}

#[tokio::test]
async fn observed_operation_stop_precedes_wrong_return_type_construction() {
    use botwork::core::signature::ValueKind;
    let operation = NativeOperation::asynchronous(
        StatementSignature::native("Stop")
            .unwrap()
            .returns(ValueKind::Int),
        |_, control| async move {
            control.cancel();
            Ok(Literal::Bool(true))
        },
    )
    .unwrap()
    .with_diagnostic_limits(DiagnosticLimits {
        text_bytes: 0,
        ..DiagnosticLimits::default()
    })
    .unwrap();
    let error = operation
        .invoke(vec![], OperationControl::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert!(error.omissions.is_some());
}

#[tokio::test]
async fn exact_diagnostic_dimensions_preserve_identity_and_lower_limits_reject_both_adapters() {
    for blocking in [false, true] {
        let original = detailed();
        let identity = original.error.clone();
        let source = Arc::downgrade(original.span.as_ref().unwrap().source());
        let size = DiagnosticLimits::default().check(&original).unwrap();
        let exact = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        let accepted = operation(original, blocking)
            .with_diagnostic_limits(exact.clone())
            .unwrap()
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap_err();
        assert!(Arc::ptr_eq(&identity, &accepted.error));
        assert_eq!(DiagnosticLimits::default().check(&accepted).unwrap(), size);
        assert_eq!(accepted.span.as_ref().unwrap().source().name(), "é");
        accepted.discard();
        assert!(source.upgrade().is_none());
        for field in 0..6 {
            let mut limits = exact.clone();
            match field {
                0 => limits.diagnostics -= 1,
                1 => limits.depth -= 1,
                2 => limits.call_frames -= 1,
                3 => limits.related_locations -= 1,
                4 => limits.text_bytes -= 1,
                _ => limits.source_bytes -= 1,
            }
            let original = detailed();
            let source = Arc::downgrade(original.span.as_ref().unwrap().source());
            let error = operation(original, blocking)
                .with_diagnostic_limits(limits)
                .unwrap()
                .invoke(vec![], OperationControl::default())
                .await
                .unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().direct_causes, 1);
            assert!(error.span.is_none() && error.call_stack.is_empty());
            assert!(source.upgrade().is_none());
        }
    }
}

#[tokio::test]
async fn absent_source_uses_the_header_and_rejected_headers_keep_only_byte_evidence() {
    for source_bytes in [0, 12] {
        // "<native>" plus "Fail"
        let error = operation(BWErr::NativeError("reason".into()).into(), false)
            .with_diagnostic_limits(DiagnosticLimits {
                source_bytes,
                ..DiagnosticLimits::default()
            })
            .unwrap()
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap_err();
        if source_bytes == 0 {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            let source = error.causes[0]
                .omissions
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap();
            assert_eq!(source.file, "<native>");
            assert_eq!((source.start_byte, source.end_byte), (0, 4));
        } else {
            assert_eq!(error.code(), DiagnosticCode::Native);
            assert_eq!(error.span.as_ref().unwrap().text(), "Fail");
        }
    }
}

#[tokio::test]
async fn host_emergency_shaped_fields_cannot_bypass_admission() {
    for blocking in [false, true] {
        let mut summary = Diagnostic::new(BWErr::NativeError("x".repeat(65_536)));
        summary.omissions = Some(Box::new(DiagnosticOmissions {
            detail_fields: 0,
            call_frames: 0,
            related_locations: 0,
            direct_causes: 0,
            label: false,
            prior_summary: false,
            source: None,
        }));
        let mut original = Diagnostic::new(BWErr::ResourceLimit {
            resource: "host",
            limit: 0,
        });
        original.causes.push(summary);
        let error = operation(original, blocking)
            .with_diagnostic_limits(DiagnosticLimits {
                text_bytes: 64,
                ..DiagnosticLimits::default()
            })
            .unwrap()
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().direct_causes, 1);
        assert!(
            DiagnosticLimits::default()
                .check(&error)
                .unwrap()
                .text_bytes
                < 1024
        );
    }
}

#[tokio::test]
async fn local_diagnostic_limits_do_not_latch_the_parent_or_change_sibling_operations() {
    let original =
        NativeOperation::asynchronous(StatementSignature::native("Fail").unwrap(), |_, _| async {
            Err(BWErr::NativeError("reason".into()).into())
        })
        .unwrap();
    let limited = original
        .clone()
        .with_diagnostic_limits(DiagnosticLimits {
            diagnostics: 0,
            ..DiagnosticLimits::default()
        })
        .unwrap();
    let control = OperationControl::default();
    for _ in 0..2 {
        assert_eq!(
            limited
                .invoke(vec![], control.clone())
                .await
                .unwrap_err()
                .code(),
            DiagnosticCode::ResourceLimit
        );
        assert_eq!(
            original
                .invoke(vec![], control.clone())
                .await
                .unwrap_err()
                .code(),
            DiagnosticCode::Native
        );
        assert!(!control.is_cancelled());
    }
}

#[tokio::test]
async fn zero_error_allowances_allow_success_and_invalid_depth_rejects_configuration() {
    let original =
        NativeOperation::asynchronous(StatementSignature::native("Go").unwrap(), |_, _| async {
            Ok(Literal::Int(7))
        })
        .unwrap();
    let error = original
        .clone()
        .with_diagnostic_limits(DiagnosticLimits {
            depth: 65,
            ..DiagnosticLimits::default()
        })
        .err()
        .unwrap();
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    let configured = original
        .with_diagnostic_limits(DiagnosticLimits {
            diagnostics: 0,
            depth: 0,
            call_frames: 0,
            related_locations: 0,
            text_bytes: 0,
            source_bytes: 0,
        })
        .unwrap();
    assert_eq!(
        configured
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap()
            .to_string(),
        "7"
    );
}

#[tokio::test]
async fn same_completion_cancellation_remains_primary_with_rejected_error_or_value() {
    for blocking in [false, true] {
        for returns_error in [false, true] {
            let signature = StatementSignature::native("Stop").unwrap();
            let result = move |control: OperationControl| {
                control.cancel();
                if returns_error {
                    Err(Diagnostic::new(BWErr::NativeError("x".repeat(4096))))
                } else {
                    Ok(Literal::String("x".repeat(4096)))
                }
            };
            let operation = if blocking {
                NativeOperation::blocking(
                    signature,
                    NonZeroUsize::new(1).unwrap(),
                    move |_, control| result(control),
                )
                .unwrap()
            } else {
                NativeOperation::asynchronous(signature, move |_, control| async move {
                    result(control)
                })
                .unwrap()
            }
            .with_diagnostic_limits(DiagnosticLimits {
                diagnostics: 0,
                ..DiagnosticLimits::default()
            })
            .unwrap()
            .with_value_limits(ValueLimits {
                string_bytes: 0,
                ..ValueLimits::default()
            })
            .unwrap();
            let parent = OperationControl::default();
            let error = operation.invoke(vec![], parent.clone()).await.unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::Cancelled);
            assert!(error.omissions.is_some());
            assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
            assert!(!parent.is_cancelled());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn blocking_timeout_retains_its_category_after_cancellation_requested_for_cleanup() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let operation = NativeOperation::blocking(
        StatementSignature::native("Wait").unwrap(),
        NonZeroUsize::new(1).unwrap(),
        move |_, control| {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            assert!(control.is_cancelled());
            Err(BWErr::NativeError("cleanup".into()).into())
        },
    )
    .unwrap()
    .with_diagnostic_limits(DiagnosticLimits {
        diagnostics: 0,
        ..DiagnosticLimits::default()
    })
    .unwrap();
    let parent = OperationControl::default()
        .child(Some(tokio::time::Instant::now() + Duration::from_secs(1)));
    let mut invocation = Box::pin(operation.invoke(vec![], parent.clone()));
    assert!(invocation
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    started_rx.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(invocation
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    release_tx.send(()).unwrap();
    let error = invocation.await.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert!(error.omissions.is_some());
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert!(!parent.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn async_deadline_observed_during_a_ready_completion_keeps_timeout_priority() {
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Wait").unwrap(), |_, _| async {
            tokio::time::advance(Duration::from_secs(1)).await;
            Err(BWErr::NativeError("reason".into()).into())
        })
        .unwrap()
        .with_diagnostic_limits(DiagnosticLimits {
            diagnostics: 0,
            ..DiagnosticLimits::default()
        })
        .unwrap();
    let control = OperationControl::default()
        .child(Some(tokio::time::Instant::now() + Duration::from_secs(1)));
    let error = operation.invoke(vec![], control).await.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert!(error.omissions.is_some());
}

#[tokio::test]
async fn panics_argument_errors_and_value_failures_use_the_same_diagnostic_boundary() {
    let factory =
        NativeOperation::asynchronous(StatementSignature::native("Fail").unwrap(), |_, _| {
            panic!("factory");
            #[allow(unreachable_code)]
            async {
                Ok(Literal::None)
            }
        })
        .unwrap();
    let future =
        NativeOperation::asynchronous(StatementSignature::native("Fail").unwrap(), |_, _| async {
            panic!("future")
        })
        .unwrap();
    let blocking = NativeOperation::blocking(
        StatementSignature::native("Fail").unwrap(),
        NonZeroUsize::new(1).unwrap(),
        |_, _| panic!("blocking"),
    )
    .unwrap();
    for original in [factory, future, blocking] {
        let operation = original
            .with_diagnostic_limits(DiagnosticLimits {
                diagnostics: 0,
                ..DiagnosticLimits::default()
            })
            .unwrap();
        let error = operation
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::NativePanic);
        let error = operation
            .invoke(vec![Literal::None], OperationControl::default())
            .await
            .unwrap_err();
        assert_eq!(error.causes[0].code(), DiagnosticCode::ParameterCount);
    }
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Value").unwrap(), |_, _| async {
            Ok(Literal::String("x".into()))
        })
        .unwrap()
        .with_value_limits(ValueLimits {
            string_bytes: 0,
            ..ValueLimits::default()
        })
        .unwrap()
        .with_diagnostic_limits(DiagnosticLimits {
            diagnostics: 0,
            ..DiagnosticLimits::default()
        })
        .unwrap();
    let error = operation
        .invoke(vec![], OperationControl::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
}
