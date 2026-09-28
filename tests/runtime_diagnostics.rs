#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticLimits},
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome},
};
use std::{collections::BTreeMap, time::Duration};

fn options(diagnostics: DiagnosticLimits) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            diagnostics,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}
fn engine() -> Engine {
    let mut engine = Engine::default();
    engine
        .register_native("Fail", |_, _| Err(BWErr::NativeError("reason".into())))
        .unwrap();
    engine
}
fn run(limits: DiagnosticLimits) -> botwork::core::run::RunResult {
    engine().run_source(
        "diagnostic",
        "|before| = |1|\nFail\n|after| = |2|",
        options(limits),
    )
}

#[test]
fn exact_runtime_tree_text_source_and_frame_limits_preserve_original_errors() {
    let baseline = run(DiagnosticLimits::default()).result.unwrap_err();
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    let limits = DiagnosticLimits {
        diagnostics: size.diagnostics,
        depth: size.depth,
        call_frames: size.call_frames,
        related_locations: size.related_locations,
        text_bytes: size.text_bytes,
        source_bytes: size.source_bytes,
    };
    let result = run(limits);
    assert_eq!(result.outcome(), RunOutcome::Failed);
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Native);
    assert_eq!(DiagnosticLimits::default().check(&error).unwrap(), size);
    assert!(error.omissions.is_none());
    assert_eq!(result.variables["before"].to_string(), "1");
    assert!(!result.variables.contains_key("after"));
}

#[test]
fn runtime_rejection_retains_one_bounded_original_through_every_unwind_boundary() {
    let baseline = run(DiagnosticLimits::default()).result.unwrap_err();
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    for field in 0..5 {
        let mut limits = DiagnosticLimits::default();
        match field {
            0 => limits.diagnostics = 0,
            1 => limits.depth = 0,
            2 => limits.call_frames = size.call_frames - 1,
            3 => limits.text_bytes = size.text_bytes - 1,
            _ => limits.source_bytes = size.source_bytes - 1,
        }
        let result = run(limits);
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        let error = result.result.unwrap_err();
        assert_eq!(error.causes.len(), 1);
        assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
        assert!(error.span.is_none() && error.call_stack.is_empty() && error.related.is_empty());
        let summary = &error.causes[0];
        assert!(summary.causes.is_empty());
        assert_eq!(summary.omissions.as_ref().unwrap().call_frames, 1);
        assert!(summary.omissions.as_ref().unwrap().source.is_some());
        assert_eq!(result.variables["before"].to_string(), "1");
    }
}

#[test]
fn handler_cause_growth_bypasses_outer_catch_and_restores_prior_binding() {
    let source = "Try { Try { First } Catch |error| { Second } } Catch { |unexpected| = |true| }";
    let mut options = options(DiagnosticLimits {
        diagnostics: 1,
        ..DiagnosticLimits::default()
    });
    options.variables = BTreeMap::from([("error".into(), Literal::Int(7))]);
    let result = Engine::default().run_source("handler", source, options);
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(result.variables["error"].to_string(), "7");
    assert!(!result.variables.contains_key("unexpected"));
    let error = result.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().direct_causes, 1);
}

#[test]
fn rethrow_related_metadata_is_admitted_and_emergency_import_sites_stay_bounded() {
    let source = "Try { Missing } Catch { Rethrow }";
    let result = Engine::default().run_source(
        "rethrow",
        source,
        options(DiagnosticLimits {
            related_locations: 0,
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(
        result.result.unwrap_err().causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .related_locations,
        1
    );

    let harness = cli_harness::Harness::new();
    std::fs::write(harness.workspace.join("module.botwork"), "Fail { Missing }").unwrap();
    let mut options = options(DiagnosticLimits {
        call_frames: 0,
        ..DiagnosticLimits::default()
    });
    options.working_directory = Some(harness.workspace.clone());
    let result = Engine::default().run_source(
        "entry",
        "Import |\"module.botwork\"| As |lib|\nTry { lib::Fail } Catch { |unexpected| = |true| }",
        options,
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(!result.variables.contains_key("unexpected"));
    let error = result.result.unwrap_err();
    assert!(error.related.is_empty());
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
    assert!(
        error.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .related_locations
            >= 1
    );
}

#[test]
fn observed_cancellation_keeps_priority_over_diagnostic_limits() {
    let mut engine = Engine::default();
    engine
        .register_native("Stop", |_, environment| {
            environment.control().cancel();
            Err(BWErr::NativeError("reason".into()))
        })
        .unwrap();
    let result = engine.run_source(
        "cancel",
        "Try { Stop } Catch { |unexpected| = |true| }",
        options(DiagnosticLimits {
            diagnostics: 0,
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(!result.variables.contains_key("unexpected"));
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert!(error.omissions.is_some());
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert!(error.causes[0].causes.is_empty());
}

#[tokio::test(start_paused = true)]
async fn observed_timeout_keeps_priority_with_zero_diagnostic_storage() {
    let control = OperationControl::default()
        .child(Some(tokio::time::Instant::now() + Duration::from_secs(1)));
    let mut context = Context::with_control(
        RunLimits {
            diagnostics: DiagnosticLimits {
                diagnostics: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        },
        control,
    )
    .unwrap();
    let program = Program::parse("timeout", "").unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert!(error.omissions.is_some());
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
}

#[test]
fn limits_latch_only_requesting_context_and_fresh_runs_remain_usable() {
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            diagnostics: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let mut clone = context.clone();
    let program = Program::parse("context", "Missing").unwrap();
    assert_eq!(
        evaluate_program_detailed(&program, &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    let empty = Program::parse("context", "").unwrap();
    assert!(evaluate_program_detailed(&empty, &mut context).is_err());
    assert!(evaluate_program_detailed(&empty, &mut clone).is_ok());
    assert_eq!(
        Engine::default()
            .run_source(
                "fresh",
                "",
                options(DiagnosticLimits {
                    diagnostics: 0,
                    ..DiagnosticLimits::default()
                })
            )
            .outcome(),
        RunOutcome::Succeeded
    );
}

#[test]
fn parse_validation_and_invalid_limit_configuration_have_explicit_boundaries() {
    let error = Engine::default()
        .run_source(
            "syntax",
            "|x| = |",
            options(DiagnosticLimits {
                source_bytes: 0,
                ..DiagnosticLimits::default()
            }),
        )
        .result
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Syntax);
    let error = Engine::default()
        .run_source(
            "config",
            "Missing",
            options(DiagnosticLimits {
                depth: 65,
                ..DiagnosticLimits::default()
            }),
        )
        .result
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    let program = Program::parse("validation", "").unwrap();
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            source_bytes: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    assert!(evaluate_program_detailed(&program, &mut context).is_ok());
}

#[test]
fn pair_and_single_statement_errors_use_the_same_context_admission() {
    use botwork::core::{
        eval::{botwork_detailed, execute_statement_detailed},
        grammar::{BWParser, Rule},
    };
    use pest::Parser;
    let limits = RunLimits {
        diagnostics: DiagnosticLimits {
            diagnostics: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    };
    let mut context = Context::with_limits(limits.clone()).unwrap();
    let pair = BWParser::parse(Rule::botwork, "Return |1|")
        .unwrap()
        .next()
        .unwrap();
    let error = botwork_detailed(pair, &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::InvalidControl);
    let program = Program::parse("statement", "|x| = |missing|").unwrap();
    let mut context = Context::with_limits(limits).unwrap();
    let error = execute_statement_detailed(&program.statements[0], &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
}

#[test]
fn cli_default_diagnostic_growth_limit_stops_cause_amplification_and_recovers() {
    let harness = cli_harness::Harness::new();
    let source = "Boom |n| { If |n > 0| { Try { Boom |n - 1| } Catch { Boom |n - 1| } } Else { Missing } }\nTry { Boom |10| } Catch { Log |\"unexpected\"| }";
    let result = harness
        .run("diagnostic-growth", source, Duration::from_secs(10))
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(error.contains("BW8001"), "{error}");
    assert!(error.contains("diagnostic call frames"), "{error}");
    assert!(error.contains("BW2002"), "{error}");
    assert!(error.len() < 4096);
    let result = harness
        .run("diagnostic-recovered", "Log |7|", Duration::from_secs(5))
        .unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, b"7\n");
}
