#![cfg(unix)]
#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn write(harness: &Harness, name: &str, source: &str) {
    fs::write(harness.workspace.join(name), source).unwrap();
}

/// A running invocation with stdout and stderr captured to files.
struct Running {
    child: Child,
    stderr: std::path::PathBuf,
    stdout: std::path::PathBuf,
}

fn spawn(harness: &Harness, name: &str, arguments: &[&str]) -> Running {
    let stdout = harness.workspace.join(format!("{name}.stdout"));
    let stderr = harness.workspace.join(format!("{name}.stderr"));
    let child = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(arguments)
        .current_dir(&harness.workspace)
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap())
        .spawn()
        .unwrap();
    Running {
        child,
        stderr,
        stdout,
    }
}

impl Running {
    fn stderr(&self) -> String {
        fs::read_to_string(&self.stderr).unwrap()
    }

    fn stdout(&self) -> String {
        fs::read_to_string(&self.stdout).unwrap()
    }

    /// Wait until stderr contains every marker.
    fn wait_for(&self, markers: &[&str]) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !markers.iter().all(|marker| self.stderr().contains(marker)) {
            assert!(
                Instant::now() < deadline,
                "waiting for {markers:?}: {}",
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn signal(&self, signal: i32) {
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, signal) }, 0);
    }

    /// The exit code, or 128 + the signal that ended the process.
    fn finish(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                use std::os::unix::process::ExitStatusExt;
                return status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap());
            }
            assert!(Instant::now() < deadline, "{}", self.stderr());
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn json(harness: &Harness, name: &str) -> Value {
    serde_json::from_slice(&fs::read(harness.workspace.join(name)).unwrap()).unwrap()
}

fn statuses(report: &Value) -> Vec<(u64, String)> {
    report["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| {
            (
                run["number"].as_u64().unwrap(),
                run["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Terminal events per run in a captured listener stream.
fn terminal_events(harness: &Harness) -> BTreeMap<u64, usize> {
    let mut terminal = BTreeMap::new();
    for line in fs::read_to_string(harness.workspace.join("events.jsonl"))
        .unwrap()
        .lines()
    {
        let event: Value = serde_json::from_str(line).unwrap();
        if matches!(
            event["event"].as_str(),
            Some("run_finished" | "run_skipped")
        ) {
            *terminal.entry(event["run"].as_u64().unwrap()).or_default() += 1;
        }
    }
    terminal
}

const CAPTURE: [&str; 6] = [
    "--listener",
    "sh",
    "--listener-arg",
    "-c",
    "--listener-arg",
    "cat > events.jsonl",
];

#[test]
fn interrupting_a_batch_gives_each_started_run_one_outcome() {
    let harness = Harness::new();
    write(&harness, "fast.botwork", "No Operation");
    for name in ["a", "b", "c"] {
        write(&harness, &format!("{name}.botwork"), "Sleep |30000|");
    }
    let mut arguments = vec![
        "--file",
        "fast.botwork",
        "--file",
        "a.botwork",
        "--file",
        "b.botwork",
        "--file",
        "c.botwork",
        "--jobs",
        "2",
        "--report-json",
        "report.json",
        "--report-html",
        "report.html",
    ];
    arguments.extend(CAPTURE);
    let mut running = spawn(&harness, "batch", &arguments);
    running.wait_for(&["[run 1] succeeded:", "[run 3] started:"]);
    let start = Instant::now();
    running.signal(libc::SIGINT);
    assert_eq!(running.finish(), 1);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "cancellation is cooperative, not a wait"
    );
    let stderr = running.stderr();
    assert!(stderr.contains("[interrupted] stopping runs; interrupt again to exit at once"));
    assert!(
        !stderr.contains("[run 4] started"),
        "admission stops: {stderr}"
    );
    for (run, label) in [(1, "succeeded"), (2, "cancelled"), (3, "cancelled")] {
        assert_eq!(
            stderr.matches(&format!("[run {run}] {label}:")).count(),
            1,
            "{stderr}"
        );
    }
    assert!(stderr.contains("[batch] 3 runs: 1 succeeded, 0 failed, 2 cancelled"));
    assert!(stderr
        .ends_with("Interrupted: 3 of 4 selected runs have outcomes; the others never started\n"));
    let report = json(&harness, "report.json");
    assert_eq!(report["complete"], false, "run 4 never started");
    assert_eq!(report["exit_code"], 1);
    assert_eq!(report["verdict"]["status"], "interrupted");
    assert_eq!(report["verdict"]["delivery"], "interrupted");
    assert_eq!(
        statuses(&report),
        [
            (1, "succeeded".into()),
            (2, "cancelled".into()),
            (3, "cancelled".into())
        ]
    );
    assert_eq!(report["runs"][1]["error"]["code"], "BW5001");
    let page = fs::read_to_string(harness.workspace.join("report.html")).unwrap();
    assert!(page.contains("This invocation was interrupted."));
    assert!(page.contains("Not every selected run has a record"));
    assert!(!harness.workspace.join("report.json.journal").exists());
    assert_eq!(
        terminal_events(&harness),
        BTreeMap::from([(1, 1), (2, 1), (3, 1)])
    );
    let events = fs::read_to_string(harness.workspace.join("events.jsonl")).unwrap();
    let trailer: Value = serde_json::from_str(events.lines().last().unwrap()).unwrap();
    assert_eq!(trailer["event"], "stream_finished");
    assert_eq!(trailer["status"], "interrupted");
}

#[test]
fn interrupted_suites_tear_down_ready_fixtures_and_keep_reruns_unavailable() {
    let harness = Harness::new();
    write(
        &harness,
        "slow.suite.botwork",
        r#"Suite |"slow"| {
    SuiteSetup { No Operation }
    SuiteTeardown { Log |"teardown ran"| }
    Case |"one"| { Sleep |30000| }
    Case |"two"| { No Operation }
}"#,
    );
    let mut running = spawn(
        &harness,
        "suite",
        &[
            "--suite",
            "slow.suite.botwork",
            "--jobs",
            "1",
            "--failures",
            "failed.json",
            "--report-json",
            "report.json",
        ],
    );
    running.wait_for(&["[case slow/one] started:"]);
    running.signal(libc::SIGTERM);
    assert_eq!(running.finish(), 1);
    assert!(
        running.stdout().contains("teardown ran"),
        "the ready fixture is torn down"
    );
    let stderr = running.stderr();
    assert!(!stderr.contains("[case slow/two] started"), "{stderr}");
    assert_eq!(
        stderr.matches("[case slow/one] cancelled:").count(),
        1,
        "{stderr}"
    );
    let report = json(&harness, "report.json");
    assert_eq!(statuses(&report), [(1, "cancelled".into())]);
    assert_eq!(report["complete"], false);
    assert_eq!(report["verdict"]["delivery"], "interrupted");
    assert_eq!(report["fixtures"][0]["suite"], "slow");
    let failed = json(&harness, "failed.json");
    assert_eq!(
        failed["complete"], false,
        "no rerun selection from partial results"
    );
}

#[test]
fn a_second_interrupt_exits_at_once_and_reconciliation_marks_runs_interrupted() {
    let harness = Harness::new();
    write(&harness, "fast.botwork", "Log |\"fast\"|");
    write(
        &harness,
        "stubborn.botwork",
        "Try {\n    Log |\"inside\"|\n    Sleep |30000|\n} Finally { Sleep |30000| }",
    );
    let mut running = spawn(
        &harness,
        "stubborn",
        &[
            "--file",
            "fast.botwork",
            "--file",
            "stubborn.botwork",
            "--jobs",
            "2",
            "--cleanup-timeout-ms",
            "30000",
            "--report-json",
            "report.json",
            "--report-html",
            "report.html",
        ],
    );
    running.wait_for(&["[run 1] succeeded:", "[run 2] started:"]);
    // `started` is printed before the script runs. Interrupt only inside the
    // Try, so that its Finally keeps the run going until the second interrupt;
    // an earlier interrupt skips the Finally and lets the batch finish.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !running.stdout().contains("inside") {
        assert!(Instant::now() < deadline, "{}", running.stderr());
        std::thread::sleep(Duration::from_millis(10));
    }
    running.signal(libc::SIGINT);
    running.wait_for(&["[interrupted] stopping runs"]);
    running.signal(libc::SIGINT);
    assert_eq!(running.finish(), 130);
    assert!(
        json(&harness, "report.json")["verdict"].is_null(),
        "the marker stays"
    );
    assert!(harness.workspace.join("report.json.journal").exists());
    let output = harness
        .command(
            "reconcile",
            &["--reconcile-report", "report.json"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "an interrupted verdict never passes"
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Reconciled report.json: 2 of 2 selected runs have records, 1 of them interrupted\n"
    );
    let report = json(&harness, "report.json");
    assert_eq!(
        statuses(&report),
        [(1, "succeeded".into()), (2, "interrupted".into())]
    );
    assert_eq!(report["complete"], true);
    assert_eq!(report["exit_code"], 1);
    assert_eq!(report["verdict"]["status"], "interrupted");
    assert_eq!(report["runs"][0]["logs"][0]["text"], "fast");
    let page = fs::read_to_string(harness.workspace.join("report.html")).unwrap();
    assert!(page.contains("<title>Botwork report: interrupted</title>"));
    assert!(!harness.workspace.join("report.json.journal").exists());
}

#[test]
fn forced_termination_blocks_new_reports_until_reconciled() {
    let harness = Harness::new();
    write(&harness, "fast.botwork", "No Operation");
    write(&harness, "slow.botwork", "Sleep |30000|");
    write(&harness, "later.botwork", "No Operation");
    let mut running = spawn(
        &harness,
        "killed",
        &[
            "--file",
            "fast.botwork",
            "--file",
            "slow.botwork",
            "--file",
            "later.botwork",
            "--jobs",
            "2",
            "--report-json",
            "report.json",
        ],
    );
    running.wait_for(&[
        "[run 1] succeeded:",
        "[run 3] started:",
        "[run 3] succeeded:",
    ]);
    // Reconciliation needs the invocation to have ended; its lock proves that.
    let early = harness
        .command(
            "early",
            &["--reconcile-report", "report.json"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(early.status.code(), Some(1));
    assert!(harness.workspace.join("report.json.journal").exists());
    running.signal(libc::SIGKILL);
    assert_eq!(running.finish(), 137);
    let blocked = harness
        .command(
            "blocked",
            &["--file", "fast.botwork", "--report-json", "report.json"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(blocked.status.code(), Some(1));
    assert!(blocked.stdout.is_empty());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("--reconcile-report"));
    let output = harness
        .command(
            "reconcile",
            &["--reconcile-report", "report.json"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report = json(&harness, "report.json");
    assert_eq!(
        statuses(&report),
        [
            (1, "succeeded".into()),
            (2, "interrupted".into()),
            (3, "succeeded".into())
        ]
    );
    let again = harness
        .command(
            "again",
            &["--reconcile-report", "report.json"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(again.status.code(), Some(1), "nothing is left to reconcile");
    let fresh = harness
        .command(
            "fresh",
            &["--file", "fast.botwork", "--report-json", "report.json"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(fresh.status.code(), Some(0));
}

#[test]
fn console_reporting_failures_keep_every_started_runs_record() {
    let harness = Harness::new();
    for name in ["a", "b", "c", "d"] {
        write(&harness, &format!("{name}.botwork"), "Sleep |500|");
    }
    // The console reader goes away after two progress lines, while both
    // admitted runs are still sleeping.
    let script = format!(
        "{{ '{}' --file a.botwork --file b.botwork --file c.botwork --file d.botwork --jobs 2 \
         --report-json report.json --listener sh --listener-arg -c --listener-arg 'cat > events.jsonl' \
         2>&1 >/dev/null; echo $? > status; }} | head -n 2 > /dev/null",
        env!("CARGO_BIN_EXE_botwork")
    );
    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(&harness.workspace)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(harness.workspace.join("status")).unwrap(),
        "1\n"
    );
    let report = json(&harness, "report.json");
    assert_eq!(report["verdict"]["delivery"], "failed");
    assert_eq!(report["complete"], false, "runs 3 and 4 never started");
    assert_eq!(
        statuses(&report),
        [(1, "succeeded".into()), (2, "succeeded".into())]
    );
    assert_eq!(terminal_events(&harness), BTreeMap::from([(1, 1), (2, 1)]));
}

#[test]
fn concurrent_failures_each_have_exactly_one_outcome_and_their_own_artifacts() {
    let harness = Harness::new();
    let mut arguments = vec!["--jobs".to_owned(), "8".into()];
    for index in 1..=16 {
        let name = format!("f{index:02}.botwork");
        write(&harness, &name, &format!("Assert |{index}| Equals |0|"));
        arguments.extend(["--file".to_owned(), name]);
    }
    arguments.extend(
        [
            "--assertion-artifacts",
            "evidence",
            "--report-json",
            "report.json",
        ]
        .map(str::to_owned),
    );
    arguments.extend(CAPTURE.map(str::to_owned));
    let borrowed: Vec<_> = arguments.iter().map(String::as_str).collect();
    let output = harness
        .command("concurrent", &borrowed, Duration::from_secs(60))
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    let report = json(&harness, "report.json");
    assert_eq!(report["complete"], true);
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 16);
    for (index, run) in runs.iter().enumerate() {
        let number = index + 1;
        assert_eq!(run["number"], number);
        assert_eq!(run["identity"]["id"], format!("f{number:02}.botwork"));
        assert_eq!(
            stderr.matches(&format!("[run {number}] failed:")).count(),
            1
        );
        let artifacts = run["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 1);
        let evidence: Value = serde_json::from_slice(
            &fs::read(Path::new(&harness.workspace).join(artifacts[0]["path"].as_str().unwrap()))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            evidence["identity"]["run"], number,
            "artifacts stay with their run"
        );
        assert_eq!(
            evidence["actual_typed_json"],
            format!(r#"{{"kind":"Int","value":{number}}}"#)
        );
    }
    let terminal = terminal_events(&harness);
    assert_eq!(terminal.len(), 16);
    assert!(terminal.values().all(|count| *count == 1));
}

#[test]
fn an_interrupted_single_file_publishes_an_interrupted_report() {
    let harness = Harness::new();
    write(&harness, "slow.botwork", "Log |\"begun\"|\nSleep |30000|");
    let mut running = spawn(
        &harness,
        "single",
        &["--file", "slow.botwork", "--report-json", "report.json"],
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !running.stdout().contains("begun") {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    running.signal(libc::SIGINT);
    assert_eq!(running.finish(), 1);
    let report = json(&harness, "report.json");
    assert_eq!(statuses(&report), [(1, "cancelled".into())]);
    assert_eq!(report["complete"], true);
    assert_eq!(report["verdict"]["delivery"], "interrupted");
    assert_eq!(report["verdict"]["status"], "interrupted");
    assert!(!harness.workspace.join("report.json.journal").exists());
}
