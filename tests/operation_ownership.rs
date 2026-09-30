use botwork::core::{
    ast::Program,
    diagnostic::{
        CallFrame, Diagnostic, DiagnosticCode, DiagnosticLimits, DiagnosticOmissions,
        RelatedLocation,
    },
    grammar::{BWErr, Literal},
    operation::{
        NativeOperation, OperationBudget, OperationControl, OperationOwnershipLimits,
        OperationUsage,
    },
    signature::StatementSignature,
};
use std::{
    future::Future,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Waker},
    time::Duration,
};

async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker completed");
}

fn echo(budget: &OperationBudget) -> NativeOperation {
    NativeOperation::asynchronous(
        StatementSignature::native("Echo |value|").unwrap(),
        |mut values, _| async move { Ok(values.pop().unwrap()) },
    )
    .unwrap()
    .with_ownership_budget(budget.clone())
}

fn resource(error: &Diagnostic) -> &str {
    let BWErr::ResourceLimit { resource, .. } = error.error.as_ref() else {
        panic!("resource error: {error}")
    };
    resource
}

fn sample_error() -> Diagnostic {
    let program = Program::parse("callback-é", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let mut error = Diagnostic::new(BWErr::NativeError("callback é failed".into())).at(span);
    error
        .causes
        .push(Diagnostic::new(BWErr::OutputError("partial output".into())).at(span));
    error.call_stack.push(CallFrame {
        signature: "read".into(),
        statement: None,
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    error.related.push(RelatedLocation {
        message: "related".into(),
        span: span.clone(),
    });
    error
}

fn failing_worker(budget: &OperationBudget, error: Diagnostic) -> NativeOperation {
    let error = Mutex::new(Some(error));
    NativeOperation::blocking(
        StatementSignature::native("Fail").unwrap(),
        NonZeroUsize::new(1).unwrap(),
        move |_, _| Err(error.lock().unwrap().take().unwrap()),
    )
    .unwrap()
    .with_ownership_budget(budget.clone())
}

#[tokio::test]
async fn completed_error_trees_share_all_diagnostic_quotas_and_release_rejected_sources() {
    for dimension in 0..6 {
        let original = sample_error();
        let source = Arc::downgrade(original.span.as_ref().unwrap().source());
        let size = DiagnosticLimits::default().check(&original).unwrap();
        let mut limits = OperationOwnershipLimits {
            diagnostics: 2 * size.diagnostics,
            call_frames: 2 * size.call_frames,
            related_locations: 2 * size.related_locations,
            text_bytes: 2 * size.text_bytes,
            source_bytes: 2 * size.source_bytes,
            ..OperationOwnershipLimits::default()
        };
        let expected = match dimension {
            0 => {
                limits.diagnostics -= 1;
                "operation diagnostic nodes"
            }
            1 => {
                limits.call_frames -= 1;
                "operation diagnostic call frames"
            }
            2 => {
                limits.related_locations -= 1;
                "operation diagnostic related locations"
            }
            3 => {
                limits.text_bytes -= 1;
                "operation diagnostic text bytes"
            }
            4 => {
                limits.source_bytes -= 1;
                "operation diagnostic source bytes"
            }
            _ => "",
        };
        let budget = OperationBudget::new(limits);
        let (release, receiver) = std::sync::mpsc::channel();
        let state = Mutex::new(Some((original, receiver)));
        let first = NativeOperation::blocking(
            StatementSignature::native("Fail").unwrap(),
            NonZeroUsize::new(1).unwrap(),
            move |_, _| {
                let (error, receiver) = state.lock().unwrap().take().unwrap();
                receiver.recv_timeout(Duration::from_secs(5)).unwrap();
                Err(error)
            },
        )
        .unwrap()
        .with_ownership_budget(budget.clone());
        let mut pending = Box::pin(first.invoke(vec![], OperationControl::default()));
        assert!(pending
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        release.send(()).unwrap();
        until(|| budget.usage().diagnostics == size.diagnostics).await;
        let before = budget.usage();
        assert_eq!(before.text_bytes, size.text_bytes);
        assert_eq!(before.source_bytes, size.source_bytes);
        let second_error = sample_error();
        let second_source = Arc::downgrade(second_error.span.as_ref().unwrap().source());
        let second = failing_worker(&budget, second_error);
        let error = second
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap_err();
        if dimension < 5 {
            assert_eq!(resource(&error), expected);
            assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
            assert!(second_source.upgrade().is_none());
            assert!(error.causes[0].span.is_none());
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().direct_causes, 1);
        } else {
            assert_eq!(error.code(), DiagnosticCode::Native);
            assert!(second_source.upgrade().is_some());
        }
        assert_eq!(budget.usage(), before);
        let delivered = pending.await.unwrap_err();
        assert_eq!(delivered.code(), DiagnosticCode::Native);
        assert_eq!(budget.usage(), OperationUsage::default());
        assert!(source.upgrade().is_some()); // Ownership transferred to the host result.
        drop((delivered, error));
        assert!(source.upgrade().is_none() && second_source.upgrade().is_none());
    }
}

#[tokio::test]
async fn host_emergency_shapes_cannot_skip_aggregate_diagnostic_admission() {
    let budget = OperationBudget::new(OperationOwnershipLimits {
        diagnostics: 0,
        ..OperationOwnershipLimits::default()
    });
    let mut error = Diagnostic::new(BWErr::ResourceLimit {
        resource: "host summary",
        limit: 0,
    });
    let mut cause = Diagnostic::new(BWErr::NativeError("original".into()));
    cause.omissions = Some(Box::new(DiagnosticOmissions {
        detail_fields: 0,
        call_frames: 0,
        related_locations: 0,
        direct_causes: 0,
        label: false,
        prior_summary: false,
        source: None,
    }));
    error.causes.push(cause);
    let operation = failing_worker(&budget, error);
    let error = operation
        .invoke(vec![], OperationControl::default())
        .await
        .unwrap_err();
    assert_eq!(resource(&error), "operation diagnostic nodes");
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert_eq!(budget.usage(), OperationUsage::default());
}

#[tokio::test(start_paused = true)]
async fn blocking_cleanup_reserves_stop_and_callback_error_together_without_losing_stop_priority() {
    for timeout in [false, true] {
        for diagnostics in [0, 1, 2] {
            let budget = OperationBudget::new(OperationOwnershipLimits {
                diagnostics,
                ..OperationOwnershipLimits::default()
            });
            let (release, receiver) = std::sync::mpsc::channel();
            let receiver = Mutex::new(receiver);
            let started = Arc::new(AtomicUsize::new(0));
            let seen = started.clone();
            let operation = NativeOperation::blocking(
                StatementSignature::native("Wait").unwrap(),
                NonZeroUsize::new(1).unwrap(),
                move |_, _| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    receiver
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                    Err(BWErr::NativeError("cleanup failed".into()).into())
                },
            )
            .unwrap()
            .with_ownership_budget(budget.clone());
            let parent = OperationControl::default();
            let control =
                parent.child(timeout.then(|| tokio::time::Instant::now() + Duration::from_secs(1)));
            let mut future = Box::pin(operation.invoke(vec![], control.clone()));
            assert!(future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending());
            until(|| started.load(Ordering::SeqCst) == 1).await;
            if timeout {
                tokio::time::advance(Duration::from_secs(1)).await;
            } else {
                control.cancel();
            }
            assert!(future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending());
            assert_eq!(budget.usage().diagnostics, usize::from(diagnostics > 0));
            assert_eq!(budget.usage().invocations, 1);
            release.send(()).unwrap();
            let error = future.await.unwrap_err();
            assert_eq!(
                error.code(),
                if timeout {
                    DiagnosticCode::Timeout
                } else {
                    DiagnosticCode::Cancelled
                }
            );
            if diagnostics == 2 {
                assert_eq!(error.causes.len(), 1);
                assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
                assert!(error.omissions.is_none());
            } else {
                assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.omissions.as_ref().unwrap().direct_causes, 1);
                assert!(error.span.is_none());
            }
            assert_eq!(budget.usage(), OperationUsage::default());
            assert!(!parent.is_cancelled());
        }
    }
}

#[test]
fn abandoned_runtime_queued_workers_retain_admission_until_the_queue_drains() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (release, receiver) = std::sync::mpsc::channel();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        started_rx.await.unwrap();
        let budget = OperationBudget::default();
        let operation = NativeOperation::blocking(
            StatementSignature::native("Read |value|").unwrap(),
            NonZeroUsize::new(1).unwrap(),
            |_, _| panic!("cancelled queued worker must skip callback"),
        )
        .unwrap()
        .with_ownership_budget(budget.clone());
        let mut future = Box::pin(operation.invoke(
            vec![Literal::String("é".into())],
            OperationControl::default(),
        ));
        assert!(future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        drop(future);
        assert_eq!(budget.usage().invocations, 1);
        assert_eq!(budget.usage().payload_bytes, 2);
        release.send(()).unwrap();
        blocker.await.unwrap();
        until(|| budget.usage() == OperationUsage::default()).await;
    });
}

#[tokio::test]
async fn rejected_deep_values_and_causes_dispose_iteratively_and_stops_precede_admission() {
    fn deep() -> Literal {
        let mut value = Literal::None;
        for _ in 0..20_000 {
            value = Literal::Array(vec![value]);
        }
        value
    }
    let budget = OperationBudget::new(OperationOwnershipLimits {
        invocations: 0,
        ..OperationOwnershipLimits::default()
    });
    let operation = echo(&budget);
    let control = OperationControl::default();
    control.cancel();
    let error = operation.invoke(vec![deep()], control).await.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    let error = operation
        .invoke(vec![deep()], OperationControl::default())
        .await
        .unwrap_err();
    assert_eq!(resource(&error), "value depth");
    assert_eq!(budget.usage(), OperationUsage::default());
    let budget = OperationBudget::default();
    let values = NativeOperation::blocking(
        StatementSignature::native("Read").unwrap(),
        NonZeroUsize::new(1).unwrap(),
        |_, _| Ok(deep()),
    )
    .unwrap()
    .with_ownership_budget(budget.clone());
    assert_eq!(
        resource(
            &values
                .invoke(vec![], OperationControl::default())
                .await
                .unwrap_err()
        ),
        "value depth"
    );
    let mut error = sample_error();
    let source = Arc::downgrade(error.span.as_ref().unwrap().source());
    for _ in 0..20_000 {
        let mut next = Diagnostic::new(BWErr::NativeError("nested".into()));
        next.causes.push(error);
        error = next;
    }
    let operation = failing_worker(&budget, error);
    let error = operation
        .invoke(vec![], OperationControl::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(source.upgrade().is_none());
    assert_eq!(budget.usage(), OperationUsage::default());
}

#[tokio::test]
async fn initial_validation_keeps_left_to_right_semantic_priority_over_later_shape_failures() {
    use botwork::core::signature::ValueKind;
    for numeric in [false, true] {
        let mut deep = Literal::None;
        for _ in 0..20_000 {
            deep = Literal::Array(vec![deep]);
        }
        let budget = OperationBudget::default();
        let operation = NativeOperation::asynchronous(
            StatementSignature::native("Read |first| |second|")
                .unwrap()
                .parameter("first", ValueKind::Int)
                .unwrap(),
            |_, _| async { panic!("invalid arguments must not reach callback") },
        )
        .unwrap()
        .with_ownership_budget(budget.clone());
        let future = operation.invoke(
            vec![
                if numeric {
                    Literal::Float(f32::NAN)
                } else {
                    Literal::Bool(true)
                },
                deep,
            ],
            OperationControl::default(),
        );
        assert_eq!(budget.usage(), OperationUsage::default());
        let error = future.await.unwrap_err();
        assert_eq!(
            error.code(),
            if numeric {
                DiagnosticCode::Arithmetic
            } else {
                DiagnosticCode::IncompatibleType
            }
        );
        assert_eq!(budget.usage(), OperationUsage::default());
    }
}

#[tokio::test]
async fn unpolled_arguments_share_atomic_limits_across_clones_and_distinct_operations() {
    let budget = OperationBudget::new(OperationOwnershipLimits {
        invocations: 2,
        values: 2,
        nodes: 6,
        payload_bytes: 12,
        ..OperationOwnershipLimits::default()
    });
    let operation = echo(&budget);
    let clone = operation
        .clone()
        .with_diagnostic_limits(DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        })
        .unwrap();
    let distinct = echo(&budget);
    let arguments = || {
        vec![Literal::Array(vec![
            Literal::String("é".into()),
            Literal::Int(7),
        ])]
    };
    let first = operation.invoke(arguments(), OperationControl::default());
    let second = clone.invoke(arguments(), OperationControl::default());
    assert_eq!(
        budget.usage(),
        OperationUsage {
            invocations: 2,
            values: 2,
            nodes: 6,
            payload_bytes: 12,
            ..OperationUsage::default()
        }
    );
    let before = budget.usage();
    let error = distinct
        .invoke(arguments(), OperationControl::default())
        .await
        .unwrap_err();
    assert_eq!(resource(&error), "operation invocations");
    assert_eq!(budget.usage(), before);
    drop(first);
    assert_eq!(budget.usage().invocations, 1);
    let replacement = distinct.invoke(arguments(), OperationControl::default());
    assert_eq!(budget.usage(), before);
    drop((second, replacement));
    assert_eq!(budget.usage(), OperationUsage::default());
    let independent = operation
        .clone()
        .with_ownership_budget(OperationBudget::default());
    let value = independent
        .invoke(arguments(), OperationControl::default())
        .await
        .unwrap();
    assert_eq!(value.to_string(), "[\"é\", 7]");
    assert_eq!(budget.usage(), OperationUsage::default());
}

#[tokio::test]
async fn argument_admission_rejects_each_zero_or_one_less_dimension_before_callbacks() {
    for dimension in 0..4 {
        for zero in [false, true] {
            let mut limits = OperationOwnershipLimits {
                invocations: 1,
                values: 2,
                nodes: 4,
                payload_bytes: 8,
                ..OperationOwnershipLimits::default()
            };
            let (field, expected) = match dimension {
                0 => (&mut limits.invocations, "operation invocations"),
                1 => (&mut limits.values, "operation values"),
                2 => (&mut limits.nodes, "operation value nodes"),
                _ => (&mut limits.payload_bytes, "operation value payload bytes"),
            };
            *field = if zero { 0 } else { *field - 1 };
            let budget = OperationBudget::new(limits);
            let operation = NativeOperation::asynchronous(
                StatementSignature::native("Read |a| |b|").unwrap(),
                |_, _| async { panic!("callback must not run") },
            )
            .unwrap()
            .with_ownership_budget(budget.clone());
            let error = operation
                .invoke(
                    vec![
                        Literal::Array(vec![Literal::Int(1), Literal::Int(2)]),
                        Literal::None,
                    ],
                    OperationControl::default(),
                )
                .await
                .unwrap_err();
            assert_eq!(resource(&error), expected);
            assert_eq!(budget.usage(), OperationUsage::default());
        }
    }
}

#[tokio::test]
async fn outputs_require_argument_plus_result_headroom_and_transfer_to_host_without_charges() {
    for dimension in 0..3 {
        for reject in [false, true] {
            let mut limits = OperationOwnershipLimits {
                invocations: 1,
                values: 2,
                nodes: 2,
                payload_bytes: 8,
                ..OperationOwnershipLimits::default()
            };
            if reject {
                match dimension {
                    0 => limits.values -= 1,
                    1 => limits.nodes -= 1,
                    _ => limits.payload_bytes -= 1,
                }
            }
            let budget = OperationBudget::new(limits);
            let effects = Arc::new(AtomicUsize::new(0));
            let seen = effects.clone();
            let operation = NativeOperation::asynchronous(
                StatementSignature::native("Read |value|").unwrap(),
                move |mut values, _| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(values.pop().unwrap()) }
                },
            )
            .unwrap()
            .with_ownership_budget(budget.clone());
            for _ in 0..2 {
                let result = operation
                    .invoke(
                        vec![Literal::String("éé".into())],
                        OperationControl::default(),
                    )
                    .await;
                if reject {
                    assert_eq!(
                        resource(&result.unwrap_err()),
                        [
                            "operation values",
                            "operation value nodes",
                            "operation value payload bytes"
                        ][dimension]
                    );
                } else {
                    assert_eq!(result.unwrap().to_string(), "éé");
                }
                assert_eq!(budget.usage(), OperationUsage::default());
            }
            assert_eq!(effects.load(Ordering::SeqCst), 2);
        }
    }
}

#[tokio::test]
async fn dropped_async_futures_release_captures_before_returning_argument_capacity() {
    struct ObserveDrop(OperationBudget, Arc<AtomicUsize>);
    impl Drop for ObserveDrop {
        fn drop(&mut self) {
            assert_eq!(self.0.usage().invocations, 1);
            assert_eq!(self.0.usage().values, 1);
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }
    let budget = OperationBudget::new(OperationOwnershipLimits {
        invocations: 1,
        ..OperationOwnershipLimits::default()
    });
    let drops = Arc::new(AtomicUsize::new(0));
    let observed_budget = budget.clone();
    let observed_drops = drops.clone();
    let operation = NativeOperation::asynchronous(
        StatementSignature::native("Wait |value|").unwrap(),
        move |values, _| {
            let guard = ObserveDrop(observed_budget.clone(), observed_drops.clone());
            async move {
                let _owned = (values, guard);
                std::future::pending().await
            }
        },
    )
    .unwrap()
    .with_ownership_budget(budget.clone());
    let parent = OperationControl::default();
    let mut future = Box::pin(operation.invoke(vec![Literal::Int(1)], parent.clone()));
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    drop(future);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(budget.usage(), OperationUsage::default());
    assert!(!parent.is_cancelled());
}

#[tokio::test]
async fn queued_and_abandoned_blocking_invocations_keep_their_own_argument_allowance() {
    let budget = OperationBudget::new(OperationOwnershipLimits {
        invocations: 2,
        ..OperationOwnershipLimits::default()
    });
    let (release, receiver) = std::sync::mpsc::channel();
    let receiver = Mutex::new(receiver);
    let started = Arc::new(AtomicUsize::new(0));
    let seen = started.clone();
    let operation = NativeOperation::blocking(
        StatementSignature::native("Wait |value|").unwrap(),
        NonZeroUsize::new(1).unwrap(),
        move |_, control| {
            seen.fetch_add(1, Ordering::SeqCst);
            receiver
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            assert!(control.is_cancelled());
            Ok(Literal::String("worker result".into()))
        },
    )
    .unwrap()
    .with_ownership_budget(budget.clone());
    let parent = OperationControl::default();
    let mut running = Box::pin(operation.invoke(vec![Literal::Int(1)], parent.clone()));
    assert!(running
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    until(|| started.load(Ordering::SeqCst) == 1).await;
    let mut queued = Box::pin(operation.invoke(vec![Literal::Int(2)], parent.clone()));
    assert!(queued
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    assert_eq!(budget.usage().invocations, 2);
    drop(queued);
    assert_eq!(budget.usage().invocations, 1);
    drop(running);
    assert_eq!(budget.usage().invocations, 1);
    assert_eq!(budget.usage().values, 1);
    release.send(()).unwrap();
    until(|| budget.usage() == OperationUsage::default()).await;
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert!(!parent.is_cancelled());
}

#[tokio::test]
async fn completed_unobserved_workers_hold_result_reservations_until_delivery_or_disposal() {
    for abandon in [false, true] {
        let budget = OperationBudget::default();
        let (release, receiver) = std::sync::mpsc::channel();
        let receiver = Mutex::new(receiver);
        let operation = NativeOperation::blocking(
            StatementSignature::native("Read |value|").unwrap(),
            NonZeroUsize::new(1).unwrap(),
            move |_, _| {
                receiver
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                Ok(Literal::String("é".repeat(1024)))
            },
        )
        .unwrap()
        .with_ownership_budget(budget.clone());
        let mut future =
            Box::pin(operation.invoke(vec![Literal::Int(7)], OperationControl::default()));
        assert!(future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        release.send(()).unwrap();
        until(|| budget.usage().values == 2).await;
        assert_eq!(budget.usage().nodes, 2);
        assert_eq!(budget.usage().payload_bytes, 2052);
        if abandon {
            drop(future);
        } else {
            let value = future.await.unwrap();
            assert_eq!(value.to_string().len(), 2048);
            assert_eq!(budget.usage(), OperationUsage::default());
        }
        until(|| budget.usage() == OperationUsage::default()).await;
    }
}

#[tokio::test]
async fn generated_operation_errors_share_exact_construction_quotas_and_release_at_public_return() {
    use botwork::core::signature::ValueKind;
    for family in 0..7 {
        let signature = if family < 3 { "Read |value|" } else { "Read" };
        let mut signature = StatementSignature::native(signature).unwrap();
        if family == 2 {
            signature = signature.parameter("value", ValueKind::Int).unwrap();
        }
        if family == 3 {
            signature = signature.returns(ValueKind::String);
        }
        let operation = if family == 6 {
            NativeOperation::blocking(signature, NonZeroUsize::new(1).unwrap(), |_, _| {
                panic!("worker panic")
            })
            .unwrap()
        } else {
            NativeOperation::asynchronous(signature, move |_, _| {
                assert_ne!(family, 4, "factory panic");
                async move {
                    assert_ne!(family, 5, "poll panic");
                    Ok(Literal::Bool(true))
                }
            })
            .unwrap()
        };
        let values = || match family {
            1 => vec![Literal::Float(f32::NAN)],
            2 => vec![Literal::Bool(true)],
            _ => vec![],
        };
        let baseline = operation
            .invoke(values(), OperationControl::default())
            .await
            .unwrap_err();
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        for fits in [false, true] {
            let budget = OperationBudget::new(OperationOwnershipLimits {
                diagnostics: size.diagnostics,
                text_bytes: size.text_bytes - usize::from(!fits),
                source_bytes: size.source_bytes,
                ..Default::default()
            });
            let limited = operation.clone().with_ownership_budget(budget.clone());
            let error = limited
                .invoke(values(), OperationControl::default())
                .await
                .unwrap_err();
            if fits {
                assert_eq!(error.to_string(), baseline.to_string(), "family {family}");
            } else {
                assert_eq!(resource(&error), "operation diagnostic text bytes");
                assert_eq!(error.causes[0].code(), baseline.code());
                assert!(error.causes[0].omissions.is_some());
            }
            assert_eq!(budget.usage(), OperationUsage::default());
            let repeated = limited
                .invoke(values(), OperationControl::default())
                .await
                .unwrap_err();
            assert_eq!(repeated.code(), error.code());
            assert_eq!(budget.usage(), OperationUsage::default());
        }
    }
}
