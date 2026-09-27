#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode,
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
    run::{Engine, RetainedDiagnosticLimits, RunLimits, RunOptions, RunOutcome},
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn options(retained_diagnostics: RetainedDiagnosticLimits) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            retained_diagnostics,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

#[test]
fn rethrow_requires_copy_headroom_before_new_metadata_and_preserves_prior_effects() {
    let source = "|error| = |7|\nTry { Try { Missing } Catch |error| { |progress| = |1|\nRethrow } } Catch |error| { |handled| = |true| }";
    for records in [1, 2] {
        let run = Engine::default().run_source(
            "copy",
            source,
            options(RetainedDiagnosticLimits {
                records,
                diagnostics: 2,
                call_frames: 0,
                related_locations: 1,
                text_bytes: 35, // Two labels/names plus the new "rethrow" message.
                source_bytes: "copy".len() + source.len(),
            }),
        );
        assert_eq!(run.variables["error"].to_string(), "7");
        assert_eq!(run.variables["progress"].to_string(), "1");
        assert_eq!(run.variables.contains_key("handled"), records == 2);
        if records == 1 {
            assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
            let error = run.result.unwrap_err();
            assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
            assert_eq!(
                error.causes[0]
                    .omissions
                    .as_ref()
                    .unwrap()
                    .related_locations,
                1
            );
            assert!(error.causes[0].span.is_none());
        } else {
            assert_eq!(run.outcome(), RunOutcome::Succeeded);
        }
    }
}

#[test]
fn rethrow_copy_reservations_release_between_caught_failures_and_latch_only_the_requester() {
    let mut context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 2,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let recovered = Program::parse(
        "recover",
        "Try { Try { Missing } Catch { Rethrow } } Catch {}",
    )
    .unwrap();
    for _ in 0..100 {
        evaluate_program_detailed(&recovered, &mut context).unwrap();
    }
    let mut context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 1,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let mut sibling = context.clone();
    assert_eq!(
        evaluate_program_detailed(&recovered, &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    let next = Program::parse(
        "next",
        "Try { Missing } Catch { |ok| = |true| }\nIf |ok == false| { Unexpected }",
    )
    .unwrap();
    assert!(evaluate_program_detailed(&next, &mut context).is_err());
    evaluate_program_detailed(&next, &mut sibling).unwrap();
}

#[test]
fn exact_handler_metrics_and_each_reduced_quota_preserve_bounded_original_evidence() {
    let source = "Try { Missing } Catch { |handled| = |true| }";
    let exact = RetainedDiagnosticLimits {
        records: 1,
        diagnostics: 1,
        call_frames: 0,
        related_locations: 0,
        text_bytes: 14, // label "source" plus the exact invocation text "Missing "
        source_bytes: "handler".len() + source.len(),
    };
    let run = Engine::default().run_source("handler", source, options(exact.clone()));
    assert_eq!(
        run.outcome(),
        RunOutcome::Succeeded,
        "{}",
        run.result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default()
    );
    assert_eq!(run.variables["handled"].to_string(), "true");
    for field in [0, 1, 4, 5] {
        let mut limits = exact.clone();
        match field {
            0 => limits.records -= 1,
            1 => limits.diagnostics -= 1,
            4 => limits.text_bytes -= 1,
            _ => limits.source_bytes -= 1,
        }
        let run = Engine::default().run_source("handler", source, options(limits));
        assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
        assert!(!run.variables.contains_key("handled"));
        let error = run.result.unwrap_err();
        assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
        assert!(error.causes[0].omissions.is_some());
        assert!(error.span.is_none() && error.call_stack.is_empty());
    }
}

#[test]
fn calls_reserve_exact_metadata_before_callback_entry_and_release_between_calls() {
    let entries = Arc::new(AtomicUsize::new(0));
    let entered = entries.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Go", move |_, _| {
            entered.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let exact = RetainedDiagnosticLimits {
        records: 1,
        diagnostics: 0,
        call_frames: 1,
        related_locations: 0,
        text_bytes: 2,
        source_bytes: 6,
    };
    assert_eq!(
        engine
            .run_source("call", "Go", options(exact.clone()))
            .outcome(),
        RunOutcome::Succeeded
    );
    for field in 0..4 {
        let mut limits = exact.clone();
        match field {
            0 => limits.records = 0,
            1 => limits.call_frames = 0,
            2 => limits.text_bytes = 1,
            _ => limits.source_bytes = 5,
        }
        assert_eq!(
            engine.run_source("call", "Go", options(limits)).outcome(),
            RunOutcome::LimitExceeded
        );
    }
    assert_eq!(entries.load(Ordering::SeqCst), 1);
    let run = engine.run_source(
        "call",
        "Go\nGo\nGo",
        options(RetainedDiagnosticLimits {
            source_bytes: 12,
            ..exact
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert_eq!(entries.load(Ordering::SeqCst), 4);
}

#[test]
fn nested_handlers_require_live_capacity_and_restore_the_previous_binding() {
    let source = "|error| = |7|\nTry { First } Catch |error| { Try { Second } Catch |error| { |handled| = |true| } }";
    for records in [1, 2] {
        let run = Engine::default().run_source(
            "nested",
            source,
            options(RetainedDiagnosticLimits {
                records,
                ..RetainedDiagnosticLimits::default()
            }),
        );
        assert_eq!(run.variables["error"].to_string(), "7");
        assert_eq!(run.variables.contains_key("handled"), records == 2);
        assert_eq!(
            run.outcome(),
            if records == 1 {
                RunOutcome::LimitExceeded
            } else {
                RunOutcome::Succeeded
            }
        );
    }
    let run = Engine::default().run_source(
        "reuse",
        "Try { First } Catch {}\nTry { Second } Catch {}",
        options(RetainedDiagnosticLimits {
            records: 1,
            ..RetainedDiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
}

#[test]
fn outer_handlers_admit_related_locations_and_complete_cause_trees() {
    for (source, related) in [
        (
            "Try { Try { Missing } Catch { Rethrow } } Catch { |handled| = |true| }",
            true,
        ),
        (
            "Try { Try { Missing } Catch { Other } } Catch { |handled| = |true| }",
            false,
        ),
    ] {
        for fits in [false, true] {
            let mut limits = RetainedDiagnosticLimits {
                records: 2, // Original handler and outgoing failure/copy coexist.
                ..RetainedDiagnosticLimits::default()
            };
            if related {
                limits.related_locations = usize::from(fits);
            } else {
                limits.diagnostics = 1 + usize::from(fits);
            }
            let run = Engine::default().run_source("causes", source, options(limits));
            assert_eq!(
                run.outcome(),
                if fits {
                    RunOutcome::Succeeded
                } else {
                    RunOutcome::LimitExceeded
                }
            );
            assert_eq!(run.variables.contains_key("handled"), fits);
        }
    }
}

#[test]
fn failed_handler_admission_releases_active_calls_and_only_latches_its_context() {
    let limits = RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 1,
            ..RetainedDiagnosticLimits::default()
        },
        ..RunLimits::default()
    };
    let mut context = Context::with_limits(limits).unwrap();
    context
        .register_native("Go", |_| Ok(Literal::None))
        .unwrap();
    let mut sibling = context.clone();
    let program = Program::parse("overlap", "Outer { Try { Missing } Catch {} }\nOuter").unwrap();
    let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    let simple = Program::parse("reuse", "Go").unwrap();
    assert!(evaluate_program_detailed(&simple, &mut context).is_err());
    evaluate_program_detailed(&simple, &mut sibling).unwrap();
}

#[test]
fn required_argument_effects_precede_call_record_admission() {
    let effects = Arc::new(AtomicUsize::new(0));
    let called = effects.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Touch", move |_, _| {
            called.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(1))
        })
        .unwrap();
    engine
        .register_native("Outer name |x|", |_, _| panic!("callee must not enter"))
        .unwrap();
    let run = engine.run_source(
        "arguments",
        "Outer name |@{ Touch }|",
        options(RetainedDiagnosticLimits {
            text_bytes: 5,
            ..RetainedDiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[test]
fn imported_calls_share_inherited_records_and_deduplicate_source_owners() {
    let harness = cli_harness::Harness::new();
    let module = "Value { Return |7| }";
    let path = harness.workspace.join("module.botwork");
    std::fs::write(&path, module).unwrap();
    let source =
        "Import |\"module.botwork\"| As |lib|\nWrapper { Return |@{ lib::Value }| }\n|x| = Wrapper";
    let bytes = "entry".len()
        + source.len()
        + path.canonicalize().unwrap().to_string_lossy().len()
        + module.len();
    for fits in [false, true] {
        let mut configuration = options(RetainedDiagnosticLimits {
            records: 2,
            call_frames: 2,
            source_bytes: bytes - usize::from(!fits),
            ..RetainedDiagnosticLimits::default()
        });
        configuration.working_directory = Some(harness.workspace.clone());
        let run = Engine::default().run_source("entry", source, configuration);
        assert_eq!(
            run.outcome(),
            if fits {
                RunOutcome::Succeeded
            } else {
                RunOutcome::LimitExceeded
            }
        );
        if fits {
            assert_eq!(run.variables["x"].to_string(), "7");
        } else {
            assert!(!run.variables.contains_key("x"));
        }
    }
}

#[test]
fn concurrent_engine_runs_have_independent_live_trackers() {
    use std::{
        sync::{mpsc, Mutex},
        time::Duration,
    };
    let mut engine = Engine::default();
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    engine
        .register_native("Hold", move |_, _| {
            held_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(Literal::None)
        })
        .unwrap();
    engine
        .register_native("Go", |_, _| Ok(Literal::None))
        .unwrap();
    let engine = Arc::new(engine);
    let worker = engine.clone();
    let limits = RetainedDiagnosticLimits {
        records: 1,
        ..RetainedDiagnosticLimits::default()
    };
    let worker_limits = limits.clone();
    let thread = std::thread::spawn(move || {
        worker
            .run_source("held", "Hold", options(worker_limits))
            .outcome()
    });
    held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let outcome = engine
        .run_source("independent", "Go", options(limits))
        .outcome();
    release_tx.send(()).unwrap();
    assert_eq!(thread.join().unwrap(), RunOutcome::Succeeded);
    assert_eq!(outcome, RunOutcome::Succeeded);
}

#[test]
fn default_and_raised_limits_allow_call_handler_overlap_with_native_errors() {
    let defaults = RetainedDiagnosticLimits::default();
    assert_eq!(
        (
            defaults.records,
            defaults.diagnostics,
            defaults.call_frames,
            defaults.related_locations
        ),
        (65_536, 16_384, 65_536, 65_536)
    );
    assert_eq!(
        (defaults.text_bytes, defaults.source_bytes),
        (32 * 1024 * 1024, 32 * 1024 * 1024)
    );
    let mut engine = Engine::default();
    engine
        .register_native("Fail", |_, _| Err(BWErr::NativeError("reason".into())))
        .unwrap();
    for records in [3, defaults.records, defaults.records + 1] {
        let run = engine.run_source(
            "overlap",
            "Outer { Try { Fail } Catch {} }\nOuter",
            options(RetainedDiagnosticLimits {
                records,
                ..defaults.clone()
            }),
        );
        assert_eq!(run.outcome(), RunOutcome::Succeeded);
    }
}

#[test]
fn outgoing_handler_causes_require_capacity_without_an_outer_catch() {
    let source = "|error| = |7|\nTry { Missing } Catch |error| { |progress| = |1|\nOther }";
    for diagnostics in [1, 2] {
        let run = Engine::default().run_source(
            "outgoing",
            source,
            options(RetainedDiagnosticLimits {
                records: 2, // The original handler remains live during the new failure.
                diagnostics,
                ..Default::default()
            }),
        );
        assert_eq!(run.variables["error"].to_string(), "7");
        assert_eq!(run.variables["progress"].to_string(), "1");
        let error = run.result.unwrap_err();
        if diagnostics == 1 {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert!(matches!(
                error.error.as_ref(),
                BWErr::ResourceLimit {
                    resource: "retained diagnostic nodes",
                    limit: 1
                }
            ));
            assert!(
                matches!(error.causes[0].error.as_ref(), BWErr::StatementNotDefined(name) if name.trim() == "Other")
            );
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().direct_causes, 1);
        } else {
            assert_eq!(error.code(), DiagnosticCode::UndefinedStatement);
            assert!(
                matches!(error.error.as_ref(), BWErr::StatementNotDefined(name) if name.trim() == "Other")
            );
            assert!(
                matches!(error.causes[0].error.as_ref(), BWErr::StatementNotDefined(name) if name.trim() == "Missing")
            );
            assert!(error.causes[0].omissions.is_none());
        }
    }
}

#[test]
fn import_unwinding_reserves_parent_sites_before_outgoing_publication() {
    let harness = cli_harness::Harness::new();
    std::fs::write(harness.workspace.join("leaf.botwork"), "Missing").unwrap();
    std::fs::write(
        harness.workspace.join("middle.botwork"),
        "Import |\"leaf.botwork\"| As |leaf|",
    )
    .unwrap();
    let source = "|progress| = |1|\nImport |\"middle.botwork\"| As |lib|\n|late| = |true|";
    for related_locations in [0, 1, 2] {
        let mut configuration = options(RetainedDiagnosticLimits {
            records: 1,
            related_locations,
            ..Default::default()
        });
        configuration.working_directory = Some(harness.workspace.clone());
        let run = Engine::default().run_source("entry", source, configuration);
        assert_eq!(run.variables["progress"].to_string(), "1");
        assert!(!run.variables.contains_key("late"));
        let error = run.result.unwrap_err();
        if related_locations == 2 {
            assert_eq!(error.code(), DiagnosticCode::UndefinedStatement);
            assert_eq!(error.related.len(), 2);
            assert!(error.related[0]
                .span
                .source()
                .name()
                .ends_with("middle.botwork"));
            assert_eq!(error.related[1].span.source().name(), "entry");
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert!(matches!(
                error.error.as_ref(),
                BWErr::ResourceLimit {
                    resource: "retained diagnostic related locations",
                    ..
                }
            ));
            assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
            let omitted = error.causes[0].omissions.as_ref().unwrap();
            assert_eq!(omitted.related_locations, 2);
            assert!(omitted
                .source
                .as_ref()
                .unwrap()
                .file
                .ends_with("leaf.botwork"));
            assert!(error.related.is_empty());
        }
    }
}

#[test]
fn native_error_context_requires_exact_overlap_with_the_active_call() {
    let entered = Arc::new(AtomicUsize::new(0));
    let count = entered.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Fail", move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Err(BWErr::NativeError("reason".into()))
        })
        .unwrap();
    for deficit in 0..=4 {
        let mut limits = RetainedDiagnosticLimits {
            records: 2,
            diagnostics: 1,
            call_frames: 2,
            related_locations: 0,
            text_bytes: 20, // Active "fail" plus error label, reason, and copied "fail".
            source_bytes: "peakFail".len(),
        };
        match deficit {
            1 => limits.records -= 1,
            2 => limits.diagnostics -= 1,
            3 => limits.call_frames -= 1,
            4 => limits.text_bytes -= 1,
            _ => (),
        }
        let run = engine.run_source("peak", "Fail", options(limits));
        let error = run.result.unwrap_err();
        if deficit == 0 {
            assert_eq!(error.code(), DiagnosticCode::Native);
            assert_eq!(error.call_stack.len(), 1);
            assert_eq!(error.call_stack[0].signature, "fail");
            assert_eq!(error.span.as_ref().unwrap().source().name(), "peak");
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().call_frames, 1);
        }
        assert_eq!(entered.load(Ordering::SeqCst), deficit + 1);
    }
}

#[test]
fn aggregate_context_failure_keeps_cancellation_primary_and_skips_catch() {
    let mut engine = Engine::default();
    engine
        .register_native("Stop", |_, environment| {
            environment.control().cancel();
            Err(BWErr::NativeError("reason".into()))
        })
        .unwrap();
    for (records, call_frames) in [(1, 3), (2, 2), (2, 3)] {
        let run = engine.run_source(
            "cancel",
            "Try { Stop } Catch { |unexpected| = |true| }",
            options(RetainedDiagnosticLimits {
                records,
                call_frames,
                ..Default::default()
            }),
        );
        assert_eq!(run.outcome(), RunOutcome::Cancelled);
        assert!(!run.variables.contains_key("unexpected"));
        let error = run.result.unwrap_err();
        if records == 1 || call_frames == 2 {
            assert!(error.omissions.is_some());
            assert_eq!(error.omissions.as_ref().unwrap().call_frames, 1);
            assert_eq!(error.omissions.as_ref().unwrap().direct_causes, 1);
            assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
            assert_eq!(
                error
                    .omissions
                    .as_ref()
                    .unwrap()
                    .source
                    .as_ref()
                    .unwrap()
                    .file,
                "cancel"
            );
        } else {
            assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
            assert_eq!(error.call_stack.len(), 1);
            assert_eq!(error.causes[0].call_stack.len(), 1);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn aggregate_context_rejection_preserves_an_observed_timeout_but_not_an_unobserved_category()
{
    use botwork::core::operation::OperationControl;
    let control = OperationControl::default().child(Some(
        tokio::time::Instant::now() + std::time::Duration::from_secs(1),
    ));
    let mut context = Context::with_control(
        RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records: 0,
                ..Default::default()
            },
            ..Default::default()
        },
        control,
    )
    .unwrap();
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let error = evaluate_program_detailed(&Program::parse("timeout", "").unwrap(), &mut context)
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert!(error.omissions.is_some());
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);

    let mut engine = Engine::default();
    engine
        .register_native("Reported", |_, _| {
            Err(BWErr::Cancelled("host category only".into()))
        })
        .unwrap();
    let run = engine.run_source(
        "reported",
        "Reported",
        options(RetainedDiagnosticLimits {
            records: 1,
            ..Default::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Cancelled);
}
