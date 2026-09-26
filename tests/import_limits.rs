#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    run::{Engine, ImportLimits, RunLimits, RunOptions, RunOutcome, MAX_MODULE_CHAIN_DEPTH},
};
use cli_harness::Harness;
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

fn run(
    harness: &Harness,
    engine: &Engine,
    source: &str,
    imports: ImportLimits,
) -> botwork::core::run::RunResult {
    engine.run_source(
        "main.botwork",
        source,
        RunOptions {
            working_directory: Some(harness.workspace.clone()),
            limits: RunLimits {
                imports,
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    )
}

fn evaluate(harness: &Harness, source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    let program = Program::parse(
        harness.workspace.join("main.botwork").to_str().unwrap(),
        source,
    )
    .unwrap();
    evaluate_program_detailed(&program, context)
}

fn marked_engine() -> (Engine, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Mark", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    (engine, calls)
}

#[test]
fn load_count_and_source_bytes_stop_before_excess_module_effects() {
    let harness = Harness::new();
    for name in ["a", "b"] {
        fs::write(harness.workspace.join(format!("{name}.botwork")), "Mark").unwrap();
    }
    let source = "Import |\"a.botwork\"| As |a|\nImport |\"b.botwork\"| As |b|";
    for (limits, count, resource) in [
        (
            ImportLimits {
                loads: 2,
                source_bytes: 8,
                ..ImportLimits::default()
            },
            2,
            None,
        ),
        (
            ImportLimits {
                loads: 1,
                ..ImportLimits::default()
            },
            1,
            Some("module load attempts"),
        ),
        (
            ImportLimits {
                source_bytes: 7,
                ..ImportLimits::default()
            },
            1,
            Some("module source bytes"),
        ),
    ] {
        let (engine, calls) = marked_engine();
        let result = run(&harness, &engine, source, limits);
        assert_eq!(calls.load(Ordering::SeqCst), count);
        match resource {
            None => assert_eq!(result.outcome(), RunOutcome::Succeeded),
            Some(resource) => {
                assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
                let error = result.result.unwrap_err();
                assert!(error.to_string().contains(resource));
                assert_eq!(error.related.len(), 1);
            }
        }
    }
}

#[test]
fn cached_aliases_reuse_load_and_byte_budgets_after_file_removal() {
    let harness = Harness::new();
    let source = "Read { Return |7| }";
    let path = harness.workspace.join("module.botwork");
    fs::write(&path, source).unwrap();
    let mut context = Context::with_limits(RunLimits {
        imports: ImportLimits {
            loads: 1,
            source_bytes: source.len(),
            paths: 1,
            bindings: 4,
            ..ImportLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    evaluate(&harness, "Import |\"module.botwork\"| As |a|", &mut context).unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(
        evaluate(
            &harness,
            "Import |\"module.botwork\"| As |b|\nb::Read",
            &mut context
        )
        .unwrap()
        .to_string(),
        "7"
    );
    let error = evaluate(&harness, "Import |\"module.botwork\"| As |c|", &mut context).unwrap_err();
    assert!(error.to_string().contains("imported bindings"));
    assert!(context.statement_signature("c::Read").unwrap().is_none());
    assert!(context.statement_signature("a::Read").unwrap().is_some());
}

#[test]
fn failed_initialization_and_parse_retries_consume_cumulative_budgets() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("broken.botwork"), "If").unwrap();
    let source = "Try { Import |\"broken.botwork\"| As |a| } Catch {}\nTry { Import |\"broken.botwork\"| As |b| } Catch { |handled| = |true| }";
    for (limits, resource) in [
        (
            ImportLimits {
                loads: 1,
                ..ImportLimits::default()
            },
            "module load attempts",
        ),
        (
            ImportLimits {
                source_bytes: 3,
                ..ImportLimits::default()
            },
            "module source bytes",
        ),
    ] {
        let result = run(&harness, &Engine::default(), source, limits);
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert!(result.result.unwrap_err().to_string().contains(resource));
        assert!(!result.variables.contains_key("handled"));
    }
}

#[test]
fn source_limits_precede_utf8_decoding_and_apply_to_empty_files() {
    let harness = Harness::new();
    let path = harness.workspace.join("module.botwork");
    for bytes in [vec![], "éé".as_bytes().to_vec(), vec![0xff; 5]] {
        fs::write(&path, &bytes).unwrap();
        let result = run(
            &harness,
            &Engine::default(),
            "Import |\"module.botwork\"| As |module|",
            ImportLimits {
                source_bytes: if bytes.is_empty() { 0 } else { 3 },
                ..ImportLimits::default()
            },
        );
        if bytes.is_empty() {
            assert_eq!(result.outcome(), RunOutcome::Succeeded);
        } else {
            assert!(result
                .result
                .unwrap_err()
                .to_string()
                .contains("module source bytes"));
        }
    }
}

#[test]
fn canonical_aliases_count_paths_without_reloading() {
    let harness = Harness::new();
    fs::create_dir(harness.workspace.join("sub")).unwrap();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Read { Return |7| }",
    )
    .unwrap();
    let source = "Import |\"module.botwork\"| As |a|\nImport |\"sub/../module.botwork\"| As |b|\n|result| = b::Read";
    let result = run(
        &harness,
        &Engine::default(),
        source,
        ImportLimits {
            paths: 2,
            loads: 1,
            ..ImportLimits::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.variables["result"].to_string(), "7");
    let result = run(
        &harness,
        &Engine::default(),
        source,
        ImportLimits {
            paths: 1,
            ..ImportLimits::default()
        },
    );
    assert!(result
        .result
        .unwrap_err()
        .to_string()
        .contains("module path cache entries"));
}

#[test]
fn exact_metadata_payload_includes_paths_qualification_and_map_keys() {
    let harness = Harness::new();
    let path = harness.workspace.join("module.botwork");
    fs::write(&path, "Read { Return |7| }").unwrap();
    // Requested path + canonical resolution + load identity; then namespace key
    // and two qualified names, original export name, and display namespace.
    let bytes = 3 * path.as_os_str().len() + 20;
    for maximum in [bytes, bytes - 1] {
        let result = run(
            &harness,
            &Engine::default(),
            "Import |\"module.botwork\"| As |a|",
            ImportLimits {
                metadata_bytes: maximum,
                ..ImportLimits::default()
            },
        );
        if maximum == bytes {
            assert_eq!(result.outcome(), RunOutcome::Succeeded);
        } else {
            assert!(result
                .result
                .unwrap_err()
                .to_string()
                .contains("import metadata bytes"));
        }
    }
}

#[test]
fn namespace_admission_is_atomic_and_preserves_previous_exports() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "One {}\nTwo {}").unwrap();
    let mut context = Context::with_limits(RunLimits {
        imports: ImportLimits {
            bindings: 5,
            ..ImportLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    evaluate(&harness, "Import |\"module.botwork\"| As |a|", &mut context).unwrap();
    let error = evaluate(&harness, "Import |\"module.botwork\"| As |b|", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(context.complete_statements("b::").is_empty());
    assert_eq!(context.complete_statements("a::").len(), 2);
}

#[test]
fn cached_dependency_chains_and_empty_reexports_obey_the_graph_ceiling() {
    let harness = Harness::new();
    for leaf in ["", "Read { Return |7| }"] {
        fs::write(harness.workspace.join("m0.botwork"), leaf).unwrap();
        for index in 1..=3 {
            fs::write(
                harness.workspace.join(format!("m{index}.botwork")),
                format!("Import |\"m{}.botwork\"| As |previous|", index - 1),
            )
            .unwrap();
        }
        let mut context = Context::with_limits(RunLimits {
            imports: ImportLimits {
                dependency_depth: 3,
                ..ImportLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        for index in 0..=2 {
            evaluate(
                &harness,
                &format!("Import |\"m{index}.botwork\"| As |m{index}|"),
                &mut context,
            )
            .unwrap();
        }
        let error =
            evaluate(&harness, "Import |\"m3.botwork\"| As |m3|", &mut context).unwrap_err();
        assert!(error.to_string().contains("module dependency depth"));
        assert!(context.complete_statements("m3::").is_empty());
    }
}

#[test]
fn cold_dependency_limit_stops_before_reading_or_initializing_the_next_module() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("outer.botwork"),
        "Import |\"inner.botwork\"| As |inner|",
    )
    .unwrap();
    fs::write(harness.workspace.join("inner.botwork"), "Mark").unwrap();
    let (engine, calls) = marked_engine();
    let result = run(
        &harness,
        &engine,
        "Import |\"outer.botwork\"| As |outer|",
        ImportLimits {
            dependency_depth: 1,
            ..ImportLimits::default()
        },
    );
    assert!(result
        .result
        .unwrap_err()
        .to_string()
        .contains("module dependency depth"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn reexport_fanout_is_bounded_before_publication() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("m0.botwork"), "Read {}").unwrap();
    for index in 1..=8 {
        fs::write(
            harness.workspace.join(format!("m{index}.botwork")),
            format!(
                "Import |\"m{}.botwork\"| As |left|\nImport |\"m{}.botwork\"| As |right|",
                index - 1,
                index - 1
            ),
        )
        .unwrap();
    }
    let result = run(
        &harness,
        &Engine::default(),
        "Import |\"m8.botwork\"| As |tree|",
        ImportLimits {
            bindings: 64,
            ..ImportLimits::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(result
        .result
        .unwrap_err()
        .to_string()
        .contains("imported bindings"));
}

#[test]
fn cloned_contexts_copy_import_counters_and_stop_latches_independently() {
    let harness = Harness::new();
    for name in ["a", "b", "c"] {
        fs::write(harness.workspace.join(format!("{name}.botwork")), "").unwrap();
    }
    let mut original = Context::with_limits(RunLimits {
        imports: ImportLimits {
            loads: 2,
            ..ImportLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    evaluate(&harness, "Import |\"a.botwork\"| As |a|", &mut original).unwrap();
    let mut cloned = original.clone();
    evaluate(&harness, "Import |\"b.botwork\"| As |b|", &mut original).unwrap();
    assert!(evaluate(&harness, "Import |\"c.botwork\"| As |c|", &mut original).is_err());
    evaluate(&harness, "Import |\"b.botwork\"| As |b|", &mut cloned).unwrap();
    cloned.checkpoint().unwrap();
}

#[test]
fn zero_budgets_and_invalid_dependency_configuration_reject_before_module_effects() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "Mark").unwrap();
    for limits in [
        ImportLimits {
            loads: 0,
            ..ImportLimits::default()
        },
        ImportLimits {
            paths: 0,
            ..ImportLimits::default()
        },
        ImportLimits {
            dependency_depth: 0,
            ..ImportLimits::default()
        },
        ImportLimits {
            metadata_bytes: 0,
            ..ImportLimits::default()
        },
    ] {
        let (engine, calls) = marked_engine();
        let result = run(
            &harness,
            &engine,
            "Import |\"module.botwork\"| As |module|",
            limits,
        );
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    let limits = RunLimits {
        imports: ImportLimits {
            dependency_depth: MAX_MODULE_CHAIN_DEPTH + 1,
            ..ImportLimits::default()
        },
        ..RunLimits::default()
    };
    assert_eq!(
        Context::with_limits(limits).err().unwrap().code(),
        DiagnosticCode::RunConfiguration
    );
}

#[test]
fn default_cli_import_count_is_bounded_including_empty_modules() {
    let harness = Harness::new();
    let mut source = String::new();
    for index in 0..=ImportLimits::default().loads {
        fs::write(harness.workspace.join(format!("m{index}.botwork")), "").unwrap();
        source.push_str(&format!("Import |\"m{index}.botwork\"| As |m{index}|\n"));
    }
    let output = harness
        .run("load-limit", &source, Duration::from_secs(5))
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("[BW8001]"));
    assert!(diagnostic.contains("module load attempts"));
}

#[test]
fn late_namespace_failure_preserves_completed_initialization_and_publishes_nothing() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "Mark\nRead {}").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut context = Context::with_limits(RunLimits {
        imports: ImportLimits {
            bindings: 1,
            ..ImportLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .register_native("Mark", move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let error = evaluate(
        &harness,
        "Try { Import |\"module.botwork\"| As |module| } Catch { Mark }",
        &mut context,
    )
    .unwrap_err();
    assert!(error.to_string().contains("imported bindings"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(context.complete_statements("module::").is_empty());
}

#[test]
fn zero_export_budgets_still_count_empty_namespaces() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("empty.botwork"), "").unwrap();
    for bindings in [0, 1] {
        let result = run(
            &harness,
            &Engine::default(),
            "Import |\"empty.botwork\"| As |empty|",
            ImportLimits {
                bindings,
                ..ImportLimits::default()
            },
        );
        assert_eq!(
            result.outcome(),
            if bindings == 1 {
                RunOutcome::Succeeded
            } else {
                RunOutcome::LimitExceeded
            }
        );
    }
}

#[test]
fn cached_module_calls_share_later_import_budgets() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("loader.botwork"),
        "Load { Import |\"child.botwork\"| As |child| }",
    )
    .unwrap();
    fs::write(harness.workspace.join("child.botwork"), "Mark").unwrap();
    let (engine, calls) = marked_engine();
    let result = run(
        &harness,
        &engine,
        "Import |\"loader.botwork\"| As |loader|\nloader::Load",
        ImportLimits {
            loads: 1,
            ..ImportLimits::default()
        },
    );
    assert!(result
        .result
        .unwrap_err()
        .to_string()
        .contains("module load attempts"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
