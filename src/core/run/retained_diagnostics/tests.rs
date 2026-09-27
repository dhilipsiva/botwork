use super::*;
use crate::core::ast::Program;

fn source() -> Arc<SourceFile> {
    Program::parse("é", "|x| = |1|").unwrap().source
}
fn size() -> DiagnosticSize {
    DiagnosticSize {
        diagnostics: 1,
        call_frames: 1,
        related_locations: 1,
        text_bytes: 12,
        ..DiagnosticSize::default()
    }
}

#[test]
fn replacement_merges_ownership_and_replaces_source_identities_without_peak_double_charging() {
    let old = source();
    let new = source();
    let bytes = old.name().len() + old.text().len();
    let tracker = Arc::new(RetainedDiagnostics::new(RetainedDiagnosticLimits {
        records: 2,
        diagnostics: 2,
        call_frames: 2,
        related_locations: 2,
        text_bytes: 24,
        source_bytes: bytes,
    }));
    let mut previous = vec![
        tracker.reserve(size(), vec![old.clone()]).unwrap(),
        tracker.reserve(size(), vec![old.clone()]).unwrap(),
    ];
    let merged = DiagnosticSize {
        diagnostics: 2,
        call_frames: 2,
        related_locations: 2,
        text_bytes: 24,
        ..Default::default()
    };
    let reservation = tracker
        .replace(merged, vec![old.clone()], &mut previous)
        .unwrap();
    assert!(previous.is_empty());
    assert_eq!(tracker.used.lock().unwrap().counts, [1, 2, 2, 2, 24]);
    assert_eq!(tracker.used.lock().unwrap().sources[&source_id(&old)], 1);
    let mut previous = vec![reservation];
    let reservation = tracker
        .replace(merged, vec![new.clone()], &mut previous)
        .unwrap();
    let used = tracker.used.lock().unwrap();
    assert_eq!(used.source_bytes, bytes);
    assert!(!used.sources.contains_key(&source_id(&old)));
    assert_eq!(used.sources[&source_id(&new)], 1);
    drop(used);
    drop(reservation);
    assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
    assert_eq!(tracker.used.lock().unwrap().source_bytes, 0);
}

#[test]
fn failed_replacement_preserves_old_reservations_for_every_dimension_and_overflow() {
    for dimension in 0..6 {
        let source = source();
        let other_source = Program::parse("other", "|x| = |2|").unwrap().source;
        let bytes = source.name().len() + source.text().len();
        let tracker = Arc::new(RetainedDiagnostics::new(RetainedDiagnosticLimits {
            records: 1,
            diagnostics: 1,
            call_frames: 1,
            related_locations: 1,
            text_bytes: 12,
            source_bytes: bytes,
        }));
        let held = tracker.reserve(size(), vec![source.clone()]).unwrap();
        let unrelated = Arc::new(RetainedDiagnostics::new(RetainedDiagnosticLimits::default()));
        let mut previous = vec![if dimension == 0 {
            unrelated.reserve(size(), vec![]).unwrap()
        } else {
            held
        }];
        // Keep the requester's occupied slot for a cross-pool failed transfer.
        let mut replacement = size();
        let mut sources = vec![source.clone()];
        match dimension {
            0 => (),
            1 => replacement.diagnostics += 1,
            2 => replacement.call_frames += 1,
            3 => replacement.related_locations += 1,
            4 => replacement.text_bytes += 1,
            _ => sources.push(other_source),
        }
        let before = tracker.used.lock().unwrap().counts;
        assert!(tracker
            .replace(replacement, sources, &mut previous)
            .is_err());
        assert_eq!(previous.len(), 1);
        assert_eq!(tracker.used.lock().unwrap().counts, before);
        assert_eq!(tracker.used.lock().unwrap().source_bytes, bytes);
        assert_eq!(previous[0].charge, [1, 1, 1, 1, 12]);
    }
    let tracker = Arc::new(RetainedDiagnostics::new(RetainedDiagnosticLimits {
        records: usize::MAX,
        diagnostics: usize::MAX,
        ..Default::default()
    }));
    let mut previous = vec![tracker
        .reserve(
            DiagnosticSize {
                diagnostics: 1,
                ..Default::default()
            },
            vec![],
        )
        .unwrap()];
    tracker.used.lock().unwrap().counts[1] = usize::MAX;
    assert!(tracker
        .replace(
            DiagnosticSize {
                diagnostics: 2,
                ..Default::default()
            },
            vec![],
            &mut previous
        )
        .is_err());
    assert_eq!(tracker.used.lock().unwrap().counts[1], usize::MAX);
    assert_eq!(previous[0].charge[1], 1);
}

fn runtime_error(budget: &RunBudget, name: &str) -> RuntimeDiagnostic {
    let program = Program::parse(name, "|x| = |1|").unwrap();
    let value = Diagnostic::new(BWErr::NativeError(name.into())).at(&program.statements[0].span);
    StoredDiagnostic {
        _reservation: Some(budget.reserve_diagnostic(&value).unwrap()),
        value,
    }
    .into()
}

#[test]
fn cancellation_carries_original_ownership_or_releases_it_after_bounded_rejection() {
    use crate::core::eval::Context;
    for diagnostics in [1, 2] {
        let control = OperationControl::default();
        let context = Context::with_control(
            RunLimits {
                diagnostics: DiagnosticLimits {
                    diagnostics,
                    ..Default::default()
                },
                ..Default::default()
            },
            control.clone(),
        )
        .unwrap();
        let budget = context.budget.as_ref().unwrap();
        let error = runtime_error(budget, "original");
        let source = Arc::downgrade(error.span.as_ref().unwrap().source());
        control.cancel();
        let result = context.after_evaluation::<()>(Err(error)).unwrap_err();
        let result = context.runtime_diagnostic(result, None, false);
        assert_eq!(result.code(), DiagnosticCode::Cancelled);
        assert_eq!(source.upgrade().is_some(), diagnostics == 2);
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
            usize::from(diagnostics == 2)
        );
        if diagnostics == 2 {
            assert_eq!(result.causes[0].code(), DiagnosticCode::Native);
        } else {
            assert!(result.is_emergency());
        }
        drop(result);
        assert!(source.upgrade().is_none());
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts,
            [0; 5]
        );
    }
}

#[test]
fn runtime_carrier_admits_merged_records_before_cause_attachment() {
    let budget = RunBudget::new(
        RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records: 2,
                ..Default::default()
            },
            ..Default::default()
        },
        OperationControl::default(),
    );
    let first = runtime_error(&budget, "first");
    let identity = first.error.clone();
    let second = runtime_error(&budget, "second");
    let merged = first.while_handling(second, Some(&budget));
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
        1
    );
    let stored = merged.store(Some(&budget)).unwrap();
    assert!(Arc::ptr_eq(&stored.value.error, &identity));
    assert_eq!(stored.value.causes.len(), 1);
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
        1
    );
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts[1],
        2
    );
    let outgoing = RuntimeDiagnostic::from(StoredDiagnostic::take(stored, Some(&budget)).unwrap());
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
        1
    );
    let public = outgoing.into_diagnostic();
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts,
        [0; 5]
    );
    assert_eq!(public.causes.len(), 1);
}

#[test]
fn runtime_disposal_rejection_and_publication_release_source_owners_and_deep_raw_errors() {
    for action in 0..3 {
        let budget = RunBudget::new(RunLimits::default(), OperationControl::default());
        let error = runtime_error(&budget, "original");
        let source = Arc::downgrade(error.span.as_ref().unwrap().source());
        match action {
            0 => drop(error),
            1 => {
                let error = error.reject(BWErr::ResourceLimit {
                    resource: "test",
                    limit: 0,
                });
                assert!(error.is_emergency());
                assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
                assert!(source.upgrade().is_none());
            }
            _ => {
                let error = error.into_diagnostic();
                assert!(source.upgrade().is_some());
                assert_eq!(
                    budget.0.retained_diagnostics.used.lock().unwrap().counts,
                    [0; 5]
                );
                error.discard();
            }
        }
        assert!(source.upgrade().is_none());
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts,
            [0; 5]
        );
        assert!(budget
            .0
            .retained_diagnostics
            .used
            .lock()
            .unwrap()
            .sources
            .is_empty());
    }
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut error = Diagnostic::new(BWErr::NativeError("leaf".into()));
            for _ in 0..100_000 {
                let mut parent = Diagnostic::new(BWErr::NativeError("parent".into()));
                parent.causes.push(error);
                error = parent;
            }
            drop(RuntimeDiagnostic::from(error));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn outgoing_rethrow_holds_shared_capacity_until_public_transfer_across_contexts() {
    use crate::core::eval::{evaluate_program_detailed, evaluate_program_runtime, Context};
    let context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 2,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let mut worker = context.clone();
    let mut rejected = context.clone();
    let mut reusable = context.clone();
    let (ready, received) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        let program = Program::parse("outgoing", "Try { Missing } Catch { Rethrow }").unwrap();
        let error = evaluate_program_runtime(&program, &mut worker).unwrap_err();
        ready.send(()).unwrap();
        gate.recv_timeout(Duration::from_secs(5)).unwrap();
        error.into_diagnostic()
    });
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    let recovered = Program::parse(
        "recover",
        "Try { Try { Missing } Catch { Rethrow } } Catch {}",
    )
    .unwrap();
    assert_eq!(
        evaluate_program_detailed(&recovered, &mut rejected)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    context.checkpoint().unwrap();
    release.send(()).unwrap();
    let public = task.join().unwrap();
    assert_eq!(public.code(), DiagnosticCode::UndefinedStatement);
    evaluate_program_detailed(&recovered, &mut reusable).unwrap();
    assert_eq!(
        context
            .budget
            .as_ref()
            .unwrap()
            .0
            .retained_diagnostics
            .used
            .lock()
            .unwrap()
            .counts,
        [0; 5]
    );
}

#[test]
fn records_share_source_charges_and_release_only_their_own_metrics() {
    let source = source();
    let bytes = source.name().len() + source.text().len();
    let tracker = Arc::new(RetainedDiagnostics::new(RetainedDiagnosticLimits {
        records: 2,
        source_bytes: bytes,
        ..RetainedDiagnosticLimits::default()
    }));
    let first = tracker.reserve(size(), vec![source.clone()]).unwrap();
    let second = tracker.reserve(size(), vec![source.clone()]).unwrap();
    assert_eq!(tracker.used.lock().unwrap().counts, [2, 2, 2, 2, 24]);
    assert_eq!(tracker.used.lock().unwrap().source_bytes, bytes);
    assert!(tracker.check_record().is_err());
    drop(first);
    assert_eq!(tracker.used.lock().unwrap().counts, [1, 1, 1, 1, 12]);
    assert_eq!(tracker.used.lock().unwrap().source_bytes, bytes);
    tracker.check_record().unwrap();
    drop(second);
    assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
    assert!(tracker.used.lock().unwrap().sources.is_empty());
    assert_eq!(tracker.used.lock().unwrap().source_bytes, 0);
}

#[test]
fn every_dimension_rejects_atomically_including_distinct_equal_sources() {
    let source = source();
    for field in 0..6 {
        let mut limits = RetainedDiagnosticLimits::default();
        match field {
            0 => limits.records = 0,
            1 => limits.diagnostics = 0,
            2 => limits.call_frames = 0,
            3 => limits.related_locations = 0,
            4 => limits.text_bytes = 11,
            _ => limits.source_bytes = 0,
        }
        let tracker = Arc::new(RetainedDiagnostics::new(limits));
        assert!(tracker.reserve(size(), vec![source.clone()]).is_err());
        assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
        assert!(tracker.used.lock().unwrap().sources.is_empty());
    }
    let bytes = source.name().len() + source.text().len();
    let tracker = Arc::new(RetainedDiagnostics::new(RetainedDiagnosticLimits {
        source_bytes: bytes,
        ..RetainedDiagnosticLimits::default()
    }));
    let first = tracker.reserve(size(), vec![source.clone()]).unwrap();
    let second = Program::parse(source.name(), source.text()).unwrap().source;
    assert!(tracker.reserve(size(), vec![second]).is_err());
    assert_eq!(tracker.used.lock().unwrap().counts, [1, 1, 1, 1, 12]);
    drop(first);
}

#[test]
fn overflow_in_counts_source_bytes_or_reference_counts_never_commits_partial_usage() {
    let source = source();
    let unlimited = RetainedDiagnosticLimits {
        records: usize::MAX,
        diagnostics: usize::MAX,
        call_frames: usize::MAX,
        related_locations: usize::MAX,
        text_bytes: usize::MAX,
        source_bytes: usize::MAX,
    };
    for index in 0..5 {
        let tracker = Arc::new(RetainedDiagnostics::new(unlimited.clone()));
        tracker.used.lock().unwrap().counts[index] = usize::MAX;
        let before = tracker.used.lock().unwrap().counts;
        assert!(tracker.reserve(size(), vec![source.clone()]).is_err());
        assert_eq!(tracker.used.lock().unwrap().counts, before);
        assert!(tracker.used.lock().unwrap().sources.is_empty());
    }
    let tracker = Arc::new(RetainedDiagnostics::new(unlimited.clone()));
    tracker.used.lock().unwrap().source_bytes = usize::MAX;
    assert!(tracker.reserve(size(), vec![source.clone()]).is_err());
    assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
    let tracker = Arc::new(RetainedDiagnostics::new(unlimited));
    tracker
        .used
        .lock()
        .unwrap()
        .sources
        .insert(source_id(&source), usize::MAX);
    assert!(tracker.reserve(size(), vec![source.clone()]).is_err());
    assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
    assert_eq!(
        tracker.used.lock().unwrap().sources[&source_id(&source)],
        usize::MAX
    );
}

#[test]
fn concurrent_admission_shares_live_record_capacity() {
    let tracker = Arc::new(RetainedDiagnostics::new(RetainedDiagnosticLimits {
        records: 1,
        ..RetainedDiagnosticLimits::default()
    }));
    let worker = tracker.clone();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let reservation = worker.reserve(size(), vec![]).unwrap();
        held_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(reservation);
    });
    held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(tracker.reserve(size(), vec![]).is_err());
    release_tx.send(()).unwrap();
    thread.join().unwrap();
    drop(tracker.reserve(size(), vec![]).unwrap());
    assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
}

#[test]
fn stored_diagnostics_transfer_unique_metadata_and_keep_shared_owners_charged() {
    for shared in [false, true] {
        let program = Program::parse("source", "|x| = |1|").unwrap();
        let weak = Arc::downgrade(&program.source);
        let mut diagnostic =
            Diagnostic::new(BWErr::NativeError("reason".into())).at(&program.statements[0].span);
        diagnostic
            .related
            .push(crate::core::diagnostic::RelatedLocation {
                message: "metadata".into(),
                span: program.statements[0].span.clone(),
            });
        let pointer = diagnostic.related[0].message.as_ptr();
        let budget = RunBudget::new(RunLimits::default(), OperationControl::default());
        let reservation = budget.reserve_diagnostic(&diagnostic).unwrap();
        let stored = Arc::new(StoredDiagnostic {
            value: diagnostic,
            _reservation: Some(reservation),
        });
        drop(program);
        let other = shared.then(|| stored.clone());
        let copied = StoredDiagnostic::take(stored, Some(&budget)).unwrap();
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
            1 + usize::from(shared)
        );
        let value = RuntimeDiagnostic::from(copied).into_diagnostic();
        if !shared {
            assert_eq!(value.related[0].message.as_ptr(), pointer);
        }
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
            usize::from(shared)
        );
        drop(other);
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
            0
        );
        assert!(weak.upgrade().is_some());
        value.discard();
        assert!(weak.upgrade().is_none());
    }
}

#[test]
fn copied_handler_admission_counts_every_dimension_and_the_prospective_related_site() {
    for dimension in 0..6 {
        for fits in [false, true] {
            let program = Program::parse("original", "|x| = |1|").unwrap();
            let site = Program::parse("rethrow-é", "|x| = |2|").unwrap();
            let span = &program.statements[0].span;
            let related = &site.statements[0].span;
            let mut error = Diagnostic::new(BWErr::NativeError("failed".into())).at(span);
            error.call_stack.push(crate::core::diagnostic::CallFrame {
                signature: "read".into(),
                call_site: span.clone(),
                definition_site: None,
            });
            let metrics = DiagnosticLimits::default().check(&error).unwrap();
            let exact = RetainedDiagnosticLimits {
                records: 2,
                diagnostics: 2 * metrics.diagnostics,
                call_frames: 2 * metrics.call_frames,
                related_locations: 1,
                text_bytes: 2 * metrics.text_bytes + "rethrow".len(),
                source_bytes: metrics.source_bytes
                    + site.source.name().len()
                    + site.source.text().len(),
            };
            let mut limits = exact.clone();
            if !fits {
                match dimension {
                    0 => limits.records -= 1,
                    1 => limits.diagnostics -= 1,
                    2 => limits.call_frames -= 1,
                    3 => limits.related_locations -= 1,
                    4 => limits.text_bytes -= 1,
                    _ => limits.source_bytes -= 1,
                }
            }
            let budget = RunBudget::new(
                RunLimits {
                    retained_diagnostics: limits,
                    ..Default::default()
                },
                OperationControl::default(),
            );
            let original = StoredDiagnostic {
                _reservation: Some(budget.reserve_diagnostic(&error).unwrap()),
                value: error,
            };
            let requester = budget.clone();
            let copied = original.copy(Some(&requester), Some(("rethrow", related)));
            assert_eq!(copied.is_ok(), fits);
            let tracker = &budget.0.retained_diagnostics;
            if let Ok(copied) = copied {
                assert!(Arc::ptr_eq(&copied.value.error, &original.value.error));
                assert_ne!(
                    copied.value.call_stack[0].signature.as_ptr(),
                    original.value.call_stack[0].signature.as_ptr()
                );
                assert_eq!(copied.value.related[0].span, *related);
                let used = tracker.used.lock().unwrap();
                assert_eq!(
                    used.counts,
                    [2, exact.diagnostics, exact.call_frames, 1, exact.text_bytes]
                );
                assert_eq!(used.source_bytes, exact.source_bytes);
                drop(used);
                drop(copied);
            } else {
                assert!(requester.checkpoint().is_err());
            }
            budget.checkpoint().unwrap();
            assert_eq!(
                tracker.used.lock().unwrap().counts,
                [1, 1, 1, 0, metrics.text_bytes]
            );
            assert_eq!(
                tracker.used.lock().unwrap().source_bytes,
                metrics.source_bytes
            );
            drop(original);
            assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
            assert!(tracker.used.lock().unwrap().sources.is_empty());
        }
    }
}

#[test]
fn copying_respects_individual_related_limits_and_observed_control_before_aggregate_limits() {
    let program = Program::parse("site", "|x| = |1|").unwrap();
    for stopped in [false, true] {
        let control = OperationControl::default();
        let budget = RunBudget::new(
            RunLimits {
                diagnostics: DiagnosticLimits {
                    related_locations: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            control.clone(),
        );
        let error = Diagnostic::new(BWErr::NativeError("original".into()));
        let original = StoredDiagnostic {
            _reservation: Some(budget.reserve_diagnostic(&error).unwrap()),
            value: error,
        };
        if stopped {
            control.cancel();
        }
        let error = original
            .copy(
                Some(&budget),
                Some(("rethrow", &program.statements[0].span)),
            )
            .err()
            .unwrap();
        assert_eq!(
            error.code(),
            if stopped {
                DiagnosticCode::Cancelled
            } else {
                DiagnosticCode::ResourceLimit
            }
        );
        if !stopped {
            assert!(matches!(
                error.error.as_ref(),
                BWErr::ResourceLimit {
                    resource: "diagnostic related locations",
                    ..
                }
            ));
        }
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
            1
        );
    }
}

#[test]
fn concurrent_copies_share_headroom_without_latching_the_original_context() {
    let budget = RunBudget::new(
        RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records: 2,
                ..Default::default()
            },
            ..Default::default()
        },
        OperationControl::default(),
    );
    let error = Diagnostic::new(BWErr::NativeError("original".into()));
    let original = Arc::new(StoredDiagnostic {
        _reservation: Some(budget.reserve_diagnostic(&error).unwrap()),
        value: error,
    });
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let requester = budget.clone();
            let original = original.clone();
            let (release, gate) = std::sync::mpsc::channel();
            releases.push(release);
            let tx = tx.clone();
            scope.spawn(move || {
                let copied = original.copy(Some(&requester), None);
                tx.send(copied.is_ok()).unwrap();
                gate.recv_timeout(Duration::from_secs(5)).unwrap();
                drop(copied);
            });
        }
        let successes = (0..2)
            .filter(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .count();
        assert_eq!(successes, 1);
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
            2
        );
        for release in releases {
            release.send(()).unwrap();
        }
    });
    budget.checkpoint().unwrap();
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
        1
    );
    drop(original);
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
        0
    );
}

#[test]
fn outgoing_mutations_admit_every_aggregate_dimension_and_release_on_rejection_or_transfer() {
    for cause in [false, true] {
        for deficit in 0..=6 {
            let program = Program::parse("primary-é", "Missing").unwrap();
            let other = Program::parse("other-é", "Other").unwrap();
            let span = &program.statements[0].span;
            let site = &other.statements[0].span;
            let mut primary = Diagnostic::new(BWErr::NativeError("primary".into())).at(span);
            primary.call_stack.push(CallFrame {
                signature: "frame".into(),
                call_site: span.clone(),
                definition_site: Some(site.clone()),
            });
            primary = primary.with_related("first", span);
            let original = Diagnostic::new(BWErr::ArithmeticError("original".into())).at(site);
            let expected = if cause {
                primary.clone().while_handling(original.clone())
            } else {
                primary.clone().with_related("added-é", site)
            };
            let size = DiagnosticLimits::default().check(&expected).unwrap();
            let mut retained = RetainedDiagnosticLimits {
                records: 1,
                diagnostics: size.diagnostics,
                call_frames: size.call_frames,
                related_locations: size.related_locations,
                text_bytes: size.text_bytes,
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
            let budget = RunBudget::new(
                RunLimits {
                    retained_diagnostics: retained,
                    ..Default::default()
                },
                OperationControl::default(),
            );
            let sibling = budget.clone();
            let result = if cause {
                RuntimeDiagnostic::from(primary).while_handling(original.into(), Some(&budget))
            } else {
                drop(original);
                RuntimeDiagnostic::from(primary).with_related("added-é", site, Some(&budget))
            };
            let sources = [
                Arc::downgrade(&program.source),
                Arc::downgrade(&other.source),
            ];
            drop((program, other));
            if deficit == 0 {
                assert_eq!(result.to_string(), expected.to_string());
                let used = budget.0.retained_diagnostics.used.lock().unwrap();
                assert_eq!(
                    used.counts,
                    [
                        1,
                        size.diagnostics,
                        size.call_frames,
                        size.related_locations,
                        size.text_bytes
                    ]
                );
                assert_eq!(used.source_bytes, size.source_bytes);
                drop(used);
                budget.checkpoint().unwrap();
            } else {
                assert!(result.is_emergency());
                assert_eq!(result.causes[0].code(), DiagnosticCode::Native);
                let omitted = result.causes[0].omissions.as_ref().unwrap();
                assert_eq!(omitted.direct_causes, usize::from(cause));
                assert_eq!(omitted.related_locations, if cause { 1 } else { 2 });
                assert_eq!(omitted.source.as_ref().unwrap().file, "primary-é");
                assert!(budget.checkpoint().is_err());
            }
            sibling.checkpoint().unwrap();
            drop(expected);
            assert_eq!(
                sources.iter().all(|source| source.upgrade().is_none()),
                deficit != 0
            );
            let public = result.into_diagnostic();
            assert_eq!(
                budget.0.retained_diagnostics.used.lock().unwrap().counts,
                [0; 5]
            );
            assert_eq!(
                budget
                    .0
                    .retained_diagnostics
                    .used
                    .lock()
                    .unwrap()
                    .source_bytes,
                0
            );
            drop(public);
            assert!(sources.iter().all(|source| source.upgrade().is_none()));
        }
    }
}

#[test]
fn mutation_replacement_retains_unrelated_records_and_refunds_failed_inputs() {
    for fits in [false, true] {
        let budget = RunBudget::new(
            RunLimits {
                retained_diagnostics: RetainedDiagnosticLimits {
                    records: 3,
                    diagnostics: 3,
                    related_locations: usize::from(fits),
                    ..Default::default()
                },
                ..Default::default()
            },
            OperationControl::default(),
        );
        let unrelated = runtime_error(&budget, "unrelated");
        let first = runtime_error(&budget, "first");
        let second = runtime_error(&budget, "second");
        let merged = first.while_handling(second, Some(&budget));
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
            2
        );
        let site = unrelated.span.as_ref().unwrap();
        let result = merged.with_related("new", site, Some(&budget));
        assert_eq!(result.is_emergency(), !fits);
        let counts = budget.0.retained_diagnostics.used.lock().unwrap().counts;
        assert_eq!(counts[0], 1 + usize::from(fits));
        assert_eq!(counts[1], 1 + 2 * usize::from(fits));
        drop(result);
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts[0],
            1
        );
        drop(unrelated);
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts,
            [0; 5]
        );
    }
}

#[test]
fn concurrent_outgoing_mutations_compete_for_one_record_without_poisoning_siblings() {
    let budget = RunBudget::new(
        RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        OperationControl::default(),
    );
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let accepted = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..2)
            .map(|_| {
                let budget = budget.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let primary = Diagnostic::new(BWErr::NativeError("primary".into())).into();
                    let cause = Diagnostic::new(BWErr::NativeError("cause".into())).into();
                    barrier.wait();
                    let result = RuntimeDiagnostic::while_handling(primary, cause, Some(&budget));
                    let accepted = result.code() == DiagnosticCode::Native;
                    assert_eq!(budget.checkpoint().is_ok(), accepted);
                    barrier.wait(); // Hold the winning reservation through both attempts.
                    drop(result);
                    accepted
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .filter(|accepted| *accepted)
            .count()
    });
    assert_eq!(accepted, 1);
    budget.checkpoint().unwrap();
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts,
        [0; 5]
    );
}

#[test]
fn mutation_rejection_preserves_observed_control_and_disposes_deep_causes_iteratively() {
    for fits in [false, true] {
        let control = OperationControl::default();
        let budget = RunBudget::new(
            RunLimits {
                retained_diagnostics: RetainedDiagnosticLimits {
                    diagnostics: 1 + usize::from(fits),
                    ..Default::default()
                },
                ..Default::default()
            },
            control.clone(),
        );
        let original = runtime_error(&budget, "original");
        let source = Arc::downgrade(original.span.as_ref().unwrap().source());
        control.cancel();
        let stopped = budget.checkpoint().unwrap_err();
        let result = RuntimeDiagnostic::from(stopped).while_handling(original, Some(&budget));
        assert_eq!(result.code(), DiagnosticCode::Cancelled);
        assert_eq!(result.is_emergency(), !fits);
        assert_eq!(source.upgrade().is_some(), fits);
        if !fits {
            assert_eq!(result.omissions.as_ref().unwrap().direct_causes, 1);
            assert_eq!(result.causes[0].code(), DiagnosticCode::ResourceLimit);
        }
        drop(result);
        assert!(source.upgrade().is_none());
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts,
            [0; 5]
        );
    }
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut cause = Diagnostic::new(BWErr::NativeError("leaf".into()));
            let leaf = Arc::downgrade(&cause.error);
            for _ in 0..100_000 {
                let mut parent = Diagnostic::new(BWErr::NativeError("parent".into()));
                parent.causes.push(cause);
                cause = parent;
            }
            let primary = Diagnostic::new(BWErr::ArithmeticError("primary".into())).into();
            let result = RuntimeDiagnostic::while_handling(primary, cause.into(), None);
            assert!(result.is_emergency());
            assert!(leaf.upgrade().is_none());
            assert_eq!(result.causes[0].code(), DiagnosticCode::Arithmetic);
            assert_eq!(
                result.causes[0].omissions.as_ref().unwrap().direct_causes,
                1
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn runtime_context_admits_every_dimension_before_metadata_and_preserves_borrowed_rejection_evidence(
) {
    for deficit in 0..=6 {
        let program = Program::parse("位置", "Missing").unwrap();
        let site = &program.statements[0].span;
        let call_source = Program::parse("caller", "Call").unwrap();
        let frame = CallFrame {
            signature: "call".into(),
            call_site: call_source.statements[0].span.clone(),
            definition_site: Some(site.clone()),
        };
        let mut original = Diagnostic::new(BWErr::NativeError("reason-é".into()));
        original
            .related
            .push(crate::core::diagnostic::RelatedLocation {
                message: "prior".into(),
                span: site.clone(),
            });
        let identity = original.error.clone();
        let context = Some((site, true));
        let expected = original
            .clone()
            .capture_context(context, std::iter::once(&frame));
        let size = DiagnosticLimits::default().check(&expected).unwrap();
        let mut limits = RetainedDiagnosticLimits {
            records: 1,
            diagnostics: size.diagnostics,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        match deficit {
            1 => limits.records -= 1,
            2 => limits.diagnostics -= 1,
            3 => limits.call_frames -= 1,
            4 => limits.related_locations -= 1,
            5 => limits.text_bytes -= 1,
            6 => limits.source_bytes -= 1,
            _ => (),
        }
        let budget = RunBudget::new(
            RunLimits {
                retained_diagnostics: limits,
                ..Default::default()
            },
            OperationControl::default(),
        );
        let sibling = budget.clone();
        let result = RuntimeDiagnostic::from(original).with_context(
            context,
            std::iter::once(&frame),
            Some(&budget),
        );
        let sources = [
            Arc::downgrade(&program.source),
            Arc::downgrade(&call_source.source),
        ];
        if deficit == 0 {
            assert_eq!(result.to_string(), expected.to_string());
            assert!(Arc::ptr_eq(&result.error, &identity));
            assert_eq!(
                budget.0.retained_diagnostics.used.lock().unwrap().counts,
                [
                    1,
                    size.diagnostics,
                    size.call_frames,
                    size.related_locations,
                    size.text_bytes
                ]
            );
        } else {
            assert!(result.is_emergency());
            assert_eq!(result.causes[0].code(), DiagnosticCode::Native);
            assert_eq!(result.causes[0].label, "expression");
            let omitted = result.causes[0].omissions.as_ref().unwrap();
            assert_eq!(omitted.call_frames, 1);
            let source = omitted.source.as_ref().unwrap();
            assert_eq!(source.file, "位置");
            assert_eq!(
                (source.start_byte, source.end_byte),
                (site.start(), site.end())
            );
            assert!(budget.checkpoint().is_err());
            assert_eq!(
                budget.0.retained_diagnostics.used.lock().unwrap().counts,
                [0; 5]
            );
        }
        sibling.checkpoint().unwrap();
        drop((expected, frame, program, call_source));
        assert_eq!(
            sources.iter().all(|source| source.upgrade().is_none()),
            deficit != 0
        );
        drop(result);
        assert!(sources.iter().all(|source| source.upgrade().is_none()));
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts,
            [0; 5]
        );
    }
}

#[test]
fn context_readmission_credits_existing_reservations_and_keeps_innermost_metadata() {
    let budget = RunBudget::new(
        RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records: 1,
                call_frames: 1,
                ..Default::default()
            },
            ..Default::default()
        },
        OperationControl::default(),
    );
    let program = Program::parse("first", "Missing").unwrap();
    let frame = CallFrame {
        signature: "inner".into(),
        call_site: program.statements[0].span.clone(),
        definition_site: None,
    };
    let error = Diagnostic::new(BWErr::NativeError("reason".into())).into();
    let mut error = RuntimeDiagnostic::with_context(
        error,
        Some((&frame.call_site, true)),
        std::iter::once(&frame),
        Some(&budget),
    );
    let initial = budget.0.retained_diagnostics.used.lock().unwrap().counts;
    let outer = Program::parse("outer", "Call").unwrap();
    let frames: Vec<_> = (0..3)
        .map(|_| CallFrame {
            signature: "outer".into(),
            call_site: outer.statements[0].span.clone(),
            definition_site: None,
        })
        .collect();
    for _ in 0..100 {
        error = error.with_context(
            Some((&frames[0].call_site, false)),
            frames.iter(),
            Some(&budget),
        );
        assert_eq!(
            budget.0.retained_diagnostics.used.lock().unwrap().counts,
            initial
        );
        assert_eq!(error.span.as_ref().unwrap().source().name(), "first");
        assert_eq!(error.call_stack[0].signature, "inner");
        assert_eq!(error.label, "expression");
    }
    let probe = budget.clone();
    let rejected = RuntimeDiagnostic::from(Diagnostic::new(BWErr::NativeError("another".into())))
        .with_context(None, std::iter::empty(), Some(&probe));
    assert!(rejected.is_emergency());
    budget.checkpoint().unwrap();
    let public = error.into_diagnostic();
    assert_eq!(
        budget.0.retained_diagnostics.used.lock().unwrap().counts,
        [0; 5]
    );
    let accepted = RuntimeDiagnostic::from(Diagnostic::new(BWErr::NativeError("another".into())))
        .with_context(None, std::iter::empty(), Some(&budget));
    assert_eq!(accepted.code(), DiagnosticCode::Native);
    drop((accepted, public));
}

#[test]
fn construction_reservations_cover_formatting_and_concurrent_contexts_until_host_transfer() {
    use crate::core::eval::Context;
    use std::sync::mpsc;
    struct Hold {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        passes: std::cell::Cell<usize>,
    }
    impl std::fmt::Display for Hold {
        fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            if self.passes.replace(self.passes.get() + 1) == 0 {
                self.entered.send(()).unwrap();
                self.release
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
            output.write_str("first")
        }
    }
    let context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 1,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let competitor = context.clone();
    let observer = context.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let hold = Hold {
            entered: entered_tx,
            release: release_rx,
            passes: std::cell::Cell::new(0),
        };
        let error =
            context.formatted_error(BWErr::NativeError, format_args!("{hold}"), None, false);
        context.checkpoint().unwrap();
        assert_eq!(hold.passes.get(), 2);
        error
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let rejected =
        competitor.formatted_error(BWErr::NativeError, format_args!("second"), None, false);
    assert!(rejected.is_emergency());
    assert!(competitor.checkpoint().is_err());
    observer.checkpoint().unwrap();
    release_tx.send(()).unwrap();
    let outgoing = worker.join().unwrap();
    assert_eq!(outgoing.code(), DiagnosticCode::Native);
    let tracker = &observer.budget.as_ref().unwrap().0.retained_diagnostics;
    assert_eq!(tracker.used.lock().unwrap().counts[0], 1);
    let host = outgoing.into_diagnostic();
    assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
    let next = observer.formatted_error(BWErr::NativeError, format_args!("next"), None, false);
    assert_eq!(next.code(), DiagnosticCode::Native);
    drop((host, next));
    assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
}

#[test]
fn construction_releases_initial_and_final_admission_on_late_formatting_failure() {
    use crate::core::eval::Context;
    struct Changing(std::cell::Cell<usize>);
    impl std::fmt::Display for Changing {
        fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let pass = self.0.replace(self.0.get() + 1);
            output.write_str("partial")?;
            if pass == 1 {
                Err(std::fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    let context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 1,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let sibling = context.clone();
    let value = Changing(std::cell::Cell::new(0));
    let rejected =
        context.formatted_error(BWErr::NativeError, format_args!("{value}"), None, false);
    assert!(rejected.is_emergency());
    assert_eq!(value.0.get(), 3); // Measurement, failed construction, bounded evidence.
    let tracker = &sibling.budget.as_ref().unwrap().0.retained_diagnostics;
    assert_eq!(tracker.used.lock().unwrap().counts, [0; 5]);
    sibling.checkpoint().unwrap();
    let accepted = sibling.formatted_error(BWErr::NativeError, format_args!("next"), None, false);
    assert_eq!(accepted.code(), DiagnosticCode::Native);
}
