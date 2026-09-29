use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Instant,
};

struct Running {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
    kill_group: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.kill_group && matches!(self.child.try_wait(), Ok(None)) {
            // The child is still ours and unreaped, so its process-group ID
            // cannot be recycled. The isolated test's CLI shares this group.
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Running {
    fn start(harness: &Harness, files: &[&str], args: &[&str], stderr: Option<Stdio>) -> Self {
        let stdout_path = harness.workspace.join("batch.stdout");
        let stderr_path = harness.workspace.join("batch.stderr");
        let mut command = Command::new(env!("CARGO_BIN_EXE_botwork"));
        for file in files {
            command.args(["--file", file]);
        }
        let child = command
            .args(args)
            .current_dir(&harness.workspace)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path).unwrap())
            .stderr(stderr.unwrap_or_else(|| fs::File::create(&stderr_path).unwrap().into()))
            .spawn()
            .unwrap();
        Self {
            child,
            stdout: stdout_path,
            stderr: stderr_path,
            kill_group: false,
        }
    }

    fn finish(&mut self) -> Output {
        self.finish_with_timeout(Duration::from_secs(10))
    }

    fn finish_with_timeout(&mut self, timeout: Duration) -> Output {
        let end = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < end, "batch did not finish before watchdog");
            thread::sleep(Duration::from_millis(2));
        };
        Output {
            status,
            stdout: fs::read(&self.stdout).unwrap(),
            stderr: fs::read(&self.stderr).unwrap_or_default(),
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

fn try_writer(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

fn writer(path: &Path, running: &mut Running) -> fs::File {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match try_writer(path) {
            Ok(file) => return file,
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                assert!(
                    running.child.try_wait().unwrap().is_none(),
                    "batch exited before read entry"
                );
                assert!(Instant::now() < end, "reader never entered: {path:?}");
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("FIFO writer: {error}"),
        }
    }
}

fn release(mut writer: fs::File, value: usize) {
    writeln!(writer, "Log |{value}|").unwrap();
    // Closing the writer supplies EOF, which releases the blocking read.
}

#[test]
fn blocked_reads_overlap_only_up_to_the_job_limit_and_completion_keeps_its_original_id() {
    let harness = Harness::new();
    let first = fifo(&harness, "first.botwork");
    let second = fifo(&harness, "second.botwork");
    let third = fifo(&harness, "third.botwork");
    let mut running = Running::start(
        &harness,
        &["first.botwork", "second.botwork", "third.botwork"],
        &["--jobs", "2"],
        None,
    );
    let a = writer(&first, &mut running);
    let b = writer(&second, &mut running);
    // Both admitted readers are stalled. Probe the third throughout a bounded
    // observation window; accidentally admitting it would connect this writer.
    let end = Instant::now() + Duration::from_millis(200);
    while Instant::now() < end {
        let error = try_writer(&third).expect_err("third reader exceeded --jobs 2");
        assert_eq!(error.raw_os_error(), Some(libc::ENXIO));
        thread::sleep(Duration::from_millis(2));
    }
    release(b, 2);
    let c = writer(&third, &mut running);
    let progress = fs::read_to_string(&running.stderr).unwrap();
    assert!(
        progress.find("[run 2] succeeded:").unwrap() < progress.find("[run 3] started:").unwrap()
    );
    assert!(!progress.contains("[run 1] succeeded:"));
    release(c, 3);
    release(a, 1);
    let output = running.finish();
    successful_runs(&output, 3);
    let text = String::from_utf8(output.stdout).unwrap();
    let mut lines: Vec<_> = text.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines, ["1", "2", "3"]);
}

#[test]
fn script_failure_keeps_admitted_and_queued_siblings_running() {
    let harness = Harness::new();
    let failing = fifo(&harness, "failing.botwork");
    let admitted = fifo(&harness, "admitted.botwork");
    let queued = fifo(&harness, "queued.botwork");
    let mut running = Running::start(
        &harness,
        &["failing.botwork", "admitted.botwork", "queued.botwork"],
        &["--jobs", "2"],
        None,
    );
    let mut a = writer(&failing, &mut running);
    let b = writer(&admitted, &mut running);
    assert_eq!(
        try_writer(&queued).unwrap_err().raw_os_error(),
        Some(libc::ENXIO),
        "third run must still be queued while both slots are occupied"
    );
    writeln!(a, "Log |10|\nLog |missing|\nLog |999|").unwrap();
    drop(a);
    // Opening the third writer proves admission after the first run failed,
    // while the second run is still blocked in its original source read.
    let mut c = writer(&queued, &mut running);
    let progress = fs::read_to_string(&running.stderr).unwrap();
    writeln!(c, "Try {{ Log |missing| }} Catch {{}}\nLog |3|").unwrap();
    drop(c);
    release(b, 2);
    let output = running.finish();
    assert!(
        progress.find("[run 1] failed:").unwrap() < progress.find("[run 3] started:").unwrap(),
        "{progress}"
    );
    assert!(!progress.contains("[run 2] succeeded:"), "{progress}");
    assert!(!progress.contains("[batch]"), "{progress}");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("10\n"),
        "earlier effects remain: {stdout}"
    );
    let mut lines: Vec<_> = stdout.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines, ["10", "2", "3"], "failed script must skip its tail");
    let stderr = String::from_utf8(output.stderr).unwrap();
    for (id, outcome) in [(1, "failed"), (2, "succeeded"), (3, "succeeded")] {
        for label in ["started", outcome] {
            assert_eq!(
                stderr.matches(&format!("[run {id}] {label}:")).count(),
                1,
                "{stderr}"
            );
        }
    }
    assert!(stderr.contains("BW2001"), "{stderr}");
    assert!(
        stderr.ends_with("[batch] 3 runs: 2 succeeded, 1 failed\n"),
        "{stderr}"
    );
}

#[test]
fn queued_runs_receive_a_fresh_timeout_after_an_expired_read_drains() {
    let harness = Harness::new();
    let first = fifo(&harness, "slow.botwork");
    fs::write(harness.workspace.join("fast.botwork"), "Log |22|").unwrap();
    let mut running = Running::start(
        &harness,
        &["slow.botwork", "fast.botwork"],
        &["--jobs", "1", "--timeout-ms", "250"],
        None,
    );
    let first = writer(&first, &mut running);
    thread::sleep(Duration::from_millis(400));
    assert!(
        running.child.try_wait().unwrap().is_none(),
        "blocking read must drain"
    );
    let progress = fs::read_to_string(&running.stderr).unwrap();
    assert!(!progress.contains("[run 2] started:"));
    release(first, 11);
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"22\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("[run 1] timed out:"), "{stderr}");
    assert!(stderr.contains("[run 2] succeeded:"), "{stderr}");
    assert!(
        stderr.ends_with("[batch] 2 runs: 1 succeeded, 0 failed, 1 timed out\n"),
        "{stderr}"
    );
}

#[test]
fn reporting_failure_stops_admission_but_drains_already_started_runs() {
    // Other tests fork CLI/rustc children. Even CLOEXEC pipe readers can remain
    // open between their fork and exec, defeating this fixture's deliberate
    // BrokenPipe. Create the pipe in a process running only this test.
    const ISOLATED: &str = "BOTWORK_TEST_ISOLATED_REPORT_FAILURE";
    if std::env::var_os(ISOLATED).is_none() {
        let harness = Harness::new();
        let stdout = harness.workspace.join("isolated.stdout");
        let stderr = harness.workspace.join("isolated.stderr");
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "linux::reporting_failure_stops_admission_but_drains_already_started_runs",
                "--nocapture",
            ])
            .env(ISOLATED, "1")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let mut isolated = Running {
            child,
            stdout,
            stderr,
            kill_group: true,
        };
        let output = isolated.finish_with_timeout(Duration::from_secs(30));
        assert!(
            output.status.success(),
            "isolated report fixture:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed;"));
        return;
    }
    let harness = Harness::new();
    let first = fifo(&harness, "first.botwork");
    let second = fifo(&harness, "second.botwork");
    fs::write(harness.workspace.join("queued.botwork"), "Log |999|").unwrap();
    let mut running = Running::start(
        &harness,
        &["first.botwork", "second.botwork", "queued.botwork"],
        &["--jobs", "2"],
        Some(Stdio::piped()),
    );
    let stderr = running.child.stderr.take().unwrap();
    let (sent, received) = mpsc::channel();
    let observer = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut headers = String::new();
        for _ in 0..2 {
            reader.read_line(&mut headers).unwrap();
        }
        drop(reader); // Future status writes fail with BrokenPipe.
        let _ = sent.send(headers);
    });
    let headers = received
        .recv_timeout(Duration::from_secs(5))
        .expect("start records before watchdog");
    observer.join().unwrap();
    assert!(
        headers.contains("[run 1] started:") && headers.contains("[run 2] started:"),
        "{headers}"
    );
    let a = writer(&first, &mut running);
    let b = writer(&second, &mut running);
    release(a, 1);
    thread::sleep(Duration::from_millis(200));
    assert!(
        running.child.try_wait().unwrap().is_none(),
        "second run still owns its read"
    );
    release(b, 2);
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stdout, b"1\n2\n",
        "admitted runs drain; queued run never starts"
    );
}

#[test]
fn failed_start_reporting_prevents_all_script_effects() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("entry.botwork"), "Log |7|").unwrap();
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let mut running = Running::start(
        &harness,
        &["entry.botwork", "entry.botwork"],
        &[],
        Some(full.into()),
    );
    let output = running.finish();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn a_blocked_status_writer_leaves_admitted_runs_schedulable() {
    let harness = Harness::new();
    let first = fifo(&harness, "first.botwork");
    // This legal argv string exceeds Linux's path limit. Its escaped status
    // record fills the undrained pipe before the second run can start.
    let long_path = "x".repeat(32768);
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut descriptors = [-1; 2];
    // SAFETY: the array has space for both descriptors; File takes each new
    // descriptor exactly once and owns its eventual close.
    assert_eq!(
        unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    let mut stderr = unsafe { fs::File::from_raw_fd(descriptors[0]) };
    let destination = unsafe { fs::File::from_raw_fd(descriptors[1]) };
    assert!(unsafe { libc::fcntl(stderr.as_raw_fd(), libc::F_SETPIPE_SZ, 4096) } >= 4096);
    let mut running = Running::start(
        &harness,
        &["first.botwork", &long_path],
        &["--jobs", "2"],
        Some(destination.into()),
    );
    release(writer(&first, &mut running), 777);
    let end = Instant::now() + Duration::from_secs(2);
    let progressed = loop {
        if fs::read(&running.stdout).unwrap() == b"777\n" {
            break true;
        }
        if Instant::now() >= end {
            break false;
        }
        thread::sleep(Duration::from_millis(2));
    };
    // Drain before asserting so an inline-write regression fails cleanly.
    let drain = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let output = running.finish();
    let stderr = String::from_utf8(drain.join().unwrap()).unwrap();
    assert!(progressed, "blocked reporting stalled an admitted run");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"777\n");
    assert!(stderr.contains("[run 1] succeeded:"));
    assert!(stderr.contains("[run 2] failed:"));
}
