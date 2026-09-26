#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    run::{Engine, RetainedNameLimits, RunLimits, RunOptions, RunOutcome},
};
use cli_harness::Harness;
use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Barrier},
};

fn options(retained_names: RetainedNameLimits) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            retained_names,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

fn context(names: usize) -> Context {
    Context::with_limits(
        options(RetainedNameLimits {
            names,
            ..RetainedNameLimits::default()
        })
        .limits,
    )
    .unwrap()
}

fn evaluate(source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(&Program::parse("names", source).unwrap(), context)
}

#[test]
fn exact_name_counts_and_utf8_bytes_preserve_unicode_identity() {
    let source = "|é| = |1|\n|e\u{301}| = |2|";
    let exact = RetainedNameLimits {
        names: 2,
        name_bytes: 3,
        total_bytes: 5,
    };
    let result = Engine::default().run_source("unicode", source, options(exact.clone()));
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.variables["é"].to_string(), "1");
    assert_eq!(result.variables["e\u{301}"].to_string(), "2");
    for (limits, resource) in [
        (
            RetainedNameLimits {
                names: 1,
                ..exact.clone()
            },
            "retained variable names",
        ),
        (
            RetainedNameLimits {
                name_bytes: 2,
                ..exact.clone()
            },
            "variable name bytes",
        ),
        (
            RetainedNameLimits {
                total_bytes: 4,
                ..exact
            },
            "retained variable name bytes",
        ),
    ] {
        let result = Engine::default().run_source("unicode", source, options(limits));
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert_eq!(result.variables.len(), 1);
        assert!(result.result.unwrap_err().to_string().contains(resource));
    }
}

#[test]
fn zero_limits_allow_no_bindings_and_empty_loops() {
    let limits = RetainedNameLimits {
        names: 0,
        name_bytes: 0,
        total_bytes: 0,
    };
    for source in [
        "",
        "For |item| In |[]| {}",
        "Try { |x| = |missing| } Catch {}",
        "Read { Return |7| }\nRead",
    ] {
        assert_eq!(
            Engine::default()
                .run_source("empty", source, options(limits.clone()))
                .outcome(),
            RunOutcome::Succeeded
        );
    }
    let result = Engine::default().run_source("binding", "|x| = |1|", options(limits));
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(result.variables.is_empty());
}

#[test]
fn existing_keys_are_reused_for_assignments_and_atomic_input_replacement() {
    let mut context = Context::with_limits(
        options(RetainedNameLimits {
            names: 1,
            name_bytes: 1,
            total_bytes: 1,
        })
        .limits,
    )
    .unwrap();
    evaluate("|x| = |1|", &mut context).unwrap();
    for _ in 0..100 {
        evaluate("|x| = |x+1|", &mut context).unwrap();
    }
    context
        .set_input_variables(BTreeMap::from([("x".into(), Literal::Int(7))]))
        .unwrap();
    assert_eq!(
        evaluate("Read { Return |x| }\nRead", &mut context)
            .unwrap()
            .to_string(),
        "7"
    );
}

#[test]
fn input_name_failures_preserve_old_bindings_and_release_partial_names() {
    let mut context = context(2);
    context
        .set_input_variables(BTreeMap::from([("old".into(), Literal::Int(7))]))
        .unwrap();
    let mut observer = context.clone();
    let error = context
        .set_input_variables(BTreeMap::from([
            ("a".into(), Literal::None),
            ("b".into(), Literal::None),
        ]))
        .unwrap_err();
    assert!(error.to_string().contains("retained variable names"));
    evaluate("|new| = |old|", &mut observer).unwrap();
    assert!(context.checkpoint().is_err());
    let mut run = options(RetainedNameLimits {
        names: 1,
        ..RetainedNameLimits::default()
    });
    run.variables = BTreeMap::from([("a".into(), Literal::None), ("b".into(), Literal::None)]);
    let result = Engine::default().run_source("inputs", "Log |\"unreachable\"|", run);
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(result.variables.is_empty());
    assert_eq!(result.steps, 0);
}

#[test]
fn oversized_host_names_reject_before_name_parsing_and_drop_deep_values_safely() {
    let mut run = options(RetainedNameLimits {
        name_bytes: 4,
        ..RetainedNameLimits::default()
    });
    let deep = (0..20_000).fold(Literal::None, |value, _| Literal::Array(vec![value]));
    run.variables = BTreeMap::from([("invalid name".repeat(10_000), deep)]);
    let result = Engine::default().run_source("host", "", run);
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(error.to_string().contains("variable name bytes"));
    assert!(error.to_string().len() < 1024);
}

#[test]
fn argument_names_are_new_allocations_and_release_on_invocation_exit() {
    let mut scoped = context(2);
    evaluate("Read |x| { |local| = |x|\nReturn |local| }", &mut scoped).unwrap();
    for _ in 0..100 {
        evaluate("Read |1|", &mut scoped).unwrap();
    }
    evaluate("|x| = |1|\n|other| = |2|", &mut scoped).unwrap();
    let mut context = context(1);
    let error = evaluate("Read |x| { Return |x| }\n|x| = |1|\nRead |2|", &mut context).unwrap_err();
    assert!(error.to_string().contains("retained variable names"));
}

#[test]
fn failed_parameter_name_batch_releases_earlier_arguments_and_skips_body() {
    let mut limited = context(1);
    let mut observer = limited.clone();
    let error = evaluate(
        "Read |first| With |second| { |effect| = |1| }\nRead |1| With |2|",
        &mut limited,
    )
    .unwrap_err();
    assert!(error.to_string().contains("retained variable names"));
    evaluate("|after| = |7|", &mut observer).unwrap();
}

#[test]
fn for_reuses_saved_name_and_restores_it_after_every_completion() {
    for body in ["", "Continue", "Break", "|bad| = |missing|"] {
        let mut context = context(1);
        evaluate("|item| = |9|", &mut context).unwrap();
        let result = evaluate(
            &format!("For |item| In |[1,2,3]| {{ {body} }}"),
            &mut context,
        );
        assert_eq!(result.is_err(), body.contains("missing"));
        assert_eq!(
            evaluate("Read { Return |item| }\nRead", &mut context)
                .unwrap()
                .to_string(),
            "9"
        );
    }
    let mut context = context(1);
    evaluate(
        "For |item| In |[1,2]| { For |item| In |[3]| {} }\n|after| = |7|",
        &mut context,
    )
    .unwrap();
}

#[test]
fn name_limit_failure_unwinds_iterator_storage_and_keeps_original_value() {
    let mut context = context(1);
    evaluate("|item| = |9|", &mut context).unwrap();
    let result = evaluate(
        "Try { For |item| In |[1]| { |new| = |2| } } Catch {}",
        &mut context,
    );
    assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit);
    // Engine snapshots preserve the saved binding even when the Context is stopped.
    let result = Engine::default().run_source(
        "restore",
        "|item| = |9|\nFor |item| In |[1]| { |new| = |2| }",
        options(RetainedNameLimits {
            names: 1,
            ..RetainedNameLimits::default()
        }),
    );
    assert_eq!(result.variables["item"].to_string(), "9");
    assert!(!result.variables.contains_key("new"));
}

#[test]
fn catch_reuses_old_names_and_preserves_original_error_on_admission_failure() {
    let mut context = context(1);
    evaluate(
        "|error| = |9|\nTry { |x| = |missing| } Catch |error| {}",
        &mut context,
    )
    .unwrap();
    assert_eq!(
        evaluate("Read { Return |error| }\nRead", &mut context)
            .unwrap()
            .to_string(),
        "9"
    );
    let result = Engine::default().run_source(
        "catch",
        "Try { |x| = |missing| } Catch |error| {}",
        options(RetainedNameLimits {
            names: 0,
            ..RetainedNameLimits::default()
        }),
    );
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
    assert!(result.variables.is_empty());
}

#[test]
fn cloned_contexts_share_name_allocations_and_keep_failures_independent() {
    let base = context(1);
    let mut first = base.clone();
    evaluate("|x| = |1|", &mut first).unwrap();
    let mut second = first.clone();
    evaluate("|x| = |2|", &mut second).unwrap();
    drop(first);
    let mut failed = base.clone();
    assert!(evaluate("|y| = |3|", &mut failed).is_err());
    assert!(base.checkpoint().is_ok());
    drop(second);
    let mut available = base;
    evaluate("|y| = |3|", &mut available).unwrap();
}

#[test]
fn module_globals_share_names_across_aliases_and_failed_modules_release_them() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("good.botwork"),
        "|global| = |7|\nRead { Return |global| }",
    )
    .unwrap();
    fs::write(
        harness.workspace.join("bad.botwork"),
        "|global| = |7|\n|bad| = |missing|",
    )
    .unwrap();
    let mut run = options(RetainedNameLimits {
        names: 1,
        ..RetainedNameLimits::default()
    });
    run.working_directory = Some(harness.workspace.clone());
    let source =
        "Import |\"good.botwork\"| As |a|\nImport |\"good.botwork\"| As |b|\na::Read\nb::Read";
    assert_eq!(
        Engine::default()
            .run_source("main.botwork", source, run.clone())
            .outcome(),
        RunOutcome::Succeeded
    );
    let source = "Try { Import |\"bad.botwork\"| As |module| } Catch {}\n|after| = |7|";
    assert_eq!(
        Engine::default()
            .run_source("main.botwork", source, run)
            .outcome(),
        RunOutcome::Succeeded
    );
}

#[test]
fn concurrent_distinct_names_share_capacity_without_shared_failure_latches() {
    let base = context(1);
    let barrier = Arc::new(Barrier::new(8));
    let outcomes = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..8)
            .map(|index| {
                let mut context = base.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let result = evaluate(&format!("|name{index}| = |1|"), &mut context);
                    barrier.wait();
                    (result.is_ok(), context)
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|(success, _)| *success).count(), 1);
    assert!(base.checkpoint().is_ok());
    drop(outcomes);
    let mut available = base;
    evaluate("|after| = |7|", &mut available).unwrap();
}

#[test]
fn cli_default_name_length_is_checked_before_publication_and_later_effects() {
    let harness = Harness::new();
    let source = format!("|{}| = |1|\nLog |\"unreachable\"|", "x".repeat(65_537));
    let output = harness
        .run("long-name", &source, std::time::Duration::from_secs(15))
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("BW8001"), "{stderr}");
    assert!(stderr.contains("variable name bytes"), "{stderr}");
    let output = harness
        .run("recovery", "Log |7|", std::time::Duration::from_secs(15))
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"7\n");
}

#[test]
fn explicitly_raised_name_limits_are_local_and_engine_runs_start_fresh() {
    let name = "x".repeat(65_537);
    let mut run = options(RetainedNameLimits {
        names: 1,
        name_bytes: name.len(),
        total_bytes: name.len(),
    });
    run.variables = BTreeMap::from([(name.clone(), Literal::None)]);
    let engine = Engine::default();
    let first = engine.run_source("first", "", run.clone());
    let second = engine.run_source("second", "", run);
    assert_eq!(first.outcome(), RunOutcome::Succeeded);
    assert_eq!(second.outcome(), RunOutcome::Succeeded);
    let result = engine.run_source(
        "defaults",
        "",
        RunOptions {
            variables: BTreeMap::from([(name, Literal::None)]),
            ..RunOptions::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
}

#[test]
fn required_rhs_effects_precede_name_failure_but_handlers_cannot_resume() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Host", move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(1))
        })
        .unwrap();
    let result = engine.run_source(
        "effects",
        "Try { |x| = Host } Catch { Host }\nHost",
        options(RetainedNameLimits {
            names: 0,
            ..RetainedNameLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(result.variables.is_empty());
}
