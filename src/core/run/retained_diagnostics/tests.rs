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
        let value = StoredDiagnostic::into_diagnostic(stored);
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
