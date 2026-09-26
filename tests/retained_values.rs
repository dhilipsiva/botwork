#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    run::{Engine, RetainedValueLimits, RunLimits, RunOptions, RunOutcome},
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

fn options(retained_values: RetainedValueLimits) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            retained_values,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

fn context(values: usize) -> Context {
    Context::with_limits(
        options(RetainedValueLimits {
            values,
            ..RetainedValueLimits::default()
        })
        .limits,
    )
    .unwrap()
}

fn read(name: &str, context: &mut Context) -> Literal {
    evaluate(
        &format!("Inspect {name} {{ Return |{name}| }}\nInspect {name}"),
        context,
    )
    .unwrap()
}

fn evaluate(source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(
        &Program::parse("retained.botwork", source).unwrap(),
        context,
    )
}

#[test]
fn exact_value_node_and_payload_limits_include_nested_keys_and_unicode() {
    let exact = RetainedValueLimits {
        values: 2,
        nodes: 5,
        payload_bytes: 11,
    };
    // First value: map + array + string + int = 4 nodes, 2+4+4 bytes.
    let source = "|a| = |{\"é\": [\"🙂\", 7]}|\n|b| = |true|";
    let result = Engine::default().run_source("counts", source, options(exact.clone()));
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    for (limits, resource) in [
        (
            RetainedValueLimits {
                values: 1,
                ..exact.clone()
            },
            "retained values",
        ),
        (
            RetainedValueLimits {
                nodes: 4,
                ..exact.clone()
            },
            "retained value nodes",
        ),
        (
            RetainedValueLimits {
                payload_bytes: 10,
                ..exact
            },
            "retained value payload bytes",
        ),
    ] {
        let result = Engine::default().run_source("counts", source, options(limits));
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert!(result.variables.contains_key("a"));
        assert!(!result.variables.contains_key("b"));
        let error = result.result.unwrap_err();
        assert!(error.to_string().contains(resource));
        assert!(error.span.is_some());
    }
}

#[test]
fn zero_limits_allow_no_bindings_and_zero_payload_allows_empty_values() {
    let zero = RetainedValueLimits {
        values: 0,
        nodes: 0,
        payload_bytes: 0,
    };
    for source in ["", "Read { Return |7| }\nRead", "Noop {}\nNoop"] {
        assert_eq!(
            Engine::default()
                .run_source("empty", source, options(zero.clone()))
                .outcome(),
            RunOutcome::Succeeded
        );
    }
    for limits in [
        zero,
        RetainedValueLimits {
            values: 1,
            nodes: 0,
            payload_bytes: 0,
        },
    ] {
        let result = Engine::default().run_source("empty", "|x| = |[]|", options(limits));
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert!(result.variables.is_empty());
    }
    let result = Engine::default().run_source(
        "empty",
        "|x| = |[]|",
        options(RetainedValueLimits {
            values: 1,
            nodes: 1,
            payload_bytes: 0,
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
}

#[test]
fn input_batches_reserve_atomically_before_effects_and_preserve_old_bindings() {
    let mut context = context(2);
    context
        .set_input_variables(BTreeMap::from([("old".into(), Literal::Int(7))]))
        .unwrap();
    let mut observer = context.clone();
    let error = context
        .set_input_variables(BTreeMap::from([
            ("a".into(), Literal::Int(1)),
            ("old".into(), Literal::Int(2)),
        ]))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    // Failed batch releases its partial reservation, and the observer keeps old data.
    assert_eq!(read("old", &mut observer).to_string(), "7");
    evaluate("|new| = |3|", &mut observer).unwrap();
    assert!(context.checkpoint().is_err());
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Mark", move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let mut run = options(RetainedValueLimits {
        values: 1,
        ..RetainedValueLimits::default()
    });
    run.variables = BTreeMap::from([("a".into(), Literal::None), ("b".into(), Literal::None)]);
    let result = engine.run_source("inputs", "Mark", run);
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(result.variables.is_empty());
    assert_eq!(result.steps, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn replacement_reserves_peak_overlap_and_preserves_destination_on_failure() {
    let result = Engine::default().run_source(
        "replace",
        "|x| = |1|\nTry { |x| = |2| } Catch { |caught| = |true| }",
        options(RetainedValueLimits {
            values: 1,
            ..RetainedValueLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(result.variables["x"].to_string(), "1");
    assert!(!result.variables.contains_key("caught"));
    let mut context = context(2);
    evaluate("|x| = |0|", &mut context).unwrap();
    for _ in 0..200 {
        evaluate("|x| = |x+1|", &mut context).unwrap();
    }
    assert_eq!(read("x", &mut context).to_string(), "200");
}

#[test]
fn call_parameters_share_the_root_budget_and_failed_admission_skips_body() {
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
        "calls",
        "Read |a| With |b| { Mark }\n|root| = |7|\nRead |1| With |2|",
        options(RetainedValueLimits {
            values: 2,
            ..RetainedValueLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(result.variables.len(), 1);
}

#[test]
fn invocation_values_release_after_return_fallthrough_and_caught_failure() {
    let mut context = context(2);
    evaluate("Read |arg| { |local| = |arg|\nReturn |local| }\nFail |arg| { |local| = |arg|\nReturn |missing| }\nEmpty |arg| { |local| = |arg| }",&mut context).unwrap();
    for _ in 0..50 {
        evaluate(
            "Read |1|\nEmpty |2|\nTry { Fail |3| } Catch {}",
            &mut context,
        )
        .unwrap();
    }
    evaluate("|a| = |1|\n|b| = |2|", &mut context).unwrap();
}

#[test]
fn for_restores_saved_value_and_releases_iterations_on_all_completions() {
    for body in ["", "Continue", "Break", "|bad| = |missing|"] {
        let mut context = context(3);
        evaluate("|item| = |9|", &mut context).unwrap();
        let mut observer = context.clone();
        let result = evaluate(
            &format!("For |item| In |[1,2,3]| {{ {body} }}"),
            &mut context,
        );
        assert_eq!(result.is_err(), body.contains("missing"));
        assert_eq!(read("item", &mut context).to_string(), "9");
        evaluate("|a| = |1|\n|b| = |2|", &mut observer).unwrap();
    }
    let mut context = context(3);
    evaluate(
        "Loop { |item| = |9|\nFor |item| In |[1,2]| { Return |item| } }\nLoop",
        &mut context,
    )
    .unwrap();
    evaluate("|a| = |1|\n|b| = |2|\n|c| = |3|", &mut context).unwrap();
}

#[test]
fn cli_defaults_stop_accumulating_individually_valid_values() {
    let harness = Harness::new();
    let mut source = format!("|base| = |\"{}\"|\n", "a".repeat(64 * 1024));
    for index in 0..512 {
        source.push_str(&format!("|copy{index}| = |base|\n"));
    }
    source.push_str("Log |\"unreachable\"|");
    let output = harness
        .run("retention", &source, std::time::Duration::from_secs(15))
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("BW8001"), "{stderr}");
    assert!(stderr.contains("retained value payload bytes"), "{stderr}");
    let output = harness
        .run("fresh", "Log |7|", std::time::Duration::from_secs(15))
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"7\n");
}

#[test]
fn for_limit_failure_restores_previous_binding_and_bypasses_catch() {
    let result = Engine::default().run_source(
        "loop",
        "|item| = |9|\nTry { For |item| In |[1,2]| {} } Catch { |caught| = |true| }",
        options(RetainedValueLimits {
            values: 2,
            ..RetainedValueLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(result.variables["item"].to_string(), "9");
    assert_eq!(result.variables.len(), 1);
}

#[test]
fn catch_binding_reserves_before_replacement_and_releases_after_handler() {
    let source = "|error| = |7|\nTry { |bad| = |missing| } Catch |error| {}";
    for maximum in [1, 2] {
        let result = Engine::default().run_source(
            "catch",
            source,
            options(RetainedValueLimits {
                values: maximum,
                ..RetainedValueLimits::default()
            }),
        );
        assert_eq!(result.variables["error"].to_string(), "7");
        if maximum == 1 {
            let error = result.result.unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
        } else {
            assert_eq!(result.outcome(), RunOutcome::Succeeded);
        }
    }
    let mut context = context(2);
    evaluate(source, &mut context).unwrap();
    evaluate("|next| = |8|", &mut context).unwrap();
}

#[test]
fn clones_count_shared_allocations_once_and_release_only_after_last_reference() {
    let mut original = context(2);
    evaluate("|x| = |1|", &mut original).unwrap();
    let mut clone = original.clone();
    evaluate("|x| = |2|", &mut clone).unwrap();
    assert_eq!(read("x", &mut original).to_string(), "1");
    assert_eq!(read("x", &mut clone).to_string(), "2");
    let mut failed = clone.clone();
    assert!(evaluate("|y| = |3|", &mut failed).is_err());
    assert!(clone.checkpoint().is_ok());
    drop(original);
    evaluate("|y| = |3|", &mut clone).unwrap();
    assert!(failed.checkpoint().is_err());
}

#[test]
fn module_globals_and_imported_call_parameters_share_retention() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("state.botwork"),
        "|global| = |7|\nRead |x| { Return |global+x| }",
    )
    .unwrap();
    let source="Import |\"state.botwork\"| As |a|\nImport |\"state.botwork\"| As |b|\na::Read |1|\nb::Read |2|";
    for maximum in [1, 2] {
        let mut run = options(RetainedValueLimits {
            values: maximum,
            ..RetainedValueLimits::default()
        });
        run.working_directory = Some(harness.workspace.clone());
        let result = Engine::default().run_source("main.botwork", source, run);
        assert_eq!(
            result.outcome(),
            if maximum == 1 {
                RunOutcome::LimitExceeded
            } else {
                RunOutcome::Succeeded
            }
        );
        if maximum == 2 {
            assert_eq!(result.result.unwrap().to_string(), "9");
        }
    }
}

#[test]
fn failed_module_initialization_releases_its_values_but_cached_modules_retain_them() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("bad.botwork"),
        "|x| = |1|\n|bad| = |missing|",
    )
    .unwrap();
    fs::write(harness.workspace.join("good.botwork"), "|x| = |1|").unwrap();
    let mut run = options(RetainedValueLimits {
        values: 1,
        ..RetainedValueLimits::default()
    });
    run.working_directory = Some(harness.workspace.clone());
    let source = "Try { Import |\"bad.botwork\"| As |bad| } Catch {}\n|x| = |2|";
    assert_eq!(
        Engine::default()
            .run_source("main.botwork", source, run.clone())
            .outcome(),
        RunOutcome::Succeeded
    );
    let source = "Import |\"good.botwork\"| As |good|\n|x| = |2|";
    let result = Engine::default().run_source("main.botwork", source, run);
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(result.variables.is_empty());
}

#[test]
fn fresh_engine_runs_have_independent_retention_even_while_results_remain_owned() {
    let engine = Engine::default();
    let run = options(RetainedValueLimits {
        values: 1,
        ..RetainedValueLimits::default()
    });
    let first = engine.run_source("first", "|x| = |1|", run.clone());
    let second = engine.run_source("second", "|x| = |2|", run);
    assert_eq!(first.outcome(), RunOutcome::Succeeded);
    assert_eq!(second.outcome(), RunOutcome::Succeeded);
    assert_eq!(first.variables["x"].to_string(), "1");
}

#[test]
fn concurrent_context_clones_share_capacity_without_poisoning_sibling_stops() {
    let original = context(1);
    let gate = Arc::new(std::sync::Barrier::new(8));
    let results = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let mut context = original.clone();
                let gate = gate.clone();
                scope.spawn(move || {
                    gate.wait();
                    let result = evaluate("|x| = |1|", &mut context);
                    gate.wait();
                    (result.is_ok(), context)
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|(success, _)| *success).count(), 1);
    assert!(original.checkpoint().is_ok());
    drop(results);
    let mut available = original;
    evaluate("|x| = |2|", &mut available).unwrap();
}
