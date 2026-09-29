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
    assert_eq!(
        lines[3],
        "[cases] 3 selected: 1 succeeded, 1 failed, 1 skipped; 1 suite fixtures failed"
    );
    assert_eq!(lines.len(), 4);
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
