#[path = "support/cli_harness.rs"]
mod cli_harness;
#[path = "suite_fixtures/failures.rs"]
mod failures;
use cli_harness::Harness;
use serde_json::{json, Value};
use std::{fs, process::Output, time::Duration};

#[cfg(target_os = "linux")]
#[path = "suite_fixtures/linux.rs"]
mod linux;

fn source(harness: &Harness, text: &str) {
    fs::write(harness.workspace.join("suite.botwork"), text).unwrap();
}
fn command(harness: &Harness, extra: &[&str]) -> Output {
    let mut args = vec!["--suite", "suite.botwork"];
    if !extra.contains(&"--list-cases") {
        args.extend(["--jobs", "1"]);
    }
    args.extend_from_slice(extra);
    harness
        .command("fixtures", &args, Duration::from_secs(15))
        .unwrap()
}
fn state(harness: &Harness) -> Value {
    serde_json::from_slice(&fs::read(harness.workspace.join("failed.json")).unwrap()).unwrap()
}
fn check(output: Output, code: i32, stdout: &str) -> String {
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(code), "{stderr}");
    assert_eq!(output.stdout, stdout.as_bytes(), "{stderr}");
    stderr
}

#[test]
fn owners_keep_distinct_state_and_each_row_receives_case_hooks() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Log |\"library\"|\nRead { Return |17| }",
    )
    .unwrap();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| { Row |"a"| Values |1| Row |"b"| Values |2| }
Library { Import |"module.botwork"| As |m| }
SuiteSetup { |n| = |99|
    |shared| = |{"value": 42}|
    Log |"suite setup"| }
SuiteTeardown { Log |n|
    Log |shared.value|
    Log |@{ m::Read }| }
CaseSetup { Log |n|
    Log |shared.value|
    |local| = |n + 10| }
CaseTeardown { Log |local| }
Case |"rows"| Using |"d"| As |n| { |shared| = |{"value": 0}|
    |local| = |local + 1| }
}"#,
    );
    check(
        command(&harness, &[]),
        0,
        "library\nsuite setup\nlibrary\n1\n42\n12\nlibrary\n2\n42\n13\n99\n42\n17\n",
    );
}

#[test]
fn case_only_hooks_and_teardown_only_suites_have_explicit_owners() {
    let harness = Harness::new();
    for wide in ["", "SuiteTeardown { Log |7| }"] {
        source(
            &harness,
            &format!(
                r#"Suite |"s"| {{
{wide}
CaseSetup {{ |x| = |1| }}
CaseTeardown {{ Log |x| }}
Case |"a"| {{ |x| = |2| }}
Case |"b"| {{}}
}}"#
            ),
        );
        check(
            command(&harness, &[]),
            0,
            if wide.is_empty() {
                "2\n1\n"
            } else {
                "2\n1\n7\n"
            },
        );
    }
}

#[test]
fn failed_case_setup_skips_its_body_and_preserves_both_failures() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| {
SuiteSetup { Log |1| }
SuiteTeardown { Log |5| }
CaseSetup { Log |2|
    |x| = |missing| }
CaseTeardown { Log |3|
    Other }
Case |"a"| { Log |999| }
Case |"b"| { Log |999| }
}"#,
    );
    let stderr = check(
        command(&harness, &["--failures", "failed.json"]),
        1,
        "1\n2\n3\n2\n3\n5\n",
    );
    assert!(
        stderr.contains("BW2001") && stderr.contains("Other"),
        "{stderr}"
    );
    assert!(
        stderr.contains("[cases] 2 selected: 0 succeeded, 2 failed"),
        "{stderr}"
    );
    assert_eq!(state(&harness)["failed"], json!(["s/a", "s/b"]));
    assert_eq!(state(&harness)["complete"], true);
}

#[test]
fn failed_suite_setup_cleans_partial_state_and_marks_unentered_cases_skipped() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| {
SuiteSetup { |opened| = |true|
    Missing }
SuiteTeardown { Log |opened|
    Other }
CaseSetup { Log |999| }
CaseTeardown { Log |999| }
Case |"a"| { Log |999| }
Case |"b"| { Log |999| }
}"#,
    );
    let stderr = check(
        command(&harness, &["--failures", "failed.json"]),
        1,
        "true\n",
    );
    assert!(
        stderr.contains("Missing") && stderr.contains("Other"),
        "{stderr}"
    );
    assert!(
        stderr.contains("0 succeeded, 0 failed, 2 skipped; 1 suite fixtures failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("[case s/a] started:"));
    assert_eq!(state(&harness)["failed"], json!(["s/a", "s/b"]));
    assert_eq!(state(&harness)["complete"], true);
}

#[test]
fn suite_teardown_failure_reruns_all_affected_selected_cases_after_repair() {
    let harness = Harness::new();
    let text = r#"Suite |"s"| {
SuiteSetup { Log |1| }
SuiteTeardown { Missing }
Case |"a"| { Log |2| }
Case |"b"| { Log |3| }
}"#;
    source(&harness, text);
    let stderr = check(
        command(&harness, &["--failures", "failed.json"]),
        1,
        "1\n2\n3\n",
    );
    assert!(
        stderr.contains("2 succeeded, 0 failed, 0 skipped; 1 suite fixtures failed"),
        "{stderr}"
    );
    assert_eq!(state(&harness)["failed"], json!(["s/a", "s/b"]));
    source(&harness, &text.replace("Missing", "Log |4|"));
    check(
        command(
            &harness,
            &["--rerun-failed", "failed.json", "--failures", "failed.json"],
        ),
        0,
        "1\n2\n3\n4\n",
    );
    assert_eq!(state(&harness)["failed"], json!([]));
}

#[test]
fn discovery_and_filtering_never_enter_unselected_fixture_owners() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| {
SuiteSetup { Log |1| }
SuiteTeardown { Log |4| }
CaseSetup { Log |2| }
CaseTeardown { Log |3| }
Case |"a"| {}
Case |"b"| { Missing }
}"#,
    );
    let listing = command(&harness, &["--list-cases"]);
    assert!(listing.status.success());
    assert!(!String::from_utf8_lossy(&listing.stdout)
        .lines()
        .any(|line| line == "1"));
    check(command(&harness, &["--case", "s/a"]), 0, "1\n2\n3\n4\n");
    fs::write(
        harness.workspace.join("other.botwork"),
        "Suite |\"other\"| { SuiteSetup { Missing } SuiteTeardown { Missing } Case |\"a\"| {} }",
    )
    .unwrap();
    check(
        command(&harness, &["--suite", "other.botwork", "--case", "s/a"]),
        0,
        "1\n2\n3\n4\n",
    );
}

#[test]
fn all_fixture_declarations_validate_before_effects_even_when_unselected() {
    let harness = Harness::new();
    for declaration in [
        "SuiteSetup {} SuiteSetup {}",
        "CaseTeardown {} cASEtEARDOWN {}",
        "SuiteTeardown { Return |1| }",
        "CaseSetup { Break }",
        "SuiteSetup { If |false| { Continue } }",
        "CaseTeardown { Rethrow }",
    ] {
        source(
            &harness,
            &format!("Suite |\"s\"| {{ {declaration} Case |\"a\"| {{ Log |999| }} }}"),
        );
        let output = command(&harness, &[]);
        assert!(!output.status.success(), "{declaration}");
        assert!(output.stdout.is_empty());
    }
    source(
        &harness,
        "Suite |\"s\"| { Case |\"a\"| { Log |999| } SuiteSetup {} }",
    );
    assert!(!command(&harness, &[]).status.success());
}

#[test]
fn fixture_names_remain_ordinary_script_names_and_declarations_ignore_case() {
    let harness = Harness::new();
    let output = harness.run("ordinary", "SuiteSetup { Return |1| }\nCaseTeardown { Return |2| }\nLog |@{ SuiteSetup } + @{ CaseTeardown }|", Duration::from_secs(5)).unwrap();
    check(output, 0, "3\n");
    source(&harness, "Suite |\"s\"| { sUITEsETUP { Log |1| } cASEsETUP { Log |2| } sUITEtEARDOWN { Log |4| } cASEtEARDOWN { Log |3| } Case |\"a\"| {} }");
    check(command(&harness, &[]), 0, "1\n2\n3\n4\n");
}

#[test]
fn budget_stops_still_unwind_both_owners_and_suite_timeout_is_explicit() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { SuiteSetup {} SuiteTeardown { Log |2| } CaseTeardown { Log |1| } Case |\"a\"| { While |true| {} } }");
    check(command(&harness, &["--max-steps", "100"]), 1, "1\n2\n");
    check(
        command(
            &harness,
            &["--timeout-ms", "30", "--max-steps", "100000000"],
        ),
        1,
        "1\n2\n",
    );
    // The suite deadline covers setup too, so it must leave the case time to
    // start; its endless loop then runs until the deadline.
    let stderr = check(
        command(
            &harness,
            &["--suite-timeout-ms", "1000", "--max-steps", "100000000"],
        ),
        1,
        "1\n2\n",
    );
    assert!(stderr.contains("timed out"), "{stderr}");
    source(&harness, "Suite |\"s\"| { Case |\"a\"| {} }");
    assert!(!command(&harness, &["--suite-timeout-ms", "100"])
        .status
        .success());
    assert!(
        !command(&harness, &["--suite-timeout-ms", "100", "--list-cases"])
            .status
            .success()
    );
}

#[test]
fn library_failure_is_inside_the_entered_owner_and_never_runs_setup_or_body() {
    let harness = Harness::new();
    for suite_owned in [false, true] {
        let hooks = if suite_owned {
            "SuiteSetup { Log |999| } SuiteTeardown { Log |1| }"
        } else {
            "CaseSetup { Log |999| } CaseTeardown { Log |1| }"
        };
        source(&harness, &format!("Suite |\"s\"| {{ Library {{ Import |\"missing.botwork\"| As |m| }} {hooks} Case |\"a\"| {{ Log |999| }} }}"));
        let stderr = check(command(&harness, &[]), 1, "1\n");
        assert!(stderr.contains("missing.botwork"));
        assert_eq!(stderr.contains("[case s/a] skipped:"), suite_owned);
    }
}

#[test]
fn empty_completed_rerun_does_not_enter_owners_even_with_suite_deadline() {
    let harness = Harness::new();
    source(
        &harness,
        "Suite |\"s\"| { SuiteSetup { Log |999| } SuiteTeardown { Log |999| } Case |\"a\"| {} }",
    );
    fs::write(
        harness.workspace.join("failed.json"),
        r#"{"format":"botwork-failed-cases","version":2,"complete":true,"failed":[]}"#,
    )
    .unwrap();
    check(
        command(
            &harness,
            &["--rerun-failed", "failed.json", "--suite-timeout-ms", "0"],
        ),
        0,
        "",
    );
}

#[test]
fn mixed_fixture_and_plain_suites_preserve_order_with_one_job() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { SuiteSetup { Log |1| } SuiteTeardown { Log |3| } Case |\"a\"| { Log |2| } }");
    fs::write(
        harness.workspace.join("plain.botwork"),
        "Suite |\"p\"| { Case |\"a\"| { Log |4| } }",
    )
    .unwrap();
    check(
        command(&harness, &["--suite", "plain.botwork"]),
        0,
        "1\n2\n3\n4\n",
    );
}
