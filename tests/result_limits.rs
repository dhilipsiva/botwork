use botwork::core::{
    diagnostic::DiagnosticCode,
    grammar::Literal,
    operation::OperationControl,
    run::{Engine, ResultLimits, RunLimits, RunOptions, RunOutcome},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

fn options(results: ResultLimits) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            results,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

#[test]
fn root_and_terminal_occurrences_use_exact_nodes_utf8_names_and_map_payload() {
    let exact = ResultLimits {
        values: 2,
        nodes: 8,
        name_bytes: 2,
        payload_bytes: 14,
    };
    let source = "|é| = |[1, {\"κ\": true}]|";
    let run = Engine::default().run_source("result", source, options(exact.clone()));
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert!(run.snapshot_error.is_none());
    assert_eq!(
        run.result.unwrap().to_string(),
        run.variables["é"].to_string()
    );
    for (limits, resource) in [
        (
            ResultLimits {
                values: 1,
                ..exact.clone()
            },
            "result values",
        ),
        (
            ResultLimits {
                nodes: 7,
                ..exact.clone()
            },
            "result nodes",
        ),
        (
            ResultLimits {
                name_bytes: 1,
                ..exact.clone()
            },
            "result name bytes",
        ),
        (
            ResultLimits {
                payload_bytes: 13,
                ..exact
            },
            "result payload bytes",
        ),
    ] {
        let run = Engine::default().run_source("result", source, options(limits));
        assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
        assert!(run.variables.is_empty());
        assert!(run.result.unwrap_err().to_string().contains(resource));
        let error = run.snapshot_error.unwrap();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert!(error.to_string().contains(resource));
        assert!(error.span.is_none());
    }
}

#[test]
fn empty_success_has_one_none_value_and_failure_has_no_terminal_value() {
    let zero = ResultLimits {
        values: 0,
        nodes: 0,
        name_bytes: 0,
        payload_bytes: 0,
    };
    let run = Engine::default().run_source("empty", "", options(zero.clone()));
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run.snapshot_error.is_some());
    let run = Engine::default().run_source("error", "Missing", options(zero.clone()));
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::UndefinedStatement
    );
    assert!(run.snapshot_error.is_none());
    let run = Engine::default().run_source(
        "empty",
        "",
        options(ResultLimits {
            values: 1,
            nodes: 1,
            ..zero
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert!(matches!(run.result, Ok(Literal::None)));
    assert!(run.snapshot_error.is_none());
}

#[test]
fn ordinary_failure_preserves_original_diagnostic_and_reports_omitted_exports() {
    let run = Engine::default().run_source(
        "failure",
        "|completed| = |7|\nFail { Return |missing| }\nFail",
        options(ResultLimits {
            values: 0,
            ..ResultLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Failed);
    assert!(run.variables.is_empty());
    let original = run.result.unwrap_err();
    assert_eq!(original.code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(original.span.unwrap().text(), "missing");
    assert_eq!(original.call_stack.len(), 1);
    assert_eq!(
        run.snapshot_error.unwrap().code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn cancellation_keeps_primary_outcome_and_independent_snapshot_error() {
    let mut engine = Engine::default();
    engine
        .register_native("Cancel", |_, environment| {
            environment.control().cancel();
            Ok(Literal::None)
        })
        .unwrap();
    let run = engine.run_source(
        "cancel",
        "|completed| = |7|\nCancel",
        options(ResultLimits {
            values: 0,
            ..ResultLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert_eq!(run.result.unwrap_err().code(), DiagnosticCode::Cancelled);
    assert!(run.snapshot_error.is_some());
    assert!(run.variables.is_empty());
    let run = engine.run_source("cancel", "|completed| = |7|\nCancel", RunOptions::default());
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert!(run.snapshot_error.is_none());
    assert_eq!(run.variables["completed"].to_string(), "7");
}

#[test]
fn execution_limit_is_preserved_when_export_also_exceeds_its_limit() {
    let mut options = options(ResultLimits {
        values: 0,
        ..ResultLimits::default()
    });
    options.limits.steps = 2;
    let run = Engine::default().run_source("steps", "|x| = |7|\n|later| = |8|", options);
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run
        .result
        .unwrap_err()
        .to_string()
        .contains("evaluation steps"));
    assert!(run
        .snapshot_error
        .unwrap()
        .to_string()
        .contains("result values"));
    assert_eq!(run.steps, 2);
    assert!(run.variables.is_empty());
}

#[test]
fn export_failure_occurs_after_required_effects_and_cannot_enter_catch() {
    let effects = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&effects);
    let mut engine = Engine::default();
    engine
        .register_native("Touch", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(7))
        })
        .unwrap();
    let run = engine.run_source(
        "effects",
        "Try { |x| = Touch } Catch { Touch }\nTouch",
        options(ResultLimits {
            values: 0,
            ..ResultLimits::default()
        }),
    );
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run.snapshot_error.is_some());
}

#[test]
fn inputs_and_all_root_bindings_export_atomically_without_private_locals() {
    let mut options = options(ResultLimits {
        values: 3,
        nodes: 3,
        name_bytes: 3,
        payload_bytes: 8,
    });
    options.variables = BTreeMap::from([
        ("a".into(), Literal::Bool(true)),
        ("bb".into(), Literal::Int(7)),
    ]);
    let run =
        Engine::default().run_source("locals", "Work { |private| = |[1,2,3]| }\nWork", options);
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert!(run.snapshot_error.is_none());
    assert_eq!(run.variables.len(), 2);
    assert!(!run.variables.contains_key("private"));
    assert!(matches!(run.result, Ok(Literal::None)));
}

#[test]
fn names_and_nested_keys_have_separate_budget_domains() {
    let mut options = options(ResultLimits {
        values: 2,
        nodes: 4,
        name_bytes: 4,
        payload_bytes: 7,
    });
    options.variables.insert(
        "café".into(),
        Literal::Map(HashMap::from([(
            "clé".into(),
            Literal::String("été".into()),
        )])),
    );
    let run = Engine::default().run_source("names", "", options.clone());
    assert!(run
        .snapshot_error
        .unwrap()
        .to_string()
        .contains("result name bytes"));
    options.limits.results.name_bytes = 5;
    let run = Engine::default().run_source("names", "", options.clone());
    assert!(run
        .snapshot_error
        .unwrap()
        .to_string()
        .contains("result payload bytes"));
    options.limits.results.payload_bytes = 9;
    let run = Engine::default().run_source("names", "", options);
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert!(run.snapshot_error.is_none());
}

#[test]
fn terminal_value_is_admitted_before_root_value_measurement() {
    let mut options = options(ResultLimits {
        values: 3,
        nodes: 3,
        name_bytes: 2,
        payload_bytes: 1,
    });
    options
        .variables
        .insert("x".into(), Literal::Array(vec![Literal::None; 3]));
    let mut engine = Engine::default();
    engine
        .register_native("Text", |_, _| Ok(Literal::String("large".into())))
        .unwrap();
    let run = engine.run_source("priority", "Text", options);
    assert!(run
        .snapshot_error
        .unwrap()
        .to_string()
        .contains("result payload bytes"));
}

#[test]
fn independent_runs_can_export_after_a_previous_export_failure() {
    let engine = Engine::default();
    let failed = engine.run_source(
        "bad",
        "|x| = |7|",
        options(ResultLimits {
            values: 1,
            ..ResultLimits::default()
        }),
    );
    assert_eq!(failed.outcome(), RunOutcome::LimitExceeded);
    let reports = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| engine.run_source("ok", "|x| = |7|", RunOptions::default())))
            .collect();
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .collect::<Vec<_>>()
    });
    for report in reports {
        assert_eq!(report.outcome(), RunOutcome::Succeeded);
        assert!(report.snapshot_error.is_none());
        assert_eq!(report.variables["x"].to_string(), "7");
    }
}

#[test]
fn default_export_caps_apply_when_storage_limits_are_raised_explicitly() {
    let mut options = RunOptions::default();
    options.limits.retained_names.total_bytes = 5 * 1024 * 1024;
    for i in 0..65 {
        let name = format!("v{i}_{}", "x".repeat(65_500));
        options.variables.insert(name, Literal::None);
    }
    let run = Engine::default().run_source("large-names", "", options.clone());
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run
        .snapshot_error
        .unwrap()
        .to_string()
        .contains("result name bytes"));
    options.limits.results.name_bytes = 5 * 1024 * 1024;
    let run = Engine::default().run_source("large-names", "", options);
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert_eq!(run.variables.len(), 65);
}

#[test]
fn early_cancel_and_configuration_failure_do_not_create_snapshot_errors() {
    let control = OperationControl::default();
    control.cancel();
    let mut options = options(ResultLimits {
        values: 0,
        nodes: 0,
        name_bytes: 0,
        payload_bytes: 0,
    });
    options
        .variables
        .insert("ignored".into(), Literal::String("data".into()));
    options.control = control;
    let run = Engine::default().run_source("cancel", "", options);
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert!(run.snapshot_error.is_none());
    assert!(run.variables.is_empty());
    let run = Engine::default().run_source(
        "timeout",
        "",
        RunOptions {
            timeout: Some(Duration::ZERO),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::TimedOut);
    assert!(run.snapshot_error.is_none());
}
