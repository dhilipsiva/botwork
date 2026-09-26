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
