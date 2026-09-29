use std::path::Path;
use std::process::Command;

fn assert_example(name: &str, expected_lines: &[&str]) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name);
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--file")
        .arg(path)
        .output()
        .expect("run bundled example");
    assert!(
        output.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "{name}: unexpected diagnostic");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        expected_lines.join("\n") + "\n",
        "{name}: example behavior changed"
    );
}

#[test]
fn process_example_captures_text_binary_and_nonzero_exit() {
    assert_example(
        "33-processes.botwork",
        &["Hello, world!", "7", "diagnostic", "[0, 255, 10]"],
    );
}

#[test]
fn operating_system_example_copies_files_and_removes_its_temporary_directory() {
    assert_example(
        "32-operating-system.botwork",
        &[
            "hello world",
            "11",
            "[\"copy.txt\", \"message.txt\"]",
            "txt",
        ],
    );
}

#[test]
fn datetime_example_resolves_a_repeated_local_time_explicitly() {
    assert_example(
        "31-datetime.botwork",
        &[
            "2024-11-03T05:30:00Z",
            "2024-11-03T01:30:00-05:00",
            "PT1H",
            "1",
            "2024-11-03 01:30:00 EST",
        ],
    );
}

#[test]
fn string_example_transforms_unicode_and_extracts_regex_values() {
    assert_example(
        "30-strings.botwork",
        &[
            "Items: tea / coffee /  / water",
            "tea, coffee, , water",
            "STRASSE",
            "🙂é",
            r#"["42", "3"]"#,
            r#"["order=42", "42"]"#,
        ],
    );
}

#[test]
fn collection_example_keeps_original_values_and_iterates_deterministically() {
    assert_example(
        "29-collections.botwork",
        &[
            r#"["coffee", 1]"#,
            r#"["tea", 4]"#,
            "[20, 30]",
            r#"[[0, "a"], [1, "b"]]"#,
            "2",
        ],
    );
}

#[test]
fn cleanup_example_releases_nested_resources_before_returning() {
    assert_example(
        "25-cleanup.botwork",
        &[
            "open outer",
            "open inner",
            "close inner",
            "close outer",
            "42",
            "recovered",
            "finished",
        ],
    );
}

#[test]
fn expressions_example_produces_expected_values() {
    assert_example(
        "01-expressions.botwork",
        &[
            "3",
            "2.67",
            "-7.62",
            "true",
            "false",
            "Hello World",
            "Hello \n World",
            r#"[3, 5.5, -7.62, true, false, "Hello World"]"#,
            r#"{"boolean": true, "decimal": 2.67, "negative": -7.62, "not_boolean": false, "number": 3, "string": "Hello World"}"#,
            r#"[11, 3, {"a": 5}, 30, false, true, false, {"a": {"b": false}}]"#,
            "27.5",
        ],
    );
}

#[test]
fn named_suite_example_discovers_and_executes_imported_helpers() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/23-named-cases.suite.botwork");
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--suite")
        .arg(path)
        .args(["--jobs", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n8\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[case arithmetic/double] succeeded:"));
    assert!(stderr.contains("[case arithmetic/composition] succeeded:"));
    assert!(stderr.ends_with("[cases] 2 selected: 2 succeeded, 0 failed\n"));
}

#[test]
fn parameterized_example_executes_inline_and_shared_rows() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/24-parameterized-cases.suite.botwork");
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--suite")
        .arg(path)
        .args(["--jobs", "1"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    assert_eq!(output.stdout, b"42\n8\n0\nhello\n");
    for id in [
        "parameters/double/answer",
        "parameters/double/small",
        "parameters/double/zero",
        "parameters/greet/hello",
    ] {
        assert_eq!(
            stderr.matches(&format!("[case {id}] succeeded:")).count(),
            1
        );
    }
    assert!(stderr.ends_with("[cases] 4 selected: 4 succeeded, 0 failed\n"));
}

#[test]
fn parallel_examples_also_run_independently_with_their_own_bindings() {
    assert_example(
        "21-parallel-first.botwork",
        &[r#"{"count": 1, "script": "first"}"#],
    );
    assert_example(
        "22-parallel-second.botwork",
        &[r#"{"count": 2, "script": "second"}"#],
    );
}

#[test]
fn precedence_example_produces_expected_values() {
    assert_example(
        "03-precedence.botwork",
        &["true", "true", "true", "false", "13", "5"],
    );
}

#[test]
fn arithmetic_example_handles_errors_and_preserves_valid_boundaries() {
    assert_example(
        "04-arithmetic-errors.botwork",
        &[
            "caught remainder error",
            "caught overflow",
            "0.125",
            "-2147483648",
        ],
    );
}

#[test]
fn power_example_checks_grouping_and_catchable_intermediate_overflow() {
    assert_example(
        "05-powers.botwork",
        &[
            "512",
            "64",
            "-4",
            "4",
            "0.0625",
            "2",
            "true",
            "caught intermediate overflow",
            "-2147483648",
        ],
    );
}

#[test]
fn keyword_example_preserves_identifier_and_statement_names() {
    assert_example(
        "06-keywords.botwork",
        &["8", "punctuation name", r#"{"order": 7, "trueValue": 8}"#],
    );
}

#[test]
fn short_circuit_example_skips_errors_and_handles_required_operands() {
    assert_example(
        "07-short-circuit.botwork",
        &["false", "true", "3", "caught required operand"],
    );
}

#[test]
fn control_flow_example_returns_exact_values_and_resumes_at_the_correct_boundary() {
    assert_example(
        "08-control-flow.botwork",
        &["7", "none", "7", "42", "none", "1", "3", "after loop"],
    );
}

#[test]
fn scope_example_preserves_callers_and_restores_loop_bindings() {
    assert_example(
        "09-scopes.botwork",
        &[
            "[1, 10]",
            "10",
            "99",
            "10",
            "120",
            "1",
            "10",
            "7",
            "inner is local",
        ],
    );
}

#[test]
fn signed_integer_example_handles_boundaries_and_catchable_failures() {
    assert_example(
        "10-signed-integers.botwork",
        &[
            "-2147483648",
            "2147483647",
            "-2147483648",
            "1",
            "0",
            "caught literal range error",
            "caught negation overflow",
            "caught positive base range error",
        ],
    );
}

#[test]
fn multilingual_example_uses_tamil_identifiers_and_preserves_combining_marks() {
    assert_example(
        "16-multilingual.botwork",
        &[
            "5",
            "தமிழ்",
            "[\"தமிழ்\", 5]",
            "3",
            "[7, 8]",
            "duplicate accented statement",
        ],
    );
}

#[test]
fn statement_name_example_matches_calls_and_preserves_definitions_after_collisions() {
    assert_example(
        "15-statement-names.botwork",
        &[
            "7",
            "caught duplicate definition",
            "7",
            "[1, 2]",
            "[2, 1]",
            "native Log preserved",
        ],
    );
}

#[test]
fn multiline_layout_example_preserves_delimiters_and_statement_boundaries() {
    assert_example(
        "14-multiline-layout.botwork",
        &["[2, 3]", "é🙂 | # { }", "5", "recovered"],
    );
}

#[test]
fn comparison_example_preserves_precision_and_checks_structural_values() {
    assert_example(
        "13-value-comparisons.botwork",
        &[
            "false",
            "true",
            "true",
            "true",
            "true",
            "true",
            "false",
            "true",
            "false",
            "false",
            "true",
            "true",
            "ordering requires numbers",
            "operands must evaluate first",
        ],
    );
}

#[test]
fn computed_access_example_handles_arbitrary_keys_and_checked_indexes() {
    assert_example(
        "12-computed-access.botwork",
        &[
            "second",
            "application/json",
            "empty key",
            "9",
            "5",
            "12",
            "caught negative index",
            "caught index type",
            "caught missing key",
            "false",
        ],
    );
}

#[test]
fn collection_access_example_reads_nested_values_and_handles_errors() {
    assert_example(
        "11-collection-access.botwork",
        &[
            "first",
            "12",
            "8",
            "caught missing key",
            "caught bounds error",
            "caught non-collection",
            "false",
        ],
    );
}

#[test]
fn catch_details_example_inspects_errors_restores_bindings_and_rethrows_originals() {
    assert_example(
        "17-catch-details.botwork",
        &[
            "BW2001",
            "missing",
            "outside",
            "rethrowing",
            "[\"BW3002\", true]",
            "outside",
            "BW2001",
        ],
    );
}

#[test]
fn syntax_example_executes_all_intended_paths() {
    assert_example(
        "02-syntaxes.botwork",
        &[
            "Hello, World!",
            "Hello, World!",
            "Hello, World!",
            "Hello, World!",
            "Hello, World!",
            "Hello, World!",
            "true",
            "false",
            "3",
            "3.3",
            "Hi, There!",
            r#"[true, false, 3, 3.3, "Hi, There!"]"#,
            r#"{"array": [1, 1.3], "bool_false": false, "bool_true": true, "dict": {"key": "value"}, "float": 3.3, "integer": 3, "string": "Hi, There!"}"#,
            "64",
            "Here is your answer:",
            "18",
            "b is greater",
            "b is greater again",
            "1",
            "2",
            "3",
            "for: i is greater than 3, breaking...",
            "for: i is lesser than 3, skipping log...",
            "for: i is lesser than 3, skipping log...",
            "3",
            "4",
            "5",
            "while: i is lesser than 3, skipping log...",
            "while: i is lesser than 3, skipping log...",
            "3",
            "4",
            "5",
            "6",
            "while: i is greater than 5, breaking...",
            "5",
            "This will always execute",
            "This will always execute",
        ],
    );
}

#[test]
fn composition_example_checks_nested_results_effect_order_and_recovery() {
    assert_example(
        "18-call-composition.botwork",
        &[
            "13", "3", "false", "120", "first", "second", "[1, 2]", "BW2001",
        ],
    );
}

#[test]
fn local_import_example_checks_namespaces_results_and_collision_recovery() {
    assert_example(
        "19-local-imports.botwork",
        &["14", "[1, 2]", "BW6003", "10"],
    );
}

#[test]
fn fixture_example_executes_both_owners_around_each_row() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/26-fixtures.suite.botwork");
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--suite")
        .arg(path)
        .args(["--jobs", "1"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    assert_eq!(
        output.stdout,
        b"suite opened\n41\ncase closed\n42\ncase closed\nsuite closed\n"
    );
    assert!(stderr.ends_with("[cases] 2 selected: 2 succeeded, 0 failed\n"));
}

#[test]
fn setup_failure_example_reports_both_errors_and_skips_unentered_cases() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/27-setup-failure.suite.botwork");
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--suite")
        .arg(path)
        .args(["--jobs", "1"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(output.stdout, b"open demo\nclose demo\n");
    assert!(stderr.find("BW2001").unwrap() < stderr.find("BW2002").unwrap());
    assert!(!stderr.contains("[case environment/first] started:"));
    assert!(stderr.ends_with(
        "[cases] 2 selected: 0 succeeded, 0 failed, 2 skipped; 1 suite fixtures failed\n"
    ));
}

#[test]
fn polling_example_observes_readiness_retries_an_action_and_catches_exhaustion() {
    assert_example("38-polling.botwork", &["3", "2", "BW9004", "2", "BW9001"]);
}

#[test]
fn builtins_example_demonstrates_checks_inspection_recovery_and_waiting() {
    assert_example(
        "28-builtins.botwork",
        &["42", "Int", "BW9001", "cleanup", "BW9002", "finished"],
    );
}

#[path = "support/http_fixture.rs"]
mod http_fixture;
#[test]
fn http_example_executes_against_loopback_endpoint() {
    let fixture = http_fixture::Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--file")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/34-http.botwork"))
        .env("BOTWORK_HTTP_URL", &fixture.url)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        include_bytes!("doc-examples/http-statements.stdout")
    );
}
