use super::*;
use crate::core::run::{RunLimits, SnapshotLimits};

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
