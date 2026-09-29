#[path = "support/cli_harness.rs"]
mod cli_harness;

use cli_harness::Harness;
use std::{fs, process::Output, time::Duration};

#[cfg(target_os = "linux")]
#[path = "parallel_cli/linux.rs"]
mod linux;

fn batch(harness: &Harness, source: &str, rest: &[&str], options: &[&str]) -> Output {
    let mut arguments = Vec::new();
    for path in rest {
        arguments.extend(["--file", *path]);
    }
    arguments.extend_from_slice(options);
    harness
        .run_with_args("entry", source, &arguments, Duration::from_secs(15))
        .unwrap()
}

fn successful_runs(output: &Output, count: usize) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    for id in 1..=count {
        assert_eq!(
            stderr.matches(&format!("[run {id}] started:")).count(),
            1,
            "{stderr}"
        );
        assert_eq!(
            stderr.matches(&format!("[run {id}] succeeded:")).count(),
            1,
            "{stderr}"
        );
    }
    assert_eq!(stderr.lines().count(), count * 2 + 1, "{stderr}");
    assert!(stderr.ends_with(&format!(
        "[batch] {count} runs: {count} succeeded, 0 failed\n"
    )));
}

#[test]
fn repeated_files_have_fresh_inputs_scopes_module_caches_and_distinct_ids() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("inputs.json"), r#"{"base": 1}"#).unwrap();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Log |\"module-init\"|\nRead { Return |9| }",
    )
    .unwrap();
    let source = "Import |\"module.botwork\"| As |first|\nImport |\"module.botwork\"| As |second|\nLocal { Return |base| }\nLog |@{ Local }|\n|base| = |base + 1|\nLog |base|";
    for jobs in ["1", "2", "64"] {
        let output = batch(
            &harness,
            source,
            &["entry.botwork", "entry.botwork"],
            &[
                "--jobs",
                jobs,
                "--vars-file",
                "inputs.json",
                "--var",
                "base=7",
            ],
        );
        successful_runs(&output, 3);
        let stdout = String::from_utf8(output.stdout).unwrap();
        let mut lines: Vec<_> = stdout.lines().collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            [
                "7",
                "7",
                "7",
                "8",
                "8",
                "8",
                "module-init",
                "module-init",
                "module-init"
            ]
        );
    }
}

#[test]
fn syntax_io_and_timeout_failures_do_not_cancel_successful_siblings() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("syntax.botwork"), "|x| = |1 +|").unwrap();
    fs::write(harness.workspace.join("loop.botwork"), "While |true| {}").unwrap();
    let output = batch(
        &harness,
        "Log |42|",
        &["syntax.botwork", "missing.botwork", "loop.botwork"],
        &[
            "--jobs",
            "2",
            "--timeout-ms",
            "100",
            "--max-steps",
            "1000000000",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"42\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[run 1] succeeded:"), "{stderr}");
    assert!(stderr.contains("[run 2] failed:"), "{stderr}");
    assert!(stderr.contains("[run 3] failed:"), "{stderr}");
    assert!(stderr.contains("[run 4] timed out:"), "{stderr}");
    assert!(stderr.contains("BW1001"), "{stderr}");
    assert!(stderr.contains("BW5002"), "{stderr}");
    assert!(
        stderr.ends_with("[batch] 4 runs: 1 succeeded, 2 failed, 1 timed out\n"),
        "{stderr}"
    );
}

#[test]
fn output_budgets_are_fresh_per_run_and_status_reporting_has_its_own_allowance() {
    let harness = Harness::new();
    let output = batch(
        &harness,
        "Log |123|",
        &["entry.botwork"],
        &["--max-output-bytes", "4"],
    );
    successful_runs(&output, 2);
    assert_eq!(output.stdout, b"123\n123\n");
    let output = batch(
        &harness,
        "Log |123|",
        &["entry.botwork"],
        &["--max-output-bytes", "0"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[run 1] limit exceeded:"), "{stderr}");
    assert!(stderr.contains("[run 2] limit exceeded:"), "{stderr}");
    assert!(
        stderr.ends_with("[batch] 2 runs: 0 succeeded, 0 failed, 2 limit exceeded\n"),
        "{stderr}"
    );
}

#[test]
fn oversized_source_is_reported_as_a_limit_before_utf8_and_leaves_siblings_running() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("oversized.botwork"),
        vec![0xff; botwork::core::syntax_limits::DEFAULT_SOURCE_BYTES + 1],
    )
    .unwrap();
    let output = batch(
        &harness,
        "Log |7|",
        &["oversized.botwork"],
        &["--jobs", "2"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"7\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[run 1] succeeded:"), "{stderr}");
    assert!(stderr.contains("[run 2] limit exceeded:"), "{stderr}");
    assert!(
        stderr.contains("BW8001") && stderr.contains("source bytes"),
        "{stderr}"
    );
    assert!(!stderr.contains("invalid utf-8"), "{stderr}");
    assert!(stderr.ends_with("[batch] 2 runs: 1 succeeded, 0 failed, 1 limit exceeded\n"));
}

#[test]
fn invalid_inputs_fail_each_run_before_script_effects_and_debug_traces() {
    let harness = Harness::new();
    let output = batch(
        &harness,
        "Log |\"unreachable\"|",
        &["missing.botwork"],
        &["--debug", "--var", "x=oops"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.matches("[BW7001]").count(), 2, "{stderr}");
    assert!(!stderr.contains("debug:"), "{stderr}");
    assert!(!stderr.contains("No such file"), "{stderr}");
}

#[test]
fn job_bounds_and_help_conflicts_are_checked_before_execution() {
    let harness = Harness::new();
    for jobs in ["0", "65", "256", "-1", "two"] {
        let output = batch(
            &harness,
            "Log |\"unreachable\"|",
            &["entry.botwork"],
            &["--jobs", jobs],
        );
        assert_eq!(output.status.code(), Some(2), "{jobs}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("[run "));
    }
    for help in ["--list-statements", "--statement-help"] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_botwork"));
        command.args([help]);
        if help == "--statement-help" {
            command.arg("Log |value|");
        }
        let output = command.args(["--jobs", "2"]).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn single_file_output_is_unchanged_with_explicit_parallel_options() {
    let harness = Harness::new();
    let baseline = harness
        .run("single", "Log |7|", Duration::from_secs(5))
        .unwrap();
    assert!(baseline.status.success());
    for jobs in ["1", "4", "64"] {
        let output = batch(&harness, "Log |7|", &[], &["--jobs", jobs]);
        assert!(output.status.success());
        assert_eq!(output.stdout, baseline.stdout);
        assert_eq!(output.stderr, baseline.stderr);
    }
}

#[test]
fn concurrent_log_records_do_not_interleave_their_bytes() {
    let harness = Harness::new();
    let a = "a".repeat(16384);
    let b = "b".repeat(16384);
    let script = |text: &str| format!("For |i| In |[1, 2, 3, 4]| {{ Log |\"{text}\"| }}");
    fs::write(harness.workspace.join("second.botwork"), script(&b)).unwrap();
    let output = batch(&harness, &script(&a), &["second.botwork"], &["--jobs", "2"]);
    successful_runs(&output, 2);
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().filter(|line| *line == a).count(), 4);
    assert_eq!(text.lines().filter(|line| *line == b).count(), 4);
    assert_eq!(text.lines().count(), 8);
}
