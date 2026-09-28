use super::*;
use std::{
    io::{self, Write},
    os::unix::fs::{symlink, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::Instant,
};

struct Running {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Running {
    fn start(harness: &Harness, args: &[&str], stderr: Option<Stdio>) -> Self {
        let stdout = harness.workspace.join("running.stdout");
        let stderr_path = harness.workspace.join("running.stderr");
        let child = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(args)
            .current_dir(&harness.workspace)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(stderr.unwrap_or_else(|| fs::File::create(&stderr_path).unwrap().into()))
            .spawn()
            .unwrap();
        Self {
            child,
            stdout,
            stderr: stderr_path,
        }
    }
    fn finish(&mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "suite child exceeded watchdog");
            thread::sleep(Duration::from_millis(2));
        };
        Output {
            status,
            stdout: fs::read(&self.stdout).unwrap(),
            stderr: fs::read(&self.stderr).unwrap_or_default(),
        }
    }
    fn wait_for(&mut self, marker: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let stderr = fs::read_to_string(&self.stderr).unwrap();
            if stderr.contains(marker) {
                return stderr;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "suite exited before {marker}: {stderr}"
            );
            assert!(Instant::now() < deadline, "missing {marker}: {stderr}");
            thread::sleep(Duration::from_millis(2));
        }
    }
}

fn fifo(harness: &Harness, name: &str) -> PathBuf {
    let path = harness.workspace.join(name);
    assert!(Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap()
        .success());
    path
}

fn try_writer(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[test]
fn row_failure_allows_queued_rows_while_a_sibling_is_blocked_and_data_stays_frozen() {
    let harness = Harness::new();
    let held = fifo(&harness, "held.botwork");
    fs::write(
        harness.workspace.join("data"),
        r#"Dataset |"d"| {
Row |"held"| Values |1|
Row |"failure"| Values |0|
Row |"queued"| Values |2|
}"#,
    )
    .unwrap();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| From |"data"|
Case |"c"| Using |"d"| As |n| {
If |n == 1| { Import |"held.botwork"| As |m| }
Log |8 / n|
}
}"#,
    );
    let mut running = Running::start(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "2",
            "--failures",
            "failed.json",
        ],
        None,
    );
    let mut pipe = writer(&held, &mut running);
    running.wait_for("[case s/c/queued] succeeded:");
    let progress = fs::read_to_string(&running.stderr).unwrap();
    assert!(
        progress.find("[case s/c/failure] failed:").unwrap()
            < progress.find("[case s/c/queued] started:").unwrap()
    );
    assert!(!progress.contains("[case s/c/held] succeeded:"));
    assert_eq!(state(&harness)["complete"], false);
    fs::write(harness.workspace.join("data"), "invalid now").unwrap();
    writeln!(pipe, "Log |11|").unwrap();
    drop(pipe);
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"4\n11\n8\n");
    assert_eq!(state(&harness)["complete"], true);
    assert_eq!(state(&harness)["failed"], json!(["s/c/failure"]));
}

#[test]
fn rows_wait_for_admission_and_receive_fresh_deadlines_and_the_discovered_values() {
    let harness = Harness::new();
    let held = fifo(&harness, "held.botwork");
    fs::write(
        harness.workspace.join("data"),
        r#"Dataset |"d"| { Row |"held"| Values |1| Row |"queued"| Values |2| }"#,
    )
    .unwrap();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| From |"data"|
Case |"c"| Using |"d"| As |n| {
If |n == 1| { Try { Import |"held.botwork"| As |m| } Catch { Log |999| } }
Log |n|
}
}"#,
    );
    let mut running = Running::start(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "1",
            "--timeout-ms",
            "250",
            "--failures",
            "failed.json",
        ],
        None,
    );
    let mut pipe = writer(&held, &mut running);
    fs::write(
        harness.workspace.join("data"),
        r#"Dataset |"d"| { Row |"queued"| Values |99| }"#,
    )
    .unwrap();
    thread::sleep(Duration::from_millis(400));
    assert!(running.child.try_wait().unwrap().is_none());
    assert!(!fs::read_to_string(&running.stderr)
        .unwrap()
        .contains("[case s/c/queued] started:"));
    writeln!(pipe, "Log |11|").unwrap();
    drop(pipe);
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"2\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[case s/c/held] timed out:"), "{stderr}");
    assert!(stderr.contains("[case s/c/queued] succeeded:"), "{stderr}");
    assert_eq!(state(&harness)["failed"], json!(["s/c/held"]));
}

#[test]
fn canonical_dataset_cache_admits_symlink_aliases_without_recounting_file_bytes() {
    let harness = Harness::new();
    let mut data = "Dataset |\"d\"| { Row |\"r\"| Values |42| }".to_owned();
    data.push_str(&" ".repeat(1024 * 1024 - data.len()));
    fs::write(harness.workspace.join("data"), data).unwrap();
    symlink("data", harness.workspace.join("alias")).unwrap();
    let files: Vec<_> = (0..9).map(|i| {
        let name = format!("s{i}.botwork");
        let path = if i % 2 == 0 { "data" } else { "./alias" };
        fs::write(harness.workspace.join(&name), format!("Suite |\"s{i}\"| {{ Dataset |\"d\"| From |\"{path}\"| Case |\"c\"| Using |\"d\"| As |n| {{ Log |n| }} }}")).unwrap();
        name
    }).collect();
    let mut args = vec!["--jobs", "1"];
    for file in &files {
        args.extend(["--suite", file]);
    }
    let output = command(&harness, &args);
    let stderr = success(&output, &"42\n".repeat(9));
    assert!(stderr.ends_with("[cases] 9 selected: 9 succeeded, 0 failed\n"));
}

fn writer(path: &Path, running: &mut Running) -> fs::File {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match try_writer(path) {
            Ok(file) => return file,
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                assert!(
                    running.child.try_wait().unwrap().is_none(),
                    "suite exited before import read"
                );
                assert!(
                    Instant::now() < deadline,
                    "suite import reader never entered"
                );
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("open FIFO writer: {error}"),
        }
    }
}

#[test]
fn final_record_write_failure_is_reported_and_retains_the_incomplete_record() {
    let harness = Harness::new();
    let module = fifo(&harness, "held.botwork");
    fs::create_dir(harness.workspace.join("records")).unwrap();
    source(
        &harness,
        "Suite |\"s\"| { Case |\"a\"| { Import |\"held.botwork\"| As |m| } }",
    );
    let mut running = Running::start(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--failures",
            "records/failed.json",
        ],
        None,
    );
    let mut held = writer(&module, &mut running);
    fs::rename(
        harness.workspace.join("records"),
        harness.workspace.join("moved-records"),
    )
    .unwrap();
    held.write_all(b"Log |42|\n").unwrap();
    drop(held);
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"42\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[cases] 1 selected: 1 succeeded, 0 failed"),
        "{stderr}"
    );
    assert!(stderr.contains("Failed-case record"), "{stderr}");
    let record: Value = serde_json::from_slice(
        &fs::read(harness.workspace.join("moved-records/failed.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["complete"], false);
    assert_eq!(record["failed"], json!([]));
}

#[test]
fn parallel_case_admission_is_bounded_and_out_of_order_completion_preserves_ids() {
    let harness = Harness::new();
    let a = fifo(&harness, "a.botwork");
    let b = fifo(&harness, "b.botwork");
    let c = fifo(&harness, "c.botwork");
    source(&harness, "Suite |\"s\"| { Case |\"a\"| { Import |\"a.botwork\"| As |m| } Case |\"b\"| { Import |\"b.botwork\"| As |m| } Case |\"c\"| { Import |\"c.botwork\"| As |m| } }");
    let mut running = Running::start(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "2",
            "--failures",
            "failed.json",
        ],
        None,
    );
    let mut first = writer(&a, &mut running);
    let mut second = writer(&b, &mut running);
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        assert_eq!(
            try_writer(&c).unwrap_err().raw_os_error(),
            Some(libc::ENXIO)
        );
        thread::sleep(Duration::from_millis(2));
    }
    writeln!(second, "Log |2|").unwrap();
    drop(second);
    let mut third = writer(&c, &mut running);
    let progress = fs::read_to_string(&running.stderr).unwrap();
    assert!(
        progress.find("[case s/b] succeeded:").unwrap()
            < progress.find("[case s/c] started:").unwrap()
    );
    assert!(!progress.contains("[case s/a] succeeded:"));
    assert_eq!(state(&harness)["complete"], false);
    writeln!(third, "Log |3|").unwrap();
    drop(third);
    writeln!(first, "Log |1|").unwrap();
    drop(first);
    let output = running.finish();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let mut lines: Vec<_> = text.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines, ["1", "2", "3"]);
    assert_eq!(state(&harness)["complete"], true);
    assert_eq!(state(&harness)["failed"], json!([]));
}

#[test]
fn failure_ids_keep_discovery_order_after_reversed_terminal_order() {
    let harness = Harness::new();
    let module = fifo(&harness, "held.botwork");
    source(&harness, "Suite |\"s\"| { Case |\"first\"| { Import |\"held.botwork\"| As |m| } Case |\"second\"| { Log |missing| } }");
    let mut running = Running::start(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "2",
            "--failures",
            "failed.json",
        ],
        None,
    );
    let mut held = writer(&module, &mut running);
    running.wait_for("[case s/second] failed:");
    held.write_all(b"Log |missing|\n").unwrap();
    drop(held);
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.find("[case s/second] failed:").unwrap()
            < stderr.find("[case s/first] failed:").unwrap()
    );
    assert!(output.stdout.is_empty());
    assert_eq!(state(&harness)["failed"], json!(["s/first", "s/second"]));
}

#[test]
fn interrupted_runs_keep_incomplete_records_and_exclusive_writer_locks_are_released() {
    let harness = Harness::new();
    source(
        &harness,
        "Suite |\"s\"| { Case |\"wait\"| { While |true| {} } }",
    );
    let mut running = Running::start(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--max-steps",
            "1000000000",
            "--failures",
            "failed.json",
        ],
        None,
    );
    running.wait_for("[case s/wait] started:");
    assert_eq!(state(&harness)["complete"], false);
    let second = command(
        &harness,
        &["--suite", "suite.botwork", "--failures", "./failed.json"],
    );
    assert_eq!(second.status.code(), Some(1));
    assert!(second.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&second.stderr).contains("] started:"));
    running.child.kill().unwrap();
    running.child.wait().unwrap();
    assert_eq!(state(&harness)["complete"], false);
    let rerun = command(
        &harness,
        &["--suite", "suite.botwork", "--rerun-failed", "failed.json"],
    );
    assert_eq!(rerun.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rerun.stderr).contains("incomplete"));
    source(&harness, "Suite |\"s\"| { Case |\"wait\"| { Log |7| } }");
    success(
        &command(
            &harness,
            &["--suite", "suite.botwork", "--failures", "failed.json"],
        ),
        "7\n",
    );
    assert_eq!(state(&harness)["complete"], true);
}

#[test]
fn reporting_failure_prevents_effects_and_leaves_failure_selection_incomplete() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { Case |\"a\"| { Log |7| } }");
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let mut running = Running::start(
        &harness,
        &["--suite", "suite.botwork", "--failures", "failed.json"],
        Some(full.into()),
    );
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(state(&harness)["complete"], false);
}

#[test]
fn timeouts_drain_case_imports_and_queued_cases_get_fresh_deadlines() {
    let harness = Harness::new();
    let path = fifo(&harness, "slow.botwork");
    source(&harness, "Suite |\"s\"| { Case |\"slow\"| { Try { Import |\"slow.botwork\"| As |m| } Catch { Log |999| } } Case |\"fast\"| { Log |22| } }");
    let mut running = Running::start(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "1",
            "--timeout-ms",
            "250",
            "--failures",
            "failed.json",
        ],
        None,
    );
    let mut pipe = writer(&path, &mut running);
    thread::sleep(Duration::from_millis(400));
    assert!(
        running.child.try_wait().unwrap().is_none(),
        "started read must drain"
    );
    assert!(!fs::read_to_string(&running.stderr)
        .unwrap()
        .contains("[case s/fast] started:"));
    writeln!(pipe, "Log |11|").unwrap();
    drop(pipe);
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"22\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[case s/slow] timed out:"), "{stderr}");
    assert!(stderr.contains("[case s/fast] succeeded:"), "{stderr}");
    assert_eq!(state(&harness)["failed"], json!(["s/slow"]));
}

#[test]
fn special_history_files_and_output_symlinks_are_rejected_without_side_effects() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { Case |\"a\"| { Log |7| } }");
    fifo(&harness, "failed.json");
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--rerun-failed", "failed.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    fs::remove_file(harness.workspace.join("failed.json")).unwrap();
    let original = json!({"format":"botwork-failed-cases","version":1,"complete":true,"failed":[]})
        .to_string();
    fs::write(harness.workspace.join("keep"), &original).unwrap();
    symlink("keep", harness.workspace.join("failed.json")).unwrap();
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--failures", "failed.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(harness.workspace.join("keep")).unwrap(),
        original
    );
    fs::remove_file(harness.workspace.join("failed.json")).unwrap();
    fs::remove_file(harness.workspace.join("failed.json.lock")).unwrap();
    symlink("keep", harness.workspace.join("failed.json.lock")).unwrap();
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--failures", "failed.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(!harness.workspace.join("failed.json").exists());
    assert_eq!(
        fs::read_to_string(harness.workspace.join("keep")).unwrap(),
        original
    );
}
