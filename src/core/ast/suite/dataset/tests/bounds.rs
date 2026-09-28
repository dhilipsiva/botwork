use super::*;

fn rows(count: usize) -> String {
    (0..count)
        .map(|i| format!("Row |\"r{i}\"| Values |1|\n"))
        .collect()
}

fn dataset(id: usize, count: usize) -> String {
    format!("Dataset |\"d{id}\"| {{ {} }}", rows(count))
}

fn bounded<T>(result: DiagnosticResult<T>, resource: &str) {
    let error = rejected(result);
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit, "{error}");
    assert!(error.to_string().contains(resource), "{error}");
}

#[test]
fn dataset_and_aggregate_row_limits_include_unused_declarations() {
    assert_eq!(
        Dataset::parse("data", &dataset(0, MAX_DATASET_ROWS))
            .unwrap()
            .rows()
            .len(),
        MAX_DATASET_ROWS
    );
    bounded(
        Dataset::parse("data", &dataset(0, MAX_DATASET_ROWS + 1)),
        "dataset rows",
    );
    let declarations: String = (0..4).map(|i| dataset(i, MAX_DATASET_ROWS)).collect();
    let suite = format!("Suite |\"s\"| {{ {declarations} Case |\"c\"| {{}} }}");
    assert!(Suite::parse("suite", &suite).is_ok());
    bounded(
        Suite::parse(
            "suite",
            &format!(
                "Suite |\"s\"| {{ {declarations} {} Case |\"c\"| {{}} }}",
                dataset(4, 1)
            ),
        ),
        "defined data rows",
    );
    let declarations: String = (0..MAX_DATASETS).map(|i| dataset(i, 1)).collect();
    assert!(Suite::parse(
        "suite",
        &format!("Suite |\"s\"| {{ {declarations} Case |\"c\"| {{}} }}")
    )
    .is_ok());
    bounded(
        Suite::parse(
            "suite",
            &format!(
                "Suite |\"s\"| {{ {declarations} {} Case |\"c\"| {{}} }}",
                dataset(MAX_DATASETS, 1)
            ),
        ),
        "dataset definitions",
    );
}

#[test]
fn expansion_is_bounded_before_filters_and_reused_data_is_counted_once() {
    let data = Arc::new(Dataset::parse("data", &dataset(0, MAX_DATASET_ROWS)).unwrap());
    let make_suite = |id: usize, cases: usize| {
        let cases: String = (0..cases)
            .map(|i| format!("Case |\"c{i}\"| Using |\"shared\"| As |row| {{}}"))
            .collect();
        Arc::new(
            Suite::parse(
                "suite",
                &format!("Suite |\"s{id}\"| {{ Dataset |\"shared\"| From |\"data\"| {cases} }}"),
            )
            .unwrap()
            .resolve_datasets(|_, _| Ok(Arc::clone(&data)))
            .unwrap(),
        )
    };
    let suites: Vec<_> = (0..4).map(|i| make_suite(i, 1)).collect();
    assert_eq!(make_suite(5, 4).run_count().unwrap(), MAX_SELECTED_CASES);
    let selected = Selection::default().select(&suites).unwrap();
    assert_eq!(selected.len(), MAX_SELECTED_CASES);
    assert_eq!(selected.last().unwrap().id(), "s3/c0/r1023");
    let filtered = Selection {
        cases: vec!["s0/c0/r0".into()],
        ..Default::default()
    };
    let mut over = suites;
    over.push(make_suite(4, 1));
    bounded(filtered.select(&over), "discovered cases");
    let declarations: String = (0..5)
        .map(|i| format!("Case |\"c{i}\"| Using |\"d0\"| As |row| {{}}"))
        .collect();
    let suite = Suite::parse(
        "suite",
        &format!("Suite |\"s\"| {{ {} {declarations} }}", dataset(0, 1024)),
    )
    .unwrap();
    bounded(suite.run_count(), "discovered case rows");
}

#[test]
fn value_depth_entries_keys_and_source_have_exact_boundaries() {
    let parse_value = |value: &str| {
        Dataset::parse(
            "data",
            &format!("Dataset |\"d\"| {{ Row |\"r\"| Values |{value}| }}"),
        )
    };
    let depth = |count| format!("{}0{}", "[".repeat(count), "]".repeat(count));
    assert!(parse_value(&depth(30)).is_ok());
    bounded(parse_value(&depth(31)), "syntax nesting");
    let entries = |count| format!("[{}]", vec!["0"; count].join(","));
    assert!(parse_value(&entries(16_384)).is_ok());
    bounded(parse_value(&entries(16_385)), "value container entries");
    let array = format!("[{},2147483648]", vec!["0"; 16_384].join(","));
    bounded(parse_value(&array), "value container entries");
    let members: String = (0..16_384).map(|i| format!("k{i}:0,")).collect();
    bounded(
        parse_value(&format!("{{{members}over:2147483648}}")),
        "value container entries",
    );
    assert!(parse_value(&format!("{{\"{}\":0}}", "a".repeat(65_536))).is_ok());
    bounded(
        parse_value(&format!("{{\"{}\":0}}", "a".repeat(65_537))),
        "value key bytes",
    );
    let mut source = dataset(0, 1);
    source.push_str(&" ".repeat(DEFAULT_SOURCE_BYTES - source.len()));
    assert_eq!(
        Dataset::parse("data", &source)
            .unwrap()
            .source()
            .text()
            .len(),
        DEFAULT_SOURCE_BYTES
    );
    source.push(' ');
    bounded(Dataset::parse("data", &source), "source bytes");
}

#[test]
fn literal_node_budget_counts_overwritten_map_values_across_rows() {
    // Each map has 16,383 parsed values plus its root; one retained key.
    let value = format!("{{{}}}", vec!["same:0"; 16_383].join(","));
    let row = |id| format!("Row |\"r{id}\"| Values |{value}|");
    let exact: String = (0..4).map(row).collect();
    let data = Dataset::parse("data", &format!("Dataset |\"d\"| {{ {exact} }}")).unwrap();
    assert_eq!(data.value_nodes(), MAX_DATA_NODES);
    assert!(matches!(data.rows()[0].value(), Literal::Map(map) if map.len() == 1));
    bounded(
        Dataset::parse(
            "data",
            &format!("Dataset |\"d\"| {{ {exact} Row |\"over\"| Values |0| }}"),
        ),
        "dataset literal nodes",
    );
}

#[test]
fn selection_accounts_for_unique_dataset_owners_and_external_sources() {
    let make = |id: usize, data: Arc<Dataset>| {
        Arc::new(
            Suite::parse(
                "suite",
                &format!(
                    "Suite |\"s{id}\"| {{ Dataset |\"d\"| From |\"data\"| Case |\"c\"| {{}} }}"
                ),
            )
            .unwrap()
            .resolve_datasets(|_, _| Ok(Arc::clone(&data)))
            .unwrap(),
        )
    };
    let data = Arc::new(Dataset::parse("data", &dataset(0, 1024)).unwrap());
    let shared: Vec<_> = (0..5).map(|id| make(id, Arc::clone(&data))).collect();
    assert_eq!(Selection::default().select(&shared).unwrap().len(), 5);
    let distinct: Vec<_> = (0..5)
        .map(|id| make(id, Arc::new((*data).clone())))
        .collect();
    assert_eq!(
        Selection::default().select(&distinct[..4]).unwrap().len(),
        4
    );
    bounded(Selection::default().select(&distinct), "defined data rows");
    let mut source = dataset(0, 1);
    source.push_str(&" ".repeat(DEFAULT_SOURCE_BYTES - source.len()));
    let large = Arc::new(Dataset::parse("large", &source).unwrap());
    let shared: Vec<_> = (0..8).map(|id| make(id, Arc::clone(&large))).collect();
    assert_eq!(Selection::default().select(&shared).unwrap().len(), 8);
    let distinct: Vec<_> = (0..8)
        .map(|id| make(id, Arc::new(Dataset::parse("large", &source).unwrap())))
        .collect();
    bounded(Selection::default().select(&distinct), "suite source bytes");
}

#[test]
fn dataset_layout_metadata_paths_and_bindings_are_validated() {
    let source =
        "### start ### dAtAsEt\n|\"d\"|\n{ rOw\n|\"r\"|\nVaLuEs\n|{key: [none, -\n2,],}|\n}";
    for source in [source.to_owned(), source.replace('\n', "\r\n")] {
        let data = Dataset::parse("data", &source).unwrap();
        assert_eq!(data.rows()[0].value().to_string(), "{\"key\": [none, -2]}");
    }
    for row in [
        "Row |\"bad/id\"| Values |1|",
        "Row |\"r\"| Named |\"\"| Values |1|",
        "Row |\"r\"| Tags |[\"a\", \"a\"]| Values |1|",
        "Row |\"r\"| Tags |[\"line\\nfeed\"]| Values |1|",
    ] {
        assert!(Dataset::parse("data", &format!("Dataset |\"d\"| {{ {row} }}")).is_err());
    }
    for binding in [
        "data.value".into(),
        "true".into(),
        "x".repeat(MAX_ID_BYTES + 1),
    ] {
        assert!(Suite::parse(
            "suite",
            &format!(
                "Suite |\"s\"| {{ {} Case |\"c\"| Using |\"d0\"| As |{binding}| {{}} }}",
                dataset(0, 1)
            )
        )
        .is_err());
    }
    for binding in ["x".repeat(MAX_ID_BYTES), "é".repeat(MAX_ID_BYTES / 2)] {
        let suite = parsed(&format!(
            "Suite |\"s\"| {{ {} Case |\"c\"| Using |\"d0\"| As |{binding}| {{}} }}",
            dataset(0, 1)
        ));
        let cases = Selection::default().select(&[suite]).unwrap();
        let inputs = cases[0]
            .bind_inputs(BTreeMap::new(), &ValueLimits::default())
            .unwrap();
        assert_eq!(inputs[&binding].to_string(), "1");
    }
    for path in ["\0".to_owned(), "a".repeat(MAX_DATASET_PATH_BYTES + 1)] {
        assert!(Suite::parse(
            "suite",
            &format!("Suite |\"s\"| {{ Dataset |\"d\"| From |\"{path}\"| Case |\"c\"| {{}} }}")
        )
        .is_err());
    }
    assert!(Suite::parse(
        "suite",
        &format!(
            "Suite |\"s\"| {{ Dataset |\"d\"| From |\"{}\"| Case |\"c\"| {{}} }}",
            "a".repeat(MAX_DATASET_PATH_BYTES)
        )
    )
    .is_ok());
}

#[test]
fn selection_limits_unique_datasets_and_literal_nodes_across_suite_sources() {
    let make_suite = |id: usize, count: usize| {
        let declarations: String = (0..count).map(|i| dataset(i, 1)).collect();
        parsed(&format!(
            "Suite |\"s{id}\"| {{ {declarations} Case |\"c\"| {{}} }}"
        ))
    };
    let first = make_suite(0, 32);
    let exact = make_suite(1, 32);
    assert_eq!(
        Selection::default()
            .select(&[Arc::clone(&first), exact])
            .unwrap()
            .len(),
        2
    );
    bounded(
        Selection::default().select(&[first, make_suite(1, 33)]),
        "discovered datasets",
    );
    let value = format!("{{{}}}", vec!["key:0"; 16_383].join(","));
    let data = Arc::new(
        Dataset::parse(
            "data",
            &format!(
                "Dataset |\"d\"| {{ Row |\"a\"| Values |{value}| Row |\"b\"| Values |{value}| }}"
            ),
        )
        .unwrap(),
    );
    assert_eq!(data.value_nodes(), 32_768);
    let make = |id, data: Arc<Dataset>| {
        Arc::new(
            Suite::parse(
                "suite",
                &format!(
                    "Suite |\"s{id}\"| {{ Dataset |\"d\"| From |\"data\"| Case |\"c\"| {{}} }}"
                ),
            )
            .unwrap()
            .resolve_datasets(|_, _| Ok(Arc::clone(&data)))
            .unwrap(),
        )
    };
    let a = make(0, Arc::clone(&data));
    let b = make(1, Arc::new((*data).clone()));
    assert_eq!(
        Selection::default()
            .select(&[Arc::clone(&a), Arc::clone(&b)])
            .unwrap()
            .len(),
        2
    );
    let c = make(2, Arc::new(Dataset::parse("tiny", &dataset(0, 1)).unwrap()));
    bounded(
        Selection::default().select(&[a, b, c]),
        "dataset literal nodes",
    );
}

#[test]
fn inline_dataset_accounting_is_local_and_counts_overwritten_literals() {
    let suite = parsed(
        r#"Suite |"s"| {
Dataset |"first"| { Row |"r"| Values |[1, 2]| }
Dataset |"second"| { Row |"r"| Values |{key: 0, key: 1}| }
Case |"c"| Using |"second"| As |data| {}
}"#,
    );
    let counts: Vec<_> = suite
        .datasets()
        .iter()
        .map(|definition| definition.data().unwrap().value_nodes())
        .collect();
    assert_eq!(counts, [3, 3]);
    assert_eq!(
        Selection::default().select(&[suite]).unwrap()[0]
            .row()
            .unwrap()
            .value()
            .to_string(),
        "{\"key\": 1}"
    );
}
