#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    run::{Engine, RunLimits, RunOptions, RunOutcome, TemporaryLimits},
};
use cli_harness::Harness;
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

fn limits(values: usize, nodes: usize, payload_bytes: usize) -> RunLimits {
    RunLimits {
        temporaries: TemporaryLimits {
            values,
            nodes,
            payload_bytes,
        },
        ..RunLimits::default()
    }
}
fn context(values: usize, nodes: usize, payload_bytes: usize) -> Context {
    Context::with_limits(limits(values, nodes, payload_bytes)).unwrap()
}
fn evaluate(source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(&Program::parse("temporary", source).unwrap(), context)
}
fn run(
    source: &str,
    values: usize,
    nodes: usize,
    payload_bytes: usize,
) -> botwork::core::run::RunResult {
    Engine::default().run_source(
        "temporary",
        source,
        RunOptions {
            limits: limits(values, nodes, payload_bytes),
            ..RunOptions::default()
        },
    )
}

#[test]
fn scalar_replacements_release_previous_statement_results_before_next_evaluation() {
    let mut context = context(1, 1, 4);
    for _ in 0..1000 {
        assert_eq!(
            evaluate("|x| = |1|\n|x| = |2|", &mut context)
                .unwrap()
                .to_string(),
            "2"
        );
    }
}

#[test]
fn empty_results_and_zero_budgets_have_explicit_boundaries() {
    assert_eq!(run("", 1, 1, 0).outcome(), RunOutcome::Succeeded);
    assert_eq!(run("", 0, 0, 0).outcome(), RunOutcome::LimitExceeded);
    assert_eq!(
        run("|x| = |true|", 1, 1, 1).outcome(),
        RunOutcome::Succeeded
    );
    let run = run("|x| = |true|", 1, 1, 0);
    assert!(run
        .result
        .unwrap_err()
        .to_string()
        .contains("temporary value payload bytes"));
    assert!(run.variables.is_empty());
}

#[test]
fn arithmetic_and_concatenation_require_operand_output_overlap() {
    for (source, nodes, bytes) in [
        ("|x| = |1+2|", 3, 12),
        ("|x| = |\"ab\"+\"cd\"|", 3, 8),
        ("|x| = |[1]+[2]|", 7, 16),
    ] {
        let accepted = run(source, 3, nodes, bytes);
        assert_eq!(
            accepted.outcome(),
            RunOutcome::Succeeded,
            "{source}: {:?}",
            accepted.result
        );
        for (values, nodes, bytes, resource) in [
            (2, nodes, bytes, "temporary values"),
            (3, nodes - 1, bytes, "temporary value nodes"),
            (3, nodes, bytes - 1, "temporary value payload bytes"),
        ] {
            let rejected = run(source, values, nodes, bytes);
            assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
            assert!(rejected.result.unwrap_err().to_string().contains(resource));
        }
    }
}

#[test]
fn unary_and_short_circuiting_release_or_reuse_their_owned_inputs() {
    assert_eq!(
        run("|x| = |!false|", 2, 2, 2).outcome(),
        RunOutcome::Succeeded
    );
    assert_eq!(run("|x| = |-7|", 1, 1, 4).outcome(), RunOutcome::Succeeded);
    assert_eq!(
        run("|x| = |false and missing|", 1, 1, 1).outcome(),
        RunOutcome::Succeeded
    );
    assert_eq!(
        run("|x| = |true or missing|", 1, 1, 1).outcome(),
        RunOutcome::Succeeded
    );
    assert_eq!(
        run("|x| = |!false|", 1, 1, 1).outcome(),
        RunOutcome::LimitExceeded
    );
}

#[test]
fn nested_containers_merge_child_ownership_and_release_between_calls() {
    let mut context = context(3, 5, 8);
    for _ in 0..1000 {
        assert_eq!(
            evaluate("|x| = |[[1],[2]]|", &mut context)
                .unwrap()
                .to_string(),
            "[[1], [2]]"
        );
    }
}

#[test]
fn duplicate_map_keys_require_replacement_overlap_and_reuse_the_existing_key() {
    let source = "|x| = |{\"a\":\"x\",\"a\":\"yz\"}|";
    assert_eq!(run(source, 2, 3, 4).outcome(), RunOutcome::Succeeded);
    let rejected = run(source, 2, 3, 3);
    assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
    assert!(rejected.variables.is_empty());
    let mut context = context(2, 3, 4);
    for _ in 0..1000 {
        evaluate(source, &mut context).unwrap();
    }
}

#[test]
fn maps_accept_exact_temporary_node_budgets() {
    for (source, nodes, bytes, expected) in [
        ("|x| = |{a: 7}|", 2, 5, r#"{"a": 7}"#),
        ("|x| = |{a: 7, b: 8}|", 3, 10, r#"{"a": 7, "b": 8}"#),
    ] {
        let accepted = run(source, 2, nodes, bytes);
        assert_eq!(
            accepted.outcome(),
            RunOutcome::Succeeded,
            "{:?}",
            accepted.result
        );
        assert_eq!(accepted.variables["x"].to_string(), expected);
        let rejected = run(source, 2, nodes - 1, bytes);
        assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
        assert!(!rejected.variables.contains_key("x"));
    }
}

#[test]
fn static_container_width_and_key_bytes_fail_before_child_effects() {
    for (source, nodes, bytes) in [
        ("|x| = |[@{ Touch }, @{ Touch }]|", 2, usize::MAX),
        ("|x| = |{\"large\":@{ Touch }}|", usize::MAX, 4),
    ] {
        let effects = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&effects);
        let mut context = context(10, nodes, bytes);
        context
            .register_native("Touch", move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(Literal::Int(7))
            })
            .unwrap();
        assert_eq!(
            evaluate(source, &mut context).unwrap_err().code(),
            DiagnosticCode::ResourceLimit
        );
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn argument_accumulation_stops_before_later_effects_or_callback_entry() {
    let effects = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&effects);
    let mut context = context(4, 4, 8);
    context
        .register_native("Touch", move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(7))
        })
        .unwrap();
    context
        .register_native("Keep |a| |b| |c| |d|", |_| panic!("callback must not run"))
        .unwrap();
    let error = evaluate("Keep |1| |2| |@{ Touch }| |@{ Touch }|", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    // Native results are admitted after host construction; the third required argument ran once.
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert!(context.checkpoint().is_err());
}

#[test]
fn a_rejected_argument_copy_skips_later_arguments_and_preserves_previous_destination() {
    let effects = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&effects);
    let mut engine = Engine::default();
    engine
        .register_native("Touch", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(7))
        })
        .unwrap();
    engine
        .register_native("Keep |a| |b| |c|", |_, _| panic!("callback must not run"))
        .unwrap();
    let run = engine.run_source(
        "arguments",
        "|x| = |9|\n|x| = Keep |1| |2| |@{ Touch }|",
        RunOptions {
            limits: limits(1, 1, 4),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(run.variables["x"].to_string(), "9");
}

#[test]
fn custom_parameters_transfer_to_storage_and_returns_keep_temporary_ownership() {
    let mut context = context(2, 2, 8);
    evaluate("Read |x| { Return |x| }", &mut context).unwrap();
    for _ in 0..1000 {
        assert_eq!(evaluate("Read |7|", &mut context).unwrap().to_string(), "7");
    }
    evaluate("Empty { Return }", &mut context).unwrap();
    for _ in 0..1000 {
        assert!(matches!(
            evaluate("Empty", &mut context).unwrap(),
            Literal::None
        ));
    }
}

#[test]
fn borrowed_access_copies_only_selected_data_and_temporary_bases_stay_charged() {
    let good = run("|data| = |[\"abcd\"]|\n|selected| = |data[0]|", 2, 2, 8);
    assert_eq!(good.outcome(), RunOutcome::Succeeded, "{:?}", good.result);
    let rejected = run("|x| = |[\"abcd\"][0]|", 3, 4, 7);
    assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
    assert!(rejected
        .result
        .unwrap_err()
        .to_string()
        .contains("temporary value payload bytes"));
}

#[test]
fn native_failures_release_held_arguments() {
    let mut context = context(2, 2, 8);
    context
        .register_native("Fail |value|", |_| {
            Err(botwork::core::grammar::BWErr::NativeError("failed".into()))
        })
        .unwrap();
    for _ in 0..100 {
        assert!(evaluate("Fail |7|", &mut context).is_err());
        context.checkpoint().unwrap();
        assert_eq!(
            evaluate("|x| = |7|", &mut context).unwrap().to_string(),
            "7"
        );
    }
}

#[test]
fn stopped_calls_preserve_iterator_bindings_and_bypass_catch() {
    let report = run(
        "|item| = |9|\nTry { For |item| In |[1]| { |x| = |1+2| } } Catch { |caught| = |true| }",
        2,
        4,
        12,
    );
    assert_eq!(report.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(report.variables["item"].to_string(), "9");
    assert!(!report.variables.contains_key("caught"));
    assert!(!report.variables.contains_key("x"));
}

#[test]
fn imports_share_temporary_limits_and_failed_invocation_does_not_publish_destination() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Read |x| { Return |x+x| }",
    )
    .unwrap();
    let program = Program::parse(
        harness.workspace.join("main.botwork").to_str().unwrap(),
        "Import |\"module.botwork\"| As |m|\n|answer| = m::Read |7|",
    )
    .unwrap();
    let engine = Engine::default();
    let good = engine.run_program(
        &program,
        RunOptions {
            limits: limits(3, 3, 12),
            ..RunOptions::default()
        },
    );
    assert_eq!(good.outcome(), RunOutcome::Succeeded, "{:?}", good.result);
    let bad = engine.run_program(
        &program,
        RunOptions {
            limits: limits(2, 2, 8),
            ..RunOptions::default()
        },
    );
    assert_eq!(bad.outcome(), RunOutcome::LimitExceeded);
    assert!(!bad.variables.contains_key("answer"));
    assert!(!bad.result.unwrap_err().related.is_empty());
}

#[test]
fn cli_large_argument_lists_fail_before_copying_beyond_default_temporary_payload() {
    let harness = Harness::new();
    let parameters = (0..65).map(|i| format!(" |p{i}|")).collect::<String>();
    let arguments = " |data|".repeat(65);
    let source = format!(
        "|data| = |\"{}\"|\nKeep{parameters} {{}}\nKeep{arguments}\nLog |\"unreachable\"|",
        "x".repeat(512 * 1024)
    );
    let output = harness
        .run("temporaries", &source, Duration::from_secs(10))
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("BW8001") && error.contains("temporary value payload bytes"),
        "{}",
        &error[..error.len().min(512)]
    );
    assert!(harness
        .run("recovery", "Log |7|", Duration::from_secs(5))
        .unwrap()
        .status
        .success());
}

#[test]
fn wrong_native_returns_drop_arguments_without_stopping_context() {
    use botwork::core::signature::{StatementSignature, ValueKind};
    let mut context = context(2, 2, 8);
    context
        .register_native_with_signature(
            StatementSignature::native("Wrong |x|")
                .unwrap()
                .returns(ValueKind::Int),
            |_| Ok(Literal::String("bad".into())),
        )
        .unwrap();
    for _ in 0..100 {
        assert!(evaluate("Wrong |7|", &mut context).is_err());
        context.checkpoint().unwrap();
        evaluate("|x| = |7|", &mut context).unwrap();
    }
}

#[test]
fn cancellation_releases_held_native_values_and_fresh_runs_remain_usable() {
    let mut engine = Engine::default();
    engine
        .register_native("Cancel |x|", |_, env| {
            env.control().cancel();
            Ok(Literal::String("discarded".into()))
        })
        .unwrap();
    let run = engine.run_source(
        "cancel",
        "|before| = |7|\nCancel |before|",
        RunOptions {
            limits: limits(1, 1, 4),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert_eq!(run.variables["before"].to_string(), "7");
    assert!(run.snapshot_error.is_none());
    assert_eq!(
        engine
            .run_source(
                "fresh",
                "|x| = |7|",
                RunOptions {
                    limits: limits(1, 1, 4),
                    ..RunOptions::default()
                }
            )
            .outcome(),
        RunOutcome::Succeeded
    );
}

#[test]
fn builtin_log_admits_its_copy_before_writing_output() {
    let rejected = run("Log |7|", 1, 1, 4);
    assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
    assert!(rejected
        .result
        .unwrap_err()
        .to_string()
        .contains("temporary values"));
}

#[test]
fn active_temporaries_are_shared_across_clones_with_independent_stop_latches() {
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let waiting = Arc::clone(&barrier);
    let mut first = context(2, 2, 8);
    first
        .register_native("Hold |x|", move |_| {
            waiting.wait();
            waiting.wait();
            Ok(Literal::None)
        })
        .unwrap();
    let mut second = first.clone();
    let (result, first) = std::thread::scope(|scope| {
        let pending = scope.spawn(move || {
            let result = evaluate("Hold |7|", &mut first);
            (result, first)
        });
        barrier.wait();
        let failure = evaluate("|x| = |1+2|", &mut second);
        barrier.wait();
        assert!(failure
            .unwrap_err()
            .to_string()
            .contains("temporary values"));
        pending.join().unwrap()
    });
    result.unwrap();
    first.checkpoint().unwrap();
    assert!(second.checkpoint().is_err());
}

#[test]
fn public_context_returns_transfer_to_host_ownership() {
    let mut context = context(1, 1, 7);
    let mut retained = Vec::new();
    for _ in 0..100 {
        retained.push(evaluate("|x| = |\"payload\"|", &mut context).unwrap());
    }
    assert_eq!(retained.len(), 100);
    context.checkpoint().unwrap();
}

#[test]
fn handler_value_admission_preserves_original_cause_and_previous_binding() {
    let rejected = run("|error| = |7|\nTry { Missing } Catch |error| {}", 2, 100, 4);
    assert_eq!(rejected.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(rejected.variables["error"].to_string(), "7");
    let diagnostic = rejected.result.unwrap_err();
    assert!(diagnostic
        .to_string()
        .contains("temporary value payload bytes"));
    assert_eq!(
        diagnostic.causes.first().unwrap().code(),
        DiagnosticCode::UndefinedStatement
    );
}

#[test]
fn required_argument_slots_are_admitted_before_any_argument_effects() {
    for (values, nodes) in [(1, 2), (2, 1)] {
        let effects = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&effects);
        let mut context = context(values, nodes, 100);
        context
            .register_native("Touch", move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(Literal::None)
            })
            .unwrap();
        context
            .register_native("Pair |a| |b|", |_| panic!("callback must not run"))
            .unwrap();
        assert_eq!(
            evaluate("Pair |@{ Touch }| |@{ Touch }|", &mut context)
                .unwrap_err()
                .code(),
            DiagnosticCode::ResourceLimit
        );
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
}
