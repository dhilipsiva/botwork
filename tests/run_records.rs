#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use botwork::core::{
    acceptance::{CaseExpectation, CaseStatus},
    ast::Program,
    operation::OperationControl,
    report::{RecordLimits, RecordOptions, RunIdentity, RunRecord},
    run::{Engine, RunOptions, RunResult},
};
use cli_harness::Harness;
use std::{fs, time::Duration};

fn recording(record: RecordOptions) -> RunOptions {
    RunOptions {
        record: Some(record),
        ..RunOptions::default()
    }
}
fn run(source: &str) -> RunRecord {
    Engine::default()
        .run_source(
            "record.botwork",
            source,
            recording(RecordOptions::default()),
        )
        .record
        .unwrap()
}
async fn run_async(source: &str, options: RunOptions) -> RunResult {
    Engine::default()
        .run_source_async("record.botwork", source, options)
        .await
}
fn statuses(record: &RunRecord) -> Vec<(&'static str, Option<CaseStatus>, Option<&'static str>)> {
    record
        .statements
        .iter()
        .map(|statement| (statement.kind, statement.status, statement.code))
        .collect()
}

#[test]
fn records_top_level_statements_logs_errors_and_timing() {
    let record = run("|a| = |1|\nLog |\"hi\"|\nAssert |a| Equals |2|\nLog |\"never\"|");
    assert_eq!((record.format, record.version), ("botwork-run", 1));
    assert_eq!(
        record.identity,
        RunIdentity::new("record.botwork", "record.botwork")
    );
    assert_eq!((record.status, record.complete), (CaseStatus::Failed, true));
    assert_eq!(
        statuses(&record),
        [
            ("assignment", Some(CaseStatus::Succeeded), None),
            ("call", Some(CaseStatus::Succeeded), None),
            ("call", Some(CaseStatus::Failed), Some("BW9001")),
        ],
        "statements after the failure never start"
    );
    assert_eq!(record.statements[2].location.line, 3);
    assert!(record
        .statements
        .windows(2)
        .all(|pair| pair[0].offset_us + pair[0].duration_us.unwrap() <= pair[1].offset_us));
    assert_eq!(record.logs.len(), 1);
    assert_eq!(
        (record.logs[0].statement, record.logs[0].text.as_str()),
        (Some(1), "hi")
    );
    let error = record.error.as_ref().unwrap();
    assert_eq!((error.code, error.status), ("BW9001", CaseStatus::Failed));
    assert!(error
        .message
        .starts_with("Assertion failed: Expected 2 (Int), got 1 (Int)"));
    assert_eq!(error.location.as_ref().unwrap().line, 3);
    for time in [
        record.started_at.as_ref().unwrap(),
        record.finished_at.as_ref().unwrap(),
    ] {
        assert!(
            chrono::DateTime::parse_from_rfc3339(time).is_ok() && time.ends_with('Z'),
            "{time}"
        );
    }
    assert!(record.started_at <= record.finished_at);
    assert_eq!(
        record.events, 9,
        "started, 3 statement pairs, 1 log, finished"
    );
    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["status"], "failed");
    assert_eq!(json["statements"][2]["status"], "failed");
    assert!(
        json.get("limits").is_none(),
        "internal state is not serialized"
    );
}

#[test]
fn recording_is_opt_in_and_identities_default_to_the_source() {
    let result = Engine::default().run_source("plain.botwork", "Log |1|", RunOptions::default());
    assert!(result.record.is_none());
    let program = Program::parse("program.botwork", "No Operation").unwrap();
    let record = Engine::default()
        .run_program(&program, recording(RecordOptions::default()))
        .record
        .unwrap();
    assert_eq!(record.identity.id, "program.botwork");
    let identity = RunIdentity::new("suite/case/row", "case / row").with_row("data", "row");
    let record = Engine::default()
        .run_program(
            &program,
            recording(RecordOptions {
                identity: identity.clone(),
                ..RecordOptions::default()
            }),
        )
        .record
        .unwrap();
    assert_eq!(
        (record.identity, record.status),
        (identity, CaseStatus::Succeeded)
    );
}

#[test]
fn expectations_decide_expected_failures_and_unexpected_passes() {
    let expected = CaseExpectation::failure("BUG-7").unwrap();
    for (source, status) in [
        ("Assert |false|", CaseStatus::ExpectedFailure),
        ("No Operation", CaseStatus::UnexpectedPass),
        ("Fail |\"not an assertion\"|", CaseStatus::Failed),
    ] {
        let record = Engine::default()
            .run_source(
                "expected.botwork",
                source,
                recording(RecordOptions {
                    expectation: expected.clone(),
                    ..RecordOptions::default()
                }),
            )
            .record
            .unwrap();
        assert_eq!(record.status, status, "{source}");
    }
}

#[tokio::test(start_paused = true)]
async fn stops_record_consistent_statuses_for_the_run_and_statement() {
    let result = run_async(
        "Log |1|\nSleep |1000|\nLog |2|",
        RunOptions {
            timeout: Some(Duration::from_millis(50)),
            ..recording(RecordOptions::default())
        },
    )
    .await;
    let record = result.record.unwrap();
    assert_eq!(record.status, CaseStatus::TimedOut);
    assert_eq!(
        statuses(&record)[1],
        ("call", Some(CaseStatus::TimedOut), Some("BW5002"))
    );
    assert_eq!(record.statements.len(), 2);

    let control = OperationControl::default();
    let cancel = control.clone();
    let (result, ()) = tokio::join!(
        run_async(
            "Sleep |1000|",
            RunOptions {
                control,
                ..recording(RecordOptions::default())
            }
        ),
        async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        }
    );
    let record = result.record.unwrap();
    assert_eq!(record.status, CaseStatus::Cancelled);
    assert_eq!(record.statements[0].status, Some(CaseStatus::Cancelled));

    let mut options = recording(RecordOptions::default());
    options.limits.steps = 20;
    let record = run_async("While |true| { No Operation }", options)
        .await
        .record
        .unwrap();
    assert_eq!(record.status, CaseStatus::LimitExceeded);
    assert_eq!(record.statements[0].code, Some("BW8001"));
    assert!(record.error.unwrap().message.contains("evaluation steps"));
}

#[test]
fn loops_nested_work_and_large_output_stay_bounded() {
    let record = run("|i| = |0|\nWhile |i < 1000| {\n    Log |i|\n    |i| = |i + 1|\n}");
    assert_eq!(
        record.statements.len(),
        2,
        "loop iterations are not statements"
    );
    assert_eq!(record.logs.len(), 256);
    assert_eq!(record.omitted_logs, 744);
    assert_eq!(
        record.logged_bytes,
        (0..1000)
            .map(|i: u32| i.to_string().len() as u64)
            .sum::<u64>()
    );
    assert!(record.logs.iter().all(|log| log.statement == Some(1)));

    let source = (0..1100)
        .map(|_| "No Operation")
        .collect::<Vec<_>>()
        .join("\n");
    let record = run(&source);
    assert_eq!(
        (record.statements.len(), record.omitted_statements),
        (1024, 76)
    );
    assert_eq!(record.status, CaseStatus::Succeeded);

    let limits = RecordLimits {
        log_record_bytes: 10,
        ..RecordLimits::default()
    };
    let record = Engine::default()
        .run_source(
            "long.botwork",
            "Log |\"ééééééééééé\"|",
            recording(RecordOptions {
                limits,
                ..RecordOptions::default()
            }),
        )
        .record
        .unwrap();
    assert_eq!(record.logs[0].bytes, 22);
    assert_eq!(record.logs[0].text, "ééééé…[truncated]");
    assert!(record.logs[0].truncated);
}

#[tokio::test]
async fn async_runs_capture_worker_logs_and_match_synchronous_records() {
    let source = "|x| = |[1, 2]|\nFor |v| In |x| { Log |v| }\nAssert |x| Equals |[1]|";
    let sync = run(source);
    let asynchronous = run_async(source, recording(RecordOptions::default()))
        .await
        .record
        .unwrap();
    assert_eq!(statuses(&sync), statuses(&asynchronous));
    let texts = |record: &RunRecord| {
        record
            .logs
            .iter()
            .map(|log| (log.statement, log.text.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(texts(&sync), texts(&asynchronous));
    assert_eq!(
        texts(&asynchronous),
        [(Some(1), "1".into()), (Some(1), "2".into())]
    );
    let scoped = run_async(
        "Try { Log |\"body\"| } Finally { Log |\"cleanup\"| }\nEventually |{timeout_ms: 1000}| { Log |\"attempt\"| }",
        recording(RecordOptions::default()),
    )
    .await
    .record
    .unwrap();
    assert_eq!(
        texts(&scoped),
        [
            (Some(0), "body".into()),
            (Some(0), "cleanup".into()),
            (Some(1), "attempt".into())
        ],
        "cleanup and polling scopes share the run's recorder"
    );
    assert_eq!(
        sync.error.as_ref().unwrap().code,
        asynchronous.error.as_ref().unwrap().code
    );
}

#[test]
fn imported_modules_contribute_logs_but_not_statements() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Log |\"loading\"|\nGreet |name| { Log |name| }",
    )
    .unwrap();
    let record = Engine::default()
        .run_source(
            "main.botwork",
            "Import |\"module.botwork\"| As |m|\nm::Greet |\"Ada\"|",
            RunOptions {
                working_directory: Some(harness.workspace.clone()),
                ..recording(RecordOptions::default())
            },
        )
        .record
        .unwrap();
    assert_eq!(record.status, CaseStatus::Succeeded, "{:?}", record.error);
    assert_eq!(
        statuses(&record)
            .iter()
            .map(|(kind, _, _)| *kind)
            .collect::<Vec<_>>(),
        ["import", "call"]
    );
    assert_eq!(
        record
            .logs
            .iter()
            .map(|log| (log.statement, log.text.as_str()))
            .collect::<Vec<_>>(),
        [(Some(0), "loading"), (Some(1), "Ada")]
    );
}

#[test]
fn failed_preparation_still_finishes_one_terminal_record() {
    let mut options = recording(RecordOptions::default());
    options.limits.evaluation_depth = 10_000;
    let record = Engine::default()
        .run_source("invalid.botwork", "Log |1|", options)
        .record
        .unwrap();
    assert_eq!((record.status, record.complete), (CaseStatus::Failed, true));
    assert!(record.statements.is_empty() && record.logs.is_empty());
    assert_eq!(record.error.unwrap().code, "BW7002");
    let record = run("Log |");
    assert_eq!(record.error.unwrap().code, "BW1001");
    assert!(record.statements.is_empty());
}
