//! Ctrl-Break on Windows interrupts as SIGINT does on Unix: the first stops runs
//! cooperatively, and a second exits at once. Each test starts the CLI in a
//! process group of its own, the only target `GenerateConsoleCtrlEvent` can
//! single out.
#![cfg(windows)]
#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use serde_json::Value;
use std::{
    fs,
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::{
    Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT},
    Threading::CREATE_NEW_PROCESS_GROUP,
};

struct Running {
    child: Child,
    stdout: PathBuf,
    stderr: PathBuf,
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
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .unwrap();
    Running {
        child,
        stdout,
        stderr,
    }
}

impl Running {
    fn stderr(&self) -> String {
        fs::read_to_string(&self.stderr).unwrap()
    }

    fn stdout(&self) -> String {
        fs::read_to_string(&self.stdout).unwrap()
    }

    fn wait_for(&self, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.stderr().contains(marker) {
            assert!(
                Instant::now() < deadline,
                "waiting for {marker:?}: {}",
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn interrupt(&self) {
        // SAFETY: sends Ctrl-Break to the child's own process group.
        let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.child.id()) };
        assert_ne!(sent, 0, "{}", std::io::Error::last_os_error());
    }

    fn finish(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap();
            }
            assert!(Instant::now() < deadline, "{}", self.stderr());
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn ctrl_break_stops_started_runs_and_publishes_an_interrupted_report() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("fast.botwork"), "No Operation").unwrap();
    fs::write(harness.workspace.join("slow.botwork"), "Sleep |30000|").unwrap();
    fs::write(harness.workspace.join("later.botwork"), "No Operation").unwrap();
    let mut running = spawn(
        &harness,
        "batch",
        &[
            "--file",
            "fast.botwork",
            "--file",
            "slow.botwork",
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
    running.wait_for("[run 3] started:");
    let start = Instant::now();
    running.interrupt();
    assert_eq!(running.finish(), 1);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "cancellation is cooperative, not a wait"
    );
    let stderr = running.stderr();
    assert!(
        stderr.contains("[interrupted] stopping runs; interrupt again to exit at once"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("[run 4] started"),
        "admission stops: {stderr}"
    );
    assert!(
        stderr.ends_with(
            "Interrupted: 3 of 4 selected runs have outcomes; the others never started\n"
        ),
        "{stderr}"
    );
    let report: Value =
        serde_json::from_slice(&fs::read(harness.workspace.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["verdict"]["status"], "interrupted");
    assert_eq!(report["verdict"]["delivery"], "interrupted");
    assert_eq!(report["complete"], false);
}

#[test]
fn a_second_ctrl_break_exits_at_once() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("fast.botwork"), "Log |\"fast\"|").unwrap();
    fs::write(
        harness.workspace.join("stubborn.botwork"),
        "Try {\n    Log |\"inside\"|\n    Sleep |30000|\n} Finally { Sleep |30000| }",
    )
    .unwrap();
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
        ],
    );
    // Interrupt only inside the Try, so that its Finally keeps the run going
    // until the second interrupt.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !running.stdout().contains("inside") {
        assert!(Instant::now() < deadline, "{}", running.stderr());
        std::thread::sleep(Duration::from_millis(10));
    }
    running.interrupt();
    running.wait_for("[interrupted] stopping runs");
    running.interrupt();
    assert_eq!(running.finish(), 130);
    // The report stays an incomplete marker with its journal to reconcile.
    assert!(harness.workspace.join("report.json.journal").exists());
}

/// A forced exit ends the processes its runs started: each worker's Job
/// Object closes with the CLI, and its descendants with it. Reconciliation
/// then marks the run interrupted.
#[test]
fn a_second_ctrl_break_ends_the_processes_runs_started() {
    let harness = Harness::new();
    let python = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .flat_map(|directory| ["python.exe", "python3.exe"].map(|name| directory.join(name)))
        .find(|path| path.is_absolute() && path.is_file())
        .expect("Python, which the repository's checks use");
    let tool =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/worker_tool.py");
    let heartbeat = harness.workspace.join("heartbeat");
    // The process starts in Finally, which runs on after the first interrupt
    // under its own cleanup allowance; its descendant writes the heartbeat.
    fs::write(
        harness.workspace.join("processes.botwork"),
        format!(
            "Try {{\n    Log |\"inside\"|\n    Sleep |30000|\n}} Finally {{\n    Run Process |{:?}| With Arguments |[{:?}, \"tree\", {:?}]|\n}}",
            python.to_str().unwrap(),
            tool.to_str().unwrap(),
            heartbeat.to_str().unwrap(),
        ),
    )
    .unwrap();
    let mut running = spawn(
        &harness,
        "processes",
        &[
            "--file",
            "processes.botwork",
            "--cleanup-timeout-ms",
            "30000",
            "--report-json",
            "report.json",
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while !running.stdout().contains("inside") {
        assert!(Instant::now() < deadline, "{}", running.stderr());
        std::thread::sleep(Duration::from_millis(10));
    }
    running.interrupt();
    running.wait_for("[interrupted] stopping runs");
    let size = || fs::metadata(&heartbeat).map_or(0, |metadata| metadata.len());
    while size() == 0 {
        assert!(Instant::now() < deadline, "{}", running.stderr());
        std::thread::sleep(Duration::from_millis(10));
    }
    running.interrupt();
    assert_eq!(running.finish(), 130);
    // The heartbeat stops once the job has ended the descendant.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let before = size();
        std::thread::sleep(Duration::from_millis(300));
        if size() == before {
            break;
        }
        assert!(Instant::now() < deadline, "the descendant outlived the CLI");
    }
    let output = harness
        .command(
            "reconcile",
            &["--reconcile-report", "report.json"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Reconciled report.json: 1 of 1 selected runs have records, 1 of them interrupted\n"
    );
}
