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
