#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use serde_json::Value;
use std::{fs, time::Duration};

const ITERATIONS: usize = 25_000;

fn write(harness: &Harness, name: &str, source: &str) {
    fs::write(harness.workspace.join(name), source).unwrap();
}

#[test]
fn a_long_logging_loop_keeps_bounded_reports_and_a_complete_stream() {
    let harness = Harness::new();
    write(
        &harness,
        "loop.botwork",
        &format!("|i| = |0|\nWhile |i < {ITERATIONS}| {{\n    Log |i|\n    |i| = |i + 1|\n}}"),
    );
    let output = harness
        .command(
            "loop",
            &[
                "--file",
                "loop.botwork",
                "--max-steps",
                "10000000",
                "--report-json",
                "report.json",
                "--report-html",
                "report.html",
                "--listener",
                "sh",
                "--listener-arg",
                "-c",
                "--listener-arg",
                "cat > events.jsonl",
                "--listener-queue",
                "1048576",
            ],
            Duration::from_secs(120),
        )
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().lines().count(),
        ITERATIONS,
        "stdout keeps every Log"
    );
    let bytes = fs::read(harness.workspace.join("report.json")).unwrap();
    assert!(bytes.len() < 64 * 1024, "{} bytes", bytes.len());
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    let run = &report["runs"][0];
    assert_eq!(
        run["statements"].as_array().unwrap().len(),
        2,
        "a loop is one statement"
    );
    assert_eq!(run["logs"].as_array().unwrap().len(), 64);
    assert_eq!(run["omitted_logs"], ITERATIONS - 64);
    let digits: usize = (0..ITERATIONS).map(|value| value.to_string().len()).sum();
    assert_eq!(run["logged_bytes"], digits, "byte counts stay exact");
    let page = fs::metadata(harness.workspace.join("report.html"))
        .unwrap()
        .len();
    assert!(page < 256 * 1024, "{page} bytes");
    // The stream carries every event, but text only within the retention limits.
    let stream = fs::read_to_string(harness.workspace.join("events.jsonl")).unwrap();
    let mut logs = 0;
    let mut with_text = 0;
    for line in stream.lines() {
        assert!(line.len() < 2048, "{line}");
        let event: Value = serde_json::from_str(line).unwrap();
        if event["event"] == "log" {
            logs += 1;
            if event["text"] != "" {
                with_text += 1;
            }
        }
    }
    assert_eq!((logs, with_text), (ITERATIONS, 64));
}

// A Windows command line holds 32,767 characters, fewer than the 4,097 file
// arguments this needs.
#[cfg(not(windows))]
#[test]
fn batch_selections_are_bounded_before_any_output() {
    let harness = Harness::new();
    write(&harness, "x.botwork", "Log |\"ran\"|");
    let mut arguments = Vec::new();
    for _ in 0..=4096 {
        arguments.extend(["--file", "x.botwork"]);
    }
    arguments.extend([
        "--report-json",
        "report.json",
        "--assertion-artifacts",
        "evidence",
    ]);
    let output = harness
        .command("bounded", &arguments, Duration::from_secs(60))
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "no run starts");
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("[BW8001] Resource limit exceeded: selected files (limit 4096)"));
    assert!(!harness.workspace.join("report.json").exists());
    assert!(!harness.workspace.join("evidence").exists());
}

#[test]
fn interrupted_publications_leave_no_temporaries_behind() {
    let harness = Harness::new();
    write(&harness, "ok.botwork", "No Operation");
    for stale in [
        ".report.json.123.4.tmp",
        ".report.html.123.5.tmp",
        ".failed.json.123.6.tmp",
    ] {
        write(&harness, stale, "cut short");
    }
    write(&harness, ".unrelated.json.1.1.tmp", "keep");
    write(
        &harness,
        "one.suite.botwork",
        "Suite |\"one\"| {\n    Case |\"case\"| { No Operation }\n}",
    );
    let output = harness
        .command(
            "sweep",
            &[
                "--suite",
                "one.suite.botwork",
                "--failures",
                "failed.json",
                "--report-json",
                "report.json",
                "--report-html",
                "report.html",
            ],
            Duration::from_secs(60),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let mut left: Vec<_> = fs::read_dir(&harness.workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    left.sort();
    assert_eq!(left, [".unrelated.json.1.1.tmp"]);
}
