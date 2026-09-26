use botwork::core::{
    ast::Program,
    ast_limits::{AstLimits, DEFAULT_AST_NODES, MAX_AST_DEPTH},
    diagnostic::DiagnosticCode,
    eval::{botwork_detailed, evaluate_program_detailed, execute_statement_detailed, Context},
    grammar::{BWParser, Rule},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
    syntax_limits::SyntaxLimits,
};
use pest::Parser;
#[path = "support/cli_harness.rs"]
mod cli_harness;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn repeated_host_statements_fail_before_any_callback_or_binding() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Mark", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(botwork::core::grammar::Literal::None)
        })
        .unwrap();
    let mut program = Program::parse("wide", "Mark").unwrap();
    program.statements = vec![program.statements[0].clone(); DEFAULT_AST_NODES];
    let run = engine.run_program(&program, RunOptions::default());
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.steps, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(run.result.unwrap_err().to_string().contains("AST nodes"));
    assert_eq!(
        program.validate().unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn exact_tree_limits_are_local_and_do_not_charge_execution_steps() {
    let program = Program::parse("exact", "|x| = |1+2|").unwrap();
    for (nodes, depth, expected) in [
        (6, 3, RunOutcome::Succeeded),
        (5, 3, RunOutcome::LimitExceeded),
        (6, 2, RunOutcome::LimitExceeded),
    ] {
        let run = Engine::default().run_program(
            &program,
            RunOptions {
                limits: RunLimits {
                    ast: AstLimits {
                        nodes,
                        depth,
                        ..AstLimits::default()
                    },
                    ..RunLimits::default()
                },
                ..RunOptions::default()
            },
        );
        assert_eq!(run.outcome(), expected);
        assert_eq!(
            run.steps,
            if expected == RunOutcome::Succeeded {
                4
            } else {
                0
            }
        );
    }
    assert_eq!(
        Engine::default()
            .run_program(&program, RunOptions::default())
            .outcome(),
        RunOutcome::Succeeded
    );
}

#[test]
fn reassembled_programs_count_source_allocations_once_and_cannot_hide_owners() {
    let first = Program::parse("first", "|x| = |1|").unwrap();
    let second = Program::parse("second", "|x| = |1|").unwrap();
    let bytes = first.source.text().len();
    let mut program = first.clone();
    program.statements.push(first.statements[0].clone());
    let limits = AstLimits {
        source_bytes: bytes,
        ..AstLimits::default()
    };
    program.validate_with_limits(&limits, bytes).unwrap();
    program.statements.push(second.statements[0].clone());
    let error = program.validate_with_limits(&limits, bytes).unwrap_err();
    assert!(error.to_string().contains("AST source bytes"));
    assert_eq!(error.span.unwrap().source().name(), "second");
    program
        .validate_with_limits(
            &AstLimits {
                source_bytes: bytes * 2,
                ..limits
            },
            bytes,
        )
        .unwrap();
    program.source = Program::parse("empty", "").unwrap().source;
    let error = program
        .validate_with_limits(&AstLimits::default(), bytes - 1)
        .unwrap_err();
    assert!(error.to_string().contains("source bytes"));
    assert_eq!(error.span.unwrap().source().name(), "first");
}

#[test]
fn empty_program_root_source_is_counted_even_without_statements() {
    let program = Program::parse("comment", "# comment").unwrap();
    let limits = AstLimits {
        nodes: 0,
        depth: 0,
        source_bytes: 9,
    };
    program.validate_with_limits(&limits, 9).unwrap();
    let error = program
        .validate_with_limits(
            &AstLimits {
                source_bytes: 8,
                ..limits
            },
            9,
        )
        .unwrap_err();
    assert!(error.to_string().contains("AST source bytes"));
    assert!(error.span.is_none());
}

#[test]
fn context_statement_program_and_pair_failures_latch_before_execution() {
    let limits = RunLimits {
        ast: AstLimits {
            nodes: 2,
            ..AstLimits::default()
        },
        ..RunLimits::default()
    };
    let program = Program::parse("assignment", "|x| = |1|").unwrap();
    for entry in 0..4 {
        let mut context = Context::with_limits(limits.clone()).unwrap();
        let error = match entry {
            0 => evaluate_program_detailed(&program, &mut context),
            1 => execute_statement_detailed(&program.statements[0], &mut context),
            _ => {
                let (rule, source) = if entry == 2 {
                    (Rule::expression, "1+2")
                } else {
                    (Rule::stmt_block, "{ |x| = |1| }")
                };
                botwork_detailed(
                    BWParser::parse(rule, source).unwrap().next().unwrap(),
                    &mut context,
                )
            }
        }
        .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(
            context.checkpoint().unwrap_err().code(),
            DiagnosticCode::ResourceLimit
        );
    }
}

#[test]
fn parsing_uses_configured_tree_budgets_before_control_validation() {
    let limits = AstLimits {
        nodes: 0,
        ..AstLimits::default()
    };
    let error =
        Program::parse_with_budgets("invalid", "Break", 100, &SyntaxLimits::default(), &limits)
            .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    let error = Program::parse_with_budgets(
        "invalid",
        "Break",
        100,
        &SyntaxLimits::default(),
        &AstLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::InvalidControl);
    let error = Engine::default()
        .run_source(
            "invalid",
            "Break",
            RunOptions {
                limits: RunLimits {
                    ast: limits,
                    ..RunLimits::default()
                },
                ..RunOptions::default()
            },
        )
        .result
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
}

#[test]
fn unsupported_tree_depth_is_rejected_at_configuration_entry() {
    let limits = RunLimits {
        ast: AstLimits {
            depth: MAX_AST_DEPTH + 1,
            ..AstLimits::default()
        },
        ..RunLimits::default()
    };
    assert_eq!(
        Context::with_limits(limits.clone()).err().unwrap().code(),
        DiagnosticCode::RunConfiguration
    );
    assert_eq!(
        Engine::default()
            .run_source(
                "empty",
                "",
                RunOptions {
                    limits,
                    ..RunOptions::default()
                }
            )
            .result
            .unwrap_err()
            .code(),
        DiagnosticCode::RunConfiguration
    );
}

#[test]
fn source_and_tree_budgets_can_be_raised_explicitly() {
    let source = "|x| = |1|\n".repeat(DEFAULT_AST_NODES / 3 + 1);
    assert_eq!(
        Program::parse("wide", &source).unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
    let limits = AstLimits {
        nodes: DEFAULT_AST_NODES + 3,
        ..AstLimits::default()
    };
    let program = Program::parse_with_budgets(
        "wide",
        &source,
        source.len(),
        &SyntaxLimits::default(),
        &limits,
    )
    .unwrap();
    program.validate_with_limits(&limits, source.len()).unwrap();
    let source = format!(
        "#{}",
        "x".repeat(botwork::core::syntax_limits::DEFAULT_SOURCE_BYTES)
    );
    let program = Program::parse_bounded(
        "large-comment",
        &source,
        source.len(),
        &SyntaxLimits::default(),
    )
    .unwrap();
    program
        .validate_with_limits(&AstLimits::default(), source.len())
        .unwrap();
    assert!(program.validate_detailed().is_err());
}

#[test]
fn imported_tree_failure_bypasses_catch_and_precedes_module_effects() {
    let harness = cli_harness::Harness::new();
    std::fs::write(
        harness.workspace.join("module.botwork"),
        "Mark\n|a| = |1|\n|b| = |2|\n|c| = |3|",
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Mark", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(botwork::core::grammar::Literal::None)
        })
        .unwrap();
    let run = engine.run_source(
        "main",
        "Try { Import |\"module.botwork\"| As |module| } Catch { |handled| = |true| }",
        RunOptions {
            working_directory: Some(harness.workspace.clone()),
            limits: RunLimits {
                ast: AstLimits {
                    nodes: 9,
                    ..AstLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(!run.variables.contains_key("handled"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let error = run.result.unwrap_err();
    assert!(error.to_string().contains("AST nodes"));
    assert!(!error.related.is_empty());
}

#[test]
fn cli_tree_admission_precedes_output_and_debug_traces() {
    let harness = cli_harness::Harness::new();
    let source = format!(
        "Log |\"unreachable\"|\n{}",
        "|x| = |1|\n".repeat(DEFAULT_AST_NODES / 3 + 1)
    );
    for debug in [false, true] {
        let output = if debug {
            harness.run_with_args(
                "wide-debug",
                &source,
                &["--debug"],
                std::time::Duration::from_secs(5),
            )
        } else {
            harness.run("wide", &source, std::time::Duration::from_secs(5))
        }
        .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("[BW8001]"));
        assert!(diagnostic.contains("AST nodes"));
        assert!(!diagnostic.contains("debug:"));
    }
}
