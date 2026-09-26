use super::*;
use crate::core::diagnostic::DiagnosticCode;

fn signature() -> StatementSignature {
    StatementSignature::native("Read |value|")
        .unwrap()
        .description("é")
        .documents_error(DiagnosticCode::Native, "bad")
        .unwrap()
}

#[test]
fn exact_signature_namespace_and_qualified_metrics_share_source_owners() {
    let signature = signature();
    assert_eq!(signature.normalized(), "read|param|");
    let plan = RegistryPlan::signature(&signature);
    assert_eq!(plan.nodes, Some(3));
    assert_eq!(plan.text_bytes, Some(32));
    assert_eq!(plan.name_bytes(), Some(11));
    assert_eq!(plan.sources.len(), 1);
    let tracker = Arc::new(RetainedRegistry::new(RetainedRegistryLimits::default()));
    let native = tracker.reserve(plan).unwrap();
    let program = Program::parse("import", "Import |\"m.botwork\"| As |é|").unwrap();
    let span = &program.statements[0].span;
    let plan = RegistryPlan::qualified(&signature, "é", "é", span);
    assert_eq!(plan.text_bytes, Some(42));
    assert_eq!(plan.name_bytes(), Some(15));
    let imported = tracker.reserve(plan).unwrap();
    assert_eq!(imported.key.as_ref(), "é::read|param|");
    let namespace = tracker.reserve(RegistryPlan::namespace("é", span)).unwrap();
    let expected_source =
        "<native>".len() + "Read |value|".len() + "import".len() + program.source.text().len();
    {
        let used = tracker.used.lock().unwrap();
        assert_eq!(
            (used.entries, used.nodes, used.text_bytes, used.source_bytes),
            (3, 7, 76, expected_source)
        );
        assert_eq!(used.sources.len(), 2);
    }
    drop(native);
    drop(namespace);
    assert_eq!(tracker.used.lock().unwrap().source_bytes, expected_source);
    let alias = imported.clone();
    drop(imported);
    assert_eq!(tracker.used.lock().unwrap().entries, 1);
    drop(alias);
    let used = tracker.used.lock().unwrap();
    assert_eq!(
        (used.entries, used.nodes, used.text_bytes, used.source_bytes),
        (0, 0, 0, 0)
    );
    assert!(used.sources.is_empty());
}

#[test]
fn every_registry_limit_is_atomic_and_checked_for_overflow() {
    let signature = signature();
    for limits in [
        RetainedRegistryLimits {
            entries: 0,
            ..RetainedRegistryLimits::default()
        },
        RetainedRegistryLimits {
            nodes: 2,
            ..RetainedRegistryLimits::default()
        },
        RetainedRegistryLimits {
            name_bytes: 10,
            ..RetainedRegistryLimits::default()
        },
        RetainedRegistryLimits {
            text_bytes: 31,
            ..RetainedRegistryLimits::default()
        },
        RetainedRegistryLimits {
            source_bytes: 19,
            ..RetainedRegistryLimits::default()
        },
    ] {
        let tracker = Arc::new(RetainedRegistry::new(limits));
        assert!(tracker
            .reserve(RegistryPlan::signature(&signature))
            .is_err());
        let used = tracker.used.lock().unwrap();
        assert_eq!(
            (used.entries, used.nodes, used.text_bytes, used.source_bytes),
            (0, 0, 0, 0)
        );
        assert!(used.sources.is_empty());
    }
    let unlimited = RetainedRegistryLimits {
        entries: usize::MAX,
        nodes: usize::MAX,
        name_bytes: usize::MAX,
        text_bytes: usize::MAX,
        source_bytes: usize::MAX,
    };
    for resource in 0..4 {
        let tracker = Arc::new(RetainedRegistry::new(unlimited.clone()));
        {
            let mut used = tracker.used.lock().unwrap();
            match resource {
                0 => used.entries = usize::MAX,
                1 => used.nodes = usize::MAX,
                2 => used.text_bytes = usize::MAX,
                _ => used.source_bytes = usize::MAX,
            }
        }
        assert!(tracker
            .reserve(RegistryPlan::signature(&signature))
            .is_err());
        assert!(tracker.used.lock().unwrap().sources.is_empty());
    }
    for resource in ["nodes", "text"] {
        let tracker = Arc::new(RetainedRegistry::new(unlimited.clone()));
        let mut plan = RegistryPlan::signature(&signature);
        if resource == "nodes" {
            plan.nodes = None;
        } else {
            plan.text_bytes = None;
        }
        assert!(tracker.reserve(plan).is_err());
        assert_eq!(tracker.used.lock().unwrap().entries, 0);
    }
}

#[test]
fn existing_keys_share_storage_and_sources_release_without_cycles() {
    let signature = signature();
    let source = Arc::downgrade(signature.header().source());
    let key: Arc<str> = signature.normalized().into();
    let tracker = Arc::new(RetainedRegistry::new(RetainedRegistryLimits::default()));
    let weak = Arc::downgrade(&tracker);
    let reservation = tracker
        .reserve(RegistryPlan::signature(&signature).with_key(key.clone()))
        .unwrap();
    assert!(Arc::ptr_eq(&key, &reservation.key));
    drop(signature);
    drop(tracker);
    assert!(source.upgrade().is_some());
    drop(reservation);
    assert!(source.upgrade().is_none());
    assert!(weak.upgrade().is_none());
}

#[test]
fn concurrent_registrations_share_source_counts_and_release_consistently() {
    let signature = signature();
    let tracker = Arc::new(RetainedRegistry::new(RetainedRegistryLimits {
        entries: 8,
        ..RetainedRegistryLimits::default()
    }));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let tracker = tracker.clone();
            let signature = &signature;
            scope.spawn(move || {
                for _ in 0..1_000 {
                    let held = tracker.reserve(RegistryPlan::signature(signature)).unwrap();
                    drop(held);
                }
            });
        }
    });
    let used = tracker.used.lock().unwrap();
    assert_eq!(
        (used.entries, used.nodes, used.text_bytes, used.source_bytes),
        (0, 0, 0, 0)
    );
    assert!(used.sources.is_empty());
}
