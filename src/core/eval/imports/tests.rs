use super::*;
use crate::core::run::{RunLimits, SnapshotLimits};

#[test]
fn imported_rethrows_keep_record_ownership_after_module_context_unwinds() {
    use crate::core::run::RetainedDiagnosticLimits;
    let mut context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 3,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let definitions =
        Program::parse("library", "Fail { Try { Missing } Catch { Rethrow } }").unwrap();
    evaluate_program_runtime(&definitions, &mut context).unwrap();
    let module = LoadedModule {
        frame: context.frames[0].clone(),
    };
    let caller = Program::parse("caller", "Fail").unwrap();
    let call = Call {
        signature: "fail".into(),
        span: caller.statements[0].span.clone(),
        arguments: vec![],
    };
    let import = Program::parse("import", "Import |\"library.botwork\"| As |lib|").unwrap();
    let error = sync_result(invoke_imported(
        &call,
        &module,
        "fail",
        vec![],
        &import.statements[0].span,
        &mut context,
    ))
    .unwrap_err();
    assert_eq!(error.call_stack.len(), 1);
    assert!(error
        .related
        .iter()
        .any(|site| site.message == "imported here"));
    let first = context
        .retain_handler(Diagnostic::new(BWErr::NativeError("first".into())))
        .unwrap();
    let second = context
        .retain_handler(Diagnostic::new(BWErr::NativeError("second".into())))
        .unwrap();
    let probe = context.clone();
    assert!(probe
        .retain_handler(Diagnostic::new(BWErr::NativeError("blocked".into())))
        .is_err());
    let public = error.into_diagnostic();
    context.checkpoint().unwrap();
    let third = context
        .retain_handler(Diagnostic::new(BWErr::NativeError("third".into())))
        .unwrap();
    assert_eq!(
        public.code(),
        crate::core::diagnostic::DiagnosticCode::UndefinedStatement
    );
    drop((first, second, third, public));
}

#[test]
fn missing_imported_exports_admit_name_and_complete_known_context() {
    use crate::core::diagnostic::{DiagnosticCode, DiagnosticLimits};

    for deficit in [
        None,
        Some("text"),
        Some("source"),
        Some("calls"),
        Some("sites"),
        Some("records"),
        Some("depth"),
    ] {
        let program = Program::parse("caller-é", "Read").unwrap();
        let imports = Program::parse("importer", "Import |\"module.botwork\"| As |lib|").unwrap();
        let call = Call {
            span: program.statements[0].span.clone(),
            signature: "read".into(),
            arguments: vec![],
        };
        let site = &imports.statements[0].span;
        let sources = [
            Arc::downgrade(&program.source),
            Arc::downgrade(&imports.source),
        ];
        let mut expected = Diagnostic::new(BWErr::StatementNotDefined("absent-é".into()))
            .at(&call.span)
            .with_related("imported here", site);
        let outer = Context::default()
            .retain_call("outer", None, &call.span, None)
            .unwrap();
        expected.call_stack.push(outer.frame.clone());
        let size = DiagnosticLimits::default().check(&expected).unwrap();
        let mut limits = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        match deficit {
            Some("text") => limits.text_bytes -= 1,
            Some("source") => limits.source_bytes -= 1,
            Some("calls") => limits.call_frames -= 1,
            Some("sites") => limits.related_locations -= 1,
            Some("records") => limits.diagnostics -= 1,
            Some("depth") => limits.depth -= 1,
            None => {}
            _ => unreachable!(),
        }
        let mut context = Context::with_limits(RunLimits {
            diagnostics: limits,
            ..RunLimits::default()
        })
        .unwrap();
        context.calls.push(outer);
        let mut sibling = context.clone();
        let module = LoadedModule {
            frame: Frame::default(),
        };
        let error = sync_result(invoke_imported(
            &call,
            &module,
            "absent-é",
            vec![],
            site,
            &mut context,
        ))
        .unwrap_err();
        if deficit.is_some() {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), expected.code());
            let omitted = error.causes[0].omissions.as_ref().unwrap();
            assert_eq!(omitted.call_frames, 1);
            assert_eq!(omitted.related_locations, 1);
            assert!(context.checkpoint().is_err());
        } else {
            assert_eq!(error.code(), expected.code());
            assert_eq!(error.to_string(), expected.to_string());
            assert_eq!(DiagnosticLimits::default().check(&error).unwrap(), size);
            context.checkpoint().unwrap();
        }
        sibling.calls.clear();
        sibling.checkpoint().unwrap();
        evaluate_program_detailed(&Program::parse("valid", "|x| = |1|").unwrap(), &mut sibling)
            .unwrap();
        context.calls.clear();
        drop((expected, call, imports, program));
        assert_eq!(
            sources.iter().all(|source| source.upgrade().is_none()),
            deficit.is_some()
        );
        drop(error);
        assert!(sources.iter().all(|source| source.upgrade().is_none()));
    }
}

#[test]
fn missing_imported_exports_bound_unicode_evidence_after_snapshot_admission() {
    use crate::core::diagnostic::{DiagnosticCode, DiagnosticLimits};

    for snapshot_entries in [0, usize::MAX] {
        let program = Program::parse("caller", "Read").unwrap();
        let call = Call {
            span: program.statements[0].span.clone(),
            signature: "read".into(),
            arguments: vec![],
        };
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            },
            snapshots: SnapshotLimits {
                entries: snapshot_entries,
                path_bytes: usize::MAX,
            },
            ..RunLimits::default()
        })
        .unwrap();
        // A captured directory error contributes one snapshot handle even in an empty module.
        context.working_directory = Err(Arc::new(std::io::Error::other("unavailable")));
        let module = LoadedModule {
            frame: Frame::default(),
        };
        let error = sync_result(invoke_imported(
            &call,
            &module,
            &"é".repeat(64 * 1024),
            vec![],
            &call.span,
            &mut context,
        ))
        .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        if snapshot_entries == 0 {
            // Site attachment now applies individual/aggregate admission too;
            // the earlier snapshot failure stays primary under zero text bytes.
            assert!(error.is_emergency());
            assert_eq!(
                error.causes[0]
                    .omissions
                    .as_ref()
                    .unwrap()
                    .related_locations,
                1
            );
            assert!(matches!(
                error.error.as_ref(),
                BWErr::ResourceLimit {
                    resource: "snapshot table entries",
                    ..
                }
            ));
        } else {
            let original = &error.causes[0];
            let BWErr::StatementNotDefined(name) = original.error.as_ref() else {
                panic!("original category")
            };
            assert!(name.starts_with('é'));
            assert!(name.len() <= 256);
            assert_eq!(original.omissions.as_ref().unwrap().detail_fields, 1);
            assert!(original.span.is_none());
        }
        assert!(context.checkpoint().is_err());
    }
}

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

#[test]
fn executables_are_found_on_the_runs_path_whatever_windows_calls_it() {
    let directory = tempfile::tempdir().unwrap();
    let file = if cfg!(windows) { "tool.exe" } else { "tool" };
    std::fs::write(directory.path().join(file), "").unwrap();
    let suffixes: &[&str] = if cfg!(windows) { &[".exe"] } else { &[""] };
    for key in ["PATH", "Path"] {
        let variables = std::collections::BTreeMap::from([(
            std::ffi::OsString::from(key),
            directory.path().as_os_str().to_owned(),
        )]);
        assert_eq!(
            on_path(&variables, "tool", suffixes),
            (key == "PATH" || cfg!(windows)).then(|| directory.path().join(file)),
            "{key}"
        );
    }
    assert_eq!(on_path(&Default::default(), "tool", suffixes), None);
}
