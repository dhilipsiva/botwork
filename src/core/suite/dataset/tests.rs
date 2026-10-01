use super::*;
use crate::core::{
    diagnostic::DiagnosticCode,
    run::{Engine, RunOptions, RunOutcome},
};
use std::collections::BTreeMap;

mod bounds;

fn parsed(source: &str) -> Arc<Suite> {
    Arc::new(Suite::parse("suite.botwork", source).unwrap())
}
fn rejected<T>(result: DiagnosticResult<T>) -> Diagnostic {
    match result {
        Err(error) => error,
        Ok(_) => panic!("dataset input must be rejected"),
    }
}

#[test]
fn literal_rows_preserve_types_metadata_order_and_original_coordinates() {
    let source = "Dataset |\"shared\"| Named |\"தமிழ்\"| Tags |[\"data\"]| {\nRow |\"b\"| Named |\"Second\"| Tags |[\"edge\"]| Values |{text: \"é\\n\", values: [none, true, false, -2147483648, 2147483647, -1.5, -0.0], nested: {inner: [{value: 7}]}, duplicate: 1, duplicate: 2}|\nRow |\"a\"| Values |7|\n}";
    let dataset = Dataset::parse("values.dataset.botwork", source).unwrap();
    assert_eq!(dataset.metadata().id(), "shared");
    assert_eq!(dataset.metadata().name(), "தமிழ்");
    assert_eq!(dataset.source().text(), source);
    assert_eq!(dataset.rows().len(), 2);
    assert_eq!(dataset.rows()[0].metadata().id(), "b");
    assert_eq!(dataset.rows()[0].metadata().name(), "Second");
    assert_eq!(dataset.rows()[1].metadata().name(), "a");
    assert_eq!(dataset.rows()[1].span().line_column().0, 3);
    let Literal::Map(data) = dataset.rows()[0].value() else {
        panic!("map")
    };
    assert_eq!(data["text"].to_string(), "é\n");
    assert_eq!(data["duplicate"].to_string(), "2");
    assert_eq!(data["nested"].to_string(), r#"{"inner": [{"value": 7}]}"#);
    let Literal::Array(values) = &data["values"] else {
        panic!("array")
    };
    assert!(matches!(values[0], Literal::None));
    assert!(matches!(values[1], Literal::Bool(true)));
    assert!(matches!(values[2], Literal::Bool(false)));
    assert!(matches!(values[3], Literal::Int(i32::MIN)));
    assert!(matches!(values[4], Literal::Int(i32::MAX)));
    assert!(matches!(values[5], Literal::Float(value) if value == -1.5));
    assert!(matches!(values[6], Literal::Float(value) if value.to_bits() == (-0.0f32).to_bits()));
}

#[test]
fn case_expansion_inherits_tags_and_keeps_case_then_row_declaration_order() {
    let suite = parsed(
        r#"Suite |"s"| Tags |["suite"]| {
Dataset |"pairs"| Tags |["dataset"]| {
Row |"z"| Named |"Zero"| Tags |["fast"]| Values |{n: 21}|
Row |"a"| Tags |["slow"]| Values |{n: 4}|
}
Library { Double |n| { Return |n * 2| } }
Case |"first"| Named |"First"| Tags |["case"]| Using |"pairs"| As |data| { |answer| = Double |data.n| }
Case |"plain"| { |answer| = |1| }
Case |"second"| Using |"pairs"| As |data| { |answer| = |data.n| }
}"#,
    );
    let runs = Selection::default().select(&[Arc::clone(&suite)]).unwrap();
    assert_eq!(suite.run_count().unwrap(), 5);
    assert_eq!(
        runs.iter().map(SelectedCase::id).collect::<Vec<_>>(),
        [
            "s/first/z",
            "s/first/a",
            "s/plain",
            "s/second/z",
            "s/second/a"
        ]
    );
    assert_eq!(runs[0].display_name(), "First / Zero");
    assert_eq!(runs[2].display_name(), "plain");
    assert_eq!(runs[0].case_id(), "s/first");
    assert_eq!(runs[0].dataset().unwrap().id(), "pairs");
    assert_eq!(
        runs[0].tags(),
        BTreeSet::from(["suite", "dataset", "case", "fast"])
    );
    let selected = Selection {
        cases: vec!["s/second".into(), "s/first/z".into(), "s/first/z".into()],
        tags: vec!["dataset".into()],
        exclude_tags: vec!["slow".into()],
        ..Default::default()
    }
    .select(&[suite])
    .unwrap();
    assert_eq!(
        selected.iter().map(SelectedCase::id).collect::<Vec<_>>(),
        ["s/first/z", "s/second/z"]
    );
    for (case, answer) in runs.iter().zip(["42", "8", "1", "21", "4"]) {
        let variables = case
            .bind_inputs(BTreeMap::new(), &ValueLimits::default())
            .unwrap();
        let result = Engine::default().run_program(
            &case.program(),
            RunOptions {
                variables,
                ..Default::default()
            },
        );
        assert_eq!(result.outcome(), RunOutcome::Succeeded);
        assert_eq!(result.variables["answer"].to_string(), answer);
    }
}

#[test]
fn row_bindings_override_common_inputs_without_mutating_shared_dataset_values() {
    let suite = parsed("Suite |\"s\"| { Dataset |\"d\"| { Row |\"r\"| Values |{n: [1, 2]}| } Case |\"c\"| Using |\"d\"| As |data| {} }");
    let runs = Selection::default().select(&[suite]).unwrap();
    let common = BTreeMap::from([
        ("data".into(), Literal::Int(99)),
        ("common".into(), Literal::Int(7)),
    ]);
    let mut inputs = runs[0]
        .bind_inputs(common, &ValueLimits::default())
        .unwrap();
    assert_eq!(inputs["common"].to_string(), "7");
    let Literal::Map(map) = inputs.get_mut("data").unwrap() else {
        panic!("map")
    };
    map.insert("n".into(), Literal::Int(99));
    let fresh = runs[0]
        .bind_inputs(BTreeMap::new(), &ValueLimits::default())
        .unwrap();
    assert_eq!(fresh["data"].to_string(), r#"{"n": [1, 2]}"#);
    assert_eq!(
        rejected(runs[0].bind_inputs(
            BTreeMap::new(),
            &ValueLimits {
                nodes: 0,
                ..Default::default()
            }
        ))
        .code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn external_datasets_require_explicit_resolution_and_aliases_share_immutable_rows() {
    let suite = Suite::parse("suite.botwork", "Suite |\"s\"| { Dataset |\"alias\"| From |\"shared.dataset.botwork\"| Case |\"c\"| Using |\"alias\"| As |data| {} }").unwrap();
    assert!(suite.run_count().is_err());
    assert!(Selection::default()
        .select(&[Arc::new(suite.clone())])
        .is_err());
    let external = Arc::new(
        Dataset::parse(
            "shared.dataset.botwork",
            "Dataset |\"original\"| Tags |[\"external\"]| { Row |\"r\"| Values |42| }",
        )
        .unwrap(),
    );
    let resolved = suite
        .resolve_datasets(|path, span, _| {
            assert_eq!(path, "shared.dataset.botwork");
            assert_eq!(span.source().name(), "suite.botwork");
            Ok(Arc::clone(&external))
        })
        .unwrap();
    assert!(Arc::ptr_eq(
        resolved.datasets()[0].data().unwrap(),
        &external
    ));
    let runs = Selection::default().select(&[Arc::new(resolved)]).unwrap();
    assert_eq!(runs[0].id(), "s/c/r");
    assert_eq!(runs[0].tags(), BTreeSet::from(["external"]));
}

#[test]
fn failed_selection_requires_exact_runnable_ids_and_ids_survive_reordered_rows() {
    for rows in [
        "Row |\"a\"| Values |1| Row |\"b\"| Values |2|",
        "Row |\"b\"| Values |3| Row |\"a\"| Named |\"Renamed\"| Values |4|",
    ] {
        let suite = parsed(&format!("Suite |\"s\"| {{ Dataset |\"d\"| {{ {rows} }} Case |\"c\"| Using |\"d\"| As |value| {{}} }}"));
        let selection = Selection {
            failed: Some(vec!["s/c/a".into()]),
            ..Default::default()
        };
        assert_eq!(
            selection.select(&[Arc::clone(&suite)]).unwrap()[0].id(),
            "s/c/a"
        );
        for id in ["s/c", "s/c/missing"] {
            assert!(Selection {
                failed: Some(vec![id.into()]),
                ..Default::default()
            }
            .select(&[Arc::clone(&suite)])
            .is_err());
        }
    }
    for id in ["s/c", "s/c/r"] {
        assert!(valid_run_id(id));
    }
    for id in ["s", "s/c/", "s/c/r/extra", "s/c/é"] {
        assert!(!valid_run_id(id));
    }
}

#[test]
fn dataset_grammar_rejects_code_invalid_numbers_duplicate_ids_and_missing_rows() {
    for body in [
        "",
        "Row |\"a\"| Values |1| Row |\"a\"| Values |2|",
        "Row |\"a\"| Values |@{ Log |1| }|",
        "Row |\"a\"| Values |missing|",
        "Row |\"a\"| Values |1 + 2|",
        "Row |\"a\"| Values |2147483648|",
        "Row |\"a\"| Values |-2147483649|",
        "Row |\"a\"| Values |9999999999999999999999999999999999999999999999.0|",
    ] {
        assert!(
            Dataset::parse("invalid", &format!("Dataset |\"d\"| {{ {body} }}")).is_err(),
            "{body}"
        );
    }
    for source in [
        "Suite |\"s\"| { Case |\"c\"| Using |\"unknown\"| As |data| {} }",
        "Suite |\"s\"| { Dataset |\"d\"| { Row |\"a\"| Values |1| } Dataset |\"d\"| From |\"missing\"| Case |\"c\"| {} }",
        "Suite |\"s\"| { Case |\"c\"| {} Dataset |\"d\"| { Row |\"a\"| Values |1| } }",
        "Suite |\"s\"| { Dataset |\"d\"| From |\"\"| Case |\"c\"| {} }",
    ] { assert!(Suite::parse("invalid", source).is_err(), "{source}"); }
    assert!(Dataset::parse(
        "invalid",
        "Dataset |\"d\"| From |\"other.dataset.botwork\"|"
    )
    .is_err());
}
