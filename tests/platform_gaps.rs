//! Facilities not yet ported off Linux fail clearly and before any effect
//! (decision D12). Each test goes when its facility's port lands.
#![cfg(not(target_os = "linux"))]

use botwork::core::{
    diagnostic::DiagnosticCode as Code,
    operation::OperationControl,
    run::{Engine, RunOptions},
    worker::{WorkerCommand, WorkerLimits, WorkerPool},
};
use std::{collections::BTreeMap, path::Path, process::Command};

/// A program and arguments that would create `marker` in its directory.
#[cfg(windows)]
const MARKER: (&str, &[&str]) = ("cmd", &["/c", "type nul > marker"]);
#[cfg(not(windows))]
const MARKER: (&str, &[&str]) = ("touch", &["marker"]);

fn arguments() -> String {
    let quoted: Vec<String> = MARKER.1.iter().map(|word| format!("{word:?}")).collect();
    format!("[{}]", quoted.join(", "))
}

fn options(directory: &Path) -> RunOptions {
    RunOptions {
        working_directory: Some(directory.into()),
        ..Default::default()
    }
}

#[test]
fn process_statements_fail_before_starting_a_process() {
    let dir = tempfile::tempdir().unwrap();
    let (program, _) = MARKER;
    for statement in ["Run Process", "Run Binary Process"] {
        for options_map in ["", r#" Options |{"timeout_ms": 1000}|"#] {
            let source = format!(
                "{statement} |{program:?}| With Arguments |{}|{options_map}",
                arguments()
            );
            let result = Engine::default().run_source("process", &source, options(dir.path()));
            let error = result.result.expect_err(&source);
            assert_eq!(error.code(), Code::RunConfiguration, "{source}");
            assert!(
                error
                    .to_string()
                    .contains("Process statements currently require Linux"),
                "{error}"
            );
        }
    }
    assert!(!dir.path().join("marker").exists());
}

#[test]
fn process_statements_fail_in_the_cli_with_their_code() {
    let dir = tempfile::tempdir().unwrap();
    let (program, _) = MARKER;
    std::fs::write(
        dir.path().join("process.botwork"),
        format!("Run Process |{program:?}| With Arguments |{}|", arguments()),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(["--file", "process.botwork"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("[BW7002]")
            && stderr.contains("Process statements currently require Linux"),
        "{stderr}"
    );
    assert!(!dir.path().join("marker").exists());
}

#[test]
fn worker_pools_refuse_entry_and_keep_their_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let (program, arguments) = MARKER;
    let pool = WorkerPool::new(WorkerLimits::default()).unwrap();
    let executable = if cfg!(windows) {
        Path::new(r"C:\Windows\System32").join(format!("{program}.exe"))
    } else {
        Path::new("/usr/bin").join(program)
    };
    // More attempts than the pool has slots: a refusal must not hold one.
    for _ in 0..=WorkerLimits::default().max_in_flight.get() {
        let command = WorkerCommand {
            executable: executable.clone(),
            arguments: arguments.iter().map(Into::into).collect(),
            directory: dir.path().into(),
            environment: BTreeMap::new(),
        };
        let error = pool
            .start(command, Vec::new(), OperationControl::default())
            .err()
            .expect("refused");
        assert_eq!(error.code(), Code::RunConfiguration);
        assert!(
            error
                .to_string()
                .contains("Isolated workers currently require Linux"),
            "{error}"
        );
    }
    let snapshot = pool.snapshot();
    assert!(snapshot.active.is_empty() && snapshot.completed.is_empty());
    assert!(!dir.path().join("marker").exists());
}

#[test]
fn process_tree_pools_are_refused_at_construction() {
    let guardian = std::env::current_exe().unwrap();
    let error = WorkerPool::with_process_tree(WorkerLimits::default(), guardian)
        .err()
        .expect("refused");
    assert_eq!(error.code(), Code::RunConfiguration);
    assert!(
        error.to_string().contains("Worker guardians require Linux"),
        "{error}"
    );
}
