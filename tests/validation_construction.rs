use botwork::core::{
    ast::{Program, StatementKind},
    diagnostic::{DiagnosticCode, DiagnosticLimits},
    eval::{botwork_detailed, evaluate_program_detailed, execute_statement_detailed, Context},
    grammar::{BWErr, BWParser, Literal, Rule},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
};
use pest::Parser;
use std::{collections::BTreeMap, sync::Arc};

fn options(diagnostics: DiagnosticLimits) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            diagnostics,
            ..RunLimits::default()
        },
        variables: BTreeMap::from([("seed".into(), Literal::Int(7))]),
        ..RunOptions::default()
    }
}

#[test]
fn validation_preserves_exact_details_and_rejects_each_context_dimension_before_effects() {
    for source in [
        "Log |1|\nReturn",
        "Log |1|\nIf |false| { Break }",
        "Unused { Continue }",
        "Try {} Catch { Nested { Rethrow } }",
        "Read |é| with |é| {}",
    ] {
        let baseline = Program::parse_detailed("é.botwork", source).unwrap_err();
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        let exact = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        let engine = Engine::default();
        let run = engine.run_source("é.botwork", source, options(exact.clone()));
        assert_eq!(run.outcome(), RunOutcome::Failed);
        assert_eq!(run.steps, 0);
        assert_eq!(run.variables["seed"].to_string(), "7");
        assert_eq!(
            run.result.unwrap_err().to_value().to_string(),
            baseline.to_value().to_string()
        );
        for dimension in 0..5 {
            let mut limits = exact.clone();
            match dimension {
                0 => limits.diagnostics = 0,
                1 => limits.depth = 0,
                2 => limits.text_bytes -= 1,
                3 => limits.source_bytes -= 1,
                _ if size.related_locations != 0 => limits.related_locations = 0,
                _ => continue,
            }
            let run = engine.run_source("é.botwork", source, options(limits));
            assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
            assert_eq!(run.steps, 0);
            assert_eq!(run.variables["seed"].to_string(), "7");
            let error = run.result.unwrap_err();
            assert_eq!(error.causes[0].code(), baseline.code());
            let omitted = error.causes[0].omissions.as_ref().unwrap();
            assert_eq!(
                omitted.detail_fields,
                if size.related_locations == 0 { 1 } else { 2 }
            );
            assert_eq!(omitted.related_locations, size.related_locations);
            assert_eq!(
                omitted.source.as_ref().unwrap().start_byte,
                baseline.span.as_ref().unwrap().start()
            );
            assert!(error.causes[0].span.is_none());
            match error.causes[0].error.as_ref() {
                BWErr::ControlFlowError(detail) => assert!(detail.contains("coordinates omitted")),
                BWErr::DuplicateParameter {
                    name,
                    original,
                    duplicate,
                } => {
                    assert_eq!(name, "é");
                    assert!(
                        original.contains("coordinates omitted")
                            && duplicate.contains("coordinates omitted")
                    );
                }
                _ => panic!("validation category"),
            }
        }
        assert_eq!(
            engine
                .run_source(
                    "fresh",
                    "|x| = |1|",
                    options(DiagnosticLimits {
                        text_bytes: 0,
                        ..DiagnosticLimits::default()
                    })
                )
                .outcome(),
            RunOutcome::Succeeded
        );
    }
}

fn moved_return(name: &str) -> Program {
    let mut program = Program::parse(name, "Read { Return |7| }").unwrap();
    let StatementKind::Define(definition) = program.statements[0].kind() else {
        panic!("definition")
    };
    program.statements = definition.body.statements.clone();
    program
}

#[test]
fn owned_program_and_statement_validation_latch_only_the_requester_and_release_sources() {
    for statement_entry in [false, true] {
        let program = moved_return("owned-é");
        let source = Arc::downgrade(&program.source);
        let mut context = Context::with_limits(
            options(DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            })
            .limits,
        )
        .unwrap();
        let mut sibling = context.clone();
        let error = if statement_entry {
            execute_statement_detailed(&program.statements[0], &mut context)
        } else {
            evaluate_program_detailed(&program, &mut context)
        }
        .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::InvalidControl);
        assert!(context.checkpoint().is_err());
        let valid = Program::parse("valid", "|x| = |1|").unwrap();
        evaluate_program_detailed(&valid, &mut sibling).unwrap();
        drop(program);
        assert!(source.upgrade().is_none());
        assert!(evaluate_program_detailed(&valid, &mut context).is_err());
    }
}

#[test]
fn pair_validation_uses_local_limits_for_blocks_definitions_and_detached_catch_bindings() {
    for (rule, source, code, fields) in [
        (
            Rule::stmt_block,
            "{ Break }",
            DiagnosticCode::InvalidControl,
            1,
        ),
        (
            Rule::stmt_return,
            "Return",
            DiagnosticCode::InvalidControl,
            1,
        ),
        (
            Rule::stmt_catch,
            "Catch |é| {}",
            DiagnosticCode::InvalidControl,
            1,
        ),
        (
            Rule::stmt_define,
            "Read |é| with |é| {}",
            DiagnosticCode::DuplicateParameter,
            2,
        ),
    ] {
        for reject in [false, true] {
            let pair = BWParser::parse(rule, source).unwrap().next().unwrap();
            let mut context = Context::with_limits(
                options(if reject {
                    DiagnosticLimits {
                        text_bytes: 0,
                        ..DiagnosticLimits::default()
                    }
                } else {
                    DiagnosticLimits::default()
                })
                .limits,
            )
            .unwrap();
            let error = botwork_detailed(pair, &mut context).unwrap_err();
            if reject {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), code);
                assert_eq!(
                    error.causes[0].omissions.as_ref().unwrap().detail_fields,
                    fields
                );
                assert!(context.checkpoint().is_err());
            } else {
                assert_eq!(error.code(), code);
                assert!(error.omissions.is_none());
                assert!(context.checkpoint().is_ok());
            }
        }
    }
}

#[test]
fn earlier_source_ast_and_cancellation_limits_preserve_validation_priority() {
    let source = "Return";
    let mut settings = options(DiagnosticLimits {
        text_bytes: 0,
        ..DiagnosticLimits::default()
    });
    settings.limits.source_bytes = 0;
    let run = Engine::default().run_source("source", source, settings);
    let error = run.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert!(!error
        .causes
        .iter()
        .any(|cause| cause.code() == DiagnosticCode::InvalidControl));

    let mut settings = options(DiagnosticLimits::default());
    settings.limits.ast.nodes = 0;
    let error = Engine::default()
        .run_source("ast", source, settings)
        .result
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(error.to_string().contains("AST nodes"));

    let settings = options(DiagnosticLimits {
        text_bytes: 0,
        ..DiagnosticLimits::default()
    });
    settings.control.cancel();
    let run = Engine::default().run_source("stopped", source, settings);
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert_eq!(run.steps, 0);
}
