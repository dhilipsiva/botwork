#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticValueLimits},
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
    run::{Engine, RunLimits, RunOptions, RunOutcome, TemporaryLimits},
    value_limits::ValueLimits,
};
use std::{collections::BTreeMap, sync::Arc};

const SOURCE: &str = "Try { |x| = |missing| } Catch |error| { |handled| = |true| }";

fn run(limits: RunLimits) -> botwork::core::run::RunResult {
    Engine::default().run_source(
        "conversion",
        SOURCE,
        RunOptions {
            variables: BTreeMap::from([("error".into(), Literal::Int(7))]),
            limits,
            ..RunOptions::default()
        },
    )
}

fn original() -> Diagnostic {
    let mut result = run(RunLimits {
        diagnostic_values: DiagnosticValueLimits {
            values: ValueLimits {
                nodes: 0,
                ..ValueLimits::default()
            },
            ..DiagnosticValueLimits::default()
        },
        ..RunLimits::default()
    })
    .result
    .unwrap_err();
    result.causes.pop().unwrap()
}

#[test]
fn exact_conversion_nodes_payload_depth_and_position_work_succeed() {
    let original = original();
    let size = original
        .value_size_with_limits(&DiagnosticValueLimits::default())
        .unwrap();
    let span = original.span.as_ref().unwrap();
    let result = run(RunLimits {
        diagnostic_values: DiagnosticValueLimits {
            values: ValueLimits {
                nodes: size.nodes,
                depth: size.depth,
                payload_bytes: size.payload_bytes,
                ..ValueLimits::default()
            },
            position_bytes: 6 * (span.start() + span.end()),
        },
        ..RunLimits::default()
    });
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.variables["error"].to_string(), "7");
    assert_eq!(result.variables["handled"].to_string(), "true");
}

#[test]
fn rejection_preserves_cause_binding_and_completed_effects_before_handler_entry() {
    let size = original()
        .value_size_with_limits(&DiagnosticValueLimits::default())
        .unwrap();
    for limits in [
        DiagnosticValueLimits {
            values: ValueLimits {
                nodes: size.nodes - 1,
                ..ValueLimits::default()
            },
            ..DiagnosticValueLimits::default()
        },
        DiagnosticValueLimits {
            values: ValueLimits {
                depth: size.depth - 1,
                ..ValueLimits::default()
            },
            ..DiagnosticValueLimits::default()
        },
        DiagnosticValueLimits {
            values: ValueLimits {
                payload_bytes: size.payload_bytes - 1,
                ..ValueLimits::default()
            },
            ..DiagnosticValueLimits::default()
        },
        DiagnosticValueLimits {
            position_bytes: 0,
            ..DiagnosticValueLimits::default()
        },
    ] {
        let result = run(RunLimits {
            diagnostic_values: limits,
            ..RunLimits::default()
        });
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert_eq!(result.variables["error"].to_string(), "7");
        assert!(!result.variables.contains_key("handled"));
        let error = result.result.unwrap_err();
        assert_eq!(error.span.unwrap().text(), "error");
        assert_eq!(error.causes.len(), 1);
        assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
        assert_eq!(error.causes[0].span.as_ref().unwrap().text(), "missing");
    }
}

#[test]
fn temporary_storage_is_reserved_for_the_complete_converted_value() {
    let size = original()
        .value_size_with_limits(&DiagnosticValueLimits::default())
        .unwrap();
    for accepted in [true, false] {
        let result = run(RunLimits {
            temporaries: TemporaryLimits {
                values: 1,
                nodes: size.nodes,
                payload_bytes: size.payload_bytes - usize::from(!accepted),
            },
            ..RunLimits::default()
        });
        assert_eq!(
            result.outcome(),
            if accepted {
                RunOutcome::Succeeded
            } else {
                RunOutcome::LimitExceeded
            }
        );
        if !accepted {
            let error = result.result.unwrap_err();
            assert!(matches!(
                error.error.as_ref(),
                BWErr::ResourceLimit {
                    resource: "temporary value payload bytes",
                    ..
                }
            ));
            assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
        }
    }
}

#[test]
fn ordinary_value_limits_still_apply_to_catch_metadata() {
    let size = original()
        .value_size_with_limits(&DiagnosticValueLimits::default())
        .unwrap();
    let result = run(RunLimits {
        values: ValueLimits {
            nodes: size.nodes - 1,
            ..ValueLimits::default()
        },
        ..RunLimits::default()
    });
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(matches!(
        result.result.unwrap_err().error.as_ref(),
        BWErr::ResourceLimit {
            resource: "value nodes",
            ..
        }
    ));
}

#[test]
fn binding_free_handlers_and_rethrow_need_no_metadata_conversion() {
    let limits = RunLimits {
        diagnostic_values: DiagnosticValueLimits {
            values: ValueLimits {
                nodes: 0,
                ..ValueLimits::default()
            },
            position_bytes: 0,
        },
        ..RunLimits::default()
    };
    let engine = Engine::default();
    let success = engine.run_source(
        "plain",
        "Try { Unknown } Catch {}",
        RunOptions {
            limits: limits.clone(),
            ..RunOptions::default()
        },
    );
    assert_eq!(success.outcome(), RunOutcome::Succeeded);
    let failure = engine.run_source(
        "plain",
        "Try { Unknown } Catch { Rethrow }",
        RunOptions {
            limits,
            ..RunOptions::default()
        },
    );
    let error = failure.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedStatement);
    assert!(error.causes.is_empty());
}

#[test]
fn resource_stop_latches_requesting_context_and_clone_failure_stays_independent() {
    let program = Program::parse("conversion", SOURCE).unwrap();
    let mut context = Context::with_limits(RunLimits {
        diagnostic_values: DiagnosticValueLimits {
            position_bytes: 0,
            ..DiagnosticValueLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let mut clone = context.clone();
    let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    let empty = Program::parse("empty", "").unwrap();
    assert_eq!(
        evaluate_program_detailed(&empty, &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    assert!(evaluate_program_detailed(&empty, &mut clone).is_ok());
}

#[test]
fn checked_host_api_borrows_original_and_legacy_conversion_remains_complete() {
    let diagnostic = original();
    let identity = diagnostic.error.clone();
    let limits = DiagnosticValueLimits {
        position_bytes: 0,
        ..DiagnosticValueLimits::default()
    };
    assert_eq!(
        diagnostic.to_value_with_limits(&limits).unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
    assert!(Arc::ptr_eq(&identity, &diagnostic.error));
    let Literal::Map(value) = diagnostic.to_value() else {
        panic!("map")
    };
    assert_eq!(value["code"].to_string(), "BW2001");
    assert_eq!(value.len(), 8);
    assert!(diagnostic.causes.is_empty());
}

#[test]
fn configurable_string_limits_require_both_conversion_and_ordinary_value_budgets() {
    let mut engine = Engine::default();
    engine
        .register_native("Fail", |_, _| Err(BWErr::NativeError("é".repeat(600_000))))
        .unwrap();
    for raised in [false, true] {
        let values = ValueLimits {
            string_bytes: 2 * 1024 * 1024,
            ..ValueLimits::default()
        };
        let result = engine.run_source(
            "native",
            "Try { Fail } Catch |error| {}",
            RunOptions {
                limits: RunLimits {
                    values: values.clone(),
                    diagnostic_values: DiagnosticValueLimits {
                        values: if raised {
                            values
                        } else {
                            ValueLimits::default()
                        },
                        ..DiagnosticValueLimits::default()
                    },
                    ..RunLimits::default()
                },
                ..RunOptions::default()
            },
        );
        assert_eq!(
            result.outcome(),
            if raised {
                RunOutcome::Succeeded
            } else {
                RunOutcome::LimitExceeded
            }
        );
        if !raised {
            assert_eq!(
                result.result.unwrap_err().causes[0].code(),
                DiagnosticCode::Native
            );
        }
    }
}

#[test]
fn invalid_conversion_depth_configuration_rejects_before_native_effects() {
    let result = Engine::default().run_source(
        "config",
        "Unknown",
        RunOptions {
            limits: RunLimits {
                diagnostic_values: DiagnosticValueLimits {
                    values: ValueLimits {
                        depth: 65,
                        ..ValueLimits::default()
                    },
                    ..DiagnosticValueLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(
        result.result.unwrap_err().code(),
        DiagnosticCode::RunConfiguration
    );
}

#[test]
fn imported_catch_rejection_shares_stop_and_preserves_import_context() {
    let project = cli_harness::Harness::new();
    std::fs::write(project.workspace.join("module.botwork"), SOURCE).unwrap();
    let run = Engine::default().run_source(
        "entry",
        "Try { Import |\"module.botwork\"| As |lib| } Catch { |unexpected| = |true| }",
        RunOptions {
            working_directory: Some(project.workspace.clone()),
            limits: RunLimits {
                diagnostic_values: DiagnosticValueLimits {
                    position_bytes: 0,
                    ..DiagnosticValueLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run.variables.is_empty());
    let error = run.result.unwrap_err();
    assert!(error
        .span
        .as_ref()
        .unwrap()
        .source()
        .name()
        .ends_with("module.botwork"));
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
    assert!(!error.related.is_empty());
}

#[test]
fn default_cli_position_budget_rejects_repeated_distant_frames_and_recovers() {
    let harness = cli_harness::Harness::new();
    let source = format!("# {}\nFail |n| {{ If |n > 0| {{ Fail |n - 1| }} Else {{ Unknown }} }}\nTry {{ Fail |12| }} Catch |error| {{ Log |\"unreachable\"| }}", "x".repeat(512 * 1024));
    let result = harness
        .run(
            "diagnostic-position",
            &source,
            std::time::Duration::from_secs(10),
        )
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(error.contains("BW8001"));
    assert!(error.contains("diagnostic position bytes"));
    assert!(error.contains("BW2002"));
    let result = harness
        .run(
            "diagnostic-recovery",
            "Log |7|",
            std::time::Duration::from_secs(5),
        )
        .unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, b"7\n");
    assert!(result.stderr.is_empty());
}
