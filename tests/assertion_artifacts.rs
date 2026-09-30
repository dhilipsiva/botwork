#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

fn records(parent: &Path) -> Vec<(PathBuf, Value)> {
    let mut records = Vec::new();
    for directory in fs::read_dir(parent).unwrap() {
        for file in fs::read_dir(directory.unwrap().path()).unwrap() {
            let path = file.unwrap().path();
            assert_eq!(path.extension().unwrap(), "json", "no partial artifacts");
            records.push((
                path.clone(),
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap(),
            ));
        }
    }
    records
}

#[test]
fn single_file_keeps_large_operands_in_private_artifacts_and_previews_in_stderr() {
    let harness = Harness::new();
    let expected = "🙂".repeat(25_000);
    let actual = format!("{expected}x");
    fs::write(
        harness.workspace.join("input.json"),
        json!({"actual":actual,"expected":expected}).to_string(),
    )
    .unwrap();
    let output = harness
        .run_with_args(
            "large",
            "Assert |actual| Equals |expected|",
            &[
                "--vars-file",
                "input.json",
                "--assertion-artifacts",
                "artifacts",
            ],
            Duration::from_secs(30),
        )
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[BW9001]")
            && stderr.contains("…[truncated]")
            && stderr.contains("[assertion artifact]"),
        "{stderr}"
    );
    assert!(stderr.len() < 4000);
    let artifacts = records(&harness.workspace.join("artifacts"));
    assert_eq!(artifacts.len(), 1);
    let (path, record) = &artifacts[0];
    assert!(path.is_file());
    assert_eq!(record["format"], "botwork-assertion");
    assert_eq!(record["version"], 1);
    assert_eq!(record["identity"]["run"], 1);
    assert!(record["identity"]["case"].is_null());
    assert!(record["source"]["file"]
        .as_str()
        .unwrap()
        .ends_with("large.botwork"));
    assert_eq!(record["source"]["line"], 1);
    assert_eq!(record["source"]["column"], 1);
    assert_eq!(record["operands_complete"], true);
    assert_eq!(record["diagnostic_omissions"], false);
    assert_eq!(
        serde_json::from_str::<Value>(record["actual_typed_json"].as_str().unwrap()).unwrap()
            ["value"],
        actual
    );
    assert_eq!(
        serde_json::from_str::<Value>(record["expected_typed_json"].as_str().unwrap()).unwrap()
            ["value"],
        expected
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
}

#[test]
fn concurrent_dataset_failures_keep_case_row_module_and_dataset_identity() {
    for shared_fixture in [false, true] {
        let harness = Harness::new();
        fs::write(
            harness.workspace.join("helper.botwork"),
            "Check |number| { Assert |number| Equals |0| }",
        )
        .unwrap();
        let fixture = if shared_fixture {
            "SuiteSetup { No Operation } SuiteTeardown { No Operation }"
        } else {
            ""
        };
        let source = format!(
            r#"Suite |"contracts"| {{
Dataset |"numbers"| {{ Row |"one"| Values |1| Row |"two"| Values |2| }}
Library {{ Import |"helper.botwork"| As |check| }}
{fixture}
Case |"positive"| Using |"numbers"| As |number| {{ check::Check |number| }}
}}"#
        );
        fs::write(harness.workspace.join("checks.suite.botwork"), source).unwrap();
        let output = harness
            .command(
                "dataset",
                &[
                    "--suite",
                    "checks.suite.botwork",
                    "--jobs",
                    "2",
                    "--assertion-artifacts",
                    "artifacts",
                ],
                Duration::from_secs(30),
            )
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("[dataset \"numbers\", row \"one\"]"),
            "{stderr}"
        );
        assert!(
            stderr.contains("[dataset \"numbers\", row \"two\"]"),
            "{stderr}"
        );
        let mut artifacts = records(&harness.workspace.join("artifacts"));
        artifacts.sort_by_key(|(_, value)| value["identity"]["run"].as_u64());
        assert_eq!(artifacts.len(), 2, "{stderr}");
        for (index, (_, record)) in artifacts.iter().enumerate() {
            let row = if index == 0 { "one" } else { "two" };
            assert_eq!(
                record["identity"]["case"],
                format!("contracts/positive/{row}")
            );
            assert_eq!(record["identity"]["row"], row);
            assert_eq!(record["identity"]["dataset"], "numbers");
            assert!(record["source"]["file"]
                .as_str()
                .unwrap()
                .ends_with("helper.botwork"));
            assert_eq!(
                serde_json::from_str::<Value>(record["actual_typed_json"].as_str().unwrap())
                    .unwrap()["value"],
                index + 1
            );
        }
    }
}

#[test]
fn repeated_file_occurrences_and_invocations_never_overwrite_artifacts() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("failure.botwork"), "Assert |false|").unwrap();
    for _ in 0..2 {
        let output = harness
            .command(
                "batch",
                &[
                    "--file",
                    "failure.botwork",
                    "--file",
                    "failure.botwork",
                    "--assertion-artifacts",
                    "artifacts",
                ],
                Duration::from_secs(30),
            )
            .unwrap();
        assert!(!output.status.success());
    }
    let artifacts = records(&harness.workspace.join("artifacts"));
    assert_eq!(artifacts.len(), 4);
    assert_eq!(
        fs::read_dir(harness.workspace.join("artifacts"))
            .unwrap()
            .count(),
        2
    );
    for run in [1, 2] {
        assert_eq!(
            artifacts
                .iter()
                .filter(|(_, value)| value["identity"]["run"] == run)
                .count(),
            2
        );
    }
}

#[test]
fn body_and_cleanup_assertions_both_remain_available() {
    let harness = Harness::new();
    let output = harness
        .run_with_args(
            "cleanup",
            "Try { Assert |false| } Finally { Assert |1| Equals |2| }",
            &["--assertion-artifacts", "artifacts"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert!(!output.status.success());
    let artifacts = records(&harness.workspace.join("artifacts"));
    assert_eq!(artifacts.len(), 2);
    assert!(artifacts
        .iter()
        .any(|(_, value)| value["cause_path"] == json!([])));
    assert!(artifacts
        .iter()
        .any(|(_, value)| value["cause_path"] == json!([0])));
}

#[test]
fn suite_fixture_failure_is_identified_without_inventing_a_case() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("fixture.suite.botwork"), r#"Suite |"owner"| { SuiteSetup { Assert |false| } SuiteTeardown { No Operation } Case |"skipped"| { No Operation } }"#).unwrap();
    let output = harness
        .command(
            "fixture",
            &[
                "--suite",
                "fixture.suite.botwork",
                "--assertion-artifacts",
                "artifacts",
            ],
            Duration::from_secs(30),
        )
        .unwrap();
    assert!(!output.status.success());
    let artifacts = records(&harness.workspace.join("artifacts"));
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].1["identity"]["suite_fixture"], "owner");
    assert!(artifacts[0].1["identity"]["case"].is_null());
}

#[test]
fn export_is_opt_in_and_caught_assertions_do_not_export_terminal_artifacts() {
    let harness = Harness::new();
    let output = harness
        .run("default", "Assert |false|", Duration::from_secs(30))
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8(output.stderr)
        .unwrap()
        .contains("[assertion artifact]"));
    let output = harness
        .run_with_args(
            "caught",
            r#"Try { Assert |false| } Catch |error| { Log |error.details.actual| }"#,
            &["--assertion-artifacts", "artifacts"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"kind":"Bool","value":false})
    );
    assert!(records(&harness.workspace.join("artifacts")).is_empty());
}

#[test]
fn unusable_artifact_destination_fails_before_script_effects() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("occupied"), "preserve").unwrap();
    let output = harness
        .run_with_args(
            "setup",
            r#"Write File |"effect.txt"| Text |"ran"| Assert |false|"#,
            &["--assertion-artifacts", "occupied"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert!(!output.status.success());
    assert!(!harness.workspace.join("effect.txt").exists());
    assert_eq!(
        fs::read(harness.workspace.join("occupied")).unwrap(),
        b"preserve"
    );
}

#[test]
fn bundled_diagnostic_suite_has_one_pass_one_failure_and_the_correct_artifact() {
    let harness = Harness::new();
    let example = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/37-assertion-diagnostics.suite.botwork");
    let output = harness
        .command(
            "example",
            &[
                "--suite",
                example.to_str().unwrap(),
                "--jobs",
                "2",
                "--assertion-artifacts",
                "artifacts",
            ],
            Duration::from_secs(30),
        )
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[case diagnostics/greeting/unchanged] succeeded:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("[case diagnostics/greeting/changed] failed:"),
        "{stderr}"
    );
    assert!(stderr.contains("difference at $[\"message\"]"), "{stderr}");
    assert!(
        stderr.ends_with("[cases] 2 selected: 1 succeeded, 1 failed\n"),
        "{stderr}"
    );
    let records = records(&harness.workspace.join("artifacts"));
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].1["identity"]["case"],
        "diagnostics/greeting/changed"
    );
}
