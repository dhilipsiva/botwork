#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    acceptance::{CaseCompletion, CaseExpectation, CaseStatus, FailureKind, PhaseOutcome},
    ast::Program,
    diagnostic::{Diagnostic, DiagnosticCode as Code},
    eval::{evaluate_program_async, evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome},
};
use std::{collections::BTreeMap, time::Duration};

fn run(source: &str) -> botwork::core::run::RunResult {
    Engine::default().run_source("builtins.botwork", source, RunOptions::default())
}

#[test]
fn assertions_use_bool_conditions_and_existing_deep_exact_equality() {
    for source in [
        "Assert |true|",
        "a S s E r T |1 < 2|",
        "Assert |1| Equals |1.0|",
        "Assert |[1, {a: [true, 3]}]| Equals |[1.0, {a: [true, 3.0]}]|",
        "Assert |@{ No Operation }| Equals |@{ No Operation }|",
        "Assert |\"தமிழ்\"| Equals |\"தமிழ்\"|",
        "Assert |{a: 1, b: 2}| Equals |{b: 2, a: 1}|",
    ] {
        let result = run(source);
        assert!(
            matches!(result.result, Ok(Literal::None)),
            "{source}: {:?}",
            result.result
        );
    }
    for source in [
        "Assert |false|",
        "Assert |16777217| Equals |16777216.0|",
        "Assert |[1, 2]| Equals |[2, 1]|",
        "Assert |true| Equals |1|",
        "Assert |{a: 1}| Equals |{b: 1}|",
        "Assert |[1]| Equals |[1, 2]|",
        "Assert |\"é\"| Equals |\"é\"|",
    ] {
        let result = run(&format!("{source}\n|unreachable| = |1|"));
        assert_eq!(result.outcome(), RunOutcome::Failed);
        let error = result.result.unwrap_err();
        assert_eq!(error.code(), Code::Assertion, "{source}");
        assert_eq!(FailureKind::from_diagnostic(&error), FailureKind::Assertion);
        assert!(error.to_string().contains("Expected"));
        assert!(!result.variables.contains_key("unreachable"));
        assert_eq!(
            error.span.as_ref().unwrap().source().name(),
            "builtins.botwork"
        );
        assert_eq!(error.call_stack.len(), 1);
    }
}

#[test]
fn explicit_failure_remains_distinct_from_assertions_and_uses_bounded_reason_metadata() {
    for reason in ["", "reason", "é\n🙂"] {
        let source = format!("Fail |{}|", serde_json::to_string(reason).unwrap());
        let error = run(&source).result.unwrap_err();
        assert_eq!(error.code(), Code::ExplicitFailure);
        assert!(matches!(&*error.error, BWErr::ExplicitFailure(actual) if actual == reason));
        assert_eq!(FailureKind::from_diagnostic(&error), FailureKind::Other);
        assert!(error.to_value().to_string().contains("BW9002"));
    }
}

#[test]
fn invalid_builtin_argument_kinds_and_names_are_operational_failures() {
    for source in [
        "Assert |1|",
        "Assert |\"true\"|",
        "Fail |1|",
        "Get Variable |1|",
        "Variable Exists |false|",
        "Variable Exists |\"x.y\"|",
        "Get Variable |\" x\"|",
        "Get Variable |\"\"|",
        "Variable Exists |\"two words\"|",
    ] {
        let error = run(source).result.unwrap_err();
        assert_eq!(error.code(), Code::IncompatibleType, "{source}: {error}");
        assert_eq!(FailureKind::from_diagnostic(&error), FailureKind::Other);
    }
    assert_eq!(
        run("Get Variable |\"absent\"|").result.unwrap_err().code(),
        Code::UndefinedVariable
    );
}

#[test]
fn inspection_observes_lexical_scope_case_sensitivity_none_and_all_value_kinds() {
    let result = run(r#"
|x| = |7|
|é| = No Operation
Assert |@{ Variable Exists |"é"| }|
Assert |@{ Variable Exists |"absent"| }| Equals |false|
Assert |@{ Variable Exists |"X"| }| Equals |false|
Read { Return |@{ Get Variable |"x"| }| }
Caller |x| { Assert |@{ Read }| Equals |7|
    Return |@{ Get Variable |"x"| }| }
Assert |@{ Caller |42| }| Equals |42|
Assert |@{ Type Of |@{ Get Variable |"é"| }| }| Equals |"None"|
Assert |@{ Type Of |1| }| Equals |"Int"|
Assert |@{ Type Of |1.0| }| Equals |"Float"|
Assert |@{ Type Of |true| }| Equals |"Bool"|
Assert |@{ Type Of |"hi"| }| Equals |"String"|
Assert |@{ Type Of |[]| }| Equals |"Array"|
Assert |@{ Type Of |{}| }| Equals |"Map"|
"#);
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn catches_cleanup_and_expected_failure_classification_preserve_unhandled_evidence() {
    let expectation = CaseExpectation::failure("BUG-42").unwrap();
    for (source, kind, status) in [
        (
            "Try { Assert |false| } Finally { No Operation }",
            FailureKind::Assertion,
            CaseStatus::ExpectedFailure,
        ),
        (
            "Try { Assert |false| } Finally { Fail |\"cleanup\"| }",
            FailureKind::Other,
            CaseStatus::Failed,
        ),
        ("Fail |\"body\"|", FailureKind::Other, CaseStatus::Failed),
    ] {
        let error = run(source).result.unwrap_err();
        assert_eq!(FailureKind::from_diagnostic(&error), kind);
        let completion = CaseCompletion::executed(
            PhaseOutcome::Succeeded,
            Some(PhaseOutcome::Failed(kind)),
            PhaseOutcome::Succeeded,
            None,
        )
        .unwrap();
        assert_eq!(completion.decide(&expectation), status);
    }
    let recovered = run(r#"Try { Assert |false| } Catch |error| {
        Assert |error.code| Equals |"BW9001"|
        Assert |error.details.reason| Equals |"Expected true, got false"|
    } Finally { No Operation }"#);
    assert!(recovered.result.is_ok(), "{:?}", recovered.result);
    let completion = CaseCompletion::executed(
        PhaseOutcome::Succeeded,
        Some(PhaseOutcome::Succeeded),
        PhaseOutcome::Succeeded,
        None,
    )
    .unwrap();
    assert_eq!(completion.decide(&expectation), CaseStatus::UnexpectedPass);
    assert_eq!(
        FailureKind::from_diagnostic_code(Code::Assertion),
        FailureKind::Other
    );
}

#[test]
fn diagnostic_truncation_and_value_copies_obey_existing_admission_limits() {
    let mut options = RunOptions::default();
    options.limits.diagnostics.text_bytes = 1;
    let result = Engine::default().run_source("limited", "Assert |false|", options);
    let error = result.result.unwrap_err();
    assert_ne!(FailureKind::from_diagnostic(&error), FailureKind::Assertion);
    assert!(error.to_string().contains("omitted"));

    let mut options = RunOptions {
        variables: BTreeMap::from([("payload".into(), Literal::String("x".repeat(1024)))]),
        ..RunOptions::default()
    };
    options.limits.temporaries.payload_bytes = 100;
    let error = Engine::default()
        .run_source("copy", "Get Variable |\"payload\"|", options)
        .result
        .unwrap_err();
    assert_eq!(error.code(), Code::ResourceLimit);
    let mut context = Context::with_limits(RunLimits::default()).unwrap();
    context.init_statements();
    // A missing variable probe must not construct an undefined-variable error.
    let value = evaluate_program_detailed(
        &Program::parse("exists", "Variable Exists |\"absent\"|").unwrap(),
        &mut context,
    )
    .unwrap();
    assert!(matches!(value, Literal::Bool(false)));
}

#[test]
fn initialization_is_idempotent_and_preserves_existing_host_and_local_overrides() {
    let mut context = Context::default();
    context
        .register_native("Assert |condition|", |_| Ok(Literal::Int(42)))
        .unwrap();
    context.init_statements();
    context.init_statements();
    assert_eq!(context.statement_signatures().len(), 9);
    let value = evaluate_program_detailed(
        &Program::parse("override", "Assert |false|").unwrap(),
        &mut context,
    )
    .unwrap();
    assert!(matches!(value, Literal::Int(42)));
    let result = run("Local { No Operation { Return |42| }\nReturn |@{ No Operation }| }\nAssert |@{ Local }| Equals |42|");
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[tokio::test(start_paused = true)]
async fn sleep_awaits_exact_virtual_time_and_cancellation_and_deadlines_prevent_the_tail() {
    let program = Program::parse("sleep", "Sleep |25|\nNo Operation").unwrap();
    let mut context = Context::default();
    context.init_statements();
    let start = tokio::time::Instant::now();
    evaluate_program_async(&program, context).await.unwrap();
    assert_eq!(
        tokio::time::Instant::now() - start,
        Duration::from_millis(25)
    );
    for timed_out in [false, true] {
        let control = OperationControl::default()
            .child(timed_out.then(|| tokio::time::Instant::now() + Duration::from_millis(5)));
        let child = control.clone();
        let mut context = Context::with_control(RunLimits::default(), control).unwrap();
        context.init_statements();
        let program =
            Program::parse("stopped", "Sleep |10000|\nFail |\"tail must not run\"|").unwrap();
        let pending = evaluate_program_async(&program, context);
        let stopping = async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if !timed_out {
                child.cancel();
            }
        };
        let (result, ()) = tokio::join!(pending, stopping);
        assert_eq!(
            result.unwrap_err().code(),
            if timed_out {
                Code::Timeout
            } else {
                Code::Cancelled
            }
        );
    }
}

#[tokio::test]
async fn sleep_rejects_negative_and_wrong_kind_parameters_and_sync_calls_before_arguments() {
    for source in ["Sleep |-1|", "Sleep |1.5|", "Sleep |true|"] {
        let result = Engine::default()
            .run_source_async("invalid", source, RunOptions::default())
            .await;
        assert_eq!(result.result.unwrap_err().code(), Code::IncompatibleType);
    }
    let result = run("Sleep |@{ Fail |\"argument must not run\"| }|");
    assert_eq!(result.result.unwrap_err().code(), Code::AsyncRuntime);
}

#[test]
fn cli_failures_show_typed_diagnostics_and_successful_recovery_runs_cleanup() {
    let harness = cli_harness::Harness::new();
    for (source, status, stdout, code) in [
        ("Assert |false|\nLog |\"unreachable\"|", 1, "", "BW9001"),
        ("Fail |\"broken\"|", 1, "", "BW9002"),
        ("Try { Assert |false| } Catch |error| { Log |error.code| } Finally { Log |\"cleanup\"| }", 0, "BW9001\ncleanup\n", ""),
    ] {
        let output = harness.run("builtins", source, Duration::from_secs(5)).unwrap();
        assert_eq!(output.status.code(), Some(status));
        assert_eq!(output.stdout, stdout.as_bytes());
        if code.is_empty() { assert!(output.stderr.is_empty()); }
        else { assert!(String::from_utf8(output.stderr).unwrap().contains(code)); }
    }
}

#[test]
fn typed_assertion_evidence_does_not_ignore_secondary_or_omitted_failures() {
    let mut error = Diagnostic::new(BWErr::AssertionFailed("false".into()));
    assert_eq!(FailureKind::from_diagnostic(&error), FailureKind::Assertion);
    error
        .causes
        .push(Diagnostic::new(BWErr::ExplicitFailure("cleanup".into())));
    assert_eq!(FailureKind::from_diagnostic(&error), FailureKind::Other);
}

#[test]
fn suite_assertions_use_case_identity_cleanup_and_finish_all_status() {
    let harness = cli_harness::Harness::new();
    let path = harness.workspace.join("checks.suite.botwork");
    std::fs::write(
        &path,
        r#"Suite |"checks"| {
        CaseSetup { |ready| = |true| }
        CaseTeardown { Log |"cleanup"| }
        Case |"bad"| { Assert |false| }
        Case |"good"| { Assert |@{ Get Variable |"ready"| }| }
    }"#,
    )
    .unwrap();
    let output = harness
        .command(
            "suite-builtins",
            &["--suite", path.to_str().unwrap(), "--jobs", "2"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"cleanup\ncleanup\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[case checks/bad] failed:"), "{stderr}");
    assert!(stderr.contains("[case checks/good] succeeded:"), "{stderr}");
    assert!(stderr.contains("BW9001"));
    assert!(stderr.ends_with("[cases] 2 selected: 1 succeeded, 1 failed\n"));
}
