use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode as Code,
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    run::{Engine, RunLimits, RunOptions, RunOutcome, TemporaryLimits},
    value_limits::ValueLimits,
};

fn run(source: &str) -> botwork::core::run::RunResult {
    Engine::default().run_source("collections.botwork", source, RunOptions::default())
}

const SUCCESS: &str = r#"
|original| = |[1, {nested: [2, 3]}]|
|changed| = Set In |original| At |0| To |9|
Append To |original| Value |4|
Assert |original| Equals |[1, {nested: [2, 3]}]|
Assert |changed| Equals |[9, {nested: [2, 3]}]|
|changed| = Append To |changed| Value |4|
Assert |changed| Equals |[9, {nested: [2, 3]}, 4]|
|changed| = Remove From |changed| At |1|
Assert |changed| Equals |[9, 4]|
Assert |@{ Get From |original| At |1| }| Equals |{nested: [2, 3]}|
Assert |@{ Create Array }| Equals |[]|
Assert |@{ Create Map }| Equals |{}|
Assert |@{ Create Map From |[]| }| Equals |{}|
|map| = Create Map From |[["z", [1]], ["", false], ["a", 2]]|
|replacement| = Set In |map| At |"z"| To |[3]|
|replacement| = Set In |replacement| At |"b"| To |true|
Assert |map| Equals |{"z": [1], "": false, "a": 2}|
Assert |replacement| Equals |{"z": [3], "": false, "a": 2, "b": true}|
|replacement| = Remove From |replacement| At |"z"|
Assert |replacement| Equals |{"": false, "a": 2, "b": true}|
Assert |@{ Get From |map| At |""| }| Equals |false|
Assert |@{ Length Of |original| }| Equals |2|
Assert |@{ Length Of |map| }| Equals |3|
Assert |@{ Length Of |[]| }| Equals |0|
Assert |@{ Length Of |{}| }| Equals |0|
Assert |@{ Repeat |{a: [1]}| Times |2| }| Equals |[{a: [1]}, {a: [1]}]|
Assert |@{ Repeat |[1]| Times |0| }| Equals |[]|
Assert |@{ Repeat |@{ No Operation }| Times |1| }| Equals |[@{ No Operation }]|
Assert |@{ Slice |[1, 2, 3]| From |1| To |3| }| Equals |[2, 3]|
Assert |@{ Slice |[1, 2, 3]| From |0| To |2| }| Equals |[1, 2]|
Assert |@{ Slice |[1, 2]| From |2| To |2| }| Equals |[]|
Assert |@{ Slice |[]| From |0| To |0| }| Equals |[]|
Assert |@{ Remove From |[1]| At |0| }| Equals |[]|
Assert |@{ Remove From |{a: 1}| At |"a"| }| Equals |{}|
Assert |@{ Append To |[]| Value |1| }| Equals |[1]|
Assert |@{ Set In |{}| At |""| To |1| }| Equals |{"": 1}|
|iterated| = |0|
For |entry| In |@{ Enumerate |[5, 7]| }| {
    Assert |entry.1| Equals |@{ Get From |[5, 7]| At |entry.0| }|
    |iterated| = |iterated + 1|
}
Assert |iterated| Equals |2|
Assert |@{ Enumerate |[5, 7]| }| Equals |[[0, 5], [1, 7]]|
Assert |@{ Enumerate |[]| }| Equals |[]|
Assert |@{ Map Keys |{}| }| Equals |[]|
Assert |@{ Map Values |{}| }| Equals |[]|
Assert |@{ Map Entries |{}| }| Equals |[]|
"#;

#[test]
fn collection_statements_return_owned_replacements_and_support_empty_boundaries() {
    let result = run(SUCCESS);
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[tokio::test]
async fn pure_collection_statements_share_the_async_evaluator_contract() {
    let result = Engine::default()
        .run_source_async("async-collections", SUCCESS, RunOptions::default())
        .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn map_iteration_orders_exact_unicode_keys_and_preserves_key_value_pairing() {
    let result = run(r#"
|m| = |{"🙂": 6, "é": 5, "é": 4, "a": 3, "A": 2, "": 1}|
Assert |@{ Map Keys |m| }| Equals |["", "A", "a", "é", "é", "🙂"]|
Assert |@{ Map Values |m| }| Equals |[1, 2, 3, 4, 5, 6]|
Assert |@{ Map Entries |m| }| Equals |[["", 1], ["A", 2], ["a", 3], ["é", 4], ["é", 5], ["🙂", 6]]|
Assert |@{ Create Map From |@{ Map Entries |m| }| }| Equals |m|
For |entry| In |@{ Map Entries |m| }| {
    Assert |@{ Get From |m| At |entry.0| }| Equals |entry.1|
}
"#);
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn membership_and_comparison_use_deep_exact_language_equality() {
    let result = run(r#"
Assert |@{ Collection Contains |[1, {a: [2]}, @{ No Operation }]| Item |{a: [2.0]}| }|
Assert |@{ Collection Contains |[1, @{ No Operation }]| Item |@{ No Operation }| }|
Assert |@{ Collection Contains |[16777216.0]| Item |16777217| }| Equals |false|
Assert |@{ Collection Contains |[]| Item |1| }| Equals |false|
Assert |@{ Collection Contains |{a: 1}| Item |"a"| }|
Assert |@{ Collection Contains |{a: "b"}| Item |"b"| }| Equals |false|
Assert |@{ Collection Contains |{"é": 1}| Item |"é"| }| Equals |false|
Assert |@{ Collections Equal |[{a: 2}, 1]| And |[{a: 2.0}, 1.0]| }|
Assert |@{ Collections Equal |{a: 1, b: 2}| And |{b: 2, a: 1}| }|
Assert |@{ Collections Equal |[16777217]| And |[16777216.0]| }| Equals |false|
Assert |@{ Collections Equal |[1, 2]| And |[2, 1]| }| Equals |false|
Assert |@{ Collections Equal |[]| And |{}| }| Equals |false|
Assert |@{ Collections Equal |{a: 1}| And |{A: 1}| }| Equals |false|
"#);
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn invalid_parameter_kinds_counts_and_entry_shapes_are_typed_failures() {
    for source in [
        "Create Map From |{}|",
        "Create Map From |[1]|",
        "Create Map From |[[]]|",
        "Create Map From |[[\"x\"]]|",
        "Create Map From |[[\"x\", 1, 2]]|",
        "Create Map From |[[1, 2]]|",
        "Create Map From |[[\"x\", 1], [\"x\", 2]]|",
        "Repeat |1| Times |-1|",
        "Repeat |1| Times |1.0|",
        "Repeat |1| Times |true|",
        "Get From |1| At |0|",
        "Set In |1| At |0| To |2|",
        "Remove From |1| At |0|",
        "Append To |{}| Value |1|",
        "Collection Contains |1| Item |1|",
        "Length Of |\"abc\"|",
        "Map Keys |[]|",
        "Map Values |[]|",
        "Map Entries |[]|",
        "Enumerate |{}|",
        "Collections Equal |1| And |[]|",
        "Collections Equal |[]| And |1|",
        "Slice |{}| From |0| To |0|",
        "Slice |[]| From |0.0| To |0|",
        "Slice |[]| From |0| To |false|",
    ] {
        let error = run(source).result.unwrap_err();
        assert_eq!(error.code(), Code::IncompatibleType, "{source}: {error}");
        assert_eq!(
            error.span.as_ref().unwrap().source().name(),
            "collections.botwork"
        );
    }
}

#[test]
fn invalid_access_uses_strict_indexes_existing_keys_and_half_open_ranges() {
    for source in [
        "Get From |[1]| At |-1|",
        "Get From |[1]| At |1|",
        "Get From |[]| At |0|",
        "Get From |[1]| At |\"0\"|",
        "Get From |[1]| At |0.0|",
        "Get From |[1]| At |false|",
        "Get From |[1]| At |[]|",
        "Get From |[1]| At |{}|",
        "Get From |{}| At |\"x\"|",
        "Get From |{a: 1}| At |0|",
        "Set In |[]| At |0| To |1|",
        "Set In |[1]| At |1| To |2|",
        "Set In |{}| At |1| To |2|",
        "Remove From |[]| At |0|",
        "Remove From |{}| At |\"x\"|",
        "Collection Contains |{}| Item |1|",
        "Slice |[1]| From |-1| To |1|",
        "Slice |[1]| From |0| To |2|",
        "Slice |[1]| From |1| To |0|",
    ] {
        let error = run(source).result.unwrap_err();
        assert_eq!(error.code(), Code::CollectionAccess, "{source}: {error}");
        assert_eq!(error.call_stack.len(), 1);
        assert_eq!(
            error.span.as_ref().unwrap().source().name(),
            "collections.botwork"
        );
    }
}

#[test]
fn failed_replacements_preserve_destinations_and_are_catchable_with_cleanup() {
    let result = run(r#"
|destination| = |[1]|
Try { |destination| = Set In |destination| At |1| To |2| }
Catch |error| { Assert |error.code| Equals |"BW3004"| }
Finally { |cleaned| = |true| }
Assert |destination| Equals |[1]|
Assert |cleaned|
"#);
    assert!(result.result.is_ok(), "{:?}", result.result);
    let failure = run("|destination| = |[1]|\n|destination| = Remove From |destination| At |1|");
    assert!(
        matches!(failure.variables["destination"], Literal::Array(ref v) if matches!(v[..], [Literal::Int(1)]))
    );
    for source in [
        "Log |\"effect\"|\n|a[0]| = |2|",
        "Log |\"effect\"|\n|a.x| = |2|",
    ] {
        assert!(Program::parse("indexed-assignment", source).is_err());
    }
}

#[test]
fn replacement_plans_count_only_selected_children_and_hold_arguments_until_result_admission() {
    for (source, payload) in [
        ("Set In |[\"12345678\"]| At |0| To |\"x\"|", 14),
        ("Remove From |[\"12345678\"]| At |0|", 12),
        ("Set In |{x: \"12345678\"}| At |\"x\"| To |\"y\"|", 13),
        ("Remove From |{x: \"12345678\"}| At |\"x\"|", 10),
        ("Slice |[\"12345678\"]| From |0| To |0|", 16),
    ] {
        for allowed in [false, true] {
            let options = RunOptions {
                limits: RunLimits {
                    temporaries: TemporaryLimits {
                        payload_bytes: payload - usize::from(!allowed),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            };
            let result = Engine::default().run_source("replacement", source, options);
            assert_eq!(
                result.result.is_ok(),
                allowed,
                "{source}: {:?}",
                result.result
            );
            if !allowed {
                assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
            }
        }
    }
}

#[test]
fn result_limits_reject_expansion_and_key_to_string_conversion() {
    for (source, values) in [
        ("Repeat |1| Times |2147483647|", ValueLimits::default()),
        (
            "Append To |[1]| Value |2|",
            ValueLimits {
                entries: 1,
                ..Default::default()
            },
        ),
        (
            "Set In |{a: 1}| At |\"b\"| To |2|",
            ValueLimits {
                entries: 1,
                ..Default::default()
            },
        ),
        (
            "Repeat |[1]| Times |2|",
            ValueLimits {
                nodes: 4,
                ..Default::default()
            },
        ),
        (
            "Repeat |[1]| Times |1|",
            ValueLimits {
                depth: 2,
                ..Default::default()
            },
        ),
        (
            "Repeat |\"1234\"| Times |3|",
            ValueLimits {
                payload_bytes: 8,
                ..Default::default()
            },
        ),
        (
            "Map Keys |{abcd: 1}|",
            ValueLimits {
                string_bytes: 3,
                ..Default::default()
            },
        ),
        (
            "Map Entries |{abcd: 1}|",
            ValueLimits {
                string_bytes: 3,
                ..Default::default()
            },
        ),
        (
            "Create Map From |[[\"abcd\", 1]]|",
            ValueLimits {
                key_bytes: 3,
                ..Default::default()
            },
        ),
        (
            "Enumerate |[1]|",
            ValueLimits {
                depth: 2,
                ..Default::default()
            },
        ),
        (
            "Map Entries |{a: 1}|",
            ValueLimits {
                depth: 2,
                ..Default::default()
            },
        ),
    ] {
        let options = RunOptions {
            limits: RunLimits {
                values,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = Engine::default().run_source("quota", source, options);
        assert_eq!(
            result.outcome(),
            RunOutcome::LimitExceeded,
            "{source}: {:?}",
            result.result
        );
    }
}

#[test]
fn collection_initialization_is_idempotent_and_preserves_host_overrides() {
    let mut context = Context::default();
    context
        .register_native("Create Array", |_| Ok(Literal::Int(42)))
        .unwrap();
    context.init_statements();
    context.init_statements();
    assert_eq!(context.statement_signatures().len(), 89);
    let value = evaluate_program_detailed(
        &Program::parse("override", "Create Array").unwrap(),
        &mut context,
    )
    .unwrap();
    assert!(matches!(value, Literal::Int(42)));
    let result = run("Local { Create Array { Return |42| }\nReturn |@{ Create Array }| }\nAssert |@{ Local }| Equals |42|");
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn arguments_remain_eager_and_fail_in_source_order() {
    for source in [
        "Repeat |@{ Fail |\"first\"| }| Times |0|",
        "Set In |[]| At |0| To |@{ Fail |\"first\"| }|",
        "Create Map From |[[1, @{ Fail |\"first\"| }]]|",
    ] {
        assert_eq!(
            run(source).result.unwrap_err().code(),
            Code::ExplicitFailure
        );
    }
}

#[test]
fn help_metadata_describes_typed_collection_inputs_results_and_errors() {
    let mut context = Context::default();
    context.init_statements();
    for (header, expected) in [
        (
            "Repeat |x| Times |n|",
            vec!["value: Any", "count: Int", "returns: Array", "BW3003"],
        ),
        (
            "Set In |a| At |k| To |v|",
            vec![
                "collection: Array | Map",
                "key: Any",
                "returns: Array | Map",
                "BW3004",
            ],
        ),
        (
            "Length Of |a|",
            vec!["collection: Array | Map", "returns: Int", "BW3002"],
        ),
        (
            "Slice |a| From |b| To |c|",
            vec![
                "array: Array",
                "start: Int",
                "end: Int",
                "returns: Array",
                "BW3004",
            ],
        ),
        ("Get From |a| At |k|", vec!["returns: Any", "BW3004"]),
    ] {
        let help = context.statement_signature(header).unwrap().unwrap().help();
        for fragment in expected {
            assert!(help.contains(fragment), "{header}: {fragment}: {help}");
        }
    }
}
