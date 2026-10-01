#![cfg(unix)]
//! Interrupts and deadlines reach nested statements, process and HTTP I/O,
//! listeners, and cleanup through the CLI.
#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use std::{
    fs,
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Whether `pid` runs: a zombie has already ended, and only its parent's wait
/// remains.
fn alive(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    !state.is_empty() && !state.starts_with('Z')
}

fn spawn_cli(workspace: &Path, arguments: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(arguments)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{path:?} never appeared");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Wait for the CLI and return its status and stdout.
fn finish(mut child: std::process::Child) -> (Option<i32>, String, Duration) {
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the CLI did not stop"
        );
        thread::sleep(Duration::from_millis(5));
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    (status.code(), stdout, start.elapsed())
}

#[test]
fn interrupts_stop_cli_processes_requests_and_nested_cleanup() {
    let harness = Harness::new();
    let workspace: PathBuf = harness.workspace.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}/", listener.local_addr().unwrap());
    thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            held.push(stream.unwrap());
        }
    });
    fs::write(
        workspace.join("process.botwork"),
        r#"Try {
    Run Process |"/bin/sh"| With Arguments |["-c", "echo $$ > child.pid; exec sleep 60"]|
} Finally {
    Log |"cleanup ran"|
}"#,
    )
    .unwrap();
    fs::write(
        workspace.join("request.botwork"),
        format!(
            "Write File |\"requesting\"| Text |\"yes\"|\nHTTP Request |\"GET\"| To |\"{server}\"|"
        ),
    )
    .unwrap();
    let child = spawn_cli(
        &workspace,
        &[
            "--file",
            "process.botwork",
            "--file",
            "request.botwork",
            "--jobs",
            "2",
        ],
    );
    wait_for(&workspace.join("child.pid"));
    wait_for(&workspace.join("requesting"));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let (code, stdout, took) = finish(child);
    assert_eq!(code, Some(1));
    assert!(took < Duration::from_secs(10), "stopped in {took:?}");
    assert_eq!(stdout, "cleanup ran\n", "Finally ran after the interrupt");
    let pid: u32 = fs::read_to_string(workspace.join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        !alive(pid),
        "the process statement's child {pid} is stopped"
    );
}

#[test]
fn deadlines_reach_cleanup_through_nested_calls_and_loops() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("nested.botwork"),
        r#"Inner { Sleep |60000| }
Outer {
    For |item| In |[1, 2]| {
        Try { Inner } Finally { Log |item| }
    }
}
Outer"#,
    )
    .unwrap();
    let output = harness
        .command(
            "nested",
            &["--file", "nested.botwork", "--timeout-ms", "200"],
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    // The first iteration's cleanup runs; the deadline stops the loop.
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "1\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("[BW5002]"));
}

#[test]
fn interrupts_reach_a_hung_listener_within_its_close_timeout() {
    let harness = Harness::new();
    let workspace = harness.workspace.clone();
    fs::write(workspace.join("slow.botwork"), "Sleep |60000|").unwrap();
    let child = spawn_cli(
        &workspace,
        &[
            "--file",
            "slow.botwork",
            "--listener",
            "sh",
            "--listener-arg",
            "-c",
            "--listener-arg",
            "echo $$ > listener.pid; exec sleep 60",
            "--listener-timeout-ms",
            "300",
        ],
    );
    wait_for(&workspace.join("listener.pid"));
    thread::sleep(Duration::from_millis(100));
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    let (code, _, took) = finish(child);
    assert_eq!(code, Some(1));
    assert!(took < Duration::from_secs(10), "stopped in {took:?}");
    let pid: u32 = fs::read_to_string(workspace.join("listener.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !alive(pid),
        "the listener {pid} is stopped with the invocation"
    );
}
