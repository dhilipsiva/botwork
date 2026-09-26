#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode,
    eval::{evaluate_program_detailed, Context},
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome, MAX_EVALUATION_DEPTH},
};
use cli_harness::Harness;
use std::{fs, time::Duration};

#[test]
fn zero_and_exact_evaluation_depth_boundaries_are_enforced_before_effects() {
    for (depth, source, expected) in [
        (0, "", RunOutcome::Succeeded),
        (0, "|x| = |1|", RunOutcome::LimitExceeded),
        (1, "|x| = |1|", RunOutcome::LimitExceeded),
        (2, "|x| = |1|", RunOutcome::Succeeded),
        (2, "|x| = |1+2|", RunOutcome::LimitExceeded),
        (3, "|x| = |1+2|", RunOutcome::Succeeded),
    ] {
        let run = Engine::default().run_source(
            "depth",
            source,
            RunOptions {
                limits: RunLimits {
                    evaluation_depth: depth,
                    ..RunLimits::default()
                },
                ..RunOptions::default()
            },
        );
        assert_eq!(run.outcome(), expected, "{depth}: {source}");
        if expected == RunOutcome::LimitExceeded {
            assert!(!run.variables.contains_key("x"));
        }
    }
}

#[test]
fn import_depth_configuration_rejects_excess_and_zero_prevents_initialization() {
    use botwork::core::run::MAX_IMPORT_DEPTH;
    let invalid = RunLimits {
        import_depth: MAX_IMPORT_DEPTH + 1,
        ..RunLimits::default()
    };
    assert_eq!(
        Context::with_limits(invalid.clone()).err().unwrap().code(),
        DiagnosticCode::RunConfiguration
    );
    assert_eq!(
        Engine::default()
            .run_source(
                "empty",
                "",
                RunOptions {
                    limits: invalid,
                    ..RunOptions::default()
                }
            )
            .result
            .unwrap_err()
            .code(),
        DiagnosticCode::RunConfiguration
    );
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Log |\"unreachable\"|",
    )
    .unwrap();
    let run = Engine::default().run_source(
        "zero",
        "Try { Import |\"module.botwork\"| As |module| } Catch { |handled| = |true| }",
        RunOptions {
            working_directory: Some(harness.workspace.clone()),
            limits: RunLimits {
                import_depth: 0,
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run
        .result
        .unwrap_err()
        .to_string()
        .contains("import initialization depth"));
    assert!(!run.variables.contains_key("handled"));
}

#[test]
fn combined_statement_and_call_depth_stops_before_stack_exhaustion() {
    let source = format!(
        "Recurse {{{}Recurse{}}}\nRecurse",
        "If |true| {".repeat(16),
        "}".repeat(16)
    );
    let run = Engine::default().run_source(
        "nested",
        &source,
        RunOptions {
            limits: RunLimits {
                call_depth: 10000,
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run
        .result
        .unwrap_err()
        .to_string()
        .contains("evaluation depth"));
}

#[test]
fn combined_module_loading_and_parser_depth_stops_before_stack_exhaustion() {
    let harness = Harness::new();
    for index in 0..=MAX_EVALUATION_DEPTH + 1 {
        let source = format!(
            "Import |\"{}.botwork\"| As |next|\nDeep {{ Return |{}1{}| }}",
            index + 1,
            "(".repeat(30),
            ")".repeat(30)
        );
        fs::write(harness.workspace.join(format!("{index}.botwork")), source).unwrap();
    }
    let run = Engine::default().run_file(
        "0.botwork",
        RunOptions {
            working_directory: Some(harness.workspace.clone()),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run
        .result
        .unwrap_err()
        .to_string()
        .contains("import initialization depth"));
}

#[test]
fn legacy_contexts_have_persistent_budgets_and_can_be_configured_without_global_state() {
    let program = Program::parse("steps", "|x| = |1|").unwrap();
    let mut context = Context::with_limits(RunLimits {
        steps: 2,
        ..RunLimits::default()
    })
    .unwrap();
    assert_eq!(
        evaluate_program_detailed(&program, &mut context)
            .unwrap()
            .to_string(),
        "1"
    );
    assert_eq!(
        evaluate_program_detailed(&program, &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    assert!(context.checkpoint().is_err());
    assert!(evaluate_program_detailed(&program, &mut Context::default()).is_ok());
    let invalid = Context::with_limits(RunLimits {
        evaluation_depth: MAX_EVALUATION_DEPTH + 1,
        ..RunLimits::default()
    });
    assert_eq!(
        invalid.err().unwrap().code(),
        DiagnosticCode::RunConfiguration
    );
    let control = OperationControl::default();
    control.cancel();
    assert_eq!(
        Context::with_control(RunLimits::default(), control)
            .err()
            .unwrap()
            .code(),
        DiagnosticCode::Cancelled
    );
}

#[test]
fn cli_runtime_flags_stop_loops_and_reject_invalid_configurations() {
    let harness = Harness::new();
    for (args, code) in [
        (vec!["--max-steps", "10"], "BW8001"),
        (vec!["--max-evaluation-depth", "1"], "BW8001"),
        (vec!["--timeout-ms", "0"], "BW5002"),
        (vec!["--max-evaluation-depth", "97"], "BW7002"),
    ] {
        let output = harness
            .run_with_args("stopped", "While |true| {}", &args, Duration::from_secs(5))
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr).unwrap().contains(code));
    }
    let output = harness
        .run(
            "default-recursion",
            "Recurse { Recurse }\nRecurse",
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("call depth"));
}

#[test]
fn imports_under_recursive_calls_reserve_stack_space_before_parsing() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        format!("Deep {{ Return |{}1{}| }}", "(".repeat(30), ")".repeat(30)),
    )
    .unwrap();
    for depth in 0..=30 {
        let source = format!("Load |n| {{ If |n > 0| {{ Load |n - 1| }} Else {{ Import |\"module.botwork\"| As |module| }} }}\nLoad |{depth}|");
        let run = Engine::default().run_source(
            "recursive-import",
            &source,
            RunOptions {
                working_directory: Some(harness.workspace.clone()),
                ..RunOptions::default()
            },
        );
        match run.outcome() {
            RunOutcome::Succeeded => assert!(depth < 10),
            RunOutcome::LimitExceeded => assert!(run
                .result
                .unwrap_err()
                .to_string()
                .contains("parser caller depth")),
            outcome => panic!("unexpected {outcome:?}"),
        }
    }
}

#[test]
fn depth_guards_release_after_success_return_and_ordinary_failure() {
    let mut context = Context::with_limits(RunLimits {
        evaluation_depth: 4,
        ..RunLimits::default()
    })
    .unwrap();
    evaluate_program_detailed(
        &Program::parse(
            "definitions",
            "Read { Return |7| }\nFail { Return |missing| }",
        )
        .unwrap(),
        &mut context,
    )
    .unwrap();
    for _ in 0..20 {
        assert_eq!(
            evaluate_program_detailed(&Program::parse("read", "Read").unwrap(), &mut context)
                .unwrap()
                .to_string(),
            "7"
        );
        assert_eq!(
            evaluate_program_detailed(&Program::parse("fail", "Fail").unwrap(), &mut context)
                .unwrap_err()
                .code(),
            DiagnosticCode::UndefinedVariable
        );
        assert_eq!(
            evaluate_program_detailed(&Program::parse("add", "|x| = |1+2|").unwrap(), &mut context)
                .unwrap()
                .to_string(),
            "3"
        );
    }
    context.checkpoint().unwrap();
}

#[test]
fn depth_exhaustion_restores_iterators_skips_handlers_and_keeps_call_sites() {
    let run = Engine::default().run_source("restore", "|item| = |9|\nRead { Return |1+2| }\nTry { For |item| In |[1]| { Read } } Catch { |handled| = |true| }", RunOptions {
        limits: RunLimits { evaluation_depth: 6, ..RunLimits::default() }, ..RunOptions::default()
    });
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.variables["item"].to_string(), "9");
    assert!(!run.variables.contains_key("handled"));
    let error = run.result.unwrap_err();
    assert_eq!(error.span.as_ref().unwrap().text(), "1");
    assert_eq!(error.call_stack[0].signature, "read");
    assert!(error.causes.is_empty());
}

#[test]
fn cloned_context_counters_and_stop_latches_are_independent() {
    let program = Program::parse("steps", "|x| = |1|").unwrap();
    let mut original = Context::with_limits(RunLimits {
        steps: 3,
        ..RunLimits::default()
    })
    .unwrap();
    evaluate_program_detailed(&program, &mut original).unwrap();
    let mut cloned = original.clone();
    assert!(evaluate_program_detailed(&program, &mut original).is_err());
    cloned.checkpoint().unwrap();
    evaluate_program_detailed(
        &Program::parse("definition", "New {}").unwrap(),
        &mut cloned,
    )
    .unwrap();
    assert!(original.checkpoint().is_err());
    cloned.checkpoint().unwrap();
}

#[test]
fn parser_pair_entry_points_observe_controls_and_share_runtime_budgets() {
    use botwork::core::{
        eval::botwork_detailed,
        grammar::{BWParser, Rule},
    };
    use pest::Parser;
    let control = OperationControl::default();
    let mut context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    control.cancel();
    let pair = BWParser::parse(Rule::EOI, "").unwrap().next().unwrap();
    assert_eq!(
        botwork_detailed(pair, &mut context).unwrap_err().code(),
        DiagnosticCode::Cancelled
    );
    let mut context = Context::with_limits(RunLimits {
        steps: 1,
        ..RunLimits::default()
    })
    .unwrap();
    let pair = BWParser::parse(Rule::expression, "1+2")
        .unwrap()
        .next()
        .unwrap();
    assert_eq!(
        botwork_detailed(pair, &mut context).unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn cached_imports_do_not_consume_another_initialization_depth() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("base.botwork"),
        "Read { Return |7| }",
    )
    .unwrap();
    fs::write(
        harness.workspace.join("outer.botwork"),
        "Import |\"base.botwork\"| As |base|",
    )
    .unwrap();
    let options = RunOptions {
        working_directory: Some(harness.workspace.clone()),
        limits: RunLimits {
            import_depth: 1,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let engine = Engine::default();
    let failed = engine.run_source(
        "fresh",
        "Import |\"outer.botwork\"| As |outer|",
        options.clone(),
    );
    assert_eq!(failed.outcome(), RunOutcome::LimitExceeded);
    let success = engine.run_source("cached", "Import |\"base.botwork\"| As |base|\nImport |\"outer.botwork\"| As |outer|\nouter::base::Read", options);
    assert_eq!(success.result.unwrap().to_string(), "7");
}

#[test]
fn reexport_dispatch_counts_depth_even_before_a_custom_body_is_entered() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let harness = Harness::new();
    fs::write(harness.workspace.join("m0.botwork"), "Read { Mark }").unwrap();
    let mut source = "Import |\"m0.botwork\"| As |m0|\n".to_owned();
    for index in 1..=12 {
        fs::write(
            harness.workspace.join(format!("m{index}.botwork")),
            format!("Import |\"m{}.botwork\"| As |previous|", index - 1),
        )
        .unwrap();
        source.push_str(&format!("Import |\"m{index}.botwork\"| As |m{index}|\n"));
    }
    source.push_str(&format!("m12::{}Read", "previous::".repeat(12)));
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::clone(&calls);
    let mut engine = Engine::default();
    engine
        .register_native("Mark", move |_, _| {
            entered.fetch_add(1, Ordering::SeqCst);
            Ok(botwork::core::grammar::Literal::None)
        })
        .unwrap();
    let run = engine.run_source(
        "reexports",
        &source,
        RunOptions {
            working_directory: Some(harness.workspace.clone()),
            limits: RunLimits {
                evaluation_depth: 8,
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    let error = run.result.unwrap_err();
    assert!(error.to_string().contains("evaluation depth"));
    assert!(error.call_stack.is_empty());
    assert!(!error.related.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cli_default_loop_budget_and_explicit_call_budget_fail_without_panicking() {
    let harness = Harness::new();
    let looped = harness
        .run("loop-budget", "While |true| {}", Duration::from_secs(5))
        .unwrap();
    assert_eq!(looped.status.code(), Some(1));
    assert!(String::from_utf8(looped.stderr)
        .unwrap()
        .contains("evaluation steps"));
    let call = harness
        .run_with_args(
            "no-calls",
            "Log |\"unreachable\"|",
            &["--max-call-depth", "0"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(call.status.code(), Some(1));
    assert!(call.stdout.is_empty());
    assert!(String::from_utf8(call.stderr)
        .unwrap()
        .contains("call depth"));
}

#[test]
fn cli_budget_options_conflict_with_statement_help_and_validate_numeric_values() {
    use std::process::Command;
    for args in [
        vec!["--list-statements", "--max-steps", "1"],
        vec!["--statement-help", "Log |value|", "--timeout-ms", "1"],
        vec!["--file", "unused", "--max-steps", "invalid"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
