use super::*;
use std::collections::HashMap;

fn options(entries: &[(&str, Literal)]) -> Literal {
    Literal::Map(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect::<HashMap<_, _>>(),
    )
}
fn codes(items: &[&str]) -> Literal {
    Literal::Array(
        items
            .iter()
            .map(|item| Literal::String((*item).into()))
            .collect(),
    )
}
type Entries = Vec<(&'static str, Literal)>;

fn parse(mode: PollMode, entries: &[(&str, Literal)]) -> Result<Policy, &'static str> {
    Policy::parse(mode, &options(entries))
}

#[test]
fn options_are_strict_bounded_and_mode_specific() {
    use Literal::{Float, Int, String as Text};
    let timeout = ("timeout_ms", Int(1));
    let rejected: Vec<(PollMode, Entries, &str)> = vec![
        (PollMode::Eventually, vec![], "Eventually requires timeout_ms"),
        (PollMode::Retry, vec![timeout.clone()], "Retry requires attempts"),
        (PollMode::Eventually, vec![timeout.clone(), ("timeout", Int(1))], OPTIONS),
        (PollMode::Eventually, vec![("timeout_ms", Int(0))], "timeout_ms must be an Int from 1 to 86400000"),
        (PollMode::Eventually, vec![("timeout_ms", Int(MAX_MS + 1))], "timeout_ms must be an Int from 1 to 86400000"),
        (PollMode::Eventually, vec![("timeout_ms", Float(1.0))], "timeout_ms must be an Int from 1 to 86400000"),
        (PollMode::Retry, vec![("attempts", Int(0))], "attempts must be an Int from 1 to 1000000"),
        (PollMode::Retry, vec![("attempts", Int(MAX_ATTEMPTS + 1))], "attempts must be an Int from 1 to 1000000"),
        (PollMode::Eventually, vec![timeout.clone(), ("interval_ms", Int(-1))], "interval_ms must be an Int from 0 to 86400000"),
        (PollMode::Eventually, vec![timeout.clone(), ("max_interval_ms", Int(MAX_MS + 1))], "max_interval_ms must be an Int from 0 to 86400000"),
        (PollMode::Eventually, vec![timeout.clone(), ("interval_ms", Int(10)), ("max_interval_ms", Int(9))], "max_interval_ms must not be less than interval_ms"),
        (PollMode::Eventually, vec![timeout.clone(), ("backoff", Float(0.99))], "backoff must be an Int or Float from 1 to 10"),
        (PollMode::Eventually, vec![timeout.clone(), ("backoff", Int(11))], "backoff must be an Int or Float from 1 to 10"),
        (PollMode::Eventually, vec![timeout.clone(), ("backoff", Text("2".into()))], "backoff must be an Int or Float from 1 to 10"),
        (PollMode::Eventually, vec![timeout.clone(), ("retry_on", codes(&[]))], "retry_on must be a nonempty Array of diagnostic code Strings"),
        (PollMode::Eventually, vec![timeout.clone(), ("retry_on", Text("BW9001".into()))], "retry_on must be a nonempty Array of diagnostic code Strings"),
        (PollMode::Eventually, vec![timeout.clone(), ("retry_on", Literal::Array(vec![Int(9001)]))], "retry_on must contain BWnnnn diagnostic code Strings"),
        (PollMode::Eventually, vec![timeout.clone(), ("retry_on", codes(&["BW9003"]))], "retry_on must contain known BWnnnn diagnostic codes"),
        (PollMode::Eventually, vec![timeout.clone(), ("retry_on", codes(&["bw9001"]))], "retry_on must contain known BWnnnn diagnostic codes"),
        (PollMode::Retry, vec![("attempts", Int(2)), ("retry_on", codes(&["BW5001"]))], "retry_on cannot include BW5001 or BW8001; run cancellation and resource limits always stop"),
        (PollMode::Retry, vec![("attempts", Int(2)), ("retry_on", codes(&["BW8001"]))], "retry_on cannot include BW5001 or BW8001; run cancellation and resource limits always stop"),
        (PollMode::Retry, vec![("attempts", Int(2)), ("retry_on", codes(&["BW9001", "BW9001"]))], "retry_on must not repeat a diagnostic code"),
    ];
    for (mode, entries, message) in rejected {
        assert_eq!(parse(mode, &entries), Err(message), "{entries:?}");
    }
    assert_eq!(
        Policy::parse(PollMode::Retry, &Int(3)),
        Err("Eventually and Retry options must be a Map")
    );
    let lower = parse(
        PollMode::Eventually,
        &[
            timeout.clone(),
            ("attempts", Int(1)),
            ("interval_ms", Int(0)),
            ("max_interval_ms", Int(0)),
            ("backoff", Int(1)),
            ("retry_on", codes(&["BW5002", "BW9001"])),
        ],
    )
    .unwrap();
    assert_eq!(
        lower,
        Policy {
            mode: PollMode::Eventually,
            timeout_ms: Some(1),
            attempts: Some(1),
            interval_ms: 0,
            backoff: 1.0,
            max_interval_ms: 0,
            retry_on: Some(vec![Code::Timeout, Code::Assertion]),
        }
    );
    let upper = parse(
        PollMode::Retry,
        &[
            ("timeout_ms", Int(MAX_MS)),
            ("attempts", Int(MAX_ATTEMPTS)),
            ("interval_ms", Int(MAX_MS)),
            ("max_interval_ms", Int(MAX_MS)),
            ("backoff", Float(10.0)),
        ],
    )
    .unwrap();
    assert_eq!(
        (upper.timeout_ms, upper.attempts, upper.backoff),
        (Some(86_400_000), Some(1_000_000), 10.0)
    );
    let defaults = parse(PollMode::Retry, &[("attempts", Int(2))]).unwrap();
    assert_eq!(
        (
            defaults.timeout_ms,
            defaults.interval_ms,
            defaults.backoff,
            defaults.max_interval_ms,
            defaults.retry_on
        ),
        (None, 100, 1.0, 60_000, None)
    );
    let slow = parse(
        PollMode::Eventually,
        &[timeout, ("interval_ms", Int(90_000))],
    )
    .unwrap();
    assert_eq!(
        slow.max_interval_ms, 90_000,
        "the default cap never undercuts interval_ms"
    );
}

#[test]
fn delays_back_off_round_down_and_stay_capped() {
    let policy = |interval_ms, backoff, max_interval_ms| Policy {
        mode: PollMode::Eventually,
        timeout_ms: Some(1),
        attempts: None,
        interval_ms,
        backoff,
        max_interval_ms,
        retry_on: None,
    };
    let doubling = policy(100, 2.0, 350);
    assert_eq!(
        [1, 2, 3, 4].map(|failures| doubling.delay_ms(failures)),
        [100, 200, 350, 350]
    );
    let fractional = policy(100, 1.5, 1_000);
    assert_eq!(
        [1, 2, 3, 4].map(|failures| fractional.delay_ms(failures)),
        [100, 150, 225, 337]
    );
    assert_eq!(policy(7, 1.0, 60_000).delay_ms(u32::MAX), 7);
    assert_eq!(policy(0, 10.0, 60_000).delay_ms(u32::MAX), 0);
    assert_eq!(policy(1, 10.0, 86_400_000).delay_ms(u32::MAX), 86_400_000);
}

#[test]
fn observations_retry_timeouts_but_actions_require_explicit_opt_in() {
    let mut observation = parse(PollMode::Eventually, &[("timeout_ms", Literal::Int(1))]).unwrap();
    let mut action = parse(PollMode::Retry, &[("attempts", Literal::Int(2))]).unwrap();
    for code in Code::ALL {
        assert!(observation.retries(code), "{code}");
        assert_eq!(action.retries(code), code != Code::Timeout, "{code}");
    }
    observation.retry_on = Some(vec![Code::Assertion]);
    action.retry_on = Some(vec![Code::Timeout]);
    for code in Code::ALL {
        assert_eq!(observation.retries(code), code == Code::Assertion);
        assert_eq!(action.retries(code), code == Code::Timeout);
    }
}

#[test]
fn history_keeps_only_recent_records_as_exact_json() {
    let mut history = History::default();
    for attempt in 1..=HISTORY_RECORDS as u32 + 4 {
        history.attempts = attempt;
        history.push(Record {
            attempt,
            started_ms: u64::from(attempt) * 10,
            duration_ms: 1,
            outcome: Code::Assertion,
        });
    }
    assert_eq!(history.recent.len(), HISTORY_RECORDS);
    assert_eq!(history.recent.front().unwrap().attempt, 5);
    assert_eq!(history.recent.back().unwrap().attempt, 20);
    let pair = VecDeque::from([
        Record {
            attempt: 1,
            started_ms: 0,
            duration_ms: 2,
            outcome: Code::Timeout,
        },
        Record {
            attempt: 2,
            started_ms: u64::MAX,
            duration_ms: 0,
            outcome: Code::RetriesExhausted,
        },
    ]);
    assert_eq!(
        Json(&pair).to_string(),
        r#"[{"attempt":1,"started_ms":0,"duration_ms":2,"outcome":"BW5002"},{"attempt":2,"started_ms":18446744073709551615,"duration_ms":0,"outcome":"BW9005"}]"#
    );
    assert_eq!(Json(&VecDeque::new()).to_string(), "[]");
    assert_eq!(millis(Duration::MAX), u64::MAX);
}

#[test]
fn exhaustion_reasons_name_the_bound_and_last_failure() {
    let eventually = parse(PollMode::Eventually, &[("timeout_ms", Literal::Int(250))]).unwrap();
    let retry = parse(PollMode::Retry, &[("attempts", Literal::Int(3))]).unwrap();
    let text = |policy, why, attempts| {
        Reason {
            policy,
            why,
            attempts,
            last: Code::Assertion,
        }
        .to_string()
    };
    let suffix = "; the last attempt failed with BW9001, retained as the first cause";
    assert_eq!(
        text(&eventually, Exhausted::Deadline, 3),
        format!("no attempt succeeded within 250 ms (3 attempts; no further attempt could start before the deadline){suffix}")
    );
    assert_eq!(
        text(&eventually, Exhausted::Interrupted, 1),
        format!("no attempt succeeded within 250 ms (1 attempt; the deadline interrupted the last){suffix}")
    );
    assert_eq!(
        text(&eventually, Exhausted::Limit, 2),
        format!("no attempt succeeded in 2 attempts (the attempt limit){suffix}")
    );
    assert_eq!(
        text(&retry, Exhausted::Limit, 3),
        format!("the action failed in 3 attempts (the attempt limit){suffix}")
    );
}

#[test]
fn diagnostic_catalogue_is_complete_unique_and_parseable() {
    fn index(code: Code) -> usize {
        match code {
            Code::Syntax => 0,
            Code::InvalidControl => 1,
            Code::DuplicateParameter => 2,
            Code::Signature => 3,
            Code::UndefinedVariable => 4,
            Code::UndefinedStatement => 5,
            Code::DuplicateStatement => 6,
            Code::ParameterCount => 7,
            Code::InvalidNumber => 8,
            Code::Arithmetic => 9,
            Code::IncompatibleType => 10,
            Code::CollectionAccess => 11,
            Code::Output => 12,
            Code::Native => 13,
            Code::NativePanic => 14,
            Code::Cancelled => 15,
            Code::Timeout => 16,
            Code::AsyncRuntime => 17,
            Code::ImportRead => 18,
            Code::ImportCycle => 19,
            Code::DuplicateNamespace => 20,
            Code::Input => 21,
            Code::RunConfiguration => 22,
            Code::SourceRead => 23,
            Code::ResourceLimit => 24,
            Code::Assertion => 25,
            Code::ExplicitFailure => 26,
            Code::ConditionNotMet => 27,
            Code::RetriesExhausted => 28,
        }
    }
    for (position, code) in Code::ALL.into_iter().enumerate() {
        assert_eq!(index(code), position);
        assert_eq!(Code::parse(code.as_str()), Some(code));
    }
    assert_eq!(Code::parse("BW9003"), None, "reserved wire-only tag");
    assert_eq!(Code::parse(""), None);
}
