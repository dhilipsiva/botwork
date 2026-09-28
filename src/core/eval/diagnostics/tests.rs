use super::*;
use crate::core::diagnostic::DiagnosticLimits;
use crate::core::run::{RetainedDiagnosticLimits, SnapshotLimits};

#[test]
fn cancellation_after_error_construction_keeps_stop_priority_and_original_cause() {
    let control = OperationControl::default();
    let context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    let original = context.runtime_diagnostic(
        Diagnostic::new(BWErr::NativeError("operation already failed".into())).into(),
        None,
        false,
    );
    assert_eq!(original.code(), DiagnosticCode::Native);
    // A concurrent cancellation can arrive after construction and before return.
    // Arrange that ordering directly so the regression needs no timing race.
    control.cancel();
    let error = context
        .after_evaluation::<Literal>(Err(original))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    assert!(error.causes[0]
        .to_string()
        .contains("operation already failed"));
}

#[test]
fn shared_handler_unwind_admits_copy_overlap_and_preserves_primary_failure_and_bindings() {
    // The snapshot owns a call and handler; the new error and copied cause overlap.
    for records in [3, 4] {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let save = captured.clone();
        let mut context = Context::with_limits(RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        context
            .register_callback(
                "snapshot",
                "Snapshot",
                Arc::new(move |_, context| {
                    *save.lock().unwrap() = Some(context.clone());
                    Ok(Literal::None)
                }),
            )
            .unwrap();
        let program = Program::parse(
            "handlers",
            "|error| = |7|\nTry { Missing } Catch |error| { Snapshot\n|progress| = |1|\nOther }",
        )
        .unwrap();
        let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
        let held = captured.lock().unwrap().take().unwrap();
        assert_eq!(context.frames[0].variables["error"].value.to_string(), "7");
        assert_eq!(
            context.frames[0].variables["progress"].value.to_string(),
            "1"
        );
        assert_eq!(
            held.handlers[0].diagnostic.value.code(),
            DiagnosticCode::UndefinedStatement
        );
        held.checkpoint().unwrap();
        if records == 3 {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
            assert!(
                matches!(error.causes[0].error.as_ref(), BWErr::StatementNotDefined(name) if name.trim() == "Other")
            );
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().direct_causes, 1);
            assert!(context.checkpoint().is_err());
        } else {
            assert_eq!(error.code(), DiagnosticCode::UndefinedStatement);
            assert!(
                matches!(error.error.as_ref(), BWErr::StatementNotDefined(name) if name.trim() == "Other")
            );
            assert!(
                matches!(error.causes[0].error.as_ref(), BWErr::StatementNotDefined(name) if name.trim() == "Missing")
            );
            context.checkpoint().unwrap();
        }
        assert!(context.calls.is_empty() && context.handlers.is_empty());
        drop(held);
    }
}

#[test]
fn unique_handler_handoff_and_duplicate_error_identity_need_no_additional_record() {
    for shared_identity in [false, true] {
        let context = Context::with_limits(RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records: 1,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        let original = context
            .retain_handler(Diagnostic::new(BWErr::NativeError("first".into())))
            .unwrap();
        let (error, extra_owner) = if shared_identity {
            (original.value.clone(), Some(original.clone()))
        } else {
            (
                Diagnostic::new(BWErr::ArithmeticError("second".into())),
                None,
            )
        };
        let error = context.finish_handler_error(error.into(), original);
        assert_eq!(error.causes.len(), usize::from(!shared_identity));
        assert_ne!(error.code(), DiagnosticCode::ResourceLimit);
        context.checkpoint().unwrap();
        drop(extra_owner);
        if !shared_identity {
            let probe = context.clone();
            assert!(probe
                .retain_handler(Diagnostic::new(BWErr::NativeError(
                    "blocked while outgoing".into()
                )))
                .is_err());
        }
        drop(error);
        drop(
            context
                .retain_handler(Diagnostic::new(BWErr::NativeError("next".into())))
                .unwrap(),
        );
    }
}

#[test]
fn cancelled_rethrow_copy_keeps_control_priority_and_bounded_original_site_evidence() {
    let control = OperationControl::default();
    let context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    let program = Program::parse("original-é", "|x| = |1|").unwrap();
    let original = context
        .retain_handler(
            Diagnostic::new(BWErr::ArithmeticError("divide by zero".into()))
                .at(&program.statements[0].span),
        )
        .unwrap();
    control.cancel();
    let error = context.rethrow_handler(&original, &program.statements[0].span);
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
    let omissions = error.causes[0].omissions.as_ref().unwrap();
    assert_eq!(omissions.related_locations, 1);
    assert_eq!(omissions.source.as_ref().unwrap().file, "original-é");
    assert!(error.is_emergency());
}

#[test]
fn context_source_guard_admission_preserves_unicode_offsets_and_latches_only_the_requester() {
    let context = Context::with_limits(RunLimits {
        source_bytes: 2,
        diagnostics: DiagnosticLimits {
            source_bytes: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let sibling = context.clone();
    let error = context.check_syntax("é", "🙂").unwrap_err();
    assert!(matches!(
        error.causes[0].error.as_ref(),
        BWErr::ResourceLimit {
            resource: "source bytes",
            limit: 2
        }
    ));
    let omitted = error.causes[0].omissions.as_ref().unwrap();
    assert_eq!(omitted.detail_fields, 0);
    let source = omitted.source.as_ref().unwrap();
    assert_eq!((source.start_byte, source.end_byte), (0, 4));
    assert_eq!(source.file, "é");
    assert!(context.checkpoint().is_err());
    assert!(sibling.checkpoint().is_ok());
    assert!(sibling.check_syntax("valid", "").is_ok());
}

#[test]
fn collision_construction_preserves_native_names_and_marks_only_replaced_location_fields() {
    let program = Program::parse("é.botwork", "Read {}").unwrap();
    let span = &program.statements[0].span;
    for native in [false, true] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        let mut sibling = context.clone();
        let error = context.duplicate_error(
            |[signature, original, duplicate]| BWErr::DuplicateStatement {
                signature,
                original,
                duplicate,
            },
            "read",
            (span, native),
            span,
            "first definition",
        );
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(
            error.causes[0].omissions.as_ref().unwrap().detail_fields,
            if native { 1 } else { 2 }
        );
        let BWErr::DuplicateStatement {
            original,
            duplicate,
            ..
        } = error.causes[0].error.as_ref()
        else {
            panic!("duplicate")
        };
        if native {
            assert_eq!(original, "é.botwork");
        } else {
            assert!(original.ends_with("coordinates omitted]"));
        }
        assert!(duplicate.ends_with("coordinates omitted]"));
        assert!(context.checkpoint().is_err());
        assert!(sibling.checkpoint().is_ok());
        let next = Program::parse("next", "|x| = |1|").unwrap();
        assert!(evaluate_program_detailed(&next, &mut context).is_err());
        evaluate_program_detailed(&next, &mut sibling).unwrap();
    }
}

#[test]
fn snapshots_share_call_and_handler_records_and_charge_their_copied_handles() {
    let program = Program::parse("snapshot", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let mut context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 2,
            ..RetainedDiagnosticLimits::default()
        },
        snapshots: SnapshotLimits {
            entries: 2,
            ..SnapshotLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .calls
        .push(context.retain_call("read", span, None).unwrap());
    context.handlers.push(HandledError {
        invocation: 0,
        diagnostic: context
            .retain_handler(Diagnostic::new(BWErr::NativeError("reason".into())).at(span))
            .unwrap(),
    });
    let held = context.try_clone().unwrap();
    assert!(Arc::ptr_eq(&context.calls[0], &held.calls[0]));
    assert!(Arc::ptr_eq(
        &context.handlers[0].diagnostic,
        &held.handlers[0].diagnostic
    ));
    assert!(context.try_clone().is_err());
    held.checkpoint().unwrap();
    let mut probe = held.clone();
    probe.calls.clear();
    probe.handlers.clear();
    context.calls.clear();
    context.handlers.clear();
    let failed = probe.clone();
    assert!(failed
        .retain_handler(Diagnostic::new(BWErr::NativeError("new".into())))
        .is_err());
    assert!(failed.checkpoint().is_err());
    probe.checkpoint().unwrap();
    drop(held);
    drop(
        probe
            .retain_handler(Diagnostic::new(BWErr::NativeError("new".into())))
            .unwrap(),
    );
    probe.checkpoint().unwrap();
}

#[test]
fn stopped_calls_release_their_record_and_cancelled_handler_admission_keeps_evidence() {
    let program = Program::parse("cancel", "|x| = |1|").unwrap();
    let control = OperationControl::default();
    let mut context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    let mut weak = std::sync::Weak::new();
    let result = context.with_call("stop", &program.statements[0].span, None, |context| {
        weak = Arc::downgrade(context.calls.last().unwrap());
        control.cancel();
        context.temporary(Literal::None)
    });
    assert_eq!(result.unwrap_err().code(), DiagnosticCode::Cancelled);
    assert!(weak.upgrade().is_none());
    assert!(context.calls.is_empty());
    let error = context
        .retain_handler(Diagnostic::new(BWErr::NativeError("original".into())))
        .err()
        .unwrap();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    assert!(error.is_emergency());
}

#[test]
fn initial_constructor_reserves_every_dimension_with_active_call_and_distinct_related_sources() {
    for deficit in 0..=6 {
        let caller = Program::parse("caller", "Call").unwrap();
        let primary = Program::parse("primary", "Import |\"x.botwork\"| As |x|").unwrap();
        let origin = Program::parse("origin", "Read").unwrap();
        let call = &caller.statements[0].span;
        let span = &primary.statements[0].span;
        let site = &origin.statements[0].span;
        let mut expected = Diagnostic::new(BWErr::ImportRead("detail-é".into()))
            .at(span)
            .with_related("imported here", site);
        expected.call_stack.push(CallFrame {
            signature: "call".into(),
            call_site: call.clone(),
            definition_site: None,
        });
        let size = DiagnosticLimits::default().check(&expected).unwrap();
        let mut retained = RetainedDiagnosticLimits {
            records: 2,
            diagnostics: size.diagnostics,
            call_frames: size.call_frames + 1,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes + "call".len(),
            source_bytes: size.source_bytes,
        };
        match deficit {
            1 => retained.records -= 1,
            2 => retained.diagnostics -= 1,
            3 => retained.call_frames -= 1,
            4 => retained.related_locations -= 1,
            5 => retained.text_bytes -= 1,
            6 => retained.source_bytes -= 1,
            _ => (),
        }
        let mut context = Context::with_limits(RunLimits {
            retained_diagnostics: retained,
            ..Default::default()
        })
        .unwrap();
        context
            .calls
            .push(context.retain_call("call", call, None).unwrap());
        let sibling = context.clone();
        let error = context.import_error(BWErr::ImportRead, format_args!("detail-é"), span, site);
        if deficit == 0 {
            assert_eq!(error.to_string(), expected.to_string());
            context.checkpoint().unwrap();
        } else {
            assert!(error.is_emergency());
            assert_eq!(error.causes[0].code(), DiagnosticCode::ImportRead);
            let omitted = error.causes[0].omissions.as_ref().unwrap();
            assert_eq!(omitted.call_frames, 1);
            assert_eq!(omitted.related_locations, 1);
            assert_eq!(omitted.source.as_ref().unwrap().file, "primary");
            assert!(context.checkpoint().is_err());
        }
        sibling.checkpoint().unwrap();
        let sources = [
            Arc::downgrade(&primary.source),
            Arc::downgrade(&origin.source),
        ];
        drop((expected, primary, origin));
        assert_eq!(
            sources.iter().all(|source| source.upgrade().is_none()),
            deficit != 0
        );
        drop(error);
        assert!(sources.iter().all(|source| source.upgrade().is_none()));
    }
}

#[test]
fn cancellation_during_message_formatting_keeps_prospective_primary_context_on_rejection() {
    struct CancelWhileFormatting(OperationControl);
    impl std::fmt::Display for CancelWhileFormatting {
        fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.cancel();
            output.write_str("reason")
        }
    }
    let control = OperationControl::default();
    let mut context = Context::with_control(
        RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                diagnostics: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        control.clone(),
    )
    .unwrap();
    let program = Program::parse("late-stop", "Read").unwrap();
    let span = &program.statements[0].span;
    context
        .calls
        .push(context.retain_call("read", span, None).unwrap());
    let error = context.formatted_error(
        BWErr::NativeError,
        format_args!("{}", CancelWhileFormatting(control)),
        Some(span),
        true,
    );
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert!(error.is_emergency());
    let omitted = error.omissions.as_ref().unwrap();
    assert_eq!(omitted.call_frames, 1);
    assert_eq!(omitted.direct_causes, 1);
    assert_eq!(omitted.source.as_ref().unwrap().file, "late-stop");
    assert_eq!(error.label, "expression");
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
}

#[test]
fn source_guard_construction_reserves_distinct_prefix_and_live_calls_before_copying() {
    let caller = Program::parse("é", "Read {}").unwrap();
    let span = &caller.statements[0].span;
    let original = BWErr::ResourceLimit {
        resource: "source bytes",
        limit: 4,
    };
    let frame = CallFrame {
        signature: "read".into(),
        call_site: span.clone(),
        definition_site: None,
    };
    let expected = DiagnosticLimits::default().source_prefix(
        original.clone(),
        "é",
        "Read {}",
        4,
        std::iter::once(&frame),
    );
    let size = DiagnosticLimits::default().check(&expected).unwrap();
    for deficit in [0, 1, 2, 3, 5, 6] {
        let mut limits = RetainedDiagnosticLimits {
            records: 2,
            diagnostics: size.diagnostics,
            call_frames: size.call_frames + 1,
            related_locations: 0,
            text_bytes: size.text_bytes + "read".len(),
            source_bytes: size.source_bytes,
        };
        match deficit {
            1 => limits.records -= 1,
            2 => limits.diagnostics -= 1,
            3 => limits.call_frames -= 1,
            5 => limits.text_bytes -= 1,
            6 => limits.source_bytes -= 1,
            _ => (),
        }
        let mut context = Context::with_limits(RunLimits {
            retained_diagnostics: limits,
            ..Default::default()
        })
        .unwrap();
        context
            .calls
            .push(context.retain_call("read", span, None).unwrap());
        let sibling = context.clone();
        let error = context.ast_error(ast::AstFailure::SourceGuard {
            error: &original,
            name: "é",
            prefix: "Read {}",
            offset: 4,
        });
        if deficit == 0 {
            assert_eq!(error.to_string(), expected.to_string());
            assert!(!Arc::ptr_eq(
                error.span.as_ref().unwrap().source(),
                &caller.source
            ));
            let owner = Arc::downgrade(error.span.as_ref().unwrap().source());
            assert!(sibling
                .clone()
                .retain_handler(Diagnostic::new(BWErr::NativeError("probe".into())))
                .is_err());
            drop(error);
            assert!(owner.upgrade().is_none());
        } else {
            assert!(error.is_emergency());
            assert!(error.span.is_none() && error.call_stack.is_empty());
            let evidence = error.causes[0].omissions.as_ref().unwrap();
            assert_eq!(evidence.call_frames, 1);
            let origin = evidence.source.as_ref().unwrap();
            assert_eq!(
                (&*origin.file, origin.start_byte, origin.end_byte),
                ("é", 4, 7)
            );
        }
        assert!(context.checkpoint().is_err());
        sibling.checkpoint().unwrap();
    }
}

#[test]
fn parser_reporters_keep_reservations_until_disposal_or_public_transfer() {
    for source in ["|x| = |1 +|", "Return", "Read |x| and |x| {}"] {
        let context = Context::with_limits(RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records: 1,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        let sibling = context.clone();
        let first = context.parse_source("first", source).unwrap_err();
        let owner = Arc::downgrade(first.span.as_ref().unwrap().source());
        let second = sibling.clone().parse_source("second", source).unwrap_err();
        assert!(second.is_emergency());
        assert_eq!(second.causes[0].code(), first.code());
        assert!(second.causes[0].omissions.is_some());
        context.checkpoint().unwrap();
        let published = first.into_diagnostic();
        let next = sibling.parse_source("next", source).unwrap_err();
        assert_eq!(next.code(), published.code());
        assert!(owner.upgrade().is_some());
        drop(published);
        assert!(owner.upgrade().is_none());
        drop(next);
        assert_eq!(
            sibling.parse_source("again", source).unwrap_err().code(),
            second.causes[0].code()
        );
    }
}

#[test]
fn stopped_source_construction_admits_both_snapshots_and_preserves_control_on_rejection() {
    for reject in [false, true] {
        let control = OperationControl::default();
        let mut context = Context::with_control(
            RunLimits {
                retained_diagnostics: RetainedDiagnosticLimits {
                    diagnostics: if reject { 1 } else { 2 },
                    ..Default::default()
                },
                ..Default::default()
            },
            control.clone(),
        )
        .unwrap();
        let caller = Program::parse("caller", "Read").unwrap();
        context.calls.push(
            context
                .retain_call("read", &caller.statements[0].span, None)
                .unwrap(),
        );
        control.cancel();
        let error = context.ast_error(ast::AstFailure::SourceGuard {
            error: &BWErr::ResourceLimit {
                resource: "source bytes",
                limit: 0,
            },
            name: "é",
            prefix: "🙂",
            offset: 0,
        });
        assert_eq!(error.code(), DiagnosticCode::Cancelled);
        if reject {
            assert!(error.is_emergency());
            assert!(error.span.is_none() && error.call_stack.is_empty());
            assert_eq!(error.omissions.as_ref().unwrap().direct_causes, 1);
            assert_eq!(error.omissions.as_ref().unwrap().call_frames, 1);
        } else {
            assert_eq!(error.call_stack.len(), 1);
            assert_eq!(error.causes[0].call_stack.len(), 1);
            assert_eq!(error.causes[0].span.as_ref().unwrap().text(), "🙂");
        }
        assert_eq!(
            context.checkpoint().unwrap_err().code(),
            DiagnosticCode::Cancelled
        );
    }
}

#[test]
fn raw_ast_limit_and_lowering_reporters_keep_all_context_reserved_through_transfer() {
    for lowered in [false, true] {
        let caller = Program::parse("caller", "Read").unwrap();
        let input = Program::parse("input-é", "|value| = |1|").unwrap();
        let span = &input.statements[0].span;
        let failure = || {
            if lowered {
                ast::AstFailure::Lowering {
                    part: "expression",
                    span: Some(span),
                }
            } else {
                ast::AstFailure::Limit {
                    resource: "AST nodes",
                    maximum: 0,
                    span: Some(span),
                }
            }
        };
        let frame = CallFrame {
            signature: "read".into(),
            call_site: caller.statements[0].span.clone(),
            definition_site: None,
        };
        let expected = failure().diagnostic(&DiagnosticLimits::default(), std::iter::once(&frame));
        let size = DiagnosticLimits::default().check(&expected).unwrap();
        for deficit in [0, 1, 2, 3, 4, 5] {
            let mut limits = RetainedDiagnosticLimits {
                records: 2,
                diagnostics: 1,
                call_frames: 2,
                related_locations: 0,
                text_bytes: size.text_bytes + "read".len(),
                source_bytes: size.source_bytes,
            };
            match deficit {
                1 => limits.records -= 1,
                2 => limits.diagnostics -= 1,
                3 => limits.call_frames -= 1,
                4 => limits.text_bytes -= 1,
                5 => limits.source_bytes -= 1,
                _ => (),
            }
            let mut context = Context::with_limits(RunLimits {
                retained_diagnostics: limits,
                ..Default::default()
            })
            .unwrap();
            context
                .calls
                .push(context.retain_call("read", &frame.call_site, None).unwrap());
            let sibling = context.clone();
            let error = context.ast_error(failure());
            if deficit == 0 {
                assert_eq!(error.to_string(), expected.to_string());
                assert!(sibling.clone().ast_error(failure()).is_emergency());
                let public = error.into_diagnostic();
                assert!(!sibling.ast_error(failure()).is_emergency());
                assert_eq!(public.to_string(), expected.to_string());
            } else {
                assert!(error.is_emergency());
                let original = &error.causes[0];
                assert_eq!(original.code(), expected.code());
                let omitted = original.omissions.as_ref().unwrap();
                assert_eq!(omitted.call_frames, 1);
                assert_eq!(omitted.source.as_ref().unwrap().file, "input-é");
                assert!(original.span.is_none());
                assert!(context.checkpoint().is_err());
            }
        }
    }
}
