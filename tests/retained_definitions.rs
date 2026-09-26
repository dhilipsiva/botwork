#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::{Program, StatementKind},
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    run::{Engine, RetainedDefinitionLimits, RunLimits, RunOptions, RunOutcome},
};
use cli_harness::Harness;
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn limits(retained_definitions: RetainedDefinitionLimits) -> RunLimits {
    RunLimits {
        retained_definitions,
        ..RunLimits::default()
    }
}

fn options(retained_definitions: RetainedDefinitionLimits) -> RunOptions {
    RunOptions {
        limits: limits(retained_definitions),
        ..RunOptions::default()
    }
}

fn context(definitions: usize) -> Context {
    Context::with_limits(limits(RetainedDefinitionLimits {
        definitions,
        ..RetainedDefinitionLimits::default()
    }))
    .unwrap()
}

fn evaluate(name: &str, source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(&Program::parse(name, source).unwrap(), context)
}

#[test]
fn exact_counts_include_source_names_and_share_source_between_definitions() {
    let source = "First {}\nSecond {}";
    let name = "é🙂";
    let exact = RetainedDefinitionLimits {
        definitions: 2,
        nodes: 6,
        source_bytes: source.len() + name.len(),
    };
    let result = Engine::default().run_source(name, source, options(exact.clone()));
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    for (budget, resource) in [
        (
            RetainedDefinitionLimits {
                definitions: 1,
                ..exact.clone()
            },
            "retained definitions",
        ),
        (
            RetainedDefinitionLimits {
                nodes: 5,
                ..exact.clone()
            },
            "retained definition nodes",
        ),
        (
            RetainedDefinitionLimits {
                source_bytes: exact.source_bytes - 1,
                ..exact
            },
            "retained definition source bytes",
        ),
    ] {
        let mut context = Context::with_limits(limits(budget)).unwrap();
        let error = evaluate(name, source, &mut context).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert!(error.to_string().contains(resource));
        assert!(context.statement_signature("Second").unwrap().is_none());
        assert_eq!(
            context.statement_signature("First").unwrap().is_some(),
            resource != "retained definition source bytes"
        );
    }
}

#[test]
fn definition_free_execution_and_native_registration_allow_zero_budget() {
    let mut context = Context::with_limits(limits(RetainedDefinitionLimits {
        definitions: 0,
        nodes: 0,
        source_bytes: 0,
    }))
    .unwrap();
    context
        .register_native("Host", |_| Ok(Literal::Int(7)))
        .unwrap();
    assert_eq!(
        evaluate("host", "Host", &mut context).unwrap().to_string(),
        "7"
    );
    evaluate("assignment", "|x| = |1|", &mut context).unwrap();
    assert_eq!(
        evaluate("definition", "First {}", &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    assert!(context.statement_signature("First").unwrap().is_none());
}

#[test]
fn repeated_context_evaluations_accumulate_definitions_and_original_source() {
    let first = "First { Return |1| }";
    let second = "Second { Return |2| }";
    let mut context = Context::with_limits(limits(RetainedDefinitionLimits {
        source_bytes: first.len() + "first".len() + second.len() + "second".len() - 1,
        ..RetainedDefinitionLimits::default()
    }))
    .unwrap();
    evaluate("first", first, &mut context).unwrap();
    let error = evaluate("second", second, &mut context).unwrap_err();
    assert!(error
        .to_string()
        .contains("retained definition source bytes"));
    let metadata = context.statement_signature("First").unwrap().unwrap();
    assert_eq!(metadata.header().source().name(), "first");
    assert!(context.statement_signature("Second").unwrap().is_none());
    assert!(context.checkpoint().is_err());
}

#[test]
fn declaration_admission_precedes_body_effects_and_preserves_earlier_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Mark", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let result = engine.run_source(
        "effects",
        "Mark\nFirst {}\nTry { Second { Mark } } Catch { Mark }\nMark",
        options(RetainedDefinitionLimits {
            definitions: 1,
            ..RetainedDefinitionLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn collisions_precede_new_reservations_and_remain_catchable() {
    let mut context = context(1);
    evaluate("initial", "First {}", &mut context).unwrap();
    let error = evaluate("duplicate", "First { Return |7| }", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::DuplicateStatement);
    assert!(context.checkpoint().is_ok());
    evaluate("handled", "Try { First {} } Catch {}\nFirst", &mut context).unwrap();
}

#[test]
fn local_definitions_release_after_normal_return_error_and_iteration() {
    for body in ["", "Return |7|", "|x| = |missing|"] {
        let mut context = context(2);
        evaluate(
            "outer",
            &format!("Outer {{ Local {{}}\n{body}\n}}"),
            &mut context,
        )
        .unwrap();
        for _ in 0..50 {
            evaluate("calls", "Try { Outer } Catch {}", &mut context).unwrap();
        }
        evaluate("after", "After {}", &mut context).unwrap();
    }
    let mut context = context(3);
    evaluate(
        "loop",
        "Outer { Local {} }\nFor |item| In |[1,2,3]| { Outer }\nAfter {}",
        &mut context,
    )
    .unwrap();
}

#[test]
fn recursive_installations_of_the_same_definition_share_its_reservation() {
    let mut context = context(2);
    let source = "Walk |n| { Local {}\nIf |n > 0| { Walk |n-1| } }\nWalk |12|";
    evaluate("recursive", source, &mut context).unwrap();
    evaluate("after", "After {}", &mut context).unwrap();
}

#[test]
fn nested_definition_node_counts_include_each_installed_root_subtree() {
    // Outer retains seven nodes including Local; installing Local adds its three.
    for nodes in [9, 10] {
        let result = Engine::default().run_source(
            "nested",
            "Outer { Local {} }\nOuter",
            options(RetainedDefinitionLimits {
                definitions: 2,
                nodes,
                ..RetainedDefinitionLimits::default()
            }),
        );
        assert_eq!(
            result.outcome(),
            if nodes == 9 {
                RunOutcome::LimitExceeded
            } else {
                RunOutcome::Succeeded
            }
        );
    }
}

#[test]
fn context_clones_share_syntax_budget_and_release_when_last_owner_drops() {
    let mut original = context(2);
    evaluate("first", "First {}", &mut original).unwrap();
    let mut clone = original.clone();
    evaluate("second", "Second {}", &mut clone).unwrap();
    let mut failed = clone.clone();
    assert_eq!(
        evaluate("third", "Third {}", &mut failed)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    assert!(original.checkpoint().is_ok());
    drop(clone);
    drop(failed);
    evaluate("third", "Third {}", &mut original).unwrap();
}

#[test]
fn source_owners_release_with_the_final_definition_even_after_scope_failure() {
    let mut context = context(2);
    let program = Program::parse("temporary", "Outer { Local {}\nReturn |missing| }").unwrap();
    let StatementKind::Define(definition) = program.statements[0].kind() else {
        panic!("definition")
    };
    let source = Arc::downgrade(definition.span.source());
    let definition = Arc::downgrade(definition);
    evaluate_program_detailed(&program, &mut context).unwrap();
    drop(program);
    evaluate("call", "Try { Outer } Catch {}", &mut context).unwrap();
    assert!(source.upgrade().is_some());
    let clone = context.clone();
    drop(context);
    assert!(source.upgrade().is_some());
    drop(clone);
    assert!(source.upgrade().is_none());
    assert!(definition.upgrade().is_none());
}

#[test]
fn module_cache_and_aliases_share_definition_ownership_and_call_cleanup() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Read { Local {}\nReturn |7| }",
    )
    .unwrap();
    let source="Import |\"module.botwork\"| As |a|\nImport |\"module.botwork\"| As |b|\na::Read\nb::Read\nAfter {}";
    let mut run = options(RetainedDefinitionLimits {
        definitions: 2,
        ..RetainedDefinitionLimits::default()
    });
    run.working_directory = Some(harness.workspace.clone());
    assert_eq!(
        Engine::default()
            .run_source("main.botwork", source, run)
            .outcome(),
        RunOutcome::Succeeded
    );
}

#[test]
fn failed_module_initialization_releases_definitions_but_successful_cache_keeps_them() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("bad.botwork"),
        "Read {}\n|bad| = |missing|",
    )
    .unwrap();
    fs::write(harness.workspace.join("good.botwork"), "Read {}").unwrap();
    for (file, outcome) in [
        ("bad", RunOutcome::Succeeded),
        ("good", RunOutcome::LimitExceeded),
    ] {
        let mut run = options(RetainedDefinitionLimits {
            definitions: 1,
            ..RetainedDefinitionLimits::default()
        });
        run.working_directory = Some(harness.workspace.clone());
        let source =
            format!("Try {{ Import |\"{file}.botwork\"| As |module| }} Catch {{}}\nAfter {{}}");
        assert_eq!(
            Engine::default()
                .run_source("main.botwork", &source, run)
                .outcome(),
            outcome
        );
    }
}

#[test]
fn concurrent_clones_admit_one_new_definition_and_keep_sibling_latches_separate() {
    let original = context(1);
    let program = Program::parse("shared", "First {}").unwrap();
    // The same owned program's Definition is charged once even across separate installs.
    let contexts = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let mut context = original.clone();
                let program = &program;
                scope.spawn(move || {
                    evaluate_program_detailed(program, &mut context).unwrap();
                    context
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mut failed = original.clone();
    assert!(evaluate("new", "Second {}", &mut failed).is_err());
    assert!(original.checkpoint().is_ok());
    drop(contexts);
    let mut available = original;
    evaluate("new", "Second {}", &mut available).unwrap();
}

#[test]
fn engine_runs_are_fresh_and_explicitly_raised_definition_limits_work() {
    let engine = Engine::default();
    for _ in 0..3 {
        assert_eq!(
            engine
                .run_source(
                    "fresh",
                    "First {}",
                    options(RetainedDefinitionLimits {
                        definitions: 1,
                        ..RetainedDefinitionLimits::default()
                    })
                )
                .outcome(),
            RunOutcome::Succeeded
        );
    }
    // One reusable source/tree avoids testing parser capacity instead of live definitions.
    let source = (0..16_385)
        .map(|index| format!("Definition {index} {{}}\n"))
        .collect::<String>();
    let ast = botwork::core::ast_limits::AstLimits {
        nodes: 100_000,
        ..Default::default()
    };
    let program =
        Program::parse_with_budgets("raised", &source, source.len(), &Default::default(), &ast)
            .unwrap();
    // Context's AST admission must also allow the intentionally wide program.
    let mut context = Context::with_limits(RunLimits {
        ast,
        retained_definitions: RetainedDefinitionLimits {
            definitions: 16_385,
            ..RetainedDefinitionLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    evaluate_program_detailed(&program, &mut context).unwrap();
    assert!(context
        .statement_signature("Definition 16384")
        .unwrap()
        .is_some());
}

#[test]
fn cli_default_definition_limit_rejects_without_running_later_effects() {
    let harness = Harness::new();
    // Splitting declarations between files stays within each program's AST limit.
    fs::write(
        harness.workspace.join("many.botwork"),
        (0..10_000)
            .map(|index| format!("Definition {index} {{}}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let mut source = String::from("Import |\"many.botwork\"| As |module|\n");
    for index in 0..6_385 {
        source.push_str(&format!("Definition {index} {{}}\n"));
    }
    source.push_str("Log |\"unreachable\"|");
    let output = harness
        .run(
            "default-definitions",
            &source,
            std::time::Duration::from_secs(15),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("BW8001"), "{stderr}");
    assert!(stderr.contains("retained definitions"), "{stderr}");
    let output = harness
        .run("recovery", "Log |7|", std::time::Duration::from_secs(15))
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"7\n");
}
