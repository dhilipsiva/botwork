#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    run::{Engine, RetainedRegistryLimits, RunLimits, RunOptions, RunOutcome},
    signature::StatementSignature,
};
use cli_harness::Harness;
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn limits(retained_registry: RetainedRegistryLimits) -> RunLimits {
    RunLimits {
        retained_registry,
        ..RunLimits::default()
    }
}

fn context(entries: usize) -> Context {
    Context::with_limits(limits(RetainedRegistryLimits {
        entries,
        ..RetainedRegistryLimits::default()
    }))
    .unwrap()
}

fn evaluate(source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(&Program::parse("registry", source).unwrap(), context)
}

fn signature() -> StatementSignature {
    StatementSignature::native("Read |value|")
        .unwrap()
        .description("é")
        .documents_error(DiagnosticCode::Native, "bad")
        .unwrap()
}

#[test]
fn native_metadata_counts_keys_labels_docs_errors_and_sources_exactly() {
    let exact = RetainedRegistryLimits {
        entries: 1,
        nodes: 3,
        name_bytes: 11,
        text_bytes: 32,
        source_bytes: 20,
    };
    let mut context = Context::with_limits(limits(exact.clone())).unwrap();
    context
        .register_native_with_signature(signature(), |values| Ok(values[0].clone()))
        .unwrap();
    context.init_statements();
    assert_eq!(evaluate("Read |7|", &mut context).unwrap().to_string(), "7");
    assert_eq!(context.statement_signatures().len(), 98);
    assert!(context
        .statement_signature("Read |x|")
        .unwrap()
        .unwrap()
        .help()
        .contains("BW4002: bad"));
    for (budget, resource) in [
        (
            RetainedRegistryLimits {
                entries: 0,
                ..exact.clone()
            },
            "registry entries",
        ),
        (
            RetainedRegistryLimits {
                nodes: 2,
                ..exact.clone()
            },
            "registry nodes",
        ),
        (
            RetainedRegistryLimits {
                name_bytes: 10,
                ..exact.clone()
            },
            "registry name bytes",
        ),
        (
            RetainedRegistryLimits {
                text_bytes: 31,
                ..exact.clone()
            },
            "registry text bytes",
        ),
        (
            RetainedRegistryLimits {
                source_bytes: 19,
                ..exact
            },
            "registry source bytes",
        ),
    ] {
        let mut context = Context::with_limits(limits(budget)).unwrap();
        let error = context
            .register_native_with_signature(signature(), |_| Ok(Literal::None))
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert!(error.to_string().contains(resource));
        assert_eq!(error.span.unwrap().source().name(), "<native>");
        assert!(context.statement_signatures().is_empty());
        assert!(context.checkpoint().is_err());
    }
}

#[test]
fn zero_registry_budgets_keep_fixed_builtin_initialization_infallible() {
    let zero = RetainedRegistryLimits {
        entries: 0,
        nodes: 0,
        name_bytes: 0,
        text_bytes: 0,
        source_bytes: 0,
    };
    let mut context = Context::with_limits(limits(zero.clone())).unwrap();
    context.init_statements();
    context.init_statements();
    assert_eq!(context.statement_signatures().len(), 97);
    assert!(context
        .statement_signature("Log |value|")
        .unwrap()
        .is_some());
    evaluate("|value| = |7|", &mut context).unwrap();
    assert!(context
        .register_native("Host", |_| Ok(Literal::None))
        .is_err());
    let engine = Engine::with_registry_limits(zero.clone());
    let result = engine.run_source(
        "empty",
        "",
        RunOptions {
            limits: limits(zero),
            ..RunOptions::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
}

#[test]
fn existing_collisions_precede_admission_and_preserve_active_contexts() {
    let mut context = context(1);
    context
        .register_native("Host", |_| Ok(Literal::Int(7)))
        .unwrap();
    let error = context
        .register_native("HOST", |_| Ok(Literal::Int(9)))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::DuplicateStatement);
    assert!(context.checkpoint().is_ok());
    assert_eq!(evaluate("Host", &mut context).unwrap().to_string(), "7");
    let error = evaluate("Host {}", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::DuplicateStatement);
    assert!(context.checkpoint().is_ok());
}

#[test]
fn failed_declarations_release_their_definition_reservation_before_publication() {
    let base = Context::with_limits(RunLimits {
        retained_registry: RetainedRegistryLimits {
            entries: 1,
            ..RetainedRegistryLimits::default()
        },
        retained_definitions: botwork::core::run::RetainedDefinitionLimits {
            definitions: 1,
            ..Default::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let mut limited = base.clone();
    limited
        .register_native("Host", |_| Ok(Literal::None))
        .unwrap();
    assert!(evaluate("Custom {}", &mut limited)
        .unwrap_err()
        .to_string()
        .contains("registry entries"));
    assert!(limited.statement_signature("Custom").unwrap().is_none());
    drop(limited);
    let mut available = base;
    evaluate("Custom {}", &mut available).unwrap();
}

#[test]
fn local_metadata_releases_after_return_error_and_repeated_calls() {
    for body in ["", "Return |7|", "|x| = |missing|"] {
        let mut context = context(2);
        evaluate(&format!("Outer {{ Local {{}}\n{body}\n}}"), &mut context).unwrap();
        for _ in 0..50 {
            evaluate("Try { Outer } Catch {}", &mut context).unwrap();
        }
        assert_eq!(context.statement_signatures().len(), 1);
        evaluate("After {}", &mut context).unwrap();
    }
}

#[test]
fn cloned_contexts_share_registry_payload_and_keep_failures_independent() {
    let mut original = context(2);
    original
        .register_native("First", |_| Ok(Literal::None))
        .unwrap();
    let mut clone = original.clone();
    clone
        .register_native("Second", |_| Ok(Literal::None))
        .unwrap();
    let mut failed = clone.clone();
    assert!(failed
        .register_native("Third", |_| Ok(Literal::None))
        .is_err());
    assert!(original.checkpoint().is_ok());
    drop(clone);
    drop(failed);
    original
        .register_native("Third", |_| Ok(Literal::None))
        .unwrap();
}

#[test]
fn engine_native_templates_are_admitted_per_run_before_inputs_or_effects() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Host", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let deep = (0..20_000).fold(Literal::None, |value, _| Literal::Array(vec![value]));
    let result = engine.run_source(
        "limited",
        "Host",
        RunOptions {
            variables: BTreeMap::from([("value".into(), deep)]),
            limits: limits(RetainedRegistryLimits {
                entries: 0,
                ..RetainedRegistryLimits::default()
            }),
            ..RunOptions::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(result
        .result
        .unwrap_err()
        .to_string()
        .contains("registry entries"));
    assert!(result.variables.is_empty());
    assert_eq!(result.steps, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for _ in 0..3 {
        let result = engine.run_source(
            "fresh",
            "Host",
            RunOptions {
                limits: limits(RetainedRegistryLimits {
                    entries: 1,
                    ..RetainedRegistryLimits::default()
                }),
                ..RunOptions::default()
            },
        );
        assert_eq!(result.outcome(), RunOutcome::Succeeded);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn native_template_admission_order_is_normalized_and_reproducible() {
    for _ in 0..10 {
        let mut engine = Engine::default();
        engine
            .register_native("Zulu", |_, _| Ok(Literal::None))
            .unwrap();
        engine
            .register_native("Alpha", |_, _| Ok(Literal::None))
            .unwrap();
        let result = engine.run_source(
            "empty",
            "",
            RunOptions {
                limits: limits(RetainedRegistryLimits {
                    entries: 1,
                    ..RetainedRegistryLimits::default()
                }),
                ..RunOptions::default()
            },
        );
        let error = result.result.unwrap_err();
        assert!(error.to_string().contains("registry entries"));
        assert_eq!(error.span.unwrap().text(), "Zulu");
    }
}

#[test]
fn explicitly_raised_template_and_run_limits_admit_large_host_documentation() {
    let length = 8 * 1024 * 1024 + 1;
    let budget = RetainedRegistryLimits {
        text_bytes: length + 8,
        ..RetainedRegistryLimits::default()
    };
    let mut engine = Engine::with_registry_limits(budget.clone());
    engine
        .register_native_with_signature(
            StatementSignature::native("Host")
                .unwrap()
                .description("x".repeat(length)),
            |_, _| Ok(Literal::None),
        )
        .unwrap();
    assert_eq!(
        engine
            .run_source("default", "Host", RunOptions::default())
            .outcome(),
        RunOutcome::LimitExceeded
    );
    let result = engine.run_source(
        "raised",
        "Host",
        RunOptions {
            limits: limits(budget),
            ..RunOptions::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
}

#[test]
fn namespace_publication_is_atomic_after_completed_module_initialization() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "First {}\nSecond {}",
    )
    .unwrap();
    let mut context = context(4);
    let mut observer = context.clone();
    let path = harness.workspace.join("main.botwork");
    let program =
        Program::parse(path.to_str().unwrap(), "Import |\"module.botwork\"| As |m|").unwrap();
    let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
    assert!(error.to_string().contains("registry entries"));
    assert!(context.statement_signature("m::First").unwrap().is_none());
    assert!(context.statement_signature("m::Second").unwrap().is_none());
    // Cached module definitions keep two entries; staged wrappers/namespaces release.
    observer
        .register_native("One", |_| Ok(Literal::None))
        .unwrap();
    observer
        .register_native("Two", |_| Ok(Literal::None))
        .unwrap();
    assert!(observer.checkpoint().is_ok());
}

#[test]
fn empty_namespaces_retain_exact_entry_source_and_preserve_collision_priority() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("empty.botwork"), "").unwrap();
    let source = "Import |\"empty.botwork\"| As |a|\nImport |\"empty.botwork\"| As |b|";
    let path = harness.workspace.join("main.botwork");
    let name = path.to_str().unwrap();
    let exact = RetainedRegistryLimits {
        entries: 2,
        nodes: 2,
        name_bytes: 1,
        text_bytes: 2,
        source_bytes: source.len() + name.len(),
    };
    let mut context = Context::with_limits(limits(exact.clone())).unwrap();
    evaluate_program_detailed(&Program::parse(name, source).unwrap(), &mut context).unwrap();
    let error = context
        .register_native("a::Other", |_| Ok(Literal::None))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::DuplicateNamespace);
    assert!(context.checkpoint().is_ok());
    let mut limited = Context::with_limits(limits(RetainedRegistryLimits {
        source_bytes: exact.source_bytes - 1,
        ..exact
    }))
    .unwrap();
    let error = evaluate_program_detailed(&Program::parse(name, source).unwrap(), &mut limited)
        .unwrap_err();
    assert!(error.to_string().contains("registry source bytes"));
}

#[test]
fn failed_module_initialization_releases_metadata_while_cache_aliases_share_it() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("bad.botwork"),
        "Read {}\n|x| = |missing|",
    )
    .unwrap();
    fs::write(
        harness.workspace.join("good.botwork"),
        "Read { Return |7| }",
    )
    .unwrap();
    let mut options = RunOptions {
        working_directory: Some(harness.workspace.clone()),
        limits: limits(RetainedRegistryLimits {
            entries: 1,
            ..RetainedRegistryLimits::default()
        }),
        ..RunOptions::default()
    };
    let source = "Try { Import |\"bad.botwork\"| As |bad| } Catch {}\nAfter {}";
    assert_eq!(
        Engine::default()
            .run_source("main.botwork", source, options.clone())
            .outcome(),
        RunOutcome::Succeeded
    );
    options.limits.retained_registry.entries = 5;
    let source =
        "Import |\"good.botwork\"| As |a|\nImport |\"good.botwork\"| As |b|\na::Read\nb::Read";
    let result = Engine::default().run_source("main.botwork", source, options);
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.result.unwrap().to_string(), "7");
}

#[test]
fn local_namespaces_release_after_calls_and_do_not_copy_inherited_native_metadata() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("empty.botwork"), "").unwrap();
    let source="Outer { Import |\"empty.botwork\"| As |local| }\nFor |item| In |[1,2,3]| { Outer }\nAfter {}";
    let result = Engine::default().run_source(
        "main.botwork",
        source,
        RunOptions {
            working_directory: Some(harness.workspace.clone()),
            limits: limits(RetainedRegistryLimits {
                entries: 2,
                ..RetainedRegistryLimits::default()
            }),
            ..RunOptions::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
}

#[test]
fn metadata_prefix_normalization_preserves_unicode_spacing_and_pipe_boundaries() {
    let mut context = context(3);
    for name in ["İtem", "Read Data", "Ready"] {
        context
            .register_native(name, |_| Ok(Literal::None))
            .unwrap();
    }
    let names = |prefix: &str| {
        context
            .complete_statements(prefix)
            .iter()
            .map(|metadata| metadata.normalized().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("İ"), vec!["i\u{307}tem"]);
    assert_eq!(names(" R E\tA D |ignored"), vec!["readdata", "ready"]);
    assert!(names(&"x".repeat(200_000)).is_empty());
    assert_eq!(names(&" \t".repeat(100_000)).len(), 3);
}

#[test]
fn registry_sources_release_after_last_context_and_namespace_owner() {
    let program = Program::parse("source", "Read {}").unwrap();
    let source = Arc::downgrade(&program.source);
    let mut context = context(1);
    evaluate_program_detailed(&program, &mut context).unwrap();
    let clone = context.clone();
    drop(program);
    drop(context);
    assert!(source.upgrade().is_some());
    drop(clone);
    assert!(source.upgrade().is_none());
}

#[test]
fn cli_registry_name_limit_rejects_declaration_before_later_effects() {
    let harness = Harness::new();
    let source = format!("{} {{}}\nLog |\"unreachable\"|", "x".repeat(65_537));
    let output = harness
        .run(
            "long-registry-name",
            &source,
            std::time::Duration::from_secs(15),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("registry name bytes"), "{stderr}");
    assert!(stderr.contains("BW8001"));
    let output = harness
        .run("recovery", "Log |7|", std::time::Duration::from_secs(15))
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"7\n");
}

#[test]
fn concurrent_clones_share_capacity_without_stopping_unrelated_contexts() {
    let base = context(1);
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let results = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let mut context = base.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let success = context
                        .register_native(&format!("Host {index}"), |_| Ok(Literal::None))
                        .is_ok();
                    barrier.wait();
                    (success, context)
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|(success, _)| *success).count(), 1);
    assert!(base.checkpoint().is_ok());
    drop(results);
    let mut available = base;
    available
        .register_native("After", |_| Ok(Literal::None))
        .unwrap();
}
