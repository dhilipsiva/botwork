#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use botwork::core::{
    diagnostic::{Diagnostic, DiagnosticCode as Code},
    grammar::Literal,
    run::{Engine, RunOptions, RunOutcome, RunResult},
    suite::{Dataset, DatasetFormat},
};
use cli_harness::{http_fixture, Harness};
use std::{collections::BTreeMap, fs, path::Path, time::Duration};

fn run_with(source: &str, variables: &[(&str, Literal)]) -> RunResult {
    let options = RunOptions {
        variables: variables
            .iter()
            .map(|(name, value)| ((*name).into(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        ..RunOptions::default()
    };
    Engine::default().run_source("data.botwork", source, options)
}
fn text(value: &str) -> Literal {
    Literal::String(value.into())
}
fn parse(json: &str) -> Result<Literal, Diagnostic> {
    let RunResult {
        result, variables, ..
    } = run_with("|value| = Parse JSON |input|", &[("input", text(json))]);
    result.map(|_| variables["value"].clone())
}
fn format(value: Literal) -> String {
    run_with("|text| = Format JSON |value|", &[("value", value)]).variables["text"].to_string()
}
fn failure(result: &RunResult) -> &Diagnostic {
    result.result.as_ref().unwrap_err()
}

#[test]
fn parse_json_shares_the_input_variable_conversion_rules() {
    for (json, canonical) in [
        ("0", "0"),
        ("-2147483648", "-2147483648"),
        ("2147483647", "2147483647"),
        ("1.0", "1.0"),
        ("1e3", "1000.0"),
        ("-0.0", "-0.0"),
        ("16777217.0", "16777216.0"),
        ("1e-50", "0.0"),
        ("3.4028235e38", "3.4028235e38"),
        ("null", "null"),
        ("true", "true"),
        (r#""café 😀 \"q\" \\ \n""#, r#""café 😀 \"q\" \\ \n""#),
        (
            r#"{"b": 1, "a": {"z": [], "y": {}}}"#,
            r#"{"a":{"y":{},"z":[]},"b":1}"#,
        ),
        (r#"{"k": 1, "k": 2}"#, r#"{"k":2}"#),
        ("\u{feff}[1, 2.5]", "[1,2.5]"),
        (" \n\t[ ] ", "[]"),
    ] {
        let value = parse(json).unwrap_or_else(|error| panic!("{json}: {error}"));
        assert_eq!(format(value), canonical, "{json}");
    }
    let value = parse(r#"{"ints": [1, -1], "floats": [1.0, 2e0]}"#).unwrap();
    let Literal::Map(map) = value else { panic!() };
    assert!(
        matches!(&map["ints"], Literal::Array(items) if items.iter().all(|item| matches!(item, Literal::Int(_))))
    );
    assert!(
        matches!(&map["floats"], Literal::Array(items) if items.iter().all(|item| matches!(item, Literal::Float(_))))
    );
}

#[test]
fn parse_json_rejections_are_catchable_and_name_the_location() {
    for (json, message) in [
        (
            "[2147483648]",
            "Parse JSON: $[0]: integer is outside -2147483648..2147483647",
        ),
        (
            r#"{"a": {"b": -2147483649}}"#,
            r#"Parse JSON: $["a"]["b"]: integer is outside"#,
        ),
        (
            "[3.5e38]",
            "Parse JSON: $[0]: decimal exceeds the finite f32 range",
        ),
        (
            r#"{"a": [1,]}"#,
            "Parse JSON: $: expected value at line 1 column 10",
        ),
        ("1 2", "trailing characters at line 1 column 3"),
        ("", "EOF while parsing a value at line 1 column 0"),
        ("NaN", "expected value at line 1 column 1"),
        (
            r#""\ud800""#,
            "unexpected end of hex escape at line 1 column 8",
        ),
        ("[01]", "invalid number"),
        ("\u{feff}\u{feff}1", "expected value"),
    ] {
        let result = run_with(
            "Try { |value| = Parse JSON |input| } Catch |error| { |code| = |error.code| |message| = |error.message| }",
            &[("input", text(json))],
        );
        assert!(result.result.is_ok(), "{json}: {:?}", result.result);
        assert_eq!(result.variables["code"].to_string(), "BW3003", "{json}");
        let text = result.variables["message"].to_string();
        assert!(text.contains(message), "{json}: {text}");
    }
    let deep = format!("{}{}", "[".repeat(129), "]".repeat(129));
    let result = run_with("|value| = Parse JSON |input|", &[("input", text(&deep))]);
    assert!(failure(&result)
        .to_string()
        .contains("JSON exceeds 128 nested containers"));
}

#[test]
fn parse_json_value_limits_stop_the_run_before_retention() {
    let deep = format!("{}1{}", "[".repeat(64), "]".repeat(64));
    let result = run_with(
        "Try { |value| = Parse JSON |input| } Catch { |caught| = |true| }",
        &[("input", text(&deep))],
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(failure(&result).to_string().contains("value depth"));
    assert!(!result.variables.contains_key("caught"));
    let mut options = RunOptions::default();
    options.limits.temporaries.payload_bytes = 64;
    options
        .variables
        .insert("input".into(), text(&format!("[\"{}\"]", "x".repeat(100))));
    let result =
        Engine::default().run_source("limits.botwork", "|value| = Parse JSON |input|", options);
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
}

#[test]
fn format_json_is_canonical_and_round_trips_every_kind() {
    let values = [
        Literal::None,
        Literal::Bool(false),
        Literal::Int(i32::MIN),
        Literal::Float(-0.0),
        Literal::Float(f32::MIN_POSITIVE),
        Literal::Float(1e-45),
        Literal::Float(0.1),
        Literal::Float(f32::MAX),
        text("tab\t nul\0 bell\u{7} quote\" slash\\ café 😀"),
        Literal::Array(vec![Literal::Int(1), Literal::Float(1.0), Literal::None]),
        Literal::Map(
            [
                ("é".to_owned(), Literal::Int(1)),
                ("Z".to_owned(), Literal::Array(vec![])),
                ("a".to_owned(), Literal::Map(Default::default())),
                ("".to_owned(), text("empty key")),
            ]
            .into(),
        ),
    ];
    for value in values {
        let result = run_with(
            r#"|text| = Format JSON |value|
|back| = Parse JSON |text|
Assert |back| Equals |value|
|again| = Format JSON |back|
Assert |again| Equals |text|"#,
            &[("value", value.clone())],
        );
        assert!(result.result.is_ok(), "{value:?}: {:?}", result.result);
        assert!(same_kinds(&value, &result.variables["back"]), "{value:?}");
    }
    assert_eq!(
        format(text("\u{1}\u{8}\u{c}\r")),
        r#""\u0001\b\f\r""#,
        "short escapes where JSON defines them"
    );
    assert_eq!(
        format(Literal::Map(
            [
                ("é".to_owned(), Literal::Int(1)),
                ("z".to_owned(), Literal::Int(2)),
                ("A".to_owned(), Literal::Int(3))
            ]
            .into()
        )),
        r#"{"A":3,"z":2,"é":1}"#,
        "keys sort by Unicode scalar value"
    );
}

/// Structural equality plus exact Int/Float kinds at every position.
fn same_kinds(left: &Literal, right: &Literal) -> bool {
    match (left, right) {
        (Literal::Array(a), Literal::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_kinds(a, b))
        }
        (Literal::Map(a), Literal::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| same_kinds(a, b)))
        }
        (Literal::Float(a), Literal::Float(b)) => a.to_bits() == b.to_bits(),
        (Literal::Int(a), Literal::Int(b)) => a == b,
        (a, b) => {
            std::mem::discriminant(a) == std::mem::discriminant(b) && a.to_string() == b.to_string()
        }
    }
}

#[test]
fn seeded_values_round_trip_through_canonical_json() {
    fn generate(seed: &mut u64, depth: usize) -> Literal {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let choice = (*seed >> 33) % if depth == 0 { 5 } else { 7 };
        let bits = (*seed >> 11) as u32;
        match choice {
            0 => Literal::None,
            1 => Literal::Bool(bits.is_multiple_of(2)),
            2 => Literal::Int(bits as i32),
            3 => {
                let value = f32::from_bits(bits);
                Literal::Float(if value.is_finite() { value } else { 1.5 })
            }
            4 => text(
                &char::from_u32(bits % 0x11_0000)
                    .map_or("\u{fffd}".into(), |ch| format!("{ch}\"\\{}", bits % 7)),
            ),
            5 => Literal::Array((0..bits % 4).map(|_| generate(seed, depth - 1)).collect()),
            _ => Literal::Map(
                (0..bits % 4)
                    .map(|index| (format!("k{index}{}", bits % 3), generate(seed, depth - 1)))
                    .collect(),
            ),
        }
    }
    let mut seed = 0x5eed_u64;
    for _ in 0..200 {
        let value = generate(&mut seed, 3);
        let result = run_with(
            "|text| = Format JSON |value|\n|back| = Parse JSON |text|",
            &[("value", value.clone())],
        );
        assert!(result.result.is_ok(), "{value:?}: {:?}", result.result);
        assert!(same_kinds(&value, &result.variables["back"]), "{value:?}");
    }
}

#[test]
fn parse_csv_reads_header_tables_with_rfc_4180_quoting() {
    for (csv, canonical) in [
        ("id,name\n1,Ann\n", r#"[{"id":"1","name":"Ann"}]"#),
        (
            "id,name\r\n1,Ann\r\n2,Bob",
            r#"[{"id":"1","name":"Ann"},{"id":"2","name":"Bob"}]"#,
        ),
        (
            "a,b\n\"x, y\",\"say \"\"hi\"\"\"\n\"line\none\",\n",
            r#"[{"a":"x, y","b":"say \"hi\""},{"a":"line\none","b":""}]"#,
        ),
        (
            "\u{feff}city\nZürich\n東京\n",
            r#"[{"city":"Zürich"},{"city":"東京"}]"#,
        ),
        ("only\n\n", r#"[{"only":""}]"#),
        ("a,b\n", "[]"),
        ("a,b", "[]"),
        (",x\n", "error"),
    ] {
        let result = run_with(
            "Try {\n|rows| = Parse CSV |input|\n|text| = Format JSON |rows|\n} Catch { |text| = |\"error\"| }",
            &[("input", text(csv))],
        );
        assert!(result.result.is_ok(), "{csv:?}: {:?}", result.result);
        assert_eq!(result.variables["text"].to_string(), canonical, "{csv:?}");
    }
}

#[test]
fn parse_csv_rejections_name_lines_and_columns() {
    for (csv, message) in [
        ("", "Parse CSV requires a header row"),
        (
            "a,b\n1\n",
            "Parse CSV: line 2: record 1 has 1 field; the header has 2",
        ),
        (
            "a,b\n1,2,3\n",
            "Parse CSV: line 2: record 1 has 3 fields; the header has 2",
        ),
        ("a,,b\n", "Parse CSV: header column 2 is empty"),
        (
            "a,b,a\n",
            r#"Parse CSV: header "a" repeats columns 1 and 3"#,
        ),
        (
            "a\nx\"y\n",
            "Parse CSV: line 2, column 2: a quote must start a quoted field",
        ),
        (
            "a\n\"x\"y\n",
            "Parse CSV: line 2, column 4: a quoted field must end at a comma or line end",
        ),
        (
            "a\n\"never\nclosed\n",
            "Parse CSV: line 2, column 1: a quoted field is not terminated",
        ),
        (
            "a\rb\n",
            "Parse CSV: line 1, column 2: a carriage return must be followed by a line feed",
        ),
        (
            "a\n\"é\n\"é\n",
            "Parse CSV: line 3, column 2: a quoted field must end at a comma or line end",
        ),
    ] {
        let result = run_with(
            "Try { |rows| = Parse CSV |input| } Catch |error| { |code| = |error.code| |message| = |error.message| }",
            &[("input", text(csv))],
        );
        assert_eq!(result.variables["code"].to_string(), "BW3003", "{csv:?}");
        let text = result.variables["message"].to_string();
        assert!(text.contains(message), "{csv:?}: {text}");
    }
    let mut options = RunOptions::default();
    options.limits.values.entries = 2;
    options
        .variables
        .insert("input".into(), text("a\n1\n2\n3\n"));
    let result =
        Engine::default().run_source("rows.botwork", "|rows| = Parse CSV |input|", options);
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
}

#[test]
fn nested_payload_paths_and_missing_fields_fail_precisely() {
    let result = run_with(
        r#"|doc| = Parse JSON |input|
|city| = |doc.users[1].address.city|
Try { |zip| = |doc.users[1].address.zip| } Catch |error| {
    |code| = |error.code|
    |path| = |error.details.path|
    |segment| = |error.details.segment|
}"#,
        &[(
            "input",
            text(r#"{"users": [{"name": "a"}, {"name": "b", "address": {"city": "Zürich"}}]}"#),
        )],
    );
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(result.variables["city"].to_string(), "Zürich");
    assert_eq!(result.variables["code"].to_string(), "BW3004");
    assert!(result.variables["segment"].to_string().contains("zip"));
    assert!(result.variables["path"].to_string().contains("address"));
}

#[test]
fn json_and_csv_dataset_files_keep_declared_rows_and_kinds() {
    let json = Dataset::from_json(
        "users.json",
        "\u{feff}[{\"id\": \"alice\", \"age\": 30, \"tags\": [\"a\"]}, {\"id\": \"bob\", \"age\": 2.5}]",
    )
    .unwrap();
    let ids: Vec<_> = json.rows().iter().map(|row| row.metadata().id()).collect();
    assert_eq!(ids, ["alice", "bob"]);
    let Literal::Map(alice) = json.rows()[0].value() else {
        panic!()
    };
    assert!(matches!(alice["age"], Literal::Int(30)));
    assert_eq!(alice["id"].to_string(), "alice");
    assert_eq!(json.rows()[1].span().line_column(), (1, 45));
    let csv = Dataset::parse_format(
        "users.csv",
        "id,age,note\r\nc1,30,\"a, b\"\r\nc2,4,\r\n",
        DatasetFormat::Csv,
    )
    .unwrap();
    let Literal::Map(row) = csv.rows()[1].value() else {
        panic!()
    };
    assert_eq!(
        (
            row["id"].to_string(),
            row["age"].to_string(),
            row["note"].to_string()
        ),
        ("c2".into(), "4".into(), String::new())
    );
    assert!(
        matches!(row["age"], Literal::String(_)),
        "CSV fields stay Strings"
    );
    assert_eq!(csv.rows()[1].span().line_column(), (3, 1));
    for (format, source, message) in [
        (
            DatasetFormat::Json,
            r#"{"id": "a"}"#,
            "expected a JSON array of row objects",
        ),
        (
            DatasetFormat::Json,
            "[1]",
            "JSON dataset row $[0] must be an object",
        ),
        (
            DatasetFormat::Json,
            r#"[{"name": "x"}]"#,
            r#"JSON dataset row $[0] requires a String "id" field"#,
        ),
        (
            DatasetFormat::Json,
            r#"[{"id": 1}]"#,
            r#"requires a String "id" field"#,
        ),
        (
            DatasetFormat::Json,
            r#"[{"id": "a"}, {"id": "a"}]"#,
            r#"Duplicate dataset row ID "a""#,
        ),
        (
            DatasetFormat::Json,
            r#"[{"id": "a b"}]"#,
            "Dataset row IDs must start with an ASCII letter",
        ),
        (
            DatasetFormat::Json,
            r#"[{"id": "a", "n": 2147483648}]"#,
            r#"$[0]["n"]: integer is outside"#,
        ),
        (
            DatasetFormat::Csv,
            "name\nx\n",
            "CSV datasets require an id column",
        ),
        (
            DatasetFormat::Csv,
            "id,id\nx,y\n",
            "header names must be nonempty and distinct",
        ),
        (
            DatasetFormat::Csv,
            "id,n\nx\n",
            "CSV dataset line 2 has 1 field; the header has 2",
        ),
        (
            DatasetFormat::Csv,
            "id\n\"x\n",
            "CSV dataset: a quoted field is not terminated",
        ),
        (
            DatasetFormat::Csv,
            "",
            "CSV datasets require a header row including id",
        ),
    ] {
        let error = Dataset::parse_format("data", source, format).unwrap_err();
        assert!(
            matches!(error.code(), Code::RunConfiguration),
            "{source}: {error}"
        );
        assert!(error.to_string().contains(message), "{source}: {error}");
    }
    let rows = (0..1025)
        .map(|index| format!("{{\"id\": \"r{index}\"}}"))
        .collect::<Vec<_>>();
    let error = Dataset::from_json("many", &format!("[{}]", rows.join(","))).unwrap_err();
    assert_eq!(error.code(), Code::ResourceLimit);
}

#[test]
fn suites_read_declared_formats_and_cache_each_format_separately() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("rows.data"), "id,value\nfirst,1\n").unwrap();
    fs::write(
        harness.workspace.join("rows.json"),
        r#"[{"id": "j1", "value": 1}]"#,
    )
    .unwrap();
    fs::write(
        harness.workspace.join("formats.suite.botwork"),
        r#"Suite |"formats"| {
    Dataset |"csv"| From csv |"rows.data"|
    Dataset |"json"| FROM Json |"rows.json"|
    Case |"typed"| Using |"csv"| As |row| { Log |@{ Type Of |row.value| }| }
    Case |"parsed"| Using |"json"| As |row| { Log |@{ Type Of |row.value| }| }
}"#,
    )
    .unwrap();
    let output = harness
        .command(
            "formats",
            &["--suite", "formats.suite.botwork", "--jobs", "1"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "String\nInt\n");
    let listed = harness
        .command(
            "list",
            &["--suite", "formats.suite.botwork", "--list-cases"],
            Duration::from_secs(30),
        )
        .unwrap();
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(
        listed.contains("formats/typed/first") && listed.contains("formats/parsed/j1"),
        "{listed}"
    );

    // One file declared in two formats is parsed separately for each format.
    fs::write(
        harness.workspace.join("both.suite.botwork"),
        r#"Suite |"both"| {
    Dataset |"table"| From CSV |"rows.data"|
    Dataset |"document"| From JSON |"rows.data"|
    Case |"c"| Using |"table"| As |row| { No Operation }
}"#,
    )
    .unwrap();
    let output = harness
        .command(
            "both",
            &["--suite", "both.suite.botwork"],
            Duration::from_secs(30),
        )
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success());
    assert!(
        stderr.contains("[BW7002]") && stderr.contains("rows.data: $: expected value"),
        "{stderr}"
    );
}

#[test]
fn dataset_driven_http_example_checks_each_row_and_reports_row_identity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = http_fixture::Fixture::serving(root.join("examples/fixtures/users"));
    let base = format!("base_url={:?}", fixture.url);
    let harness = Harness::new();
    let example = root.join("examples/39-dataset-http.suite.botwork");
    let output = harness
        .command(
            "http-example",
            &[
                "--suite",
                example.to_str().unwrap(),
                "--jobs",
                "2",
                "--var",
                &base,
            ],
            Duration::from_secs(30),
        )
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.ends_with("[cases] 2 selected: 2 succeeded, 0 failed\n"),
        "{stderr}"
    );
    // A changed expectation fails only its own row, naming the dataset row.
    fs::create_dir_all(harness.workspace.join("datasets")).unwrap();
    fs::write(
        harness.workspace.join("datasets/users.json"),
        r#"[{"id": "alice", "name": "Alice Doe", "roles": ["admin", "editor"], "city": "Zürich"}, {"id": "bob", "name": "Bob", "roles": [], "city": "Osaka"}]"#,
    )
    .unwrap();
    fs::copy(&example, harness.workspace.join("contract.suite.botwork")).unwrap();
    let output = harness
        .command(
            "http-mismatch",
            &["--suite", "contract.suite.botwork", "--var", &base],
            Duration::from_secs(30),
        )
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success());
    assert!(stderr.contains("[case users/profile/bob] failed: \"profile / bob\" [dataset \"expected\", row \"bob\"]"), "{stderr}");
    assert!(
        stderr.contains(r#"difference at $: expected "Osaka" (String), got "東京" (String)"#),
        "{stderr}"
    );
    assert!(
        stderr.ends_with("[cases] 2 selected: 1 succeeded, 1 failed\n"),
        "{stderr}"
    );
    let output = harness
        .command(
            "http-missing",
            &[
                "--suite",
                example.to_str().unwrap(),
                "--var",
                &format!("base_url={:?}", format!("{}/missing", fixture.url)),
            ],
            Duration::from_secs(30),
        )
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("expected 200 (Int), got 404 (Int)"),
        "{stderr}"
    );
}
