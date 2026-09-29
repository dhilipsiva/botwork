use super::*;
use crate::core::ast::SourceFile;

fn location() -> SourceLocation {
    let source = Arc::new(SourceFile::from_owned_parts(
        "t.botwork".into(),
        "Log |1|".into(),
    ));
    SourceLocation::from_span(&Span::from_source_range(source, 0, 7).unwrap())
}
fn events(events: Vec<Event>) -> Vec<EventRecord> {
    events
        .into_iter()
        .enumerate()
        .map(|(sequence, event)| EventRecord {
            sequence: sequence as u64,
            event,
        })
        .collect()
}
fn started() -> Event {
    Event::RunStarted {
        identity: RunIdentity::new("t", "t"),
        started_at: "2026-01-01T00:00:00.000000Z".into(),
    }
}
fn statement(index: u64) -> Event {
    Event::StatementStarted {
        index,
        kind: "call",
        location: location(),
        offset_us: 10 * index,
    }
}
fn finished_statement(index: u64, status: CaseStatus) -> Event {
    Event::StatementFinished {
        index,
        status,
        code: None,
        offset_us: 10 * index + 4,
    }
}
fn finished(status: CaseStatus) -> Event {
    Event::RunFinished {
        status,
        finished_at: "2026-01-01T00:00:01.000000Z".into(),
        duration_us: 1_000_000,
        error: None,
    }
}
fn fold(list: Vec<Event>) -> Result<RunRecord, RecordError> {
    RunRecord::from_events(
        RunIdentity::default(),
        RecordLimits::default(),
        &events(list),
    )
}

#[test]
fn a_complete_stream_folds_into_one_terminal_record() {
    let record = fold(vec![
        started(),
        statement(0),
        finished_statement(0, CaseStatus::Succeeded),
        statement(1),
        finished_statement(1, CaseStatus::Failed),
        finished(CaseStatus::Failed),
    ])
    .unwrap();
    assert_eq!(
        (record.status, record.complete, record.events),
        (CaseStatus::Failed, true, 6)
    );
    assert_eq!(record.statements[1].duration_us, Some(4));
    assert_eq!(record.statements[1].status, Some(CaseStatus::Failed));
    assert_eq!(record.identity, RunIdentity::new("t", "t"));
}

#[test]
fn partial_streams_are_interrupted_never_passed() {
    for list in [
        vec![],
        vec![started()],
        vec![started(), statement(0)],
        vec![
            started(),
            statement(0),
            finished_statement(0, CaseStatus::Succeeded),
        ],
    ] {
        let record = fold(list).unwrap();
        assert_eq!(record.status, CaseStatus::Interrupted);
        assert!(!record.complete);
        assert!(record.finished_at.is_none());
    }
    let open = fold(vec![started(), statement(0)]).unwrap();
    assert_eq!(
        open.statements[0].status, None,
        "unfinished statements have no status"
    );
}

#[test]
fn skipped_work_has_one_event_and_no_start() {
    let skipped = Event::RunSkipped {
        identity: RunIdentity::new("s/c", "c"),
        reason: SkipReason::SuiteSetupFailed,
        recorded_at: "2026-01-01T00:00:00.000000Z".into(),
    };
    let record = fold(vec![skipped.clone()]).unwrap();
    assert_eq!(
        (
            record.status,
            record.complete,
            record.skip_reason,
            record.started_at
        ),
        (
            CaseStatus::Skipped,
            true,
            Some(SkipReason::SuiteSetupFailed),
            None
        )
    );
    assert_eq!(
        fold(vec![skipped.clone(), started()]).unwrap_err(),
        RecordError::AfterTerminal
    );
    assert_eq!(
        fold(vec![started(), skipped]).unwrap_err(),
        RecordError::AlreadyStarted
    );
}

#[test]
fn ordering_violations_are_rejected_without_changing_the_record() {
    for (list, error) in [
        (vec![statement(0)], RecordError::NotStarted),
        (
            vec![finished(CaseStatus::Succeeded)],
            RecordError::NotStarted,
        ),
        (vec![started(), started()], RecordError::AlreadyStarted),
        (
            vec![
                started(),
                finished(CaseStatus::Succeeded),
                finished(CaseStatus::Failed),
            ],
            RecordError::AfterTerminal,
        ),
        (vec![started(), statement(1)], RecordError::StatementOrder),
        (
            vec![started(), statement(0), statement(1)],
            RecordError::StatementOrder,
        ),
        (
            vec![started(), finished_statement(0, CaseStatus::Succeeded)],
            RecordError::StatementOrder,
        ),
        (
            vec![
                started(),
                statement(0),
                finished_statement(1, CaseStatus::Succeeded),
            ],
            RecordError::StatementOrder,
        ),
    ] {
        assert_eq!(fold(list.clone()).unwrap_err(), error, "{list:?}");
    }
    let mut record = RunRecord::new(RunIdentity::default(), RecordLimits::default());
    record
        .apply(&EventRecord {
            sequence: 0,
            event: started(),
        })
        .unwrap();
    let before = record.clone();
    assert_eq!(
        record.apply(&EventRecord {
            sequence: 5,
            event: statement(0)
        }),
        Err(RecordError::Sequence {
            expected: 1,
            found: 5
        })
    );
    assert_eq!(record, before);
}

#[test]
fn retention_limits_count_omitted_work() {
    let limits = RecordLimits {
        statements: 1,
        logs: 2,
        log_bytes: 5,
        artifacts: 0,
        ..RecordLimits::default()
    };
    let log = |text: &str| {
        Event::Log(LogRecord {
            statement: None,
            offset_us: 0,
            bytes: text.len() as u64,
            text: text.into(),
            truncated: false,
        })
    };
    let record = RunRecord::from_events(
        RunIdentity::default(),
        limits,
        &events(vec![
            started(),
            statement(0),
            finished_statement(0, CaseStatus::Succeeded),
            statement(1),
            finished_statement(1, CaseStatus::Succeeded),
            log("abc"),
            log("defg"),
            log("h"),
            log("i"),
            Event::Artifact(ArtifactRecord {
                kind: "assertion".into(),
                path: "a.json".into(),
            }),
            finished(CaseStatus::Succeeded),
        ]),
    )
    .unwrap();
    assert_eq!((record.statements.len(), record.omitted_statements), (1, 1));
    assert_eq!(
        record
            .logs
            .iter()
            .map(|log| log.text.as_str())
            .collect::<Vec<_>>(),
        ["abc", "h"]
    );
    assert_eq!((record.omitted_logs, record.logged_bytes), (2, 9));
    assert_eq!((record.artifacts.len(), record.omitted_artifacts), (0, 1));
}

#[test]
fn bounded_text_keeps_whole_scalars_and_marks_cuts() {
    assert_eq!(bounded(&"héllo", 2), ("h…[truncated]".into(), true));
    assert_eq!(bounded(&"héllo", 3), ("hé…[truncated]".into(), true));
    assert_eq!(bounded(&"héllo", 6), ("héllo".into(), false));
    assert_eq!(measure(&"héllo"), 6);
    assert_eq!(
        timestamp(SystemTime::UNIX_EPOCH),
        "1970-01-01T00:00:00.000000Z"
    );
}

#[test]
fn run_status_uses_the_acceptance_decision() {
    let assertion = Diagnostic::new(crate::core::grammar::BWErr::AssertionFailed("x".into()));
    let timeout = Diagnostic::new(crate::core::grammar::BWErr::Timeout("x".into()));
    let expected = CaseExpectation::failure("known").unwrap();
    assert_eq!(
        decide(Ok(()), &CaseExpectation::default()),
        CaseStatus::Succeeded
    );
    assert_eq!(decide(Ok(()), &expected), CaseStatus::UnexpectedPass);
    assert_eq!(
        decide(Err(&assertion), &CaseExpectation::default()),
        CaseStatus::Failed
    );
    assert_eq!(
        decide(Err(&assertion), &expected),
        CaseStatus::ExpectedFailure
    );
    assert_eq!(decide(Err(&timeout), &expected), CaseStatus::TimedOut);
}

#[test]
fn error_causes_are_listed_depth_first_within_the_limit() {
    use crate::core::grammar::BWErr;
    let leaf = |error: BWErr| Diagnostic::new(error);
    let branch = leaf(BWErr::ExplicitFailure("b".into()))
        .while_handling(leaf(BWErr::AssertionFailed("c".into())))
        .while_handling(leaf(BWErr::Timeout("d".into())));
    let root = leaf(BWErr::NativeError("root".into()))
        .while_handling(branch)
        .while_handling(leaf(BWErr::Cancelled("e".into())));
    let record = ErrorRecord::from_diagnostic(&root, &RecordLimits::default());
    assert_eq!(record.code, "BW4002");
    assert_eq!(record.status, CaseStatus::Failed);
    assert_eq!(record.causes, ["BW9002", "BW9001", "BW5002", "BW5001"]);
    assert_eq!(record.omitted_causes, 0);
    let limited = ErrorRecord::from_diagnostic(
        &root,
        &RecordLimits {
            causes: 2,
            message_bytes: 3,
            ..RecordLimits::default()
        },
    );
    assert_eq!(
        (limited.causes, limited.omitted_causes),
        (vec!["BW9002", "BW9001"], 2)
    );
    assert!(limited.truncated && limited.message.ends_with("…[truncated]"));
}
