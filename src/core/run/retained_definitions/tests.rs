use super::*;
use crate::core::ast::StatementKind;

fn definitions(source: &str) -> Vec<Arc<Definition>> {
    Program::parse("source", source)
        .unwrap()
        .statements
        .iter()
        .map(|statement| {
            let StatementKind::Define(definition) = statement.kind() else {
                panic!("definition")
            };
            Arc::clone(definition)
        })
        .collect()
}

fn measure(definition: &Definition) -> DefinitionSize {
    measure_definition(definition, &AstLimits::default(), usize::MAX).unwrap()
}

#[test]
fn counts_unique_roots_and_sources_and_releases_after_last_registration() {
    let roots = definitions("First {}\nSecond {}");
    assert_eq!(measure(&roots[0]).nodes, 3);
    let bytes = roots[0].span.source().text().len() + "source".len();
    let tracker = Arc::new(RetainedDefinitions::new(RetainedDefinitionLimits {
        definitions: 2,
        nodes: 6,
        source_bytes: bytes,
    }));
    let first = tracker.reserve(&roots[0], measure(&roots[0])).unwrap();
    let alias = first.clone();
    let repeated = tracker.reserve(&roots[0], measure(&roots[0])).unwrap();
    let second = tracker.reserve(&roots[1], measure(&roots[1])).unwrap();
    {
        let used = tracker.used.lock().unwrap();
        assert_eq!(used.definitions.len(), 2);
        assert_eq!(used.nodes, 6);
        assert_eq!(used.source_bytes, bytes);
        assert_eq!(used.sources.len(), 1);
    }
    drop(first);
    drop(alias);
    drop(repeated);
    {
        let used = tracker.used.lock().unwrap();
        assert_eq!(used.nodes, 3);
        assert_eq!(used.source_bytes, bytes);
    }
    drop(second);
    let used = tracker.used.lock().unwrap();
    assert_eq!(used.nodes, 0);
    assert_eq!(used.source_bytes, 0);
    assert!(used.definitions.is_empty());
    assert!(used.sources.is_empty());
}

#[test]
fn rejected_admission_preserves_every_counter_and_handles_overflow() {
    let roots = definitions("First {}\nSecond {}");
    for limits in [
        RetainedDefinitionLimits {
            definitions: 0,
            ..RetainedDefinitionLimits::default()
        },
        RetainedDefinitionLimits {
            nodes: 2,
            ..RetainedDefinitionLimits::default()
        },
        RetainedDefinitionLimits {
            source_bytes: 1,
            ..RetainedDefinitionLimits::default()
        },
    ] {
        let tracker = Arc::new(RetainedDefinitions::new(limits));
        assert!(tracker.reserve(&roots[0], measure(&roots[0])).is_err());
        let used = tracker.used.lock().unwrap();
        assert_eq!((used.nodes, used.source_bytes), (0, 0));
        assert!(used.definitions.is_empty());
        assert!(used.sources.is_empty());
    }
    for resource in ["nodes", "source", "references", "source references"] {
        let tracker = Arc::new(RetainedDefinitions::new(RetainedDefinitionLimits {
            definitions: usize::MAX,
            nodes: usize::MAX,
            source_bytes: usize::MAX,
        }));
        let held = tracker.reserve(&roots[0], measure(&roots[0])).unwrap();
        let (nodes, bytes) = {
            let used = tracker.used.lock().unwrap();
            (used.nodes, used.source_bytes)
        };
        {
            let mut used = tracker.used.lock().unwrap();
            match resource {
                "nodes" => used.nodes = usize::MAX,
                "source" => used.source_bytes = usize::MAX,
                "references" => {
                    *used.definitions.get_mut(&definition_id(&roots[0])).unwrap() = usize::MAX
                }
                _ => {
                    used.sources
                        .get_mut(&source_id(roots[0].span.source()))
                        .unwrap()
                        .references = usize::MAX
                }
            }
        }
        let other = definitions("Third {}").remove(0);
        let root = match resource {
            "references" => &roots[0],
            "source" => &other,
            _ => &roots[1],
        };
        assert!(tracker.reserve(root, measure(root)).is_err(), "{resource}");
        {
            let mut used = tracker.used.lock().unwrap();
            assert_eq!(used.definitions.len(), 1);
            assert_eq!(used.sources.len(), 1);
            used.nodes = nodes;
            used.source_bytes = bytes;
            *used.definitions.get_mut(&definition_id(&roots[0])).unwrap() = 1;
            used.sources
                .get_mut(&source_id(roots[0].span.source()))
                .unwrap()
                .references = 1;
        }
        drop(held);
    }
}

#[test]
fn no_ownership_cycles_keep_definition_or_source_alive() {
    let root = definitions("First {}").remove(0);
    let weak_definition = Arc::downgrade(&root);
    let weak_source = Arc::downgrade(root.span.source());
    let tracker = Arc::new(RetainedDefinitions::new(RetainedDefinitionLimits::default()));
    let weak_tracker = Arc::downgrade(&tracker);
    let held = tracker.reserve(&root, measure(&root)).unwrap();
    drop(root);
    drop(tracker);
    assert!(weak_definition.upgrade().is_some());
    assert!(weak_source.upgrade().is_some());
    drop(held);
    assert!(weak_definition.upgrade().is_none());
    assert!(weak_source.upgrade().is_none());
    assert!(weak_tracker.upgrade().is_none());
}

#[test]
fn equal_text_in_different_allocations_consumes_separate_source_allowance() {
    let first = definitions("First {}").remove(0);
    let second = definitions("First {}").remove(0);
    let maximum = "First {}".len() + "source".len();
    let tracker = Arc::new(RetainedDefinitions::new(RetainedDefinitionLimits {
        source_bytes: maximum,
        ..RetainedDefinitionLimits::default()
    }));
    let held = tracker.reserve(&first, measure(&first)).unwrap();
    let error = tracker.reserve(&second, measure(&second)).err().unwrap();
    assert!(error
        .to_string()
        .contains("retained definition source bytes"));
    drop(held);
    assert!(tracker.reserve(&second, measure(&second)).is_ok());
}

#[test]
fn repeated_concurrent_registration_and_drop_keeps_counts_consistent() {
    let roots = definitions("First {}\nSecond {}");
    let tracker = Arc::new(RetainedDefinitions::new(RetainedDefinitionLimits {
        definitions: 2,
        nodes: 6,
        ..RetainedDefinitionLimits::default()
    }));
    std::thread::scope(|scope| {
        for index in 0..8 {
            let tracker = tracker.clone();
            let root = roots[index % 2].clone();
            scope.spawn(move || {
                for _ in 0..1_000 {
                    let reservation = tracker.reserve(&root, measure(&root)).unwrap();
                    let clone = reservation.clone();
                    drop(reservation);
                    drop(clone);
                }
            });
        }
    });
    let used = tracker.used.lock().unwrap();
    assert_eq!((used.nodes, used.source_bytes), (0, 0));
    assert!(used.definitions.is_empty());
    assert!(used.sources.is_empty());
}
