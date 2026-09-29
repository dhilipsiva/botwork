use botwork::core::{
    diagnostic::DiagnosticCode as Code,
    grammar::Literal,
    run::{Engine, RunLimits, RunOptions, RunOutcome, TemporaryLimits},
    value_limits::ValueLimits,
};

fn run(source: &str) -> botwork::core::run::RunResult {
    Engine::default().run_source("datetime.botwork", source, RunOptions::default())
}

const SUCCESS: &str = r#"
Assert |@{ Parse Date Time |"2040-02-29T05:45:00.120000000+05:45"| }| Equals |"2040-02-29T00:00:00.12Z"|
Assert |@{ Parse Date Time |"0000-02-29T00:00:00Z"| }| Equals |"0000-02-29T00:00:00Z"|
Assert |@{ Convert Date Time |"2024-01-01T00:00:00Z"| To |"Asia/Kathmandu"| }| Equals |"2024-01-01T05:45:00+05:45"|
Assert |@{ Convert Date Time |"2024-01-01T00:00:00Z"| To |"-03:30"| }| Equals |"2023-12-31T20:30:00-03:30"|
Assert |@{ Format Date Time |"2024-07-01T00:00:00Z"| Using |"%F %T %Z %:z %% %s"| In |"America/New_York"| }| Equals |"2024-06-30 20:00:00 EDT -04:00 % 1719792000"|
Assert |@{ Format Date Time |"2024-01-01T00:00:00Z"| Using |""| In |"Z"| }| Equals |""|
Assert |@{ Parse Date Time |"2024-02-29 12:34:56.123456789"| Using |"%F %T%.9f"| In |"+05:30"| Choosing |"reject"| }| Equals |"2024-02-29T07:04:56.123456789Z"|
Assert |@{ Parse Date Time |"2024-11-03 01:30:00"| Using |"%F %T"| In |"America/New_York"| Choosing |"earlier"| }| Equals |"2024-11-03T05:30:00Z"|
Assert |@{ Parse Date Time |"2024-11-03 01:30:00"| Using |"%F %T"| In |"America/New_York"| Choosing |"later"| }| Equals |"2024-11-03T06:30:00Z"|
Assert |@{ Parse Date Time |"2024-04-07 01:45:00"| Using |"%F %T"| In |"Australia/Lord_Howe"| Choosing |"later"| }| Equals |"2024-04-06T15:15:00Z"|
Assert |@{ Add Duration |"P1D"| To Date Time |"2024-03-09T12:00:00-05:00"| }| Equals |"2024-03-10T17:00:00Z"|
Assert |@{ Convert Date Time |"2024-03-10T17:00:00Z"| To |"America/New_York"| }| Equals |"2024-03-10T13:00:00-04:00"|
Assert |@{ Add Duration |"PT0.000000001S"| To Date Time |"1969-12-31T23:59:59.999999999Z"| }| Equals |"1970-01-01T00:00:00Z"|
Assert |@{ Subtract Duration |"PT0.000000001S"| From Date Time |"1970-01-01T00:00:00Z"| }| Equals |"1969-12-31T23:59:59.999999999Z"|
Assert |@{ Difference Between Date Times |"1970-01-01T00:00:00Z"| And |"1970-01-02T00:00:00.000000001Z"| }| Equals |"-P1DT0.000000001S"|
Assert |@{ Compare Date Times |"2024-01-01T05:30:00+05:30"| And |"2024-01-01T00:00:00Z"| }| Equals |0|
Assert |@{ Compare Date Times |"0000-01-01T00:00:00Z"| And |"9999-12-31T23:59:59Z"| }| Equals |-1|
Assert |@{ Compare Date Times |"9999-12-31T23:59:59Z"| And |"0000-01-01T00:00:00Z"| }| Equals |1|
Assert |@{ Parse Duration |"+P1DT25H90M60.1200S"| }| Equals |"P2DT2H31M0.12S"|
Assert |@{ Parse Duration |"P2W"| }| Equals |"P14D"|
Assert |@{ Parse Duration |"-PT0.000S"| }| Equals |"PT0S"|
Assert |@{ Create Duration |-1| In |"microseconds"| }| Equals |"-PT0.000001S"|
Assert |@{ Duration Seconds |"-PT0.000000001S"| }| Equals |"-0.000000001"|
Assert |@{ Duration Nanoseconds |"P1D"| }| Equals |"86400000000000"|
Assert |@{ Add Durations |"PT1S"| And |"-PT0.000000001S"| }| Equals |"PT0.999999999S"|
Assert |@{ Subtract Durations |"PT1S"| Minus |"PT2S"| }| Equals |"-PT1S"|
Assert |@{ Compare Durations |"P1W"| And |"P7D"| }| Equals |0|
Assert |@{ Compare Durations |"-PT1S"| And |"PT0S"| }| Equals |-1|
Assert |@{ Compare Durations |"PT0S"| And |"-PT1S"| }| Equals |1|
"#;

#[test]
fn timestamps_and_durations_preserve_exact_instants_and_explicit_zones() {
    let result = run(SUCCESS);
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[tokio::test]
async fn date_time_catalogue_has_same_async_results() {
    let result = Engine::default()
        .run_source_async("async", SUCCESS, RunOptions::default())
        .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn invalid_inputs_reject_missing_offsets_precision_loss_and_calendar_durations() {
    for timestamp in [
        "2024-01-01",
        "2024-01-01T00:00:00",
        "2024-01-01 00:00:00Z",
        "2024-01-01t00:00:00z",
        "2024-01-01T00:00:00-00:00",
        "2024-01-01T00:00:00+24:00",
        "2024-01-01T00:00:00+00:60",
        "2024-01-01T00:00:00+0000",
        "2024-01-01T00:00:00.Z",
        "2024-01-01T00:00:00.1234567890Z",
        "2024-02-30T00:00:00Z",
        "2023-02-29T00:00:00Z",
        "2016-12-31T23:59:60Z",
        "2024-01-01T00:00:00🙂",
        "🙂",
        "",
    ] {
        assert_invalid(&format!("Parse Date Time |{timestamp:?}|"));
    }
    for duration in [
        "",
        "P",
        "PT",
        "P1DT",
        "PT1H1H",
        "PT1S1M",
        "P1Y",
        "P1M",
        "P1W1D",
        "PT1D",
        "P1.5D",
        "PT.5S",
        "PT1.S",
        "PT1.0000000000S",
        "PT1,5S",
        "PT-1S",
        "P1🙂",
        "pt1s",
        " PT1S",
    ] {
        assert_invalid(&format!("Parse Duration |{duration:?}|"));
    }
    for source in [
        "Parse Date Time |1|",
        "Create Duration |1.5| In |\"seconds\"|",
        "Create Duration |1| In |\"second\"|",
        "Compare Durations |\"PT1S\"| And |1|",
        "Convert Date Time |\"2024-01-01T00:00:00Z\"| To |\"local\"|",
        "Convert Date Time |\"2024-01-01T00:00:00Z\"| To |\"america/new_york\"|",
    ] {
        assert_invalid(source);
    }
}

fn assert_invalid(source: &str) {
    let error = run(source).result.unwrap_err();
    assert_eq!(error.code(), Code::IncompatibleType, "{source}: {error}");
    assert_eq!(
        error.span.as_ref().unwrap().source().name(),
        "datetime.botwork"
    );
}

#[test]
fn local_times_require_valid_choices_and_reject_gaps_or_implicit_zone_fields() {
    for (text, format, zone, choice) in [
        (
            "2024-03-10 02:30:00",
            "%F %T",
            "America/New_York",
            "earlier",
        ),
        ("2024-03-10 02:30:00", "%F %T", "America/New_York", "later"),
        ("2024-11-03 01:30:00", "%F %T", "America/New_York", "reject"),
        (
            "2024-10-06 02:15:00",
            "%F %T",
            "Australia/Lord_Howe",
            "reject",
        ),
        ("2024-01-01 00:00:00", "%F %T", "UTC", "EARLIER"),
        ("2024-01-01 00:00:00 +0100", "%F %T %z", "UTC", "reject"),
        ("2024-01-01 00:00:00 UTC", "%F %T %Z", "UTC", "reject"),
        ("0", "%s", "UTC", "reject"),
        ("2024-01-01T00:00:00Z", "%+", "UTC", "reject"),
        (
            "2024-01-01 00:00:00.1234567890",
            "%F %T%.f",
            "UTC",
            "reject",
        ),
        (
            "2024-01-01 00:00:00.1234567890",
            "%F %T%.9f",
            "UTC",
            "reject",
        ),
        ("2016-12-31 23:59:60", "%F %T", "UTC", "reject"),
        ("2024-01-01", "%F", "UTC", "reject"),
        ("", "%Q", "UTC", "reject"),
    ] {
        assert_invalid(&format!(
            "Parse Date Time |{text:?}| Using |{format:?}| In |{zone:?}| Choosing |{choice:?}|"
        ));
    }
    for format in ["%", "%Q", "%::::z"] {
        assert_invalid(&format!(
            "Format Date Time |\"2024-01-01T00:00:00Z\"| Using |{format:?}| In |\"UTC\"|"
        ));
    }
}

#[test]
fn historic_second_offsets_are_preserved_or_explicitly_rejected() {
    let result = run(r#"
Assert |@{ Format Date Time |"1900-01-01T00:00:00Z"| Using |"%F %T %::z"| In |"Europe/Paris"| }| Equals |"1900-01-01 00:09:21 +00:09:21"|
Assert |@{ Parse Date Time |"1900-01-01 00:09:21"| Using |"%F %T"| In |"Europe/Paris"| Choosing |"reject"| }| Equals |"1900-01-01T00:00:00Z"|
"#);
    assert!(result.result.is_ok(), "{:?}", result.result);
    let error = run("Convert Date Time |\"1900-01-01T00:00:00Z\"| To |\"Europe/Paris\"|")
        .result
        .unwrap_err();
    assert_eq!(error.code(), Code::Arithmetic);
}

#[test]
fn checked_arithmetic_rejects_duration_and_timestamp_overflow() {
    for source in [
        "Add Durations |\"PT170141183460469231731687303715.884105727S\"| And |\"PT0.000000001S\"|",
        "Subtract Durations |\"-PT170141183460469231731687303715.884105728S\"| Minus |\"PT0.000000001S\"|",
        "Parse Duration |\"PT170141183460469231731687303715.884105728S\"|",
        "Add Duration |\"PT0.000000001S\"| To Date Time |\"9999-12-31T23:59:59.999999999Z\"|",
        "Subtract Duration |\"PT0.000000001S\"| From Date Time |\"0000-01-01T00:00:00Z\"|",
        "Convert Date Time |\"9999-12-31T23:59:59Z\"| To |\"+01:00\"|",
        "Parse Date Time |\"0000-01-01T00:00:00+00:01\"|",
        "Convert Date Time |\"2100-01-01T00:00:00Z\"| To |\"America/New_York\"|",
        "Convert Date Time |\"1799-12-31T23:59:59Z\"| To |\"Europe/Paris\"|",
        "Parse Date Time |\"2100-01-01 00:00:00\"| Using |\"%F %T\"| In |\"Europe/Paris\"| Choosing |\"reject\"|",
    ] {
        let error = run(source).result.unwrap_err();
        assert_eq!(error.code(), Code::Arithmetic, "{source}: {error}");
    }
}

#[test]
fn all_duration_units_and_large_exact_counts_are_usable() {
    for (unit, nanos) in [
        ("weeks", 604800000000000i128),
        ("days", 86400000000000),
        ("hours", 3600000000000),
        ("minutes", 60000000000),
        ("seconds", 1000000000),
        ("milliseconds", 1000000),
        ("microseconds", 1000),
        ("nanoseconds", 1),
    ] {
        let source = format!("Assert |@{{ Duration Nanoseconds |@{{ Create Duration |2147483647| In |{unit:?}| }}| }}| Equals |\"{}\"|", nanos * 2147483647);
        let result = run(&source);
        assert!(result.result.is_ok(), "{:?}", result.result);
    }
}

#[test]
fn current_time_is_explicit_offset_wall_time_with_nanosecond_precision() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i128;
    let result = run("|now| = |@{ Current Date Time In |\"+05:45\"| }|");
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i128;
    assert!(result.result.is_ok(), "{:?}", result.result);
    let Literal::String(value) = &result.variables["now"] else {
        panic!()
    };
    let date = chrono::DateTime::parse_from_rfc3339(value).unwrap();
    let actual =
        i128::from(date.timestamp()) * 1_000_000_000 + i128::from(date.timestamp_subsec_nanos());
    assert!((before..=after).contains(&actual));
    assert_eq!(date.offset().local_minus_utc(), 20700);
}

#[test]
fn failed_assignment_is_catchable_and_keeps_previous_value_and_call_context() {
    let result = run(r#"
|value| = |"old"|
Try { |value| = |@{ Parse Date Time |"invalid"| }| } Catch |error| {
    Assert |error.code| Equals |"BW3003"|
}
Assert |value| Equals |"old"|
"#);
    assert!(result.result.is_ok(), "{:?}", result.result);
    let error = run("Parse Duration |\"P1Y\"|").result.unwrap_err();
    assert_eq!(error.call_stack.len(), 1);
}

#[test]
fn formatting_admits_output_bytes_and_live_argument_overlap() {
    let source =
        "Format Date Time |\"2024-01-01T00:00:00Z\"| Using |\"%Y%Y%Y%Y%Y%Y\"| In |\"UTC\"|";
    for (strings, temporaries, expected) in [
        (23, usize::MAX, false),
        (24, usize::MAX, true),
        (24, 58, false),
        (24, 59, true),
    ] {
        let result = Engine::default().run_source(
            "budget",
            source,
            RunOptions {
                limits: RunLimits {
                    values: ValueLimits {
                        string_bytes: strings,
                        ..Default::default()
                    },
                    temporaries: TemporaryLimits {
                        payload_bytes: temporaries,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(
            result.result.is_ok(),
            expected,
            "{strings}/{temporaries}: {:?}",
            result.result
        );
        if !expected {
            assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        }
    }
}

#[test]
fn metadata_is_typed_idempotent_and_respects_host_overrides() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let mut context = Context::default();
    context
        .register_native("Parse Duration |text|", |_| Ok(Literal::Int(42)))
        .unwrap();
    context.init_statements();
    context.init_statements();
    assert_eq!(context.statement_signatures().len(), 100);
    let signature = context
        .statement_signature("Create Duration |n| In |u|")
        .unwrap()
        .unwrap();
    let help = signature.help();
    for fragment in [
        "amount: Int",
        "unit: String",
        "returns: String",
        "BW3002",
        "BW3003",
    ] {
        assert!(help.contains(fragment), "{help}");
    }
    let result = evaluate_program_detailed(
        &Program::parse("override", "Parse Duration |\"ignored\"|").unwrap(),
        &mut context,
    )
    .unwrap();
    assert!(matches!(result, Literal::Int(42)));
}
