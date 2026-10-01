//! The default worker pool and the process statements on every supported
//! platform: a process group on Linux and macOS, a Job Object on Windows. The
//! worker is tests/support/worker_tool.py, which behaves the same everywhere.

use botwork::core::{
    diagnostic::DiagnosticCode,
    grammar::Literal,
    operation::OperationControl,
    run::{Engine, RunOptions},
    worker::{WorkerCleanup, WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool, WorkerReport},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn python() -> PathBuf {
    let names: &[&str] = if cfg!(windows) {
        &["python.exe", "python3.exe"]
    } else {
        &["python3"]
    };
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_absolute() && path.is_file())
        .expect("Python, which the repository's checks use")
}

fn tool() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/worker_tool.py")
}

fn command(arguments: &[&str]) -> WorkerCommand {
    let mut environment = BTreeMap::new();
    // Python on Windows needs SystemRoot to start; worker pools pass only
    // what the command names.
    if let Some(root) = std::env::var_os("SystemRoot") {
        environment.insert(OsString::from("SystemRoot"), root);
    }
    WorkerCommand {
        executable: python(),
        arguments: std::iter::once(tool().into_os_string())
            .chain(arguments.iter().map(OsString::from))
            .collect(),
        directory: std::env::temp_dir(),
        environment,
    }
}

fn limits() -> WorkerLimits {
    WorkerLimits {
        timeout: Duration::from_secs(20),
        cleanup_timeout: Duration::from_secs(5),
        ..Default::default()
    }
}

fn run(pool: &WorkerPool, arguments: &[&str], input: &[u8]) -> WorkerReport {
    let handle = pool
        .start(
            command(arguments),
            input.to_vec(),
            OperationControl::default(),
        )
        .unwrap();
    wait(handle)
}

fn wait(handle: botwork::core::worker::WorkerHandle) -> WorkerReport {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(60), handle.wait())
                .await
                .expect("supervision finishes")
        })
}

/// Whether no process has this ID any longer.
#[cfg(unix)]
fn gone(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(windows)]
fn gone(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE,
        },
    };
    // SAFETY: opens the process by ID, checks the result, and closes it.
    unsafe {
        let process = OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if process.is_null() {
            return true;
        }
        let exited = WaitForSingleObject(process, 0) == WAIT_OBJECT_0;
        CloseHandle(process);
        exited
    }
}

#[test]
fn workers_copy_binary_input_and_report_their_exit() {
    let pool = WorkerPool::new(limits()).unwrap();
    let input = b"hello\0\xff\r\n\x1a".to_vec();
    let report = run(&pool, &["cat"], &input);
    assert_eq!(
        report.outcome,
        WorkerOutcome::Succeeded,
        "{:?}",
        report.diagnostic
    );
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert_eq!(report.stdout, input);
    assert!(report.io_complete && report.progress_complete);
    assert_eq!(report.stdin_written, input.len());
    assert!(report.exit_status.unwrap().success());
    assert!(pool.snapshot().active.is_empty());
}

#[test]
fn nonzero_exits_fail_with_their_status_and_output() {
    let pool = WorkerPool::new(limits()).unwrap();
    let report = run(&pool, &["fail"], b"");
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert_eq!(report.exit_status.unwrap().code(), Some(3));
    assert_eq!(report.stderr, b"broken");
    assert!(report.diagnostic.unwrap().to_string().contains("exited"));
}

#[test]
fn output_beyond_its_limit_stops_the_worker() {
    let pool = WorkerPool::new(WorkerLimits {
        stdout_bytes: 1024,
        ..limits()
    })
    .unwrap();
    let report = run(&pool, &["flood"], b"");
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert!(report.stdout.len() <= 1024);
    assert!(report
        .diagnostic
        .unwrap()
        .to_string()
        .contains("worker stdout bytes"));
}

#[test]
fn deadlines_and_cancellation_end_a_running_worker() {
    let pool = WorkerPool::new(WorkerLimits {
        timeout: Duration::from_secs(3),
        ..limits()
    })
    .unwrap();
    let start = Instant::now();
    let report = run(&pool, &["sleep"], b"");
    assert_eq!(report.outcome, WorkerOutcome::TimedOut);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert_eq!(report.diagnostic.unwrap().code(), DiagnosticCode::Timeout);
    assert!(start.elapsed() < Duration::from_secs(30));

    let pool = WorkerPool::new(limits()).unwrap();
    let handle = pool
        .start(command(&["sleep"]), vec![], OperationControl::default())
        .unwrap();
    // Cancel once the worker has started.
    let begun = Instant::now();
    while pool
        .snapshot()
        .active
        .first()
        .and_then(|worker| worker.pid)
        .is_none()
    {
        assert!(
            begun.elapsed() < Duration::from_secs(20),
            "worker never started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    handle.cancel();
    let report = wait(handle);
    assert_eq!(report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    assert!(pool.snapshot().active.is_empty());
}

#[test]
fn large_input_and_output_flow_together_without_deadlock() {
    let pool = WorkerPool::new(WorkerLimits {
        request_bytes: 2 * 1024 * 1024,
        stdout_bytes: 2 * 1024 * 1024,
        ..limits()
    })
    .unwrap();
    let size = 1024 * 1024;
    let report = run(
        &pool,
        &["backpressure", &size.to_string()],
        &vec![b'z'; size],
    );
    assert_eq!(
        report.outcome,
        WorkerOutcome::Succeeded,
        "{:?}",
        report.diagnostic
    );
    assert_eq!(report.stdout.len(), size);
    assert_eq!(report.stderr, size.to_string().as_bytes());
    assert_eq!(report.stdin_written, size);
}

#[test]
fn descendants_end_with_their_worker() {
    let pool = WorkerPool::new(limits()).unwrap();
    let report = run(&pool, &["orphan"], b"");
    assert_eq!(
        report.outcome,
        WorkerOutcome::Succeeded,
        "{:?}",
        report.diagnostic
    );
    assert_eq!(report.cleanup, WorkerCleanup::Reaped);
    let descendant: u32 = String::from_utf8(report.stdout).unwrap().parse().unwrap();
    // The worker's group or job is terminated once it exits.
    let begun = Instant::now();
    while !gone(descendant) {
        assert!(
            begun.elapsed() < Duration::from_secs(10),
            "descendant {descendant} outlived its worker"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn process_statements_run_the_platforms_programs() {
    let directory = tempfile::tempdir().unwrap();
    let source = r#"
|failed| = Run Process |python| With Arguments |[tool, "fail"]|
Assert |failed.success| Equals |false|
Assert |failed.exit_code| Equals |3|
Assert |failed.stderr| Equals |"broken"|
|copied| = Run Binary Process |python| With Arguments |[tool, "cat"]| Options |{"stdin": [0, 255, 10]}|
Assert |copied.stdout| Equals |[0, 255, 10]|
Assert |copied.success| Equals |true|
# An option replaces the inherited variable it names; on Windows the host
# spells it Path, which the option's PATH names too.
|path| = Run Process |python| With Arguments |[tool, "env", "PATH"]| Options |{"environment": {"PATH": "chosen"}}|
Assert |path.stdout| Equals |"chosen"|
"#;
    let text = |path: PathBuf| Literal::String(path.to_str().unwrap().into());
    let result = Engine::default().run_source(
        "processes",
        source,
        RunOptions {
            working_directory: Some(directory.path().into()),
            variables: BTreeMap::from([
                ("python".to_owned(), text(python())),
                ("tool".to_owned(), text(tool())),
            ]),
            ..RunOptions::default()
        },
    );
    assert!(result.result.is_ok(), "{:?}", result.result);
}
