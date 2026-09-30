#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use std::{fs, time::Duration};

fn batch(harness: &Harness, scripts: &[(&str, &str)], extra: &[&str]) -> (Option<i32>, String) {
    let mut args = Vec::new();
    for (name, source) in scripts {
        fs::write(harness.workspace.join(name), source).unwrap();
        args.extend(["--file", name]);
    }
    args.extend(extra);
    let output = harness
        .command("batch", &args, Duration::from_secs(60))
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stderr).unwrap(),
    )
}
/// The recap block and summary: every line from `[failures]` onward.
fn tail(stderr: &str) -> Vec<&str> {
    let lines: Vec<_> = stderr.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("[failures]"))
        .unwrap_or(lines.len() - 1);
    lines[start..].to_vec()
}

#[test]
fn successful_batches_end_with_a_summary_and_no_recap() {
    let harness = Harness::new();
    let (code, stderr) = batch(
        &harness,
        &[("a.botwork", "Log |1|"), ("b.botwork", "No Operation")],
        &["--jobs", "1"],
    );
    assert_eq!(code, Some(0));
    assert!(!stderr.contains("[failures]"), "{stderr}");
    assert_eq!(tail(&stderr), ["[batch] 2 runs: 2 succeeded, 0 failed"]);
}

#[test]
fn failures_are_recapped_by_status_before_a_consistent_summary() {
    let harness = Harness::new();
    let (code, stderr) = batch(
        &harness,
        &[
            ("ok.botwork", "No Operation"),
            ("assert.botwork", "No Operation\nAssert |1| Equals |2|"),
            ("slow.botwork", "Sleep |10000|"),
            ("loud.botwork", "Log |\"too long for the budget\"|"),
            ("syntax.botwork", "Log |"),
        ],
        &[
            "--jobs",
            "1",
            "--timeout-ms",
            "200",
            "--max-output-bytes",
            "8",
        ],
    );
    assert_eq!(code, Some(1));
    let path = |name: &str| name.to_owned();
    assert_eq!(
        tail(&stderr),
        [
            "[failures] 4:".to_owned(),
            format!(
                "  [run 2] failed {:?} (BW9001 at {}:2:1)",
                path("assert.botwork"),
                path("assert.botwork")
            ),
            format!(
                "  [run 3] timed out {:?} (BW5002 at {}:1:1)",
                path("slow.botwork"),
                path("slow.botwork")
            ),
            format!(
                "  [run 4] limit exceeded {:?} (BW8001 at {}:1:1)",
                path("loud.botwork"),
                path("loud.botwork")
            ),
            format!(
                "  [run 5] failed {:?} (BW1001 at {}:1:6)",
                path("syntax.botwork"),
                path("syntax.botwork")
            ),
            "[batch] 5 runs: 1 succeeded, 2 failed, 1 timed out, 1 limit exceeded".to_owned(),
        ]
    );
    for (run, label) in [
        (2, "failed"),
        (3, "timed out"),
        (4, "limit exceeded"),
        (5, "failed"),
    ] {
        assert_eq!(
            stderr.matches(&format!("[run {run}] {label}:")).count(),
            1,
            "progress lines stay unique: {stderr}"
        );
    }
}

#[test]
fn suite_recaps_name_cases_and_fixtures_and_count_skips() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("mixed.suite.botwork"),
        r#"Suite |"good"| {
    Case |"pass"| { No Operation }
    Case |"fail"| {
        No Operation
        Assert |false|
    }
}"#,
    )
    .unwrap();
    fs::write(
        harness.workspace.join("broken.suite.botwork"),
        r#"Suite |"broken"| {
    SuiteSetup { |x| = |missing| }
    SuiteTeardown { No Operation }
    Case |"blocked"| { No Operation }
}"#,
    )
    .unwrap();
    let output = harness
        .command(
            "suites",
            &[
                "--suite",
                "mixed.suite.botwork",
                "--suite",
                "broken.suite.botwork",
                "--jobs",
                "1",
            ],
            Duration::from_secs(60),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines = tail(&stderr);
    assert_eq!(lines[0], "[failures] 2:");
    assert!(
        lines[1..3].contains(&"  [case good/fail] failed (BW9001 at mixed.suite.botwork:5:9)"),
        "{stderr}"
    );
    assert!(
        lines[1..3]
            .contains(&"  [suite broken] fixture failed (BW2001 at broken.suite.botwork:2:25)"),
        "{stderr}"
    );
    // The failed fixture's skipped case is in the failed-case record too.
    assert_eq!(
        lines[3],
        "  rerun one with --case ID, or record them with --failures PATH and rerun them with --rerun-failed PATH"
    );
    assert_eq!(
        lines[4],
        "[cases] 3 selected: 1 succeeded, 1 failed, 1 skipped; 1 suite fixtures failed"
    );
    assert_eq!(lines.len(), 5);
}

/// The IDs of the cases a suite run selected, from its JSON report.
fn selected(harness: &Harness, args: &[&str]) -> (Option<i32>, Vec<String>, String) {
    let report = harness.workspace.join("selected.json");
    let _ = fs::remove_file(&report);
    let mut args = args.to_vec();
    args.extend(["--report-json", "selected.json", "--jobs", "1"]);
    let output = harness
        .command("suites", &args, Duration::from_secs(60))
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap();
    let ids = json["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["identity"]["id"].as_str().unwrap().to_owned())
        .collect();
    (
        output.status.code(),
        ids,
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn suite_recaps_end_with_a_rerun_command_that_selects_the_failed_cases() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("checkout.suite.botwork"),
        r#"Suite |"checkout"| {
    Dataset |"orders"| {
        Row |"one"| Values |{expected: 1}|
        Row |"two"| Values |{expected: 3}|
        Row |"three"| Values |{expected: 3}|
    }
    Case |"empty"| { Assert |0| Equals |0| }
    Case |"total"| Using |"orders"| As |order| {
        Assert |order.expected| Equals |1|
    }
}"#,
    )
    .unwrap();
    // Without a record, one failure is rerun by its ID.
    fs::write(
        harness.workspace.join("single.suite.botwork"),
        r#"Suite |"single"| {
    Case |"good"| { No Operation }
    Case |"bad"| { Fail |"no"| }
}"#,
    )
    .unwrap();
    let (status, ids, stderr) = selected(&harness, &["--suite", "single.suite.botwork"]);
    assert_eq!((status, ids.len()), (Some(1), 2), "{stderr}");
    let lines = tail(&stderr);
    assert_eq!(
        lines[lines.len() - 2],
        "  rerun it with --case single/bad",
        "{stderr}"
    );
    let (status, ids, stderr) = selected(
        &harness,
        &["--suite", "single.suite.botwork", "--case", "single/bad"],
    );
    assert_eq!(
        (status, ids),
        (Some(1), vec!["single/bad".to_owned()]),
        "{stderr}"
    );

    // With a record, the hint names it, quoted for the shell, and rerunning it
    // selects exactly the failed rows.
    let (status, ids, stderr) = selected(
        &harness,
        &[
            "--suite",
            "checkout.suite.botwork",
            "--failures",
            "my failures.json",
        ],
    );
    assert_eq!((status, ids.len()), (Some(1), 4), "{stderr}");
    let lines = tail(&stderr);
    assert_eq!(lines[0], "[failures] 2:", "{stderr}");
    assert_eq!(
        lines[3], "  rerun them with --rerun-failed 'my failures.json'",
        "{stderr}"
    );
    assert_eq!(
        lines[4], "[cases] 4 selected: 2 succeeded, 2 failed",
        "{stderr}"
    );
    let (status, mut ids, stderr) = selected(
        &harness,
        &[
            "--suite",
            "checkout.suite.botwork",
            "--rerun-failed",
            "my failures.json",
        ],
    );
    ids.sort();
    assert_eq!(
        (status, ids),
        (
            Some(1),
            vec![
                "checkout/total/three".to_owned(),
                "checkout/total/two".to_owned()
            ]
        ),
        "{stderr}"
    );
}

#[test]
fn long_recaps_are_bounded_while_counts_stay_exact() {
    let harness = Harness::new();
    let scripts: Vec<_> = (0..55)
        .map(|index| (format!("f{index:02}.botwork"), "Fail |\"no\"|"))
        .collect();
    let borrowed: Vec<_> = scripts
        .iter()
        .map(|(name, source)| (name.as_str(), *source))
        .collect();
    let (code, stderr) = batch(&harness, &borrowed, &["--jobs", "8"]);
    assert_eq!(code, Some(1));
    let lines = tail(&stderr);
    assert_eq!(lines[0], "[failures] 55:");
    assert_eq!(lines.len(), 1 + 50 + 1 + 1);
    assert_eq!(lines[51], "  …and 5 more");
    assert_eq!(lines[52], "[batch] 55 runs: 0 succeeded, 55 failed");
}

#[test]
fn exit_status_matches_the_summary_for_every_outcome_mix() {
    for (scripts, success) in [
        (vec!["No Operation"], true),
        (vec!["No Operation", "Fail |\"x\"|"], false),
        (vec!["Sleep |10000|"], false),
        (vec!["No Operation", "No Operation"], true),
    ] {
        let harness = Harness::new();
        let named: Vec<_> = scripts
            .iter()
            .enumerate()
            .map(|(index, source)| (format!("s{index}.botwork"), *source))
            .collect();
        let borrowed: Vec<_> = named
            .iter()
            .map(|(name, source)| (name.as_str(), *source))
            .collect();
        let extra: Vec<&str> = if scripts.len() == 1 {
            // A single --file keeps the plain script mode; add a second success.
            vec!["--file", "s0.botwork", "--timeout-ms", "100"]
        } else {
            vec!["--timeout-ms", "100"]
        };
        let (code, stderr) = batch(&harness, &borrowed, &extra);
        let summary = *tail(&stderr).last().unwrap();
        let clean = summary.ends_with(&format!(
            "{} succeeded, 0 failed",
            summary
                .split(": ")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
        ));
        assert_eq!(code == Some(0), success, "{stderr}");
        assert_eq!(clean, success, "summary and exit status agree: {summary}");
    }
}
