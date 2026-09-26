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
