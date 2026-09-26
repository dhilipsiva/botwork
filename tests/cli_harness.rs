#[path = "support/cli_harness.rs"]
mod cli_harness;

use cli_harness::Harness;
use std::time::Duration;

#[test]
fn harness_keeps_streams_and_status_separate_and_removes_only_its_workspace() {
    let harness = Harness::new();
    let path = harness.workspace.clone();
    let second = Harness::new();
    assert_ne!(path, second.workspace);
    let output = harness
        .run(
            "failure",
            "Log |\"before\"|\nLog |missing|",
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\n");
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("missing"));
    drop(harness);
    assert!(!path.exists());
    assert!(second.workspace.exists());
}

#[test]
fn timeout_terminates_and_reaps_a_nonterminating_script_and_allows_the_next_case() {
    let harness = Harness::new();
    let error = harness
        .run_with_args(
            "nonterminating",
            "While |true| {}",
            &["--max-steps", "18446744073709551615"],
            Duration::from_millis(100),
        )
        .unwrap_err();
    assert!(error.contains("nonterminating timed out"));
    assert!(error.contains("seed=none"));
    let output = harness
        .run("after-timeout", "Log |7|", Duration::from_secs(5))
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"7\n");
    assert!(output.stderr.is_empty());
}
