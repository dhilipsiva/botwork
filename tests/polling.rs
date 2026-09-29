use botwork::core::{
    acceptance::FailureKind,
    diagnostic::{Diagnostic, DiagnosticCode as Code, DiagnosticLimits},
    grammar::BWErr,
    operation::OperationControl,
    run::{Engine, RunOptions, RunOutcome, RunResult},
    worker::protocol::WorkerProtocol,
};
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;

async fn run_with(source: &str, options: RunOptions) -> RunResult {
    Engine::default()
        .run_source_async("polling.botwork", source, options)
        .await
}
async fn run(source: &str) -> RunResult {
    run_with(source, RunOptions::default()).await
}
fn text(result: &RunResult, name: &str) -> String {
    result.variables[name].to_string()
}
/// Returns the reason, total attempts, and parsed recent history.
fn exhaustion(error: &Diagnostic) -> (&str, u32, Vec<Value>) {
    let (BWErr::ConditionNotMet {
        reason,
        attempts,
        history,
    }
    | BWErr::RetriesExhausted {
        reason,
        attempts,
        history,
    }) = &*error.error
    else {
        panic!("polling exhaustion expected: {error}")
    };
    let history: Value = serde_json::from_str(history).unwrap();
    (
        reason,
        attempts.parse().unwrap(),
        history.as_array().unwrap().clone(),
    )
}
fn started(history: &[Value]) -> Vec<u64> {
    history
        .iter()
        .map(|record| record["started_ms"].as_u64().unwrap())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn eventually_succeeds_on_a_scheduled_attempt_and_keeps_its_bindings() {
    let start = Instant::now();
    let result = run(r#"|count| = |0|
Eventually |{timeout_ms: 1000, interval_ms: 100}| {
    |count| = |count + 1|
    Assert |count| Equals |3|
}"#)
    .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(Instant::now() - start, Duration::from_millis(200));
    assert_eq!(text(&result, "count"), "3");
}

#[tokio::test(start_paused = true)]
async fn deadline_exhaustion_retains_attempts_history_last_failure_and_header() {
    let start = Instant::now();
    let result = run(r#"|count| = |0|
Eventually |{timeout_ms: 250, interval_ms: 100}| {
    |count| = |count + 1|
    Assert |{count: count}| Equals |{count: 0}|
}"#)
    .await;
    assert_eq!(Instant::now() - start, Duration::from_millis(200));
    assert_eq!(
        text(&result, "count"),
        "3",
        "the last attempt's bindings remain"
    );
    let error = result.result.clone().unwrap_err();
    assert_eq!(error.code(), Code::ConditionNotMet);
    let (reason, attempts, history) = exhaustion(&error);
    assert_eq!(
        reason,
        "no attempt succeeded within 250 ms (3 attempts; no further attempt could start before the deadline); the last attempt failed with BW9001, retained as the first cause"
    );
    assert_eq!(attempts, 3);
    assert_eq!(started(&history), [0, 100, 200]);
    assert!(history
        .iter()
        .enumerate()
        .all(|(index, record)| record["attempt"] == index as u64 + 1
            && record["duration_ms"] == 0
            && record["outcome"] == "BW9001"));
    let span = error.span.as_ref().unwrap();
    assert_eq!(span.line_column(), (2, 1));
    assert_eq!(
        span.text(),
        "Eventually |{timeout_ms: 250, interval_ms: 100}|"
    );
    assert_eq!(error.causes.len(), 1);
    let BWErr::AssertionMismatch { actual, .. } = &*error.causes[0].error else {
        panic!("the last assertion evidence is retained")
    };
    assert!(actual.contains(r#""value":3"#), "{actual}");
    assert_eq!(FailureKind::from_diagnostic(&error), FailureKind::Other);

    // A next start exactly at the deadline is already too late.
    let start = Instant::now();
    let result = run(r#"Eventually |{timeout_ms: 200, interval_ms: 100}| { Fail |"no"| }"#).await;
    assert_eq!(Instant::now() - start, Duration::from_millis(100));
    let error = result.result.clone().unwrap_err();
    let (reason, attempts, _) = exhaustion(&error);
    assert!(
        reason.contains("(2 attempts; no further attempt could start"),
        "{reason}"
    );
    assert_eq!(attempts, 2);
}

#[tokio::test(start_paused = true)]
async fn backoff_caps_and_attempt_limits_shape_the_schedule() {
    let start = Instant::now();
    let result = run(r#"Eventually |{timeout_ms: 10000, interval_ms: 50, backoff: 2, max_interval_ms: 120, attempts: 5}| {
    Fail |"never"|
}"#)
    .await;
    assert_eq!(Instant::now() - start, Duration::from_millis(390));
    let error = result.result.clone().unwrap_err();
    let (reason, attempts, history) = exhaustion(&error);
    assert!(
        reason.starts_with("no attempt succeeded in 5 attempts (the attempt limit)"),
        "{reason}"
    );
    assert_eq!(attempts, 5);
    assert_eq!(started(&history), [0, 50, 150, 270, 390]);
    assert_eq!(error.causes[0].code(), Code::ExplicitFailure);
}

#[tokio::test(start_paused = true)]
async fn the_deadline_interrupts_an_attempt_in_progress_and_remains_catchable() {
    let start = Instant::now();
    let result = run(r#"|count| = |0|
Try {
    Eventually |{timeout_ms: 300, interval_ms: 1000}| {
        |count| = |count + 1|
        Try { Sleep |1000| } Catch { |swallowed| = |true| } Finally { |cleaned| = |true| }
    }
} Catch |error| {
    |code| = |error.code|
    |cause| = |error.causes[0].code|
    |attempts| = |error.details.attempts|
    |reason| = |error.details.reason|
}
|after| = |true|"#)
    .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(Instant::now() - start, Duration::from_millis(300));
    assert_eq!(text(&result, "code"), "BW9004");
    assert_eq!(text(&result, "cause"), "BW5002");
    assert_eq!(text(&result, "attempts"), "1");
    assert!(text(&result, "reason").contains("the deadline interrupted the last"));
    assert!(
        !result.variables.contains_key("swallowed"),
        "an attempt's deadline bypasses Catch inside the attempt"
    );
    assert_eq!(text(&result, "cleaned"), "true");
    assert_eq!(text(&result, "count"), "1");
    assert_eq!(text(&result, "after"), "true");
}

#[tokio::test(start_paused = true)]
async fn finally_runs_once_per_attempt() {
    let result = run(r#"|cleanups| = |0|
Try {
    Eventually |{timeout_ms: 1000, interval_ms: 10, attempts: 3}| {
        Try { Fail |"attempt"| } Finally { |cleanups| = |cleanups + 1| }
    }
} Catch { No Operation }"#)
    .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(text(&result, "cleanups"), "3");
}

#[tokio::test(start_paused = true)]
async fn retry_repeats_actions_to_its_bound_and_reports_each_repetition() {
    let start = Instant::now();
    let result = run(r#"|runs| = |0|
Retry |{attempts: 3, interval_ms: 10}| {
    |runs| = |runs + 1|
    Fail |"unavailable"|
}"#)
    .await;
    assert_eq!(Instant::now() - start, Duration::from_millis(20));
    assert_eq!(text(&result, "runs"), "3");
    let error = result.result.clone().unwrap_err();
    assert_eq!(error.code(), Code::RetriesExhausted);
    let (reason, attempts, history) = exhaustion(&error);
    assert!(
        reason.starts_with("the action failed in 3 attempts (the attempt limit)"),
        "{reason}"
    );
    assert_eq!((attempts, started(&history)), (3, vec![0, 10, 20]));
    assert!(error
        .help()
        .contains("each attempt may have repeated the action's effects"));

    let result = run(r#"|runs| = |0|
Retry |{attempts: 5, interval_ms: 10}| {
    |runs| = |runs + 1|
    If |runs < 2| { Fail |"transient"| }
}"#)
    .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(
        text(&result, "runs"),
        "2",
        "success stops further repetition"
    );

    let result = run(r#"|runs| = |0|
Retry |{attempts: 10, timeout_ms: 25, interval_ms: 10}| {
    |runs| = |runs + 1|
    Fail |"unavailable"|
}"#)
    .await;
    let error = result.result.clone().unwrap_err();
    let (reason, attempts, _) = exhaustion(&error);
    assert!(
        reason.contains(
            "within 25 ms (3 attempts; no further attempt could start before the deadline)"
        ),
        "{reason}"
    );
    assert_eq!((attempts, text(&result, "runs")), (3, "3".into()));
}

#[tokio::test(start_paused = true)]
async fn retry_on_selects_codes_and_other_failures_propagate_unchanged() {
    let result = run(r#"|runs| = |0|
Retry |{attempts: 3, retry_on: ["BW4002"]}| {
    |runs| = |runs + 1|
    Fail |"not transient"|
}"#)
    .await;
    let error = result.result.clone().unwrap_err();
    assert_eq!(error.code(), Code::ExplicitFailure);
    assert!(error.causes.is_empty());
    assert_eq!(text(&result, "runs"), "1");

    let result = run(r#"|count| = |0|
Eventually |{timeout_ms: 1000, retry_on: ["BW9001"]}| {
    |count| = |count + 1|
    |missing| = |{}.value|
}"#)
    .await;
    assert_eq!(
        result.result.clone().unwrap_err().code(),
        Code::CollectionAccess
    );
    assert_eq!(text(&result, "count"), "1");

    let result = run(r#"|count| = |0|
Eventually |{timeout_ms: 1000, interval_ms: 1, retry_on: ["BW3004"]}| {
    |count| = |count + 1|
    If |count < 3| { |missing| = |{}.value| }
}"#)
    .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(text(&result, "count"), "3");
}

#[tokio::test(start_paused = true)]
async fn run_deadlines_and_cancellation_stop_waits_and_bypass_catch() {
    let start = Instant::now();
    let result = run_with(
        r#"Try {
    Eventually |{timeout_ms: 10000, interval_ms: 100}| { Assert |false| }
} Catch { |caught| = |true| }
|tail| = |true|"#,
        RunOptions {
            timeout: Some(Duration::from_millis(150)),
            ..RunOptions::default()
        },
    )
    .await;
    assert_eq!(Instant::now() - start, Duration::from_millis(150));
    assert_eq!(result.outcome(), RunOutcome::TimedOut);
    assert!(!result.variables.contains_key("caught"));
    assert!(!result.variables.contains_key("tail"));
    let error = result.result.clone().unwrap_err();
    assert_eq!(error.code(), Code::Timeout);
    assert_eq!(
        error.causes[0].code(),
        Code::Assertion,
        "the last observation is retained"
    );

    let control = OperationControl::default();
    let cancel = control.clone();
    let (result, ()) = tokio::join!(
        run_with(
            r#"Try {
    Retry |{attempts: 100, interval_ms: 100}| { Fail |"again"| }
} Catch { |caught| = |true| }"#,
            RunOptions {
                control,
                ..RunOptions::default()
            },
        ),
        async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            cancel.cancel();
        }
    );
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(!result.variables.contains_key("caught"));
}

#[tokio::test(start_paused = true)]
async fn shared_step_limits_inside_attempts_stop_the_run() {
    let mut options = RunOptions::default();
    options.limits.steps = 60;
    let result = run_with(
        r#"Try {
    Eventually |{timeout_ms: 10000, interval_ms: 0}| { Assert |false| }
} Catch { |caught| = |true| }"#,
        options,
    )
    .await;
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(
        result.result.clone().unwrap_err().code(),
        Code::ResourceLimit
    );
    assert!(!result.variables.contains_key("caught"));
    assert_eq!(result.steps, 60, "attempt steps are charged to the run");

    // A quota latched only by the attempt is still a run stop, never a retry.
    let mut options = RunOptions::default();
    options.limits.output.total_bytes = 8;
    let start = Instant::now();
    let result = run_with(
        r#"|count| = |0|
Try {
    Eventually |{timeout_ms: 1000, interval_ms: 1}| {
        |count| = |count + 1|
        Log |"this line is too long"|
    }
} Catch { |caught| = |true| }"#,
        options,
    )
    .await;
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(text(&result, "count"), "1");
    assert!(!result.variables.contains_key("caught"));
    assert_eq!(
        Instant::now() - start,
        Duration::ZERO,
        "no wait before the stop"
    );
    let error = result.result.clone().unwrap_err();
    assert!(
        error.causes.is_empty(),
        "the limit propagates unchanged: {error}"
    );
    assert_eq!(
        error.span.as_ref().unwrap().text(),
        r#"Log |"this line is too long"|"#
    );
}

#[tokio::test]
async fn synchronous_runs_reject_before_options_or_body_effects() {
    for keyword in ["Eventually", "Retry"] {
        let result = Engine::default().run_source(
            "sync.botwork",
            &format!(
                r#"{keyword} |@{{ Fail |"options must not run"| }}| {{ Fail |"body must not run"| }}"#
            ),
            RunOptions::default(),
        );
        let error = result.result.clone().unwrap_err();
        assert_eq!(error.code(), Code::AsyncRuntime, "{error}");
        assert!(error.causes.is_empty());
    }
}

#[tokio::test]
async fn invalid_options_fail_at_the_options_before_the_body() {
    for (options, message) in [
        (
            "{timeout_ms: 0}",
            "timeout_ms must be an Int from 1 to 86400000",
        ),
        ("{attempts: 3}", "Eventually requires timeout_ms"),
        ("{timeout_ms: 5, unknown: 1}", "Use only timeout_ms"),
        ("[1]", "options must be a Map"),
    ] {
        let result = run(&format!("Eventually |{options}| {{ |ran| = |true| }}")).await;
        let error = result.result.clone().unwrap_err();
        assert_eq!(error.code(), Code::IncompatibleType);
        assert!(error.to_string().contains(message), "{error}");
        assert_eq!(error.span.as_ref().unwrap().text(), options);
        assert!(!result.variables.contains_key("ran"));
    }
}

#[tokio::test]
async fn control_transfer_cannot_leave_an_attempt_but_return_completes_it() {
    for source in [
        "While |true| { Eventually |{timeout_ms: 10}| { Break } }",
        "For |x| In |[1]| { Retry |{attempts: 1}| { Continue } }",
        r#"Try { Fail |"x"| } Catch { Eventually |{timeout_ms: 10}| { Rethrow } }"#,
        "Eventually |{timeout_ms: 10}| { Return |1| }",
    ] {
        let error = run(source).await.result.unwrap_err();
        assert_eq!(error.code(), Code::InvalidControl, "{source}: {error}");
    }
    let result = run(r#"Poll Value |limit| {
    |seen| = |0|
    Eventually |{timeout_ms: 1000, interval_ms: 1}| {
        |seen| = |seen + 1|
        If |seen < limit| { Fail |"not yet"| }
        Return |seen * 10|
    }
    Fail |"unreachable"|
}
|value| = Poll Value |3|"#)
    .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(text(&result, "value"), "30");
}

#[tokio::test(start_paused = true)]
async fn history_is_bounded_while_attempts_count_everything() {
    let result = run(r#"Retry |{attempts: 40, interval_ms: 0}| { Fail |"again"| }"#).await;
    let error = result.result.clone().unwrap_err();
    let (_, attempts, history) = exhaustion(&error);
    assert_eq!(attempts, 40);
    assert_eq!(history.len(), 16);
    assert_eq!(history[0]["attempt"], 25);
    assert_eq!(history[15]["attempt"], 40);
}

#[tokio::test(start_paused = true)]
async fn nested_polling_keeps_inner_exhaustion_and_outer_deadlines() {
    let result = run(
        r#"Eventually |{timeout_ms: 1000, interval_ms: 100, attempts: 2}| {
    Eventually |{timeout_ms: 50, interval_ms: 20}| { Assert |false| }
}"#,
    )
    .await;
    let error = result.result.clone().unwrap_err();
    assert_eq!(exhaustion(&error).1, 2);
    assert_eq!(error.causes[0].code(), Code::ConditionNotMet);
    assert_eq!(error.causes[0].causes[0].code(), Code::Assertion);

    let start = Instant::now();
    let result = run(r#"Eventually |{timeout_ms: 30, interval_ms: 1000}| {
    Eventually |{timeout_ms: 1000, interval_ms: 10}| { Assert |false| }
}"#)
    .await;
    assert_eq!(Instant::now() - start, Duration::from_millis(30));
    let error = result.result.clone().unwrap_err();
    let (reason, attempts, _) = exhaustion(&error);
    assert!(
        reason.contains("the deadline interrupted the last"),
        "{reason}"
    );
    assert_eq!(attempts, 1);
    assert_eq!(error.causes[0].code(), Code::Timeout);
    assert_eq!(result.outcome(), RunOutcome::Failed);
}

#[tokio::test(start_paused = true)]
async fn keywords_are_case_insensitive_and_reserved() {
    let result = run(r#"eVeNtUaLlY |{timeout_ms: 10}| { |a| = |1| }
RETRY |{attempts: 1}| { |b| = |2| }
Retrying Soon { |c| = |3| }
Retrying Soon"#)
    .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(text(&result, "a"), "1");
    assert_eq!(text(&result, "b"), "2");
    for source in [
        "Retry Later |x| { No Operation }",
        "Eventually Ready { No Operation }",
    ] {
        let error = run(source).await.result.unwrap_err();
        assert_eq!(error.code(), Code::Syntax, "{source}: {error}");
    }
}

#[tokio::test(start_paused = true)]
async fn exhaustion_evidence_is_admitted_under_diagnostic_budgets() {
    let source = r#"Retry |{attempts: 2, interval_ms: 0}| { Fail |"x"| }"#;
    let baseline = run(source).await.result.unwrap_err();
    let cause = DiagnosticLimits::default()
        .check(&baseline.causes[0])
        .unwrap();
    let mut options = RunOptions::default();
    options.limits.diagnostics.text_bytes = cause.text_bytes;
    let error = run_with(source, options).await.result.unwrap_err();
    assert_eq!(error.code(), Code::ResourceLimit, "{error}");
    assert_eq!(
        FailureKind::from_diagnostic(&error),
        FailureKind::LimitExceeded
    );
    // Every attempt was admitted; only the exhaustion record exceeded the budget.
    assert_eq!(error.causes[0].code(), Code::RetriesExhausted);
    assert!(error.causes[0].omissions.is_some());
}

#[test]
fn worker_protocol_round_trips_both_exhaustion_shapes() {
    let protocol = WorkerProtocol::default();
    for error in [
        BWErr::ConditionNotMet {
            reason: "no attempt succeeded".into(),
            attempts: "2".into(),
            history: "[]".into(),
        },
        BWErr::RetriesExhausted {
            reason: "the action failed".into(),
            attempts: "3".into(),
            history: r#"[{"attempt":3}]"#.into(),
        },
    ] {
        let error = Diagnostic::new(error);
        let encoded = protocol.encode_response(Err(&error)).unwrap();
        let decoded = protocol.decode_response(&encoded).unwrap().unwrap_err();
        assert_eq!(decoded.code(), error.code());
        assert_eq!(format!("{:?}", decoded.error), format!("{:?}", error.error));
        assert_eq!(encoded, protocol.encode_response(Err(&decoded)).unwrap());
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn operation_timeouts_are_observations_but_ambiguous_actions_by_default() {
    const SLOW: &str = r#"|runs| = |runs + 1|
    Run Process |"/bin/sleep"| With Arguments |["5"]| Options |{"timeout_ms": 20}|"#;
    let started = std::time::Instant::now();
    let result = run(&format!(
        "|runs| = |0|\nEventually |{{timeout_ms: 10000, interval_ms: 1, attempts: 2}}| {{\n    {SLOW}\n}}"
    ))
    .await;
    let error = result.result.clone().unwrap_err();
    assert_eq!(error.code(), Code::ConditionNotMet, "{error}");
    assert_eq!(error.causes[0].code(), Code::Timeout);
    assert_eq!(text(&result, "runs"), "2");

    let result = run(&format!(
        "|runs| = |0|\nTry {{\nRetry |{{attempts: 3, interval_ms: 1}}| {{\n    {SLOW}\n}}\n}} Catch {{ |caught| = |true| }}"
    ))
    .await;
    assert_eq!(result.outcome(), RunOutcome::TimedOut);
    assert_eq!(
        text(&result, "runs"),
        "1",
        "an ambiguous action is not repeated"
    );
    assert!(
        !result.variables.contains_key("caught"),
        "an unretried timeout keeps its run-stop meaning"
    );

    let result = run(&format!(
        "|runs| = |0|\nRetry |{{attempts: 3, interval_ms: 1, retry_on: [\"BW5002\"]}}| {{\n    {SLOW}\n}}"
    ))
    .await;
    assert_eq!(
        result.result.clone().unwrap_err().code(),
        Code::RetriesExhausted
    );
    assert_eq!(text(&result, "runs"), "3");
    assert!(started.elapsed() < Duration::from_secs(4));
}
