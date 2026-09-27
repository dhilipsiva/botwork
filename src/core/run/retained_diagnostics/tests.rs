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
        let value = copied.into_diagnostic();
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
