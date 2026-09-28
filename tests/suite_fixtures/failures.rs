use super::*;

fn with_jobs(harness: &Harness, jobs: &str, extra: &[&str]) -> Output {
    let mut args = vec![
        "--suite",
        "suite.botwork",
        "--jobs",
        jobs,
        "--failures",
        "failed.json",
    ];
    args.extend_from_slice(extra);
    harness
        .command("failure-policy", &args, Duration::from_secs(15))
        .unwrap()
}
fn lines(output: &Output) -> Vec<String> {
    let mut lines: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

#[test]
fn failed_suite_skips_only_selected_cases_and_does_not_stop_another_suite() {
    for jobs in ["1", "2"] {
        let harness = Harness::new();
        let failed = r#"Suite |"s"| {
SuiteSetup { MissingSetup }
SuiteTeardown { Log |"released"|
    CleanupFailed }
CaseSetup { Log |"case setup"| }
CaseTeardown { Log |"case teardown"| }
Case |"a"| { Log |"a"| }
Case |"b"| { Log |"b"| }
Case |"excluded"| Tags |["omit"]| { Log |"excluded"| }
}"#;
        source(&harness, failed);
        fs::write(
            harness.workspace.join("healthy.botwork"),
            "Suite |\"healthy\"| { Case |\"a\"| { Log |\"sibling\"| } }",
        )
        .unwrap();
        let args = ["--suite", "healthy.botwork", "--exclude-tag", "omit"];
        let output = with_jobs(&harness, jobs, &args);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(lines(&output), ["released", "sibling"]);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.find("MissingSetup").unwrap() < stderr.find("CleanupFailed").unwrap(),
            "{stderr}"
        );
        for id in ["s/a", "s/b"] {
            assert_eq!(
                stderr.matches(&format!("[case {id}] skipped:")).count(),
                1,
                "{stderr}"
            );
            for status in ["started", "succeeded", "failed"] {
                assert!(
                    !stderr.contains(&format!("[case {id}] {status}:")),
                    "{stderr}"
                );
            }
        }
        assert!(!stderr.contains("[case s/excluded]"));
        assert!(stderr.contains("[case healthy/a] succeeded:"));
        assert!(
            stderr.ends_with(
                "[cases] 3 selected: 1 succeeded, 0 failed, 2 skipped; 1 suite fixtures failed\n"
            ),
            "{stderr}"
        );
        assert_eq!(state(&harness)["failed"], json!(["s/a", "s/b"]));
        assert_eq!(state(&harness)["complete"], true);

        source(
            &harness,
            &failed
                .replace("MissingSetup", "")
                .replace("CleanupFailed", ""),
        );
        let output = with_jobs(
            &harness,
            jobs,
            &[
                "--suite",
                "healthy.botwork",
                "--exclude-tag",
                "omit",
                "--rerun-failed",
                "failed.json",
            ],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert_eq!(
            lines(&output),
            [
                "a",
                "b",
                "case setup",
                "case setup",
                "case teardown",
                "case teardown",
                "released"
            ]
        );
        assert!(!stderr.contains("healthy/a") && !stderr.contains("s/excluded"));
        assert_eq!(state(&harness)["failed"], json!([]));
    }
}

#[test]
fn recovered_cleanup_does_not_pass_failed_case_setup_and_only_that_row_reruns() {
    let harness = Harness::new();
    let failed = r#"Suite |"s"| {
Dataset |"d"| { Row |"bad"| Values |0| Row |"good"| Values |1| }
CaseSetup { If |n == 0| { |x| = |missing| } }
CaseTeardown { Try { |x| = |1 / 0| } Catch { Log |100 + n| } }
Case |"rows"| Using |"d"| As |n| { Log |n| }
}"#;
    source(&harness, failed);
    let stderr = check(
        command(&harness, &["--failures", "failed.json"]),
        1,
        "100\n1\n101\n",
    );
    assert_eq!(stderr.matches("[case s/rows/bad] started:").count(), 1);
    assert_eq!(stderr.matches("[case s/rows/bad] failed:").count(), 1);
    assert!(!stderr.contains("skipped:") && !stderr.contains("[case s/rows/bad] succeeded:"));
    assert!(
        !stderr.contains("BW3002"),
        "caught teardown errors stay handled: {stderr}"
    );
    assert!(stderr.contains("BW2001"));
    assert!(stderr.ends_with("[cases] 2 selected: 1 succeeded, 1 failed\n"));
    assert_eq!(state(&harness)["failed"], json!(["s/rows/bad"]));
    source(&harness, &failed.replace("missing", "0"));
    check(
        command(
            &harness,
            &["--rerun-failed", "failed.json", "--failures", "failed.json"],
        ),
        0,
        "0\n100\n",
    );
    assert_eq!(state(&harness)["failed"], json!([]));
}

#[test]
fn shared_cleanup_failure_keeps_case_outcomes_and_both_local_causes_visible() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| { Row |"bad"| Values |0| Row |"good"| Values |1| }
SuiteTeardown { SuiteCleanupFailed }
CaseSetup { If |n == 0| { |x| = |missing| } }
CaseTeardown { If |n == 0| { |x| = |1 / 0| } }
Case |"rows"| Using |"d"| As |n| { Log |n| }
}"#,
    );
    let stderr = check(command(&harness, &["--failures", "failed.json"]), 1, "1\n");
    let primary = stderr.find("BW2001").unwrap();
    let cleanup = stderr.find("BW3002").unwrap();
    let suite_cleanup = stderr.find("SuiteCleanupFailed").unwrap();
    assert!(primary < cleanup && cleanup < suite_cleanup, "{stderr}");
    assert_eq!(stderr.matches("[case s/rows/bad] failed:").count(), 1);
    assert_eq!(stderr.matches("[case s/rows/good] succeeded:").count(), 1);
    assert!(!stderr.contains("[case s/rows/good] failed:"));
    assert!(stderr.ends_with(
        "[cases] 2 selected: 1 succeeded, 1 failed, 0 skipped; 1 suite fixtures failed\n"
    ));
    assert_eq!(
        state(&harness)["failed"],
        json!(["s/rows/bad", "s/rows/good"])
    );
}

#[test]
fn excluding_every_case_is_a_selection_error_without_skipped_outcomes() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| Tags |["omit"]| {
SuiteSetup { Log |999| }
SuiteTeardown { Log |999| }
Case |"a"| { Log |999| }
}"#,
    );
    let stderr = check(
        command(
            &harness,
            &["--exclude-tag", "omit", "--failures", "failed.json"],
        ),
        1,
        "",
    );
    assert!(!stderr.contains("[case ") && !stderr.contains("[cases]"));
    assert_eq!(state(&harness)["complete"], false);
}
