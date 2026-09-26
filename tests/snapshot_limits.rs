#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome, SnapshotLimits},
};
use cli_harness::Harness;
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

fn limits(entries: usize, path_bytes: usize) -> RunLimits {
    RunLimits {
        snapshots: SnapshotLimits {
            entries,
            path_bytes,
        },
        ..RunLimits::default()
    }
}
fn evaluate(source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(&Program::parse("snapshots", source).unwrap(), context)
}
fn cwd_bytes() -> usize {
    std::env::current_dir().unwrap().as_os_str().len()
}
fn import(harness: &Harness, context: &mut Context) -> DiagnosticResult<Literal> {
    let program = Program::parse(
        harness.workspace.join("main.botwork").to_str().unwrap(),
        "Import |\"module.botwork\"| As |m|",
    )
    .unwrap();
    evaluate_program_detailed(&program, context)
}

#[test]
fn checked_clone_counts_all_root_tables_and_directory_bytes() {
    let mut context = Context::with_limits(limits(2, cwd_bytes())).unwrap();
    evaluate("|x| = |7|\nRead { Return |x| }", &mut context).unwrap();
    let mut copy = context.try_clone().unwrap();
    evaluate("|x| = |9|", &mut copy).unwrap();
    assert_eq!(evaluate("Read", &mut copy).unwrap().to_string(), "9");
    assert_eq!(evaluate("Read", &mut context).unwrap().to_string(), "7");
    assert!(context
        .try_clone()
        .err()
        .unwrap()
        .to_string()
        .contains("snapshot table entries"));
    copy.checkpoint().unwrap();
}

#[test]
fn zero_and_exact_directory_budgets_apply_before_cloning() {
    let context = Context::with_limits(limits(0, cwd_bytes())).unwrap();
    context.try_clone().unwrap();
    assert!(context
        .try_clone()
        .err()
        .unwrap()
        .to_string()
        .contains("snapshot path bytes"));
    let context = Context::with_limits(limits(0, 0)).unwrap();
    assert!(context.try_clone().is_err());
    let mut context = Context::with_limits(limits(0, 0)).unwrap();
    assert_eq!(
        evaluate("|x| = |1|", &mut context).unwrap().to_string(),
        "1"
    );
}

#[test]
fn infallible_clone_is_host_owned_and_preserves_existing_state_and_stops() {
    let mut context = Context::with_limits(limits(0, 0)).unwrap();
    evaluate("Read { Return |7| }", &mut context).unwrap();
    let mut clone = context.clone();
    assert_eq!(evaluate("Read", &mut clone).unwrap().to_string(), "7");
    assert!(clone.try_clone().is_err());
    assert!(clone.clone().checkpoint().is_err());
    assert_eq!(evaluate("Read", &mut context).unwrap().to_string(), "7");
}

#[test]
fn cancellation_precedes_snapshot_admission() {
    let control = OperationControl::default();
    let context = Context::with_control(limits(0, 0), control.clone()).unwrap();
    control.cancel();
    assert_eq!(
        context.try_clone().err().unwrap().code(),
        DiagnosticCode::Cancelled
    );
}

#[test]
fn template_copy_admission_precedes_inputs_and_script_effects_and_resets_per_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let mut engine = Engine::default();
    engine
        .register_native("Touch", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(7))
        })
        .unwrap();
    let run = engine.run_source(
        "snapshot",
        "Touch",
        RunOptions {
            variables: BTreeMap::from([("input".into(), Literal::Int(1))]),
            limits: limits(1, 0),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run
        .result
        .unwrap_err()
        .to_string()
        .contains("snapshot table entries"));
    assert!(run.variables.is_empty());
    assert_eq!(run.steps, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for _ in 0..3 {
        assert_eq!(
            engine
                .run_source(
                    "snapshot",
                    "Touch",
                    RunOptions {
                        limits: limits(2, 0),
                        ..RunOptions::default()
                    }
                )
                .outcome(),
            RunOutcome::Succeeded
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn fixed_builtin_table_slot_is_included_in_engine_snapshot_count() {
    let engine = Engine::default();
    assert_eq!(
        engine
            .run_source(
                "empty",
                "",
                RunOptions {
                    limits: limits(0, 0),
                    ..RunOptions::default()
                }
            )
            .outcome(),
        RunOutcome::LimitExceeded
    );
    assert_eq!(
        engine
            .run_source(
                "empty",
                "",
                RunOptions {
                    limits: limits(1, 0),
                    ..RunOptions::default()
                }
            )
            .outcome(),
        RunOutcome::Succeeded
    );
}

#[test]
fn imports_admit_cache_and_frame_copies_at_exact_entry_and_path_boundaries() {
    let harness = Harness::new();
    let module = harness.workspace.join("module.botwork");
    fs::write(&module, "|x| = |7|\nRead { Return |x| }").unwrap();
    let per_snapshot = 3 * module.as_os_str().len() + cwd_bytes();
    let mut context = Context::with_limits(limits(5, per_snapshot * 2)).unwrap();
    import(&harness, &mut context).unwrap(); // one resolved path entry
    assert_eq!(evaluate("m::Read", &mut context).unwrap().to_string(), "7"); // frame 2 + cache 2
    let error = evaluate("m::Read", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(error.to_string().contains("snapshot table entries"));
    assert_eq!(error.span.unwrap().text(), "m::Read");
    assert!(!error.related.is_empty());
    assert!(context.statement_signature("m::Read").unwrap().is_some());

    let mut context = Context::with_limits(limits(5, per_snapshot * 2 - 1)).unwrap();
    import(&harness, &mut context).unwrap();
    let error = evaluate("m::Read", &mut context).unwrap_err();
    assert!(error.to_string().contains("snapshot path bytes"));
}

#[test]
fn native_inheritance_counts_only_visible_native_table_entries() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Read { Return |@{ Host }| }",
    )
    .unwrap();
    let mut context = Context::with_limits(limits(2, usize::MAX)).unwrap();
    context
        .register_native("Host", |_| Ok(Literal::Int(7)))
        .unwrap();
    evaluate("Local {}", &mut context).unwrap();
    import(&harness, &mut context).unwrap(); // resolved path + Host; Local isn't inherited
    assert!(context.statement_signature("m::Read").unwrap().is_some());
    let mut context = Context::with_limits(limits(1, usize::MAX)).unwrap();
    context.init_statements();
    assert!(import(&harness, &mut context)
        .unwrap_err()
        .to_string()
        .contains("snapshot table entries"));
    assert!(context.statement_signature("m::Read").unwrap().is_none());
}

#[test]
fn import_snapshot_failure_preserves_prior_effects_without_module_effects() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "Touch\nRead {}").unwrap();
    let effects = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&effects);
    let mut context = Context::with_limits(limits(1, usize::MAX)).unwrap();
    context
        .register_native("Touch", move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    evaluate("Touch", &mut context).unwrap();
    assert!(import(&harness, &mut context).is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert!(context.statement_signature("m::Read").unwrap().is_none());
}

#[test]
fn cached_aliases_avoid_copies_but_checked_host_copies_include_namespace_entries() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "Read {}").unwrap();
    // Initialization costs one entry; root has two namespaces/two wrappers and cache has two entries.
    let mut context = Context::with_limits(limits(7, usize::MAX)).unwrap();
    import(&harness, &mut context).unwrap();
    let program = Program::parse(
        harness.workspace.join("main.botwork").to_str().unwrap(),
        "Import |\"module.botwork\"| As |other|",
    )
    .unwrap();
    fs::remove_file(harness.workspace.join("module.botwork")).unwrap();
    evaluate_program_detailed(&program, &mut context).unwrap();
    let cloned = context.try_clone().unwrap();
    assert!(cloned.statement_signature("other::Read").unwrap().is_some());
    assert!(context.try_clone().is_err());
}

#[test]
fn nested_cache_transfer_preserves_dependencies_after_outer_failure() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Import |\"dependency.botwork\"| As |inner|\nMissing",
    )
    .unwrap();
    fs::write(
        harness.workspace.join("dependency.botwork"),
        "Read { Return |7| }",
    )
    .unwrap();
    let mut context = Context::default();
    assert_eq!(
        import(&harness, &mut context).unwrap_err().code(),
        DiagnosticCode::UndefinedStatement
    );
    fs::remove_file(harness.workspace.join("dependency.botwork")).unwrap();
    let program = Program::parse(
        harness.workspace.join("main.botwork").to_str().unwrap(),
        "Import |\"dependency.botwork\"| As |dep|\ndep::Read",
    )
    .unwrap();
    assert_eq!(
        evaluate_program_detailed(&program, &mut context)
            .unwrap()
            .to_string(),
        "7"
    );
}

#[test]
fn failing_imported_calls_keep_cache_and_clean_invocation_state() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Read { Import |\"dependency.botwork\"| As |dep| Missing }",
    )
    .unwrap();
    fs::write(
        harness.workspace.join("dependency.botwork"),
        "Read { Return |7| }",
    )
    .unwrap();
    let mut context = Context::default();
    import(&harness, &mut context).unwrap();
    evaluate("Try { m::Read } Catch { |handled| = |true| }", &mut context).unwrap();
    fs::remove_file(harness.workspace.join("dependency.botwork")).unwrap();
    let program = Program::parse(
        harness.workspace.join("main.botwork").to_str().unwrap(),
        "Import |\"dependency.botwork\"| As |dep|\ndep::Read",
    )
    .unwrap();
    assert_eq!(
        evaluate_program_detailed(&program, &mut context)
            .unwrap()
            .to_string(),
        "7"
    );
    assert_eq!(
        evaluate("|answer| = |handled|", &mut context)
            .unwrap()
            .to_string(),
        "true"
    );
}

#[test]
fn imported_snapshot_failure_bypasses_catch_and_restores_loop_binding() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "Read {}").unwrap();
    let mut context = Context::with_limits(limits(1, usize::MAX)).unwrap();
    import(&harness, &mut context).unwrap();
    let error = evaluate(
        "|item| = |9|\nTry { For |item| In |[1]| { m::Read } } Catch { |caught| = |true| }",
        &mut context,
    )
    .unwrap_err();
    assert!(error.to_string().contains("snapshot table entries"));
    // The stopped Context can still be inspected through Engine-independent registry queries.
    assert!(context.statement_signature("m::Read").unwrap().is_some());
    assert!(context.checkpoint().is_err());
}

#[test]
fn concurrent_checked_copies_charge_one_shared_source_context() {
    let mut context = Context::with_limits(limits(1, usize::MAX)).unwrap();
    evaluate("Read {}", &mut context).unwrap();
    let successes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| context.try_clone().is_ok()))
            .collect();
        handles
            .into_iter()
            .map(|handle| usize::from(handle.join().unwrap()))
            .sum::<usize>()
    });
    assert_eq!(successes, 1);
    assert!(context.checkpoint().is_err());
}

#[test]
fn cli_repeated_imported_calls_hit_default_copy_work_budget() {
    let harness = Harness::new();
    let mut module = (0..128)
        .map(|i| format!("|v{i}| = |{i}|\n"))
        .collect::<String>();
    module.push_str("Read { Return |1| }");
    fs::write(harness.workspace.join("module.botwork"), module).unwrap();
    let output = harness
        .run(
            "snapshots",
            "Import |\"module.botwork\"| As |m|\nWhile |true| { m::Read }\nLog |\"unreachable\"|",
            Duration::from_secs(10),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("BW8001") && error.contains("snapshot table entries"),
        "{error}"
    );
    assert!(harness
        .run("recovery", "Log |7|", Duration::from_secs(5))
        .unwrap()
        .status
        .success());
}
