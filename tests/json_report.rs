#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use serde_json::{json, Value};
use std::{fs, path::Path, process::Output, time::Duration};

const SCHEMA: &str = include_str!("../docs/json-report.schema.json");

/// Validate `value` against the subset of JSON Schema the report schema uses.
/// Unknown keywords fail, so the schema never claims a rule nothing enforces.
fn validate(schema: &Value, root: &Value, value: &Value, at: &str) -> Result<(), String> {
    let rules = schema.as_object().expect("schema object");
    for key in rules.keys() {
        let known = [
            "$schema",
            "$id",
            "$defs",
            "$ref",
            "title",
            "description",
            "type",
            "const",
            "enum",
            "properties",
            "required",
            "additionalProperties",
            "items",
            "oneOf",
            "minimum",
            "pattern",
        ];
        assert!(known.contains(&key.as_str()), "unsupported keyword {key}");
    }
    if let Some(reference) = rules.get("$ref").and_then(Value::as_str) {
        let name = reference.strip_prefix("#/$defs/").expect("local reference");
        validate(&root["$defs"][name], root, value, at)?;
    }
    if let Some(types) = rules.get("type") {
        let types: Vec<&str> = match types {
            Value::String(name) => vec![name],
            names => names
                .as_array()
                .unwrap()
                .iter()
                .map(|name| name.as_str().unwrap())
                .collect(),
        };
        let matches = |name: &str| match name {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_u64() || value.is_i64(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            other => panic!("unsupported type {other}"),
        };
        if !types.iter().any(|name| matches(name)) {
            return Err(format!("{at}: {value} is not {types:?}"));
        }
    }
    if let Some(expected) = rules.get("const") {
        if value != expected {
            return Err(format!("{at}: {value} is not {expected}"));
        }
    }
    if let Some(options) = rules.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            return Err(format!("{at}: {value} is not one of {options:?}"));
        }
    }
    if let (Some(minimum), Some(number)) = (rules.get("minimum"), value.as_i64()) {
        if number < minimum.as_i64().unwrap() {
            return Err(format!("{at}: {number} is below {minimum}"));
        }
    }
    if let (Some(pattern), Some(text)) = (rules.get("pattern"), value.as_str()) {
        if !regex::Regex::new(pattern.as_str().unwrap())
            .unwrap()
            .is_match(text)
        {
            return Err(format!("{at}: {text:?} does not match {pattern}"));
        }
    }
    if let Some(object) = value.as_object() {
        let properties = rules.get("properties").and_then(Value::as_object);
        for name in rules
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !object.contains_key(name.as_str().unwrap()) {
                return Err(format!("{at}: missing {name}"));
            }
        }
        for (name, field) in object {
            match properties.and_then(|properties| properties.get(name)) {
                Some(property) => validate(property, root, field, &format!("{at}.{name}"))?,
                None if rules.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{at}: undocumented field {name}"))
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(array)) = (rules.get("items"), value.as_array()) {
        for (index, item) in array.iter().enumerate() {
            validate(items, root, item, &format!("{at}[{index}]"))?;
        }
    }
    if let Some(options) = rules.get("oneOf").and_then(Value::as_array) {
        let matched = options
            .iter()
            .filter(|option| validate(option, root, value, at).is_ok())
            .count();
        if matched != 1 {
            return Err(format!(
                "{at}: {value} matches {matched} oneOf alternatives"
            ));
        }
    }
    Ok(())
}

fn conforms(report: &Value) {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    if let Err(error) = validate(&schema, &schema, report, "$") {
        panic!("{error}\n{report:#}");
    }
}

fn command(harness: &Harness, arguments: &[&str]) -> Output {
    harness
        .command("report", arguments, Duration::from_secs(60))
        .unwrap()
}

fn write(harness: &Harness, name: &str, source: &str) {
    fs::write(harness.workspace.join(name), source).unwrap();
}

/// Read a report and check the rules every report satisfies.
fn report(harness: &Harness, output: &Output) -> Value {
    let value: Value =
        serde_json::from_slice(&fs::read(harness.workspace.join("report.json")).unwrap()).unwrap();
    conforms(&value);
    if value["complete"] == true {
        assert_eq!(
            value["exit_code"].as_i64().map(|code| code as i32),
            output.status.code(),
            "the report and the process agree"
        );
        let runs = value["runs"].as_array().unwrap();
        assert_eq!(value["verdict"]["cases"]["total"], runs.len());
        for (index, run) in runs.iter().enumerate() {
            assert_eq!(run["number"], index + 1, "runs are ordered by number");
        }
    }
    value
}

fn statuses(report: &Value) -> Vec<&str> {
    report["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["status"].as_str().unwrap())
        .collect()
}

#[test]
fn single_file_reports_describe_the_run_and_the_exit_status() {
    let harness = Harness::new();
    write(&harness, "pass.botwork", "Log |\"ready\"|\n|x| = |1|");
    let output = command(
        &harness,
        &["--file", "pass.botwork", "--report-json", "report.json"],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty(), "a single file stays quiet");
    let value = report(&harness, &output);
    assert_eq!(value["complete"], true);
    assert_eq!(value["mode"], "file");
    assert_eq!(value["verdict"]["status"], "succeeded");
    let run = &value["runs"][0];
    assert_eq!(
        run["identity"],
        json!({"id": "pass.botwork", "name": "pass.botwork", "dataset": null, "row": null})
    );
    assert_eq!(run["statements"][0]["kind"], "call");
    assert_eq!(run["statements"][1]["kind"], "assignment");
    assert_eq!(run["statements"][1]["location"]["line"], 2);
    assert_eq!(
        run["logs"],
        json!([{"statement": 0, "offset_us": run["logs"][0]["offset_us"], "bytes": 5, "text": "ready", "truncated": false}])
    );

    write(
        &harness,
        "fail.botwork",
        "No Operation\nAssert |1| Equals |2|",
    );
    let output = command(
        &harness,
        &["--file", "fail.botwork", "--report-json", "report.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = report(&harness, &output);
    let error = &value["runs"][0]["error"];
    assert_eq!(error["code"], "BW9001");
    assert_eq!(error["location"]["line"], 2);
    assert_eq!(value["runs"][0]["statements"][1]["code"], "BW9001");
    assert_eq!(value["verdict"]["status"], "failed");
}

#[test]
fn batch_reports_keep_selection_order_and_every_status() {
    let harness = Harness::new();
    write(&harness, "slow.botwork", "Sleep |10000|");
    write(&harness, "ok.botwork", "No Operation");
    write(&harness, "syntax.botwork", "Log |");
    write(
        &harness,
        "loud.botwork",
        "Log |\"too long for the budget\"|",
    );
    let output = command(
        &harness,
        &[
            "--file",
            "slow.botwork",
            "--file",
            "ok.botwork",
            "--file",
            "syntax.botwork",
            "--file",
            "loud.botwork",
            "--file",
            "missing.botwork",
            "--jobs",
            "5",
            "--timeout-ms",
            "300",
            "--max-output-bytes",
            "8",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = report(&harness, &output);
    assert_eq!(value["mode"], "batch");
    assert_eq!(
        statuses(&value),
        [
            "timed_out",
            "succeeded",
            "failed",
            "limit_exceeded",
            "failed"
        ]
    );
    let codes: Vec<_> = value["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["error"]["code"].clone())
        .collect();
    assert_eq!(
        codes,
        [
            json!("BW5002"),
            Value::Null,
            json!("BW1001"),
            json!("BW8001"),
            json!("BW7003")
        ]
    );
    let cases = &value["verdict"]["cases"];
    assert_eq!(
        (
            cases["succeeded"].clone(),
            cases["failed"].clone(),
            cases["timed_out"].clone(),
            cases["limit_exceeded"].clone()
        ),
        (json!(1), json!(2), json!(1), json!(1))
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.ends_with("[batch] 5 runs: 1 succeeded, 2 failed, 1 timed out, 1 limit exceeded\n"),
        "the console summary counts what the report counts: {stderr}"
    );
}

#[test]
fn suite_reports_include_rows_skips_and_shared_fixtures() {
    let harness = Harness::new();
    write(
        &harness,
        "rows.suite.botwork",
        r#"Suite |"rows"| {
    Dataset |"numbers"| {
        Row |"one"| Values |1|
        Row |"two"| Values |2|
    }
    SuiteSetup { No Operation }
    SuiteTeardown { No Operation }
    Case |"positive"| Using |"numbers"| As |number| { Assert |number < 2| }
}"#,
    );
    write(
        &harness,
        "broken.suite.botwork",
        r#"Suite |"broken"| {
    SuiteSetup { |x| = |missing| }
    SuiteTeardown { No Operation }
    Case |"blocked"| { No Operation }
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "rows.suite.botwork",
            "--suite",
            "broken.suite.botwork",
            "--jobs",
            "1",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = report(&harness, &output);
    assert_eq!(value["mode"], "suites");
    assert_eq!(statuses(&value), ["succeeded", "failed", "skipped"]);
    assert_eq!(
        value["runs"][1]["identity"],
        json!({"id": "rows/positive/two", "name": "positive / two", "dataset": "numbers", "row": "two"})
    );
    let skipped = &value["runs"][2];
    assert_eq!(skipped["identity"]["id"], "broken/blocked");
    assert_eq!(skipped["skip_reason"], "suite_setup_failed");
    assert!(skipped["started_at"].is_null() && skipped["statements"] == json!([]));
    let mut fixtures = value["fixtures"].as_array().unwrap().clone();
    fixtures.sort_by_key(|fixture| fixture["suite"].as_str().unwrap().to_owned());
    assert_eq!(fixtures[0]["suite"], "broken");
    assert_eq!(fixtures[0]["status"], "failed");
    assert_eq!(fixtures[0]["error"]["code"], "BW2001");
    assert_eq!(
        fixtures[1],
        json!({"suite": "rows", "status": "succeeded", "error": null})
    );
    assert_eq!(value["verdict"]["fixture_failures"], 1);
    assert_eq!(value["verdict"]["cases"]["skipped"], 1);
}

#[test]
fn assertion_artifacts_are_attached_to_their_runs() {
    let harness = Harness::new();
    write(&harness, "a.botwork", "Assert |1| Equals |2|");
    write(&harness, "b.botwork", "No Operation");
    for files in [&["a.botwork"][..], &["b.botwork", "a.botwork"]] {
        let mut arguments = vec![
            "--report-json",
            "report.json",
            "--assertion-artifacts",
            "evidence",
        ];
        for file in files {
            arguments.extend(["--file", file]);
        }
        let output = command(&harness, &arguments);
        assert_eq!(output.status.code(), Some(1));
        let value = report(&harness, &output);
        let runs = value["runs"].as_array().unwrap();
        let failed = runs.last().unwrap();
        assert_eq!(failed["identity"]["id"], "a.botwork");
        let artifact = &failed["artifacts"][0];
        assert_eq!(artifact["kind"], "assertion");
        let evidence: Value = serde_json::from_slice(
            &fs::read(harness.workspace.join(artifact["path"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        assert_eq!(evidence["format"], "botwork-assertion");
        for run in &runs[..runs.len() - 1] {
            assert_eq!(run["artifacts"], json!([]));
        }
    }
}

#[test]
fn reports_are_incomplete_until_the_verdict_is_delivered() {
    let harness = Harness::new();
    write(
        &harness,
        "peek.botwork",
        "Log |@{ Read File |\"report.json\"| }|",
    );
    let output = command(
        &harness,
        &["--file", "peek.botwork", "--report-json", "report.json"],
    );
    assert_eq!(output.status.code(), Some(0));
    let marker: Value = serde_json::from_slice(&output.stdout).unwrap();
    conforms(&marker);
    assert_eq!(marker["complete"], false);
    assert_eq!(marker["mode"], "file");
    assert!(marker["exit_code"].is_null() && marker["verdict"].is_null());
    assert_eq!(marker["runs"], json!([]));
    assert_eq!(report(&harness, &output)["complete"], true);

    // Suite discovery fails after the marker, so the marker stays.
    let output = command(
        &harness,
        &[
            "--suite",
            "absent.suite.botwork",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let value = report(&harness, &output);
    assert_eq!(value["complete"], false);
    assert_eq!(value["mode"], "suites");
    assert!(value["finished_at"].is_null() && value["exit_code"].is_null());
}

#[test]
fn unrelated_outputs_are_never_replaced_or_shared() {
    let harness = Harness::new();
    write(&harness, "effect.botwork", "Log |\"ran\"|");
    write(&harness, "report.json", "{\"format\": \"something else\"}");
    let output = command(
        &harness,
        &["--file", "effect.botwork", "--report-json", "report.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "no run starts before the report is accepted"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("never replaced"));
    assert_eq!(
        fs::read_to_string(harness.workspace.join("report.json")).unwrap(),
        "{\"format\": \"something else\"}"
    );

    fs::remove_file(harness.workspace.join("report.json")).unwrap();
    write(
        &harness,
        "only.suite.botwork",
        "Suite |\"only\"| {\n    Case |\"one\"| { No Operation }\n}",
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "only.suite.botwork",
            "--failures",
            "report.json",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "one path cannot hold both outputs"
    );
    assert!(output.stdout.is_empty());
    assert_eq!(report(&harness, &output)["complete"], false);
    assert!(!Path::new(&harness.workspace.join("report.json")).is_dir());
}

#[test]
fn report_exit_codes_match_the_process_for_every_outcome_mix() {
    for (scripts, code) in [
        (vec!["No Operation"], 0),
        (vec!["No Operation", "Fail |\"x\"|"], 1),
        (vec!["Sleep |10000|"], 1),
        (vec!["No Operation", "No Operation"], 0),
    ] {
        let harness = Harness::new();
        let mut arguments = vec![
            "--timeout-ms".to_owned(),
            "200".into(),
            "--report-json".into(),
            "report.json".into(),
        ];
        for (index, source) in scripts.iter().enumerate() {
            let name = format!("s{index}.botwork");
            write(&harness, &name, source);
            arguments.extend(["--file".to_owned(), name]);
        }
        let borrowed: Vec<_> = arguments.iter().map(String::as_str).collect();
        let output = command(&harness, &borrowed);
        assert_eq!(output.status.code(), Some(code));
        let value = report(&harness, &output);
        assert_eq!(value["complete"], true);
        assert_eq!(value["exit_code"], code);
        assert_eq!(value["verdict"]["complete"], true);
        assert_eq!(value["verdict"]["delivery"], "complete");
    }
}

#[test]
fn the_schema_rejects_undocumented_missing_and_unknown_values() {
    let harness = Harness::new();
    write(&harness, "fail.botwork", "Assert |false|");
    let output = command(
        &harness,
        &["--file", "fail.botwork", "--report-json", "report.json"],
    );
    let valid = report(&harness, &output);
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    type Mutation = (&'static str, fn(&mut Value));
    let mutations: [Mutation; 6] = [
        ("undocumented field", |value| value["extra"] = json!(1)),
        ("missing field", |value| {
            value["runs"][0].as_object_mut().unwrap().remove("events");
        }),
        ("unknown status", |value| {
            value["runs"][0]["status"] = json!("passed")
        }),
        ("bad code", |value| {
            value["runs"][0]["error"]["code"] = json!("E1")
        }),
        ("bad timestamp", |value| {
            value["started_at"] = json!("yesterday")
        }),
        ("newer version", |value| value["version"] = json!(2)),
    ];
    for (name, mutate) in mutations {
        let mut value = valid.clone();
        mutate(&mut value);
        assert!(
            validate(&schema, &schema, &value, "$").is_err(),
            "{name} must be rejected"
        );
    }
}

/// The contents of the first fenced block after `opening`.
fn fenced<'a>(document: &'a str, opening: &str) -> &'a str {
    let start = document.find(opening).expect("documented block") + opening.len();
    let length = document[start..].find("\n```\n").expect("closing fence") + 1;
    &document[start..start + length]
}

/// Replace values that vary between runs, keeping whether each was present.
fn normalize(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, field) in object.iter_mut() {
                if field.is_null() {
                    continue;
                }
                if key.ends_with("_at") {
                    *field = json!("<timestamp>");
                } else if matches!(key.as_str(), "duration_us" | "offset_us") {
                    *field = json!(0);
                } else {
                    normalize(field);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize),
        _ => {}
    }
}

#[test]
fn the_documented_report_is_what_the_documented_suite_produces() {
    let document = include_str!("../docs/json-report.md");
    let suite = fenced(
        document,
        "<!-- botwork-test: json-report-suite -->\n```botwork-suite\n",
    );
    let mut documented: Value = serde_json::from_str(fenced(document, "```json\n")).unwrap();
    conforms(&documented);
    let harness = Harness::new();
    write(&harness, "checkout.suite.botwork", suite);
    let output = command(
        &harness,
        &[
            "--suite",
            "checkout.suite.botwork",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let mut actual = report(&harness, &output);
    normalize(&mut documented);
    normalize(&mut actual);
    assert_eq!(actual, documented);
}
