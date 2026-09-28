use super::*;
use std::{
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
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
fn idle_suite_owner_uses_no_case_slot_and_teardown_waits_for_every_borrower() {
    let harness = Harness::new();
    let a = fifo(&harness, "a.botwork");
    let b = fifo(&harness, "b.botwork");
    let c = fifo(&harness, "c.botwork");
    source(
        &harness,
        r#"Suite |"s"| {
SuiteSetup { Log |0| }
SuiteTeardown { Log |4| }
Case |"a"| { Import |"a.botwork"| As |m| }
Case |"b"| { Import |"b.botwork"| As |m| }
Case |"c"| { Import |"c.botwork"| As |m| }
}"#,
    );
    let mut running = Running::start(&harness, &["--suite", "suite.botwork", "--jobs", "2"], None);
    let mut first = writer(&a, &mut running);
    let mut second = writer(&b, &mut running);
    assert_eq!(
        try_writer(&c).unwrap_err().raw_os_error(),
        Some(libc::ENXIO)
    );
    writeln!(second, "Log |2|").unwrap();
    drop(second);
    let mut third = writer(&c, &mut running);
    writeln!(third, "Log |3|").unwrap();
    drop(third);
    running.wait_for("[case s/c] succeeded:");
    assert_eq!(fs::read(&running.stdout).unwrap(), b"0\n2\n3\n");
    writeln!(first, "Log |1|").unwrap();
    drop(first);
    check(running.finish(), 0, "0\n2\n3\n1\n4\n");
}

#[test]
fn blocked_suite_setup_allows_another_owner_within_the_global_slot_limit() {
    let harness = Harness::new();
    let a = fifo(&harness, "setup-a.botwork");
    let b = fifo(&harness, "setup-b.botwork");
    let c = fifo(&harness, "setup-c.botwork");
    for name in ["a", "b", "c"] {
        fs::write(
            harness.workspace.join(format!("{name}.botwork")),
            format!(
                r#"Suite |"{name}"| {{
SuiteSetup {{ Import |"setup-{name}.botwork"| As |m| }}
SuiteTeardown {{ Log |"end-{name}"| }}
Case |"one"| {{ Log |"case-{name}"| }}
}}"#
            ),
        )
        .unwrap();
    }
    let mut running = Running::start(
        &harness,
        &[
            "--suite",
            "a.botwork",
            "--suite",
            "b.botwork",
            "--suite",
            "c.botwork",
            "--jobs",
            "2",
        ],
        None,
    );
    let first = writer(&a, &mut running);
    let second = writer(&b, &mut running);
    assert_eq!(
        try_writer(&c).unwrap_err().raw_os_error(),
        Some(libc::ENXIO)
    );
    drop(second);
    // Second suite's case and teardown progress while first setup is held.
    running.wait_for("[suite b] teardown succeeded");
    let third = writer(&c, &mut running);
    assert_eq!(fs::read(&running.stdout).unwrap(), b"case-b\nend-b\n");
    drop(third);
    running.wait_for("[suite c] teardown succeeded");
    drop(first);
    check(
        running.finish(),
        0,
        "case-b\nend-b\ncase-c\nend-c\ncase-a\nend-a\n",
    );
}

#[test]
fn suite_deadline_skips_queued_borrowers_while_case_deadlines_reset() {
    for (flag, expected, failed) in [
        ("--suite-timeout-ms", "1\n3\n", json!(["s/slow", "s/fast"])),
        ("--timeout-ms", "1\n2\n1\n3\n", json!(["s/slow"])),
    ] {
        let harness = Harness::new();
        let held = fifo(&harness, "held.botwork");
        source(
            &harness,
            r#"Suite |"s"| {
SuiteSetup {}
SuiteTeardown { Log |3| }
CaseTeardown { Log |1| }
Case |"slow"| { Import |"held.botwork"| As |m| }
Case |"fast"| { Log |2| }
}"#,
        );
        let mut running = Running::start(
            &harness,
            &[
                "--suite",
                "suite.botwork",
                "--jobs",
                "1",
                flag,
                "250",
                "--failures",
                "failed.json",
            ],
            None,
        );
        let pipe = writer(&held, &mut running);
        thread::sleep(Duration::from_millis(400));
        assert!(running.child.try_wait().unwrap().is_none());
        assert!(fs::read(&running.stdout).unwrap().is_empty());
        drop(pipe);
        let stderr = check(running.finish(), 1, expected);
        assert!(stderr.contains("[case s/slow] timed out:"), "{stderr}");
        assert_eq!(state(&harness)["failed"], failed);
        if flag == "--suite-timeout-ms" {
            assert!(stderr.contains("[case s/fast] skipped:"), "{stderr}");
            assert!(stderr.contains("[suite s] fixture timed out:"), "{stderr}");
        } else {
            assert!(stderr.contains("[case s/fast] succeeded:"), "{stderr}");
        }
    }
}

#[test]
fn report_failure_stops_admission_and_drains_borrowers_before_shared_cleanup() {
    use std::io::{BufRead, BufReader};
    const ISOLATED: &str = "BOTWORK_FIXTURE_ISOLATED_REPORT_FAILURE";
    if std::env::var_os(ISOLATED).is_none() {
        let harness = Harness::new();
        let stdout = harness.workspace.join("isolated.stdout");
        let stderr = harness.workspace.join("isolated.stderr");
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "linux::report_failure_stops_admission_and_drains_borrowers_before_shared_cleanup",
                "--nocapture",
            ])
            .env(ISOLATED, "1")
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let mut isolated = Running {
            child,
            stdout,
            stderr,
        };
        let output = isolated.finish();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let harness = Harness::new();
    let a = fifo(&harness, "a.botwork");
    let b = fifo(&harness, "b.botwork");
    source(
        &harness,
        r#"Suite |"s"| {
SuiteSetup {}
SuiteTeardown { Log |4| }
CaseTeardown { Log |3| }
Case |"a"| { Import |"a.botwork"| As |m| }
Case |"b"| { Import |"b.botwork"| As |m| }
Case |"queued"| { Log |999| }
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
        Some(Stdio::piped()),
    );
    let stderr = running.child.stderr.take().unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let observer = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut headers = String::new();
        while !headers.contains("[case s/b] started:") {
            assert_ne!(reader.read_line(&mut headers).unwrap(), 0);
        }
        drop(reader);
        sent.send(()).unwrap();
    });
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    observer.join().unwrap();
    let mut first = writer(&a, &mut running);
    let mut second = writer(&b, &mut running);
    writeln!(first, "Log |1|").unwrap();
    drop(first);
    thread::sleep(Duration::from_millis(200));
    assert!(running.child.try_wait().unwrap().is_none());
    assert_eq!(fs::read(&running.stdout).unwrap(), b"1\n3\n");
    writeln!(second, "Log |2|").unwrap();
    drop(second);
    check(running.finish(), 1, "1\n3\n2\n3\n4\n");
    assert_eq!(state(&harness)["complete"], false);
}

#[test]
fn failed_initial_suite_report_prevents_setup_and_keeps_history_incomplete() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { SuiteSetup { Log |999| } SuiteTeardown { Log |999| } Case |\"a\"| { Log |999| } }");
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let mut running = Running::start(
        &harness,
        &["--suite", "suite.botwork", "--failures", "failed.json"],
        Some(full.into()),
    );
    check(running.finish(), 1, "");
    assert_eq!(state(&harness)["complete"], false);
}
