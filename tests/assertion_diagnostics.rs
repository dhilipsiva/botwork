use botwork::core::{
    acceptance::FailureKind,
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLimits, DiagnosticValueLimits},
    grammar::{BWErr, Literal},
    run::{Engine, RunOptions},
    worker::protocol::WorkerProtocol,
};
use serde_json::{json, Value};

fn failure(source: &str, options: RunOptions) -> Diagnostic {
    Engine::default()
        .run_source("assertions.botwork", source, options)
        .result
        .unwrap_err()
}
fn details(diagnostic: &Diagnostic) -> (&str, Value, Value) {
    let BWErr::AssertionMismatch {
        reason,
        actual,
        expected,
    } = &*diagnostic.error
    else {
        panic!("structured assertion expected: {diagnostic}")
    };
    (
        reason,
        serde_json::from_str(actual).unwrap(),
        serde_json::from_str(expected).unwrap(),
    )
}

#[test]
fn nested_string_difference_names_the_key_index_and_unicode_scalar() {
    let error = failure(
        r#"Assert |{items: ["same", "café"]}| Equals |{items: ["same", "café"]}|"#,
        RunOptions::default(),
    );
    let (reason, actual, expected) = details(&error);
    assert!(reason.contains("difference at $[\"items\"][1]"), "{reason}");
    assert!(
        reason.contains("Unicode scalar index 3: expected 'e' (U+0065), got 'é' (U+00E9)"),
        "{reason}"
    );
    assert_eq!(actual["value"]["items"]["value"][1]["value"], "café");
    assert_eq!(expected["value"]["items"]["value"][1]["value"], "café");
    assert_eq!(error.code(), DiagnosticCode::Assertion);
    assert_eq!(FailureKind::from_diagnostic(&error), FailureKind::Assertion);
    assert_eq!(error.span.as_ref().unwrap().line_column(), (1, 1));
    assert_eq!(error.call_stack.len(), 1);
}

#[test]
fn missing_entries_differ_from_none_and_array_lengths_are_visible() {
    for (source, path, message) in [
        (
            r#"Assert |{}| Equals |{x: @{ No Operation }}|"#,
            "$[\"x\"]",
            "expected none (None), got <missing>",
        ),
        (
            r#"Assert |{x: false}| Equals |{}|"#,
            "$[\"x\"]",
            "expected <missing>, got false (Bool)",
        ),
        (
            "Assert |[1]| Equals |[1, 2]|",
            "$[1]",
            "array lengths: expected 2, got 1",
        ),
        (
            "Assert |[1, 2]| Equals |[1]|",
            "$[1]",
            "expected <missing>, got 2 (Int)",
        ),
    ] {
        let error = failure(source, RunOptions::default());
        let reason = details(&error).0;
        assert!(
            reason.contains(path) && reason.contains(message),
            "{reason}"
        );
    }
}

#[test]
fn map_differences_choose_the_first_sorted_differing_key() {
    for _ in 0..30 {
        let error = failure(
            r#"Assert |{z: false, b: [1, 4], a: 1}| Equals |{b: [1.0, 3], a: 1.0, z: true}|"#,
            RunOptions::default(),
        );
        assert!(details(&error).0.contains("difference at $[\"b\"][1]"));
    }
}

#[test]
fn numeric_and_nested_operand_kinds_survive_full_artifacts() {
    let error = failure(
        "Assert |[16777217, -0.0, @{ No Operation }]| Equals |[16777216.0, 0, false]|",
        RunOptions::default(),
    );
    let (reason, actual, expected) = details(&error);
    assert!(reason.contains("difference at $[0]"));
    assert_eq!(actual["value"][0], json!({"kind":"Int","value":16777217}));
    assert_eq!(
        expected["value"][0],
        json!({"kind":"Float","value":16777216.0})
    );
    assert!(actual["value"][1]["value"]
        .as_f64()
        .unwrap()
        .is_sign_negative());
    assert_eq!(actual["value"][2], json!({"kind":"None","value":null}));
}

#[test]
fn long_values_have_bounded_previews_and_complete_permitted_operands() {
    let prefix = "🙂".repeat(25_000);
    let actual = format!("{prefix}a");
    let expected = format!("{prefix}b");
    let mut options = RunOptions::default();
    options
        .variables
        .insert("actual".into(), Literal::String(actual.clone()));
    options
        .variables
        .insert("expected".into(), Literal::String(expected.clone()));
    let error = failure("Assert |actual| Equals |expected|", options);
    let (reason, a, b) = details(&error);
    assert!(reason.len() < 1600, "{}", reason.len());
    assert!(reason.contains("…[truncated]"));
    assert!(reason.contains("Unicode scalar index 25000"));
    assert_eq!(a["value"], actual);
    assert_eq!(b["value"], expected);
    assert!(error.omissions.is_none());
    assert_eq!(FailureKind::from_diagnostic(&error), FailureKind::Assertion);
    let mut limits = DiagnosticValueLimits::default();
    limits.values.string_bytes = 1024;
    assert!(error.to_value_with_limits(&limits).is_err());
    assert_eq!(
        details(&error).1["value"],
        actual,
        "conversion rejection preserves the diagnostic"
    );
}

#[test]
fn invisible_characters_and_string_endings_are_explicit() {
    let actual = "hello\n\0\u{1b}\"\\";
    let mut options = RunOptions::default();
    options
        .variables
        .insert("actual".into(), Literal::String(actual.into()));
    let error = failure("Assert |actual| Equals |\"hello\"|", options);
    let (reason, value, _) = details(&error);
    assert!(
        reason.contains("Unicode scalar index 5: expected <end of string>, got '\\n' (U+000A)"),
        "{reason}"
    );
    assert!(!reason.contains('\0') && !reason.contains('\u{1b}'));
    assert_eq!(value["value"], actual);
}

#[test]
fn long_map_keys_truncate_the_path_without_hiding_the_differing_values() {
    let key = "key\"\n🙂".repeat(100);
    let mut options = RunOptions::default();
    options.variables.insert(
        "actual".into(),
        Literal::Map([(key.clone(), Literal::Int(1))].into()),
    );
    options.variables.insert(
        "expected".into(),
        Literal::Map([(key.clone(), Literal::Int(2))].into()),
    );
    let error = failure("Assert |actual| Equals |expected|", options);
    let (reason, actual, _) = details(&error);
    assert!(
        reason.contains("…[truncated]: expected 2 (Int), got 1 (Int)"),
        "{reason}"
    );
    assert_eq!(actual["value"][&key]["value"], 1);
}

#[test]
fn diagnostic_budgets_charge_full_operands_and_reject_without_a_clean_assertion_verdict() {
    let source = "Assert |[1, 2]| Equals |[1, 3]|";
    let baseline = failure(source, RunOptions::default());
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    for retained in [false, true] {
        for allowed in [false, true] {
            let mut options = RunOptions::default();
            let bytes = size.text_bytes - usize::from(!allowed);
            if retained {
                // The active call and its copied diagnostic frame overlap during construction.
                options.limits.retained_diagnostics.text_bytes =
                    bytes + baseline.call_stack[0].signature.len();
            } else {
                options.limits.diagnostics.text_bytes = bytes;
            }
            let error = failure(source, options);
            if allowed {
                assert_eq!(details(&error).1, details(&baseline).1);
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(
                    FailureKind::from_diagnostic(&error),
                    FailureKind::LimitExceeded
                );
                assert_eq!(error.causes[0].code(), DiagnosticCode::Assertion);
                assert!(error.causes[0].omissions.is_some());
            }
        }
    }
}

#[test]
fn catch_and_rethrow_keep_operand_artifacts_and_original_location() {
    let result = Engine::default().run_source(
        "caught.botwork",
        r#"Try {
    Assert |false|
} Catch |error| {
    |saved| = |error.details|
    Rethrow
}"#,
        RunOptions::default(),
    );
    let Literal::Map(saved) = &result.variables["saved"] else {
        panic!("saved details")
    };
    assert_eq!(saved["reason"].to_string(), "Expected true, got false");
    assert_eq!(
        serde_json::from_str::<Value>(&saved["expected"].to_string()).unwrap(),
        json!({"kind":"Bool","value":true})
    );
    let error = result.result.unwrap_err();
    assert_eq!(error.span.as_ref().unwrap().line_column(), (2, 5));
    assert_eq!(details(&error).1, json!({"kind":"Bool","value":false}));
    assert!(error.causes.is_empty());
}

#[tokio::test]
async fn sync_async_and_worker_protocol_preserve_the_same_evidence() {
    let source = "Assert |[1, false]| Equals |[1.0, true]|";
    let sync = failure(source, RunOptions::default());
    let asynchronous = Engine::default()
        .run_source_async("assertions.botwork", source, RunOptions::default())
        .await
        .result
        .unwrap_err();
    assert_eq!(details(&sync), details(&asynchronous));
    let protocol = WorkerProtocol::default();
    for error in [
        sync,
        Diagnostic::new(BWErr::AssertionFailed("legacy".into())),
    ] {
        let encoded = protocol.encode_response(Err(&error)).unwrap();
        let decoded = protocol.decode_response(&encoded).unwrap().unwrap_err();
        assert_eq!(decoded.code(), DiagnosticCode::Assertion);
        assert_eq!(format!("{:?}", decoded.error), format!("{:?}", error.error));
        assert_eq!(encoded, protocol.encode_response(Err(&decoded)).unwrap());
    }
}
