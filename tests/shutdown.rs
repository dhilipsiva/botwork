#![cfg(unix)]
//! A stopped run returns within its stop grace even when started blocking work
//! cannot stop: a full output pipe, a FIFO nobody opens, or a callback that
//! ignores its control. That work is abandoned, and cleanup gets its own bound.
#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use botwork::core::{
    diagnostic::DiagnosticCode,
    operation::{NativeOperation, OperationControl},
    run::{Engine, RunOptions},
    signature::StatementSignature,
};
use cli_harness::Harness;
use std::{
    fs::{self, File},
    io::PipeReader,
    num::NonZeroUsize,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const ABANDONED: &str = "did not stop within 100 ms of the stop and was abandoned";

fn fifo(path: &Path) {
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
}

/// Let every reader blocked opening `path` return end of file.
fn release(path: &Path) {
    let _ = fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path);
}

fn spawn(workspace: &Path, arguments: &[&str], stdout: Stdio) -> Child {
    Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(arguments)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(File::create(workspace.join("stderr.txt")).unwrap())
        .spawn()
        .unwrap()
}

/// Wait at most `bound` for the CLI; return its status, stderr, and elapsed time.
fn finish(workspace: &Path, mut child: Child, bound: Duration) -> (Option<i32>, String, Duration) {
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > bound {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the CLI outlived {bound:?}: {}",
                fs::read_to_string(workspace.join("stderr.txt")).unwrap()
            );
        }
        thread::sleep(Duration::from_millis(5));
    };
    let stderr = fs::read_to_string(workspace.join("stderr.txt")).unwrap();
    (status.code(), stderr, start.elapsed())
}

fn wait_for(workspace: &Path, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let stderr = fs::read_to_string(workspace.join("stderr.txt")).unwrap_or_default();
        if stderr.contains(marker) {
            return;
        }
        assert!(Instant::now() < deadline, "{marker:?} never appeared");
        thread::sleep(Duration::from_millis(5));
    }
}

/// A pipe whose read end stays open and is never read.
fn unread_pipe() -> (PipeReader, Stdio) {
    let (read, write) = std::io::pipe().unwrap();
    (read, Stdio::from(write))
}

// Logs 128 KiB records: the first one fills a 64 KiB pipe and blocks.
const LOUD: &str = r#"|text| = |"x"|
|i| = |0|
While |i < 17| {
    |text| = |text + text|
    |i| = |i + 1|
}
Write File |"started.txt"| Text |"started"|
While |true| { Log |text| }
"#;
const LOUD_OUTPUT: [&str; 4] = [
    "--max-output-record-bytes",
    "1000000",
    "--max-output-bytes",
    "100000000",
];

#[test]
fn a_destination_that_never_reads_cannot_hold_a_stopped_cli() {
    let harness = Harness::new();
    let workspace = &harness.workspace;
    fs::write(workspace.join("loud.botwork"), LOUD).unwrap();
    // A deadline.
    let (_read, stdout) = unread_pipe();
    let mut arguments = vec!["--file", "loud.botwork", "--timeout-ms", "300"];
    arguments.extend(["--stop-grace-ms", "100"]);
    arguments.extend(LOUD_OUTPUT);
    let child = spawn(workspace, &arguments, stdout);
    let (status, stderr, _) = finish(workspace, child, Duration::from_secs(10));
    assert_eq!(status, Some(1), "{stderr}");
    assert!(
        stderr.contains("[BW5002]") && stderr.contains(ABANDONED),
        "{stderr}"
    );
    // One interrupt, with no second signal.
    fs::remove_file(workspace.join("started.txt")).unwrap();
    let (_read, stdout) = unread_pipe();
    let mut arguments = vec!["--file", "loud.botwork", "--stop-grace-ms", "100"];
    arguments.extend(LOUD_OUTPUT);
    let child = spawn(workspace, &arguments, stdout);
    let started = workspace.join("started.txt");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the script never started");
        thread::sleep(Duration::from_millis(5));
    }
    thread::sleep(Duration::from_millis(200));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let (status, stderr, elapsed) = finish(workspace, child, Duration::from_secs(10));
    assert_eq!(status, Some(1), "{stderr}");
    assert!(
        stderr.contains("[BW5001]") && stderr.contains(ABANDONED),
        "{stderr}"
    );
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[test]
fn blocked_reads_and_preparation_end_within_the_stop_grace() {
    let harness = Harness::new();
    let workspace = &harness.workspace;
    fifo(&workspace.join("pipe.fifo"));
    fs::write(workspace.join("ok.botwork"), "Log |\"ok\"|\n").unwrap();
    fs::write(
        workspace.join("read.botwork"),
        "Log |@{ Read File |\"pipe.fifo\"| }|\n",
    )
    .unwrap();
    fs::write(
        workspace.join("ok.suite.botwork"),
        "Suite |\"s\"| {\n    Case |\"one\"| { Log |\"ok\"| }\n}\n",
    )
    .unwrap();
    fs::write(
        workspace.join("fixture.suite.botwork"),
        "Suite |\"f\"| {\n    SuiteSetup { No Operation }\n    Case |\"one\"| { Log |\"ok\"| }\n}\n",
    )
    .unwrap();
    let grace = ["--stop-grace-ms", "100"];
    let cases: [&[&str]; 5] = [
        // A statement's read.
        &["--file", "read.botwork", "--timeout-ms", "300"],
        // Loading the entry file, then input variables, for a file run.
        &["--file", "pipe.fifo", "--timeout-ms", "300"],
        &[
            "--file",
            "ok.botwork",
            "--vars-file",
            "pipe.fifo",
            "--timeout-ms",
            "300",
        ],
        // A suite case without a fixture, then a fixture owner.
        &[
            "--suite",
            "ok.suite.botwork",
            "--vars-file",
            "pipe.fifo",
            "--timeout-ms",
            "300",
        ],
        &[
            "--suite",
            "fixture.suite.botwork",
            "--vars-file",
            "pipe.fifo",
            "--suite-timeout-ms",
            "300",
        ],
    ];
    for arguments in cases {
        let child = spawn(workspace, &[arguments, &grace].concat(), Stdio::null());
        let (status, stderr, elapsed) = finish(workspace, child, Duration::from_secs(10));
        assert_eq!(status, Some(1), "{arguments:?}: {stderr}");
        assert!(
            stderr.contains("[BW5002]") && stderr.contains(ABANDONED),
            "{arguments:?}: {stderr}"
        );
        assert!(
            elapsed >= Duration::from_millis(400) && elapsed < Duration::from_secs(5),
            "{arguments:?}: {elapsed:?}"
        );
    }
}

#[test]
fn interrupts_reach_suite_cases_without_fixtures() {
    let harness = Harness::new();
    let workspace = &harness.workspace;
    fs::write(
        workspace.join("slow.suite.botwork"),
        "Suite |\"slow\"| {\n    Case |\"one\"| { Sleep |30000| }\n}\n",
    )
    .unwrap();
    let child = spawn(workspace, &["--suite", "slow.suite.botwork"], Stdio::null());
    wait_for(workspace, "[case slow/one] started:");
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let (status, stderr, elapsed) = finish(workspace, child, Duration::from_secs(10));
    assert_eq!(status, Some(1), "{stderr}");
    assert!(stderr.contains("[case slow/one] cancelled:"), "{stderr}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[test]
fn cleanup_failures_and_blocked_cleanup_keep_the_run_bounded() {
    let harness = Harness::new();
    let workspace = &harness.workspace;
    fifo(&workspace.join("pipe.fifo"));
    fs::write(
        workspace.join("fails.botwork"),
        "Try { Sleep |30000| } Finally { Fail |\"cleanup failed\"| }\n",
    )
    .unwrap();
    fs::write(
        workspace.join("blocked.botwork"),
        "Try { Fail |\"body failed\"| } Finally { Log |@{ Read File |\"pipe.fifo\"| }| }\n",
    )
    .unwrap();
    // Failing cleanup after a deadline: the stop stays the outcome.
    let child = spawn(
        workspace,
        &["--file", "fails.botwork", "--timeout-ms", "200"],
        Stdio::null(),
    );
    let (status, stderr, _) = finish(workspace, child, Duration::from_secs(10));
    assert_eq!(status, Some(1), "{stderr}");
    let (stop, cleanup) = (
        stderr.find("[BW5002]").expect("the deadline"),
        stderr.find("cleanup failed").expect("the cleanup failure"),
    );
    assert!(stop < cleanup, "{stderr}");
    // Cleanup blocked on a FIFO ends at its timeout plus the grace.
    let child = spawn(
        workspace,
        &[
            "--file",
            "blocked.botwork",
            "--cleanup-timeout-ms",
            "300",
            "--stop-grace-ms",
            "100",
        ],
        Stdio::null(),
    );
    let (status, stderr, elapsed) = finish(workspace, child, Duration::from_secs(10));
    assert_eq!(status, Some(1), "{stderr}");
    assert!(
        stderr.contains("body failed") && stderr.contains("[BW5002]") && stderr.contains(ABANDONED),
        "{stderr}"
    );
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

/// Runs its closure when dropped, so a failed assertion releases blocked work
/// instead of leaving the test runtime waiting for it.
struct Release<F: FnMut()>(F);

impl<F: FnMut()> Drop for Release<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// Fail, rather than hang, when a stopped run is not bounded.
async fn bounded<F: std::future::Future>(run: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("the stopped run outlived its shutdown bound")
}

fn options(directory: &Path) -> RunOptions {
    let mut options = RunOptions {
        working_directory: Some(directory.to_path_buf()),
        control: OperationControl::default().with_stop_grace(Duration::from_millis(100)),
        timeout: Some(Duration::from_millis(200)),
        ..RunOptions::default()
    };
    options.limits.cleanup.timeout = Duration::from_millis(300);
    options
}

#[tokio::test]
async fn blocked_engine_reads_and_their_cleanup_are_abandoned_after_the_grace() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pipe.fifo");
    fifo(&path);
    let _release = Release(|| release(&path));
    let source = "Try { Log |@{ Read File |\"pipe.fifo\"| }| } Finally { Log |@{ Read File |\"pipe.fifo\"| }| }";
    let start = Instant::now();
    let result = bounded(Engine::default().run_source_async(
        "blocked.botwork",
        source,
        options(directory.path()),
    ))
    .await;
    let elapsed = start.elapsed();
    let error = result.result.expect_err("the deadline stops the run");
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    // The body's read and the cleanup's read are each abandoned under the run's grace.
    let text = error.to_string();
    assert_eq!(text.matches(ABANDONED).count(), 2, "{text}");
    // Deadline, grace, cleanup timeout, grace.
    assert!(
        elapsed >= Duration::from_millis(700) && elapsed < Duration::from_secs(5),
        "{elapsed:?}"
    );
}

#[tokio::test]
async fn an_uncooperative_blocking_operation_is_abandoned_but_keeps_its_capacity() {
    let (unblock, blocked) = mpsc::channel::<()>();
    let blocked = Arc::new(Mutex::new(blocked));
    let _release = Release(|| {
        let _ = unblock.send(());
        let _ = unblock.send(());
    });
    let returned = Arc::new(AtomicBool::new(false));
    let operation = {
        let returned = Arc::clone(&returned);
        NativeOperation::blocking(
            StatementSignature::native("Wait |value|").unwrap(),
            NonZeroUsize::new(1).unwrap(),
            move |values, _control| {
                // Ignores its control entirely.
                let _ = blocked.lock().unwrap().recv();
                returned.store(true, Ordering::SeqCst);
                Ok(values.into_iter().next().unwrap())
            },
        )
        .unwrap()
    };
    let mut engine = Engine::default();
    engine.register_operation(operation).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let abandoned =
        bounded(engine.run_source_async("wait.botwork", "Wait |1|", options(directory.path())))
            .await;
    let elapsed = start.elapsed();
    let error = abandoned.result.expect_err("the deadline stops the run");
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert!(error.to_string().contains(ABANDONED), "{error}");
    assert!(
        elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(5),
        "{elapsed:?}"
    );
    assert!(!returned.load(Ordering::SeqCst), "the callback still runs");
    // Its single permit stays with the abandoned callback, so the next call queues
    // and times out without starting.
    let queued =
        bounded(engine.run_source_async("wait.botwork", "Wait |2|", options(directory.path())))
            .await;
    let error = queued.result.expect_err("no capacity before the deadline");
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    assert!(!error.to_string().contains(ABANDONED), "{error}");
    // Once the abandoned callback returns, its capacity serves the next run.
    drop(_release);
    let mut run = options(directory.path());
    run.timeout = None;
    let finished = bounded(engine.run_source_async("wait.botwork", "Wait |3|", run)).await;
    assert!(finished.result.is_ok(), "{:?}", finished.result);
    assert!(returned.load(Ordering::SeqCst));
}
