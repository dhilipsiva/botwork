#![cfg(unix)]
#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    process::Output,
    time::{Duration, Instant},
};

const CAPTURE: [&str; 6] = [
    "--listener",
    "sh",
    "--listener-arg",
    "-c",
    "--listener-arg",
    "cat > events.jsonl",
];

fn command(harness: &Harness, arguments: &[&str]) -> Output {
    harness
        .command("listener", arguments, Duration::from_secs(60))
        .unwrap()
}

fn write(harness: &Harness, name: &str, source: &str) {
    fs::write(harness.workspace.join(name), source).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// Read the captured stream and check the rules every stream satisfies:
/// contiguous positions, a header first, and gapless per-run sequences that
/// start with run_started or run_skipped and end with a terminal event.
fn stream(harness: &Harness) -> Vec<Value> {
    let text = fs::read_to_string(harness.workspace.join("events.jsonl")).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (position, line) in lines.iter().enumerate() {
        assert_eq!(line["position"], position, "{line}");
    }
    assert_eq!(lines[0]["event"], "stream_started");
    assert_eq!(lines[0]["format"], "botwork-events");
    assert_eq!(lines[0]["version"], 1);
    for (run, events) in runs(&lines) {
        for (sequence, event) in events.iter().enumerate() {
            assert_eq!(event["sequence"], sequence, "run {run}: {event}");
        }
        assert!(
            matches!(
                events[0]["event"].as_str(),
                Some("run_started" | "run_skipped")
            ),
            "run {run} starts: {}",
            events[0]
        );
        assert!(
            matches!(
                events.last().unwrap()["event"].as_str(),
                Some("run_finished" | "run_skipped")
            ),
            "run {run} ends"
        );
    }
    lines
}

fn runs(lines: &[Value]) -> BTreeMap<u64, Vec<&Value>> {
    let mut runs = BTreeMap::<u64, Vec<&Value>>::new();
    for line in lines {
        if let Some(run) = line["run"].as_u64() {
            runs.entry(run).or_default().push(line);
        }
    }
    runs
}

fn events<'a>(lines: &'a [Value], name: &str) -> Vec<&'a Value> {
    lines.iter().filter(|line| line["event"] == name).collect()
}

#[test]
fn batch_streams_frame_every_run_and_agree_with_the_report() {
    let harness = Harness::new();
    write(&harness, "a.botwork", "Log |\"a\"|\n|x| = |1|");
    write(&harness, "b.botwork", "Sleep |20|\nAssert |false|");
    write(&harness, "c.botwork", "No Operation");
    let mut arguments = vec![
        "--file",
        "a.botwork",
        "--file",
        "b.botwork",
        "--file",
        "c.botwork",
        "--jobs",
        "3",
        "--report-json",
        "report.json",
    ];
    arguments.extend(CAPTURE);
    let output = command(&harness, &arguments);
    assert_eq!(output.status.code(), Some(1));
    let lines = stream(&harness);
    assert_eq!(lines[0]["mode"], "batch");
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["event"], "stream_finished");
    assert_eq!(trailer["status"], "failed");
    assert_eq!(trailer["cases"]["total"], 3);
    assert_eq!(trailer["cases"]["failed"], 1);
    assert_eq!(trailer["fixture_failures"], 0);
    let report: Value =
        serde_json::from_slice(&fs::read(harness.workspace.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["verdict"]["delivery"], "complete");
    let runs = runs(&lines);
    assert_eq!(runs.keys().copied().collect::<Vec<_>>(), [1, 2, 3]);
    for record in report["runs"].as_array().unwrap() {
        let events = &runs[&record["number"].as_u64().unwrap()];
        // The stream carries exactly the events each record folded.
        assert_eq!(record["events"], events.len());
        assert_eq!(events[0]["identity"], record["identity"]);
        assert_eq!(events.last().unwrap()["status"], record["status"]);
    }
    let log = events(&lines, "log")[0];
    assert_eq!(
        (log["run"].clone(), log["text"].clone()),
        (json!(1), json!("a"))
    );
    assert_eq!(
        events(&lines, "statement_finished")
            .iter()
            .find(|event| event["run"] == 2 && event["index"] == 1)
            .unwrap()["code"],
        "BW9001"
    );
}

#[test]
fn suite_streams_include_rows_skips_and_fixture_outcomes() {
    let harness = Harness::new();
    write(
        &harness,
        "rows.suite.botwork",
        r#"Suite |"rows"| {
    Dataset |"numbers"| {
        Row |"one"| Values |1|
        Row |"two"| Values |2|
    }
    SuiteSetup { No Operation }
    SuiteTeardown { No Operation }
    Case |"small"| Using |"numbers"| As |number| { Assert |number < 2| }
}"#,
    );
    write(
        &harness,
        "broken.suite.botwork",
        r#"Suite |"broken"| {
    SuiteSetup { |x| = |missing| }
    SuiteTeardown { No Operation }
    Case |"blocked"| { No Operation }
}"#,
    );
    let mut arguments = vec![
        "--suite",
        "rows.suite.botwork",
        "--suite",
        "broken.suite.botwork",
        "--jobs",
        "1",
    ];
    arguments.extend(CAPTURE);
    let output = command(&harness, &arguments);
    assert_eq!(output.status.code(), Some(1));
    let lines = stream(&harness);
    assert_eq!(lines[0]["mode"], "suites");
    let started = events(&lines, "run_started");
    assert_eq!(
        started[1]["identity"],
        json!({"id": "rows/small/two", "name": "small / two", "dataset": "numbers", "row": "two"})
    );
    let skipped = events(&lines, "run_skipped");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0]["run"], 3);
    assert_eq!(skipped[0]["identity"]["id"], "broken/blocked");
    assert_eq!(skipped[0]["reason"], "suite_setup_failed");
    let fixtures: BTreeMap<_, _> = events(&lines, "fixture_finished")
        .into_iter()
        .map(|fixture| (fixture["suite"].as_str().unwrap(), fixture))
        .collect();
    assert_eq!(fixtures["rows"]["status"], "succeeded");
    assert_eq!(fixtures["broken"]["status"], "failed");
    assert_eq!(fixtures["broken"]["error"]["code"], "BW2001");
    let broken = lines
        .iter()
        .position(|line| line["event"] == "fixture_finished" && line["suite"] == "broken")
        .unwrap();
    let skip = lines
        .iter()
        .position(|line| line["event"] == "run_skipped")
        .unwrap();
    assert!(
        broken < skip,
        "the failed fixture precedes the skips it causes"
    );
    let trailer = lines.last().unwrap();
    assert_eq!(trailer["event"], "stream_finished");
    assert_eq!(trailer["fixture_failures"], 1);
    assert_eq!(trailer["cases"]["skipped"], 1);
}

#[test]
fn listener_failures_fail_delivery_without_changing_runs() {
    let harness = Harness::new();
    // Events after the sleep reach a listener that has certainly closed stdin.
    write(
        &harness,
        "late.botwork",
        "Sleep |500|\nLog |\"still runs\"|",
    );
    for (script, reason) in [
        (
            "cat > /dev/null; exit 3",
            "listener exited with exit status: 3",
        ),
        ("exec 0<&-; exit 0", "writing event"),
    ] {
        let output = command(
            &harness,
            &[
                "--file",
                "late.botwork",
                "--report-json",
                "report.json",
                "--listener",
                "sh",
                "--listener-arg",
                "-c",
                "--listener-arg",
                script,
            ],
        );
        assert_eq!(output.status.code(), Some(1), "{script}");
        assert_eq!(output.stdout, b"still runs\n", "the run is unaffected");
        let stderr = stderr(&output);
        assert!(
            stderr.contains("Listener sh:") && stderr.contains(reason),
            "{stderr}"
        );
        let report: Value =
            serde_json::from_slice(&fs::read(harness.workspace.join("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["complete"], true);
        assert_eq!(report["runs"][0]["status"], "succeeded");
        assert_eq!(report["verdict"]["delivery"], "failed");
        assert_eq!(report["verdict"]["complete"], false);
        assert_eq!(report["exit_code"], 1);
    }
}

#[test]
fn slow_listeners_are_detached_without_slowing_runs() {
    let harness = Harness::new();
    write(
        &harness,
        "loud.botwork",
        "|i| = |0|\nWhile |i < 3000| {\n    Log |i|\n    |i| = |i + 1|\n}",
    );
    let start = Instant::now();
    let output = command(
        &harness,
        &[
            "--file",
            "loud.botwork",
            "--listener",
            "sh",
            "--listener-arg",
            "-c",
            "--listener-arg",
            "sleep 30",
            "--listener-queue",
            "8",
            "--listener-timeout-ms",
            "300",
        ],
    );
    assert!(start.elapsed() < Duration::from_secs(20));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout.clone())
            .unwrap()
            .lines()
            .count(),
        3000,
        "every Log still reaches stdout"
    );
    let stderr = stderr(&output);
    assert!(
        stderr.contains("listener fell 8 items behind and was detached"),
        "{stderr}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn hung_listeners_and_their_processes_are_stopped_at_the_close_timeout() {
    let harness = Harness::new();
    write(&harness, "ok.botwork", "No Operation");
    let start = Instant::now();
    let output = command(
        &harness,
        &[
            "--file",
            "ok.botwork",
            "--listener",
            "sh",
            "--listener-arg",
            "-c",
            "--listener-arg",
            "sleep 30 & echo $! > helper.pid; wait",
            "--listener-timeout-ms",
            "200",
        ],
    );
    let waited = start.elapsed();
    assert!(waited >= Duration::from_millis(200) && waited < Duration::from_secs(20));
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("did not finish within the close timeout"),
        "{stderr}"
    );
    let helper = fs::read_to_string(harness.workspace.join("helper.pid")).unwrap();
    let status = std::path::Path::new("/proc")
        .join(helper.trim())
        .join("status");
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_to_string(&status)
        .is_ok_and(|status| !status.lines().any(|line| line.starts_with("State:\tZ")))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        fs::read_to_string(&status).map_or(true, |status| status
            .lines()
            .any(|line| line.starts_with("State:\tZ"))),
        "the listener's own processes are stopped too"
    );
}

#[test]
fn listener_setup_errors_stop_before_any_run() {
    let harness = Harness::new();
    write(&harness, "effect.botwork", "Log |\"ran\"|");
    let output = command(
        &harness,
        &[
            "--file",
            "effect.botwork",
            "--listener",
            "./missing-listener",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "no run starts");
    assert!(stderr(&output).contains("Listener ./missing-listener:"));
    for arguments in [
        &["--file", "effect.botwork", "--listener-arg", "x"][..],
        &["--file", "effect.botwork", "--listener-queue", "8"],
        &[
            "--file",
            "effect.botwork",
            "--listener",
            "cat",
            "--listener-queue",
            "0",
        ],
        &[
            "--file",
            "effect.botwork",
            "--listener",
            "cat",
            "--listener-timeout-ms",
            "0",
        ],
    ] {
        let output = command(&harness, arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn stopped_invocations_close_the_stream_without_a_trailer() {
    let harness = Harness::new();
    let mut arguments = vec!["--suite", "absent.suite.botwork"];
    arguments.extend(CAPTURE);
    let output = command(&harness, &arguments);
    assert_eq!(output.status.code(), Some(1));
    let lines = stream(&harness);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "stream_started");
}

/// Replace values that vary between runs, keeping whether each was present.
fn normalize(value: &mut Value) {
    if let Value::Object(object) = value {
        for (key, field) in object.iter_mut() {
            if field.is_null() {
                continue;
            }
            if key.ends_with("_at") {
                *field = json!("<timestamp>");
            } else if matches!(key.as_str(), "duration_us" | "offset_us") {
                *field = json!(0);
            } else {
                normalize(field);
            }
        }
    }
}

/// The contents of the first fenced block after `opening`.
fn fenced<'a>(document: &'a str, opening: &str) -> &'a str {
    let start = document.find(opening).expect("documented block") + opening.len();
    let length = document[start..].find("\n```\n").expect("closing fence") + 1;
    &document[start..start + length]
}

#[test]
fn the_documented_stream_is_what_the_documented_script_produces() {
    let document = include_str!("../docs/listeners.md");
    let script = fenced(
        document,
        "<!-- botwork-test: listener-stream -->\n```botwork\n",
    );
    let documented: Vec<Value> = fenced(document, "```jsonl\n")
        .lines()
        .map(|line| {
            let mut value = serde_json::from_str(line).unwrap();
            normalize(&mut value);
            value
        })
        .collect();
    let harness = Harness::new();
    write(&harness, "listener-stream.botwork", script);
    let mut arguments = vec!["--file", "listener-stream.botwork"];
    arguments.extend(CAPTURE);
    let output = command(&harness, &arguments);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let actual: Vec<Value> = stream(&harness)
        .into_iter()
        .map(|mut value| {
            normalize(&mut value);
            value
        })
        .collect();
    assert_eq!(actual, documented);
}
