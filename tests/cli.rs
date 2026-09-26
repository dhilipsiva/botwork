use std::path::PathBuf;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(args)
        .output()
        .expect("run botwork CLI")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn help_describes_file_argument_on_stdout() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--file"));
    assert!(output.stderr.is_empty());
}

#[test]
fn version_matches_package_version() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("botwork {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_file_argument_fails_on_stderr() {
    let output = run(&[]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--file"));
}

#[test]
fn nonexistent_file_fails_on_stderr() {
    let path = fixture("does-not-exist.botwork");
    assert!(!path.exists());
    let output = run(&["--file", path.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn assignment_program_succeeds_without_output() {
    let path = fixture("assignment.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn caught_evaluation_error_succeeds_without_diagnostics() {
    let path = fixture("caught-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn runtime_error_reports_file_and_variable_and_stops_execution() {
    let path = fixture("runtime-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("runtime-error.botwork"));
    assert!(diagnostic.contains("Variable not defined: missing"));
    assert!(!diagnostic.contains("must not execute"));
}

#[test]
fn syntax_error_reports_source_location_on_stderr() {
    let path = fixture("syntax-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("syntax-error.botwork"));
    assert!(diagnostic.contains("1:14"));
    assert!(diagnostic.contains("expected"));
}

#[test]
fn runtime_failure_preserves_prior_output_but_skips_later_statements() {
    let path = fixture("runtime-after-output.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let messages = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(messages.contains("before failure"));
    assert!(messages.contains("Variable not defined: missing"));
    assert!(!messages.contains("after failure"));
}

#[test]
fn syntax_failure_prevents_execution_of_the_whole_program() {
    let path = fixture("syntax-after-output.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("syntax-after-output.botwork"));
    assert!(!diagnostic.contains("must not execute"));
}

#[test]
fn log_writes_readable_values_to_stdout() {
    let path = fixture("log-values.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "hello\n7\n1.5\ntrue\n[1, \"x\", false]\n{\"a\": 1, \"z\": 2}\nfirst\nsecond\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn debug_traces_locations_without_changing_stdout() {
    let path = fixture("log-values.botwork");
    let normal = run(&["--file", path.to_str().unwrap()]);
    let debug = run(&["--debug", "--file", path.to_str().unwrap()]);
    assert!(debug.status.success());
    assert_eq!(debug.stdout, normal.stdout);
    let trace = String::from_utf8(debug.stderr).unwrap();
    let lines: Vec<_> = trace.lines().collect();
    assert_eq!(lines.len(), 7);
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(
            *line,
            format!("debug: {}:{}:1: call", path.display(), index + 1)
        );
    }
    assert!(
        !trace.contains("hello"),
        "trace should not copy literal values"
    );
}
