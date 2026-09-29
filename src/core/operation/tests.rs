use super::*;

#[tokio::test]
async fn operation_numeric_guards_use_local_limits_before_argument_and_result_messages() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for invalid_return in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let operation = NativeOperation::asynchronous(
            StatementSignature::native("Read |value|").unwrap(),
            move |_, _| {
                seen.fetch_add(1, Ordering::SeqCst);
                async { Ok(Literal::Array(vec![Literal::Float(f32::NAN)])) }
            },
        )
        .unwrap();
        let arguments = || {
            vec![if invalid_return {
                Literal::Int(1)
            } else {
                Literal::Array(vec![Literal::Float(f32::INFINITY)])
            }]
        };
        let baseline = operation
            .invoke(arguments(), OperationControl::default())
            .await
            .unwrap_err();
        assert_eq!(baseline.code(), DiagnosticCode::Arithmetic);
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        for text_bytes in [size.text_bytes, size.text_bytes - 1, 0] {
            calls.store(0, Ordering::SeqCst);
            let limited = operation
                .clone()
                .with_diagnostic_limits(DiagnosticLimits {
                    text_bytes,
                    ..DiagnosticLimits::default()
                })
                .unwrap();
            let error = limited
                .invoke(arguments(), OperationControl::default())
                .await
                .unwrap_err();
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(invalid_return));
            if text_bytes == size.text_bytes {
                assert_eq!(error.to_string(), baseline.to_string());
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
                assert!(error.causes[0].span.is_none());
                assert!(!error.causes[0].omissions.as_ref().unwrap().prior_summary);
            }
        }
    }
}

#[tokio::test]
async fn operation_guards_admit_complete_header_before_arity_and_capacity_errors() {
    for closed in [false, true] {
        let operation = if closed {
            let operation = NativeOperation::blocking(
                StatementSignature::native("Read").unwrap(),
                NonZeroUsize::new(1).unwrap(),
                |_, _| panic!("callback must not run"),
            )
            .unwrap();
            let Implementation::Blocking { capacity, .. } = &operation.implementation else {
                unreachable!()
            };
            capacity.close();
            operation
        } else {
            NativeOperation::asynchronous(
                StatementSignature::native("Read |value|").unwrap(),
                |_, _| async { panic!("callback must not run") },
            )
            .unwrap()
        };
        let baseline = operation
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap_err();
        assert_eq!(
            baseline.code(),
            if closed {
                DiagnosticCode::AsyncRuntime
            } else {
                DiagnosticCode::ParameterCount
            }
        );
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
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
                3 => limits.diagnostics = 0,
                4 => limits.depth = 0,
                _ => limits.text_bytes = 0,
            }
            let limited = operation.clone().with_diagnostic_limits(limits).unwrap();
            let parent = OperationControl::default();
            let error = limited.invoke(vec![], parent.clone()).await.unwrap_err();
            assert!(!parent.is_cancelled());
            if dimension == 0 {
                assert_eq!(error.to_string(), baseline.to_string());
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), baseline.code());
                assert!(error.causes[0].span.is_none());
                assert!(!error.causes[0].omissions.as_ref().unwrap().prior_summary);
            }
            parent.cancel();
            let stopped = limited.invoke(vec![], parent).await.unwrap_err();
            assert_eq!(stopped.code(), DiagnosticCode::Cancelled);
        }
    }
}

#[test]
fn absent_runtime_guard_admits_header_and_text_without_entering_callback() {
    use std::task::Waker;
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Read").unwrap(), |_, _| async {
            panic!("callback must not run")
        })
        .unwrap();
    let invoke = |operation: &NativeOperation| {
        let mut future = std::pin::pin!(operation.invoke(vec![], OperationControl::default()));
        let Poll::Ready(result) = future
            .as_mut()
            .poll(&mut TaskContext::from_waker(Waker::noop()))
        else {
            panic!("missing runtime must fail immediately")
        };
        result.unwrap_err()
    };
    let baseline = invoke(&operation);
    assert_eq!(baseline.code(), DiagnosticCode::AsyncRuntime);
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    for text_bytes in [size.text_bytes, size.text_bytes - 1, 0] {
        let limited = operation
            .clone()
            .with_diagnostic_limits(DiagnosticLimits {
                text_bytes,
                ..DiagnosticLimits::default()
            })
            .unwrap();
        let error = invoke(&limited);
        if text_bytes == size.text_bytes {
            assert_eq!(error.to_string(), baseline.to_string());
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::AsyncRuntime);
            assert!(error.causes[0].span.is_none());
        }
    }
}

#[test]
fn operation_builder_guards_admit_exact_default_source_ownership_and_release_rejections() {
    use crate::core::{
        ast::{Program, StatementKind},
        ast_limits::AstLimits,
        syntax_limits::SyntaxLimits,
    };
    let maximum = DiagnosticLimits::default().source_bytes;
    for invalid_capacity in [false, true] {
        for extra in [0, 1] {
            let text = if invalid_capacity { "Read" } else { "Read {}" };
            let name = "x".repeat(maximum - text.len() + extra);
            let signature = if invalid_capacity {
                StatementSignature::native_at(&name, text).unwrap()
            } else {
                let program = Program::parse_with_budgets(
                    &name,
                    text,
                    1024,
                    &SyntaxLimits::default(),
                    &AstLimits {
                        source_bytes: usize::MAX,
                        ..AstLimits::default()
                    },
                )
                .unwrap();
                let StatementKind::Define(definition) = program.statements[0].kind() else {
                    panic!("definition")
                };
                definition.signature_metadata()
            };
            let source = Arc::downgrade(signature.header().source());
            let error = if invalid_capacity {
                NativeOperation::blocking(
                    signature,
                    NonZeroUsize::new(Semaphore::MAX_PERMITS + 1).unwrap(),
                    |_, _| panic!("callback must not run"),
                )
            } else {
                NativeOperation::asynchronous(signature, |_, _| async {
                    panic!("callback must not run")
                })
            }
            .err()
            .unwrap();
            if extra == 0 {
                assert_eq!(error.code(), DiagnosticCode::Signature);
                assert_eq!(
                    DiagnosticLimits::default()
                        .check(&error)
                        .unwrap()
                        .source_bytes,
                    maximum
                );
                assert!(source.upgrade().is_some());
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::Signature);
                assert!(source.upgrade().is_none());
                assert!(
                    error.causes[0]
                        .omissions
                        .as_ref()
                        .unwrap()
                        .source
                        .as_ref()
                        .unwrap()
                        .file_truncated
                );
            }
            drop(error);
            assert!(source.upgrade().is_none());
        }
    }
}

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
                let (summary, limit): (&Diagnostic, &Diagnostic) = if stop == 2 {
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
        Diagnostic::new(BWErr::NativeError("reason".into())),
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
            let error = admit_error(
                &limits,
                &signature,
                Diagnostic::new(original),
                observed,
                false,
            );
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

// Exercise production admission while transferring constructed test values through
// the same public ownership boundary before installing a different test pool.
fn admit_error(
    limits: &DiagnosticLimits,
    signature: &StatementSignature,
    error: impl Into<PendingDiagnostic>,
    preserve_stop: bool,
    admitted: bool,
) -> Diagnostic {
    let error = error.into().value.into_inner();
    let operation =
        NativeOperation::asynchronous(signature.clone(), |_, _| async { Ok(Literal::None) })
            .unwrap()
            .with_diagnostic_limits(limits.clone())
            .unwrap();
    operation
        .track_error(error, preserve_stop, admitted, &None)
        .into_inner()
        .into_inner()
}

fn panic_error(limits: &DiagnosticLimits, signature: &StatementSignature) -> PendingDiagnostic {
    NativeOperation::asynchronous(signature.clone(), |_, _| async { Ok(Literal::None) })
        .unwrap()
        .with_diagnostic_limits(limits.clone())
        .unwrap()
        .panic_error()
}

#[test]
fn children_and_clones_inherit_the_stop_grace() {
    assert_eq!(
        OperationControl::default().stop_grace(),
        std::time::Duration::from_secs(2)
    );
    let grace = std::time::Duration::from_millis(125);
    let parent = OperationControl::default().with_stop_grace(grace);
    let child = parent.child(Some(Instant::now()));
    assert_eq!(
        (parent.clone().stop_grace(), child.stop_grace()),
        (grace, grace)
    );
    assert_eq!(child.child(None).stop_grace(), grace);
    let stop = child.abandoned(Diagnostic::new(BWErr::Timeout("deadline".into())));
    assert_eq!(stop.code(), DiagnosticCode::Timeout);
    let text = stop.to_string();
    assert!(
        text.contains("[BW5003]")
            && text.contains("did not stop within 125 ms of the stop and was abandoned"),
        "{text}"
    );
}

#[tokio::test]
async fn within_grace_keeps_finished_work_and_abandons_the_rest() {
    let control = OperationControl::default().with_stop_grace(std::time::Duration::from_millis(50));
    assert_eq!(control.within_grace(async { 7 }).await, Some(7));
    let start = std::time::Instant::now();
    assert_eq!(
        control.within_grace(std::future::pending::<()>()).await,
        None
    );
    let waited = start.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(50)
            && waited < std::time::Duration::from_millis(1500),
        "{waited:?}"
    );
    // A zero grace still takes work that is already finished.
    let none = control.clone().with_stop_grace(std::time::Duration::ZERO);
    assert_eq!(none.within_grace(async { 8 }).await, Some(8));
}

#[test]
fn without_a_time_driver_the_grace_cannot_be_measured_so_work_is_awaited() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let control = OperationControl::default().with_stop_grace(std::time::Duration::ZERO);
    let value = runtime.block_on(control.within_grace(async {
        tokio::task::yield_now().await;
        9
    }));
    assert_eq!(value, Some(9));
}
