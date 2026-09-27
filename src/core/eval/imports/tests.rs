use super::*;
use crate::core::run::{RunLimits, SnapshotLimits};

#[test]
fn working_directory_errors_share_one_unformatted_owner_across_snapshots() {
    #[derive(Debug)]
    struct NoDisplay;
    impl std::fmt::Display for NoDisplay {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("a snapshot must not format the captured I/O error")
        }
    }
    impl std::error::Error for NoDisplay {}
    let reason = Arc::new(std::io::Error::other(NoDisplay));
    let weak = Arc::downgrade(&reason);
    let mut context = Context::with_limits(RunLimits {
        snapshots: SnapshotLimits {
            entries: 2,
            path_bytes: 0,
        },
        ..RunLimits::default()
    })
    .unwrap();
    context.working_directory = Err(reason.clone());
    let unchecked = context.clone();
    let mut checked = context.try_clone().unwrap(); // One copied error handle.
    context
        .charge_snapshot(context.isolated_snapshot_size())
        .unwrap();
    let module = isolated(Frame::default(), &context); // Second copied handle.
    for copy in [&unchecked, &checked, &module] {
        assert!(Arc::ptr_eq(
            copy.working_directory.as_ref().unwrap_err(),
            &reason
        ));
    }
    let error = context.try_clone().err().unwrap();
    assert!(matches!(
        error.error.as_ref(),
        BWErr::ResourceLimit {
            resource: "snapshot table entries",
            limit: 2
        }
    ));
    assert!(context.checkpoint().is_err());
    checked.checkpoint().unwrap();
    let program = Program::parse("no import", "|value| = |7|").unwrap();
    evaluate_program_detailed(&program, &mut checked).unwrap();
    drop(reason);
    drop(context);
    drop(unchecked);
    drop(module);
    assert!(weak.upgrade().is_some());
    drop(checked);
    assert!(weak.upgrade().is_none());
}

#[test]
fn captured_working_directory_errors_format_only_the_admitted_import_detail() {
    use crate::core::diagnostic::{DiagnosticCode, DiagnosticLimits};
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Debug)]
    struct CountedError(Arc<AtomicUsize>);
    impl std::fmt::Display for CountedError {
        fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for _ in 0..512 {
                self.0.fetch_add(1, Ordering::SeqCst);
                output.write_str("é")?;
            }
            Ok(())
        }
    }
    impl std::error::Error for CountedError {}
    for reject in [false, true] {
        let visits = Arc::new(AtomicUsize::new(0));
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: if reject {
                    0
                } else {
                    DiagnosticLimits::default().text_bytes
                },
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        context.working_directory = Err(Arc::new(std::io::Error::other(CountedError(
            visits.clone(),
        ))));
        let mut sibling = context.try_clone().unwrap();
        let valid = Program::parse("valid", "|value| = |1|").unwrap();
        evaluate_program_detailed(&valid, &mut sibling).unwrap();
        assert_eq!(visits.load(Ordering::SeqCst), 0);
        let program = Program::parse("main-é", "Import |\"module.botwork\"| As |lib|").unwrap();
        let source = Arc::downgrade(&program.source);
        let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
        drop(program);
        if reject {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            let original = &error.causes[0];
            assert_eq!(original.code(), DiagnosticCode::ImportRead);
            let omitted = original.omissions.as_ref().unwrap();
            assert_eq!(omitted.related_locations, 1);
            assert_eq!(omitted.detail_fields, 1);
            assert!(!omitted.prior_summary);
            assert!(source.upgrade().is_none());
            assert_eq!(visits.load(Ordering::SeqCst), 129);
            assert!(context.checkpoint().is_err());
        } else {
            assert_eq!(error.code(), DiagnosticCode::ImportRead);
            let BWErr::ImportRead(reason) = error.error.as_ref() else {
                panic!("import reason")
            };
            assert_eq!(reason, &"é".repeat(512));
            assert_eq!(error.related.len(), 1);
            assert_eq!(visits.load(Ordering::SeqCst), 1024); // Count, then construct.
            assert!(source.upgrade().is_some());
            context.checkpoint().unwrap();
        }
        sibling.checkpoint().unwrap();
    }
}

#[test]
fn streamed_cycle_paths_preserve_original_display_and_order() {
    for loading in [
        vec![],
        vec![PathBuf::from("first-é.botwork")],
        vec![
            PathBuf::from("first-é.botwork"),
            PathBuf::from("second.botwork"),
        ],
    ] {
        let repeated = Path::new("first-é.botwork");
        let expected = loading
            .iter()
            .map(|path| path.display().to_string())
            .chain(std::iter::once(repeated.display().to_string()))
            .collect::<Vec<_>>()
            .join(" -> ");
        assert_eq!(
            ImportChain {
                loading: &loading,
                repeated
            }
            .to_string(),
            expected
        );
    }
}

#[test]
fn working_directory_import_errors_admit_borrowed_details_and_originating_site() {
    use crate::core::diagnostic::{DiagnosticCode, DiagnosticLimits};
    let program = Program::parse("main", "Import |\"module.botwork\"| As |lib|").unwrap();
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context.working_directory = Err(Arc::new(std::io::Error::other("é".repeat(64 * 1024))));
    let mut sibling = context.clone();
    let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::ImportRead);
    let omitted = error.causes[0].omissions.as_ref().unwrap();
    assert_eq!(omitted.related_locations, 1);
    assert_eq!(omitted.detail_fields, 1);
    assert!(context.checkpoint().is_err());
    assert!(sibling.checkpoint().is_ok());
    let valid = Program::parse("valid", "|x| = |1|").unwrap();
    evaluate_program_detailed(&valid, &mut sibling).unwrap();
}

#[test]
fn cache_measurement_counts_native_path_bytes_and_clones_live_entries_only() {
    let mut cache = ModuleCache::default();
    let requested = PathBuf::from("é.botwork");
    let canonical = PathBuf::from("/canonical/é.botwork");
    let expected = requested.as_os_str().len() + canonical.as_os_str().len() * 2;
    cache.loaded.insert(
        canonical.clone(),
        Arc::new(LoadedModule {
            frame: Frame::default(),
        }),
    );
    cache.resolved.insert(requested, canonical.clone());
    cache.loaded.reserve(16_384);
    cache.resolved.reserve(16_384);
    let mut size = SnapshotSize::default();
    cache.snapshot_size(&mut size);
    let budget = RunBudget::new(
        RunLimits {
            snapshots: SnapshotLimits {
                entries: 2,
                path_bytes: expected,
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    );
    budget.charge_snapshot(size).unwrap();
    let copied = cache.clone();
    assert!(copied.loaded.capacity() <= 7);
    assert!(copied.resolved.capacity() <= 7);
    assert!(Arc::ptr_eq(
        &copied.loaded[&canonical],
        &cache.loaded[&canonical]
    ));
    let budget = RunBudget::new(
        RunLimits {
            snapshots: SnapshotLimits {
                entries: 2,
                path_bytes: expected - 1,
            },
            ..RunLimits::default()
        },
        OperationControl::default(),
    );
    assert!(budget
        .charge_snapshot(size)
        .unwrap_err()
        .to_string()
        .contains("snapshot path bytes"));
}

#[test]
fn stopped_imported_snapshot_restores_iterator_and_skips_catch() {
    let mut module_context = Context::default();
    let program = Program::parse("module", "Read {}").unwrap();
    evaluate_program_detailed(&program, &mut module_context).unwrap();
    let metadata = Arc::new(
        module_context
            .statement_signature("Read")
            .unwrap()
            .unwrap()
            .qualified("m", "m"),
    );
    let import_site = metadata.header().clone();
    let module = Arc::new(LoadedModule {
        frame: module_context.frames.remove(0),
    });
    let mut context = Context::with_limits(RunLimits {
        snapshots: SnapshotLimits {
            entries: 0,
            path_bytes: usize::MAX,
        },
        ..RunLimits::default()
    })
    .unwrap();
    context.frames[0].statements.insert(
        Arc::from("m::read"),
        StmtType::Imported {
            module,
            exported: Arc::from("read"),
            metadata,
            import_site,
            _registry: None,
        },
    );
    let program = Program::parse(
        "caller",
        "|item| = |9|\nTry { For |item| In |[1]| { m::Read } } Catch { |caught| = |true| }",
    )
    .unwrap();
    let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
    assert!(error.to_string().contains("snapshot table entries"));
    assert_eq!(context.get_variable_ref("item").unwrap().to_string(), "9");
    assert!(context.get_variable_ref("caught").is_err());
    assert_eq!(context.frames.len(), 1);
    assert_eq!(context.current, 0);
    assert!(context.calls.is_empty());
    assert!(context.handlers.is_empty());
}
