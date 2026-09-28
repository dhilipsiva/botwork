#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    diagnostic::DiagnosticCode,
    run::{Engine, ImportLimits, RunLimits, RunOptions, RunOutcome},
};
use cli_harness::Harness;
use std::{fs, time::Duration};

fn options(harness: &Harness) -> RunOptions {
    RunOptions {
        working_directory: Some(harness.workspace.clone()),
        inherit_environment: false,
        ..RunOptions::default()
    }
}

#[tokio::test]
async fn regular_file_bytes_utf8_limits_and_errors_match_synchronous_execution() {
    let harness = Harness::new();
    let engine = Engine::default();
    for (name, bytes, limit) in [
        ("exact.botwork", b"|x| = |7|".as_slice(), 9),
        ("excess.botwork", b"|x| = |7|".as_slice(), 8),
        ("invalid.botwork", b"\xff".as_slice(), 1),
        ("size-before-utf8.botwork", b"\xff".as_slice(), 0),
        ("empty.botwork", b"".as_slice(), 0),
        ("unicode.botwork", "|x| = |\"é\"|".as_bytes(), 13),
    ] {
        fs::write(harness.workspace.join(name), bytes).unwrap();
        let mut options = options(&harness);
        options.limits.source_bytes = limit;
        let synchronous = engine.run_file(name, options.clone());
        let asynchronous = engine.run_file_async(name, options).await;
        assert_eq!(asynchronous.outcome(), synchronous.outcome(), "{name}");
        assert_eq!(
            format!("{:?}", asynchronous.variables),
            format!("{:?}", synchronous.variables),
            "{name}"
        );
        match (synchronous.result, asynchronous.result) {
            (Ok(left), Ok(right)) => assert_eq!(left.to_string(), right.to_string(), "{name}"),
            (Err(left), Err(right)) => {
                assert_eq!(left.code(), right.code(), "{name}");
                assert_eq!(left.to_string(), right.to_string(), "{name}");
            }
            _ => panic!("execution mode changed file outcome: {name}"),
        }
    }
    for path in ["missing.botwork", "."] {
        let result = engine
            .run_file_async(
                path,
                RunOptions {
                    timeout: Some(Duration::from_secs(5)),
                    ..options(&harness)
                },
            )
            .await;
        assert_eq!(
            result.result.unwrap_err().code(),
            DiagnosticCode::SourceRead
        );
    }
    let cli = harness
        .run("cli-read", "Log |7|", Duration::from_secs(10))
        .unwrap();
    assert!(cli.status.success());
    assert_eq!(cli.stdout, b"7\n");
}

#[tokio::test]
async fn imports_keep_cache_accounting_and_reject_excess_bytes_before_utf8() {
    let harness = Harness::new();
    let module = "Read { Return |9| }";
    fs::write(harness.workspace.join("module.botwork"), module).unwrap();
    let engine = Engine::default();
    let run_options = RunOptions {
        limits: RunLimits {
            imports: ImportLimits {
                loads: 1,
                source_bytes: module.len(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..options(&harness)
    };
    let result = engine.run_source_async(
        "entry.botwork",
        "Import |\"module.botwork\"| As |first|\nImport |\"module.botwork\"| As |second|\n|answer| = second::Read",
        run_options,
    ).await;
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.result
    );
    assert_eq!(result.variables["answer"].to_string(), "9");

    fs::write(harness.workspace.join("bad.botwork"), [0xff]).unwrap();
    let result = engine
        .run_source_async(
            "entry.botwork",
            "|kept| = |7|\nTry { Import |\"bad.botwork\"| As |bad| } Catch { |caught| = |true| }",
            RunOptions {
                limits: RunLimits {
                    imports: ImportLimits {
                        source_bytes: 0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..options(&harness)
            },
        )
        .await;
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    let error = result.result.unwrap_err();
    assert!(error.to_string().contains("module source bytes"));
    assert!(error
        .related
        .iter()
        .any(|site| site.message == "imported here"));
    assert_eq!(result.variables["kept"].to_string(), "7");
    assert!(!result.variables.contains_key("caught"));
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use botwork::core::operation::OperationControl;
    use std::{
        io::{self, Write},
        os::unix::fs::OpenOptionsExt,
        path::PathBuf,
        process::Command,
        sync::mpsc,
        thread,
        time::Instant,
    };

    fn writer(
        path: PathBuf,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        mpsc::Sender<()>,
        thread::JoinHandle<bool>,
    ) {
        assert!(Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = mpsc::channel();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut pipe = loop {
                match fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&path)
                {
                    Ok(pipe) => break pipe,
                    Err(error)
                        if error.raw_os_error() == Some(libc::ENXIO)
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("open FIFO writer: {error}"),
                }
            };
            let _ = entered.send(());
            // Always release a reader after the watchdog, including on assertion
            // failure. The returned flag proves the executor released us first.
            let released = wait.recv_timeout(Duration::from_secs(5)).is_ok();
            match pipe.write_all(b"|loaded| = |9|\nRead { Return |loaded| }") {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                Err(error) => panic!("write FIFO: {error}"),
            }
            released
        });
        (ready, release, worker)
    }

    async fn stalled_read(import: bool, cancel: bool) {
        let harness = Harness::new();
        let (ready, release, writer) = writer(harness.workspace.join("slow.botwork"));
        let control = OperationControl::default();
        let stop = control.clone();
        let options = RunOptions {
            control,
            ..options(&harness)
        };
        let task = tokio::spawn(async move {
            let engine = Engine::default();
            if import {
                engine
                    .run_source_async(
                        "entry.botwork",
                        "|kept| = |7|\nImport |\"slow.botwork\"| As |slow|\n|answer| = slow::Read",
                        options,
                    )
                    .await
            } else {
                engine.run_file_async("slow.botwork", options).await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        assert!(!task.is_finished());
        let sibling = Engine::default()
            .run_source_async("sibling", "|answer| = |6 * 7|", super::options(&harness))
            .await;
        assert_eq!(sibling.outcome(), RunOutcome::Succeeded);
        assert_eq!(sibling.variables["answer"].to_string(), "42");
        if cancel {
            stop.cancel();
        }
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            writer.join().unwrap(),
            "filesystem work blocked the async executor until the watchdog released it"
        );
        assert_eq!(
            result.outcome(),
            if cancel {
                RunOutcome::Cancelled
            } else {
                RunOutcome::Succeeded
            },
            "{:?}",
            result.result
        );
        if cancel {
            assert!(!result.variables.contains_key("loaded"));
            assert!(!result.variables.contains_key("answer"));
        } else {
            assert_eq!(
                result.variables[if import { "answer" } else { "loaded" }].to_string(),
                "9"
            );
        }
        if import {
            assert_eq!(result.variables["kept"].to_string(), "7");
        }
    }

    #[tokio::test]
    async fn blocked_entry_read_leaves_sibling_runs_responsive() {
        stalled_read(false, false).await;
    }

    #[tokio::test]
    async fn blocked_import_read_leaves_sibling_runs_responsive() {
        stalled_read(true, false).await;
    }

    #[tokio::test]
    async fn cancelled_entry_read_drains_before_returning_without_execution() {
        stalled_read(false, true).await;
    }

    #[tokio::test]
    async fn cancelled_import_read_preserves_prior_state_without_module_publication() {
        stalled_read(true, true).await;
    }
}
