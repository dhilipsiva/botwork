#![cfg(unix)]

use botwork::core::{
    diagnostic::DiagnosticCode as Code,
    grammar::Literal,
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome},
};
use std::{collections::BTreeMap, fs, path::Path, time::Duration};

/// Process statements share one host-wide budget of in-flight bytes, which
/// rejects at once when exhausted (docs/processes.md), so tests that each
/// reserve a large share of it take turns rather than depend on how many the
/// harness runs at once.
fn serial() -> &'static tokio::sync::Mutex<()> {
    static SERIAL: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    SERIAL.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn options(directory: &Path) -> RunOptions {
    RunOptions {
        working_directory: Some(directory.into()),
        inherit_environment: false,
        ..Default::default()
    }
}
fn run(directory: &Path, source: &str) -> botwork::core::run::RunResult {
    Engine::default().run_source("process.botwork", source, options(directory))
}
fn ok(directory: &Path, source: &str) {
    let result = run(directory, source);
    assert!(result.result.is_ok(), "{source}: {:?}", result.result);
}

const BASIC: &str = r#"
|p| = Run Process |"/usr/bin/printf"| With Arguments |["<%s>", "hello world", "", "*.txt", "$(touch injected)", "; touch injected", "é"]|
Assert |p.stdout| Equals |"<hello world><><*.txt><$(touch injected)><; touch injected><é>"|
Assert |p.stderr| Equals |""|
Assert |p.exit_code| Equals |0|
Assert |p.signal| Equals |@{ No Operation }|
Assert |p.success|
Assert |@{ Length Of |p| }| Equals |5|
|p| = Run Process |"/bin/sh"| With Arguments |["-c", "printf output; printf error >&2; exit 7"]|
Assert |p.stdout| Equals |"output"|
Assert |p.stderr| Equals |"error"|
Assert |p.exit_code| Equals |7|
Assert |p.success| Equals |false|
|p| = Run Process |"/bin/sh"| With Arguments |["-c", "kill -TERM $$"]|
Assert |p.exit_code| Equals |@{ No Operation }|
Assert |p.signal| Equals |15|
Assert |p.success| Equals |false|
"#;

#[test]
fn cli_parallel_scripts_and_suite_fixture_phases_receive_host_snapshots() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let check = r#"
|p| = Run Process |"/bin/sh"| With Arguments |["-c", "printf '%s' \"$BOTWORK_PROCESS_TEST\"; printf '%s' \"$PWD\" >&2"]|
Assert |p.stdout| Equals |"inherited"|
Assert |p.stderr| Equals |@{ Working Directory }|
Assert |@{ Get Environment Variable |"BOTWORK_PROCESS_TEST"| }| Equals |"inherited"|
Log |"checked"|
"#;
    fs::write(dir.path().join("one.botwork"), check).unwrap();
    fs::write(dir.path().join("suite.botwork"), format!("Suite |\"process\"| {{\nSuiteSetup {{ {check} }}\nSuiteTeardown {{ {check} }}\nCaseSetup {{ {check} }}\nCaseTeardown {{ {check} }}\nCase |\"one\"| {{ {check} }}\n}}\n")).unwrap();
    for (arguments, count) in [
        (vec!["--file", "one.botwork"], 1),
        (
            vec![
                "--file",
                "one.botwork",
                "--file",
                "one.botwork",
                "--jobs",
                "2",
            ],
            2,
        ),
        (vec!["--suite", "suite.botwork", "--jobs", "1"], 5),
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(arguments)
            .current_dir(dir.path())
            .env("BOTWORK_PROCESS_TEST", "inherited")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, "checked\n".repeat(count).as_bytes());
    }
}

#[test]
fn metadata_is_typed_idempotent_and_preserves_host_overrides() {
    let _serial = serial().blocking_lock();
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let mut context = Context::default();
    context
        .register_native("Run Process |exe| With Arguments |args|", |_| {
            Ok(Literal::Int(42))
        })
        .unwrap();
    context.init_statements();
    context.init_statements();
    assert_eq!(context.statement_signatures().len(), 100);
    let help = context
        .statement_signature("Run Binary Process |e| With Arguments |a| Options |o|")
        .unwrap()
        .unwrap()
        .help();
    for fragment in [
        "executable: String",
        "arguments: Array",
        "options: Map",
        "returns: Map",
        "BW3003",
        "BW7002",
        "BW8001",
        "BW5001",
        "BW5002",
        "BW5003",
    ] {
        assert!(help.contains(fragment), "{help}");
    }
    let value = evaluate_program_detailed(
        &Program::parse("override", "Run Process |\"x\"| With Arguments |[]|").unwrap(),
        &mut context,
    )
    .unwrap();
    assert!(matches!(value, Literal::Int(42)));
    let error = evaluate_program_detailed(
        &Program::parse(
            "environment",
            "Run Binary Process |\"/usr/bin/true\"| With Arguments |[]|",
        )
        .unwrap(),
        &mut context,
    )
    .unwrap_err();
    assert_eq!(error.code(), Code::RunConfiguration);
}

#[test]
fn command_entries_and_stdin_have_independent_prelaunch_limits() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    for count in [16_383, 16_384] {
        let result = Engine::default().run_source(
            "entries",
            "Run Process |\"/usr/bin/true\"| With Arguments |args|",
            RunOptions {
                variables: BTreeMap::from([(
                    "args".into(),
                    Literal::Array(vec![Literal::String(String::new()); count]),
                )]),
                environment: BTreeMap::from([("A".into(), Some("x".into()))]),
                ..options(dir.path())
            },
        );
        assert_eq!(
            result.result.is_ok(),
            count == 16_383,
            "{:?}",
            result.result
        );
        if count == 16_384 {
            assert!(result
                .result
                .unwrap_err()
                .to_string()
                .contains("process command entries"));
        }
    }
    let mut limits = RunLimits::default();
    limits.values.string_bytes += 1;
    let result = Engine::default().run_source(
        "input-cap",
        r#"Run Process |"/usr/bin/touch"| With Arguments |["marker"]| Options |{"stdin": input}|"#,
        RunOptions {
            variables: BTreeMap::from([(
                "input".into(),
                Literal::String("x".repeat(1024 * 1024 + 1)),
            )]),
            limits,
            ..options(dir.path())
        },
    );
    assert!(result
        .result
        .unwrap_err()
        .to_string()
        .contains("process input bytes"));
    assert!(!dir.path().join("marker").exists());
}

#[test]
fn command_workspace_and_result_overlap_are_admitted_at_the_exact_boundary() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    // The run directory is canonical, and the temporary directory may be
    // reached through a link, as on macOS.
    let root = botwork::core::paths::canonicalize(dir.path()).unwrap();
    let command = root.as_os_str().len() + 1 + "/usr/bin/touch".len() + 1 + "marker".len() + 1;
    let arguments =
        "/usr/bin/touch".len() + "marker".len() + "stdout_limit".len() + "stderr_limit".len() + 8;
    let maximum = 3 * command + arguments + 43;
    for below in [true, false] {
        let mut limits = RunLimits::default();
        limits.temporaries.payload_bytes = maximum - usize::from(below);
        let result = Engine::default().run_source("overlap", r#"Run Process |"/usr/bin/touch"| With Arguments |["marker"]| Options |{"stdout_limit": 0, "stderr_limit": 0}|"#, RunOptions { limits, ..options(dir.path()) });
        assert_eq!(
            result.result.is_ok(),
            !below,
            "maximum={maximum}: {:?}",
            result.result
        );
        assert_eq!(dir.path().join("marker").exists(), !below);
        if below {
            assert!(result
                .result
                .unwrap_err()
                .to_string()
                .contains("temporary value payload bytes"));
        }
    }
}

#[test]
fn imported_process_statements_keep_the_run_directory() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("library")).unwrap();
    fs::write(
        dir.path().join("library/process.botwork"),
        "Where { Return |@{ Run Process |\"/bin/pwd\"| With Arguments |[]| }| }",
    )
    .unwrap();
    let result = Engine::default().run_source(
        "imports",
        r#"
Import |"library/process.botwork"| As |library|
|p| = library::Where
Assert |p.stdout| Equals |expected|
"#,
        RunOptions {
            variables: BTreeMap::from([(
                "expected".into(),
                // The child starts in the canonical run directory.
                Literal::String(format!(
                    "{}\n",
                    botwork::core::paths::canonicalize(dir.path())
                        .unwrap()
                        .display()
                )),
            )]),
            ..options(dir.path())
        },
    );
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[tokio::test]
async fn synchronous_call_inside_a_runtime_and_async_local_timeout_are_supported() {
    let _serial = serial().lock().await;
    let dir = tempfile::tempdir().unwrap();
    ok(
        dir.path(),
        "Run Process |\"/usr/bin/true\"| With Arguments |[]|",
    );
    let result = Engine::default()
        .run_source_async(
            "deadline",
            r#"
Try { Run Process |"/usr/bin/touch"| With Arguments |["marker"]| Options |{"timeout_ms": 0}| }
Catch |error| { Write File |"caught"| Text |"bad"| }
Finally { Write File |"finally"| Text |"yes"| }
"#,
            options(dir.path()),
        )
        .await;
    assert_eq!(
        result.outcome(),
        RunOutcome::TimedOut,
        "{:?}",
        result.result
    );
    assert!(!dir.path().join("marker").exists());
    assert!(!dir.path().join("caught").exists());
    assert_eq!(fs::read(dir.path().join("finally")).unwrap(), b"yes");
}

#[test]
fn inherited_run_deadline_stops_a_longer_process_allowance() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let result = Engine::default().run_source(
        "inherited-deadline",
        BLOCK,
        RunOptions {
            timeout: Some(Duration::from_millis(50)),
            ..options(dir.path())
        },
    );
    assert_eq!(
        result.outcome(),
        RunOutcome::TimedOut,
        "{:?}",
        result.result
    );
    if let Ok(pid) = fs::read_to_string(dir.path().join("started")) {
        assert!(!Path::new(&format!("/proc/{}", pid.trim())).exists());
    }
}

#[test]
fn literal_arguments_and_exit_or_signal_results_work_synchronously() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    ok(dir.path(), BASIC);
    assert!(!dir.path().join("injected").exists());
}

#[tokio::test]
async fn literal_arguments_and_exit_or_signal_results_work_asynchronously() {
    let _serial = serial().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let result = Engine::default()
        .run_source_async("async-process", BASIC, options(dir.path()))
        .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert!(!dir.path().join("injected").exists());
}

#[test]
fn text_and_binary_stdin_preserve_nul_bytes_and_empty_streams() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let result = Engine::default().run_source("input", r#"
|p| = Run Process |"/bin/cat"| With Arguments |[]| Options |{"stdin": input}|
Assert |p.stdout| Equals |input|
|p| = Run Binary Process |"/bin/cat"| With Arguments |[]| Options |{"stdin": [0, 255, 128, 10, 13]}|
Assert |p.stdout| Equals |[0, 255, 128, 10, 13]|
Assert |p.stderr| Equals |[]|
|p| = Run Binary Process |"/usr/bin/true"| With Arguments |[]|
Assert |p.stdout| Equals |[]|
Assert |p.exit_code| Equals |0|
|p| = Run Process |"/bin/cat"| With Arguments |[]| Options |{"stdin": @{ No Operation }, "stdout_limit": 0, "stderr_limit": 0}|
Assert |p.stdout| Equals |""|
"#, RunOptions { variables: BTreeMap::from([("input".into(), Literal::String("é\0\n".into()))]), ..options(dir.path()) });
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn invalid_utf8_is_rejected_on_either_stream_but_binary_capture_is_exact() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    for redirect in ["", " >&2"] {
        let source = format!(
            r#"Run Process |"/bin/sh"| With Arguments |["-c", "printf '\\377'{redirect}"]|"#
        );
        let error = run(dir.path(), &source).result.unwrap_err();
        assert_eq!(error.code(), Code::IncompatibleType, "{error}");
        assert!(error.to_string().contains(if redirect.is_empty() {
            "stdout"
        } else {
            "stderr"
        }));
    }
    ok(
        dir.path(),
        r#"
|p| = Run Binary Process |"/bin/sh"| With Arguments |["-c", "printf '\\377'; printf '\\200' >&2"]|
Assert |p.stdout| Equals |[255]|
Assert |p.stderr| Equals |[128]|
"#,
    );
}

#[test]
fn option_and_argument_validation_precedes_process_creation() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    for invalid in [
        r#"{"unknown": true}"#,
        r#"{"directory": ""}"#,
        r#"{"directory": 1}"#,
        r#"{"environment": []}"#,
        r#"{"environment": {"": "x"}}"#,
        r#"{"environment": {"A=B": "x"}}"#,
        r#"{"environment": {"A": 1}}"#,
        r#"{"inherit_environment": 1}"#,
        r#"{"stdin": []}"#,
        r#"{"timeout_ms": -1}"#,
        r#"{"timeout_ms": 0.5}"#,
        r#"{"cleanup_timeout_ms": -1}"#,
        r#"{"stdout_limit": -1}"#,
        r#"{"stderr_limit": false}"#,
    ] {
        let source = format!(
            r#"Run Process |"/usr/bin/touch"| With Arguments |["marker"]| Options |{invalid}|"#
        );
        let error = run(dir.path(), &source).result.unwrap_err();
        assert_eq!(error.code(), Code::IncompatibleType, "{source}: {error}");
        assert!(!dir.path().join("marker").exists());
    }
    for input in [r#""text""#, "[256]", "[-1]", "[1.0]", "[true]"] {
        let source = format!(
            r#"Run Binary Process |"/usr/bin/touch"| With Arguments |["marker"]| Options |{{"stdin": {input}}}|"#
        );
        assert_eq!(
            run(dir.path(), &source).result.unwrap_err().code(),
            Code::IncompatibleType
        );
        assert!(!dir.path().join("marker").exists());
    }
    for source in [
        r#"Run Process |"/usr/bin/touch"| With Arguments |["marker", 1]|"#,
        r#"Run Process |""| With Arguments |[]|"#,
        r#"Run Process |1| With Arguments |[]|"#,
        r#"Run Process |"/usr/bin/true"| With Arguments |{}|"#,
        r#"Run Process |"/usr/bin/true"| With Arguments |[]| Options |[]|"#,
    ] {
        assert_eq!(
            run(dir.path(), source).result.unwrap_err().code(),
            Code::IncompatibleType
        );
        assert!(!dir.path().join("marker").exists());
    }
}

#[test]
fn nul_in_native_command_fields_is_rejected_before_launch() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    for source in [
        "Run Process |bad| With Arguments |[]|",
        "Run Process |\"/usr/bin/touch\"| With Arguments |[\"marker\", bad]|",
        "Run Process |\"/usr/bin/touch\"| With Arguments |[\"marker\"]| Options |{\"directory\": bad}|",
        "Run Process |\"/usr/bin/touch\"| With Arguments |[\"marker\"]| Options |{\"environment\": {\"A\": bad}}|",
    ] {
        let result = Engine::default().run_source("nul", source, RunOptions {
            variables: BTreeMap::from([("bad".into(), Literal::String("x\0y".into()))]), ..options(dir.path())
        });
        assert_eq!(result.result.unwrap_err().code(), Code::IncompatibleType);
        assert!(!dir.path().join("marker").exists());
    }
}

#[test]
fn directories_and_environment_are_per_call_with_explicit_path_search() {
    let _serial = serial().blocking_lock();
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    // The run directory is canonical; see above.
    let root = botwork::core::paths::canonicalize(dir.path()).unwrap();
    let global = std::env::current_dir().unwrap();
    fs::create_dir(dir.path().join("child")).unwrap();
    fs::write(
        dir.path().join("child/local"),
        "#!/bin/sh\nprintf '%s:%s:%s' \"$A\" \"${B-unset}\" \"$EMPTY\"\nprintf '%s' \"$PWD\" >&2\n",
    )
    .unwrap();
    fs::set_permissions(
        dir.path().join("child/local"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let result = Engine::default().run_source("environment", r#"
|p| = Run Process |"local"| With Arguments |[]| Options |{"directory": "child", "environment": {"PATH": ".", "A": "new", "B": @{ No Operation }, "EMPTY": ""}}|
Assert |p.stdout| Equals |"new:unset:"|
Assert |p.stderr| Equals |child|
|p| = Run Process |"./local"| With Arguments |[]| Options |{"directory": "child", "inherit_environment": false}|
Assert |p.stdout| Equals |":unset:"|
|p| = Run Process |"child/local"| With Arguments |[]|
Assert |p.stdout| Equals |"old:kept:"|
Assert |p.stderr| Equals |root|
Assert |@{ Get Environment Variable |"A"| }| Equals |"old"|
Assert |@{ Get Environment Variable |"B"| }| Equals |"kept"|
Assert |@{ Working Directory }| Equals |root|
"#, RunOptions {
        variables: BTreeMap::from([("root".into(), Literal::String(root.to_str().unwrap().into())), ("child".into(), Literal::String(root.join("child").to_str().unwrap().into()))]),
        environment: BTreeMap::from([("A".into(), Some("old".into())), ("B".into(), Some("kept".into()))]),
        ..options(dir.path())
    });
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(std::env::current_dir().unwrap(), global);
}

#[test]
fn non_utf8_environment_snapshot_is_passed_without_lossy_conversion() {
    let _serial = serial().blocking_lock();
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let result = Engine::default().run_source(
        "native-environment",
        r#"
|p| = Run Binary Process |"/usr/bin/env"| With Arguments |[]|
Assert |p.stdout| Equals |[255, 61, 128, 10]|
"#,
        RunOptions {
            environment: BTreeMap::from([(
                std::ffi::OsString::from_vec(vec![255]),
                Some(std::ffi::OsString::from_vec(vec![128])),
            )]),
            ..options(dir.path())
        },
    );
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn capture_limits_are_exact_and_overflow_bypasses_catch() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    for stream in ["stdout", "stderr"] {
        for count in [0, 1, 7] {
            for overflow in [false, true] {
                let text = "a".repeat(count + usize::from(overflow));
                let redirect = if stream == "stderr" { " >&2" } else { "" };
                let source = format!(
                    r#"
Try {{ Run Process |"/bin/sh"| With Arguments |["-c", "printf '{text}'{redirect}"]| Options |{{"{stream}_limit": {count}}}| }}
Catch |error| {{ Write File |"caught"| Text |"bad"| }}
Finally {{ Write File |"finally"| Text |"yes"| }}
"#
                );
                let result = run(dir.path(), &source);
                assert_eq!(
                    result.result.is_ok(),
                    !overflow,
                    "{source}: {:?}",
                    result.result
                );
                if overflow {
                    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
                }
                assert!(!dir.path().join("caught").exists());
                assert_eq!(fs::read(dir.path().join("finally")).unwrap(), b"yes");
            }
        }
    }
}

#[test]
fn output_admission_precedes_launch_even_if_program_would_print_nothing() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    for binary in [false, true] {
        for too_small in [true, false] {
            let mut limits = RunLimits::default();
            if binary {
                limits.values.entries = 100 - usize::from(too_small);
            } else {
                limits.values.string_bytes = 100 - usize::from(too_small);
            }
            let source = format!(
                r#"Run {}Process |"/usr/bin/touch"| With Arguments |["marker"]| Options |{{"stdout_limit": 100, "stderr_limit": 0}}|"#,
                if binary { "Binary " } else { "" }
            );
            let result = Engine::default().run_source(
                "preflight",
                &source,
                RunOptions {
                    limits,
                    ..options(dir.path())
                },
            );
            assert_eq!(result.result.is_ok(), !too_small, "{:?}", result.result);
            assert_eq!(dir.path().join("marker").exists(), !too_small);
            if !too_small {
                fs::remove_file(dir.path().join("marker")).unwrap();
            }
        }
    }
}

#[test]
fn launch_and_decoding_errors_preserve_destination_and_call_frames() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    ok(
        dir.path(),
        r#"
Wrapper |exe| { Run Process |exe| With Arguments |[]| }
|kept| = |42|
Try { |kept| = Wrapper |"/no-such-botwork-process"| }
Catch |error| { Assert |error.code| Equals |"BW5003"| }
Assert |kept| Equals |42|
"#,
    );
    let result = run(
        dir.path(),
        "Wrapper { Run Process |\"/no-such-botwork-process\"| With Arguments |[]| }\nWrapper",
    );
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), Code::AsyncRuntime);
    // Frames show each statement as written.
    assert!(
        error.to_string().contains("in `Wrapper` called at"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("in `Run Process |executable| With Arguments |arguments|` called at"),
        "{error}"
    );
}

#[test]
fn closed_stdin_is_an_error_even_when_child_exits_successfully() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let result = Engine::default().run_source(
        "incomplete",
        r#"Run Process |"/usr/bin/true"| With Arguments |[]| Options |{"stdin": input}|"#,
        RunOptions {
            variables: BTreeMap::from([("input".into(), Literal::String("x".repeat(1024 * 1024)))]),
            ..options(dir.path())
        },
    );
    assert_eq!(result.result.unwrap_err().code(), Code::AsyncRuntime);
}

async fn file_ready(path: &Path) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = fs::read_to_string(path) {
                if !text.trim().is_empty() {
                    break text;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("process handshake")
}
async fn reaped(pid: &str) {
    let path = format!("/proc/{}", pid.trim());
    tokio::time::timeout(Duration::from_secs(5), async {
        while Path::new(&path).exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("direct child is reaped");
}
const BLOCK: &str =
    r#"Run Process |"/bin/sh"| With Arguments |["-c", "echo $$ > started; while :; do :; done"]|"#;

#[tokio::test]
async fn cancellation_reaps_child_and_does_not_block_current_thread_runtime() {
    let _serial = serial().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = OperationControl::default();
    let engine = Engine::default();
    let source = format!("Try {{ {BLOCK} }} Catch |error| {{ Write File |\"caught\"| Text |\"bad\"| }} Finally {{ Write File |\"finally\"| Text |\"yes\"| }}");
    let pending = engine.run_source_async(
        "cancel",
        &source,
        RunOptions {
            control: control.clone(),
            ..options(dir.path())
        },
    );
    let (result, pid) = tokio::join!(pending, async {
        let pid = file_ready(&dir.path().join("started")).await;
        control.cancel();
        pid
    });
    assert_eq!(
        result.outcome(),
        RunOutcome::Cancelled,
        "{:?}",
        result.result
    );
    reaped(&pid).await;
    assert!(!dir.path().join("caught").exists());
    assert_eq!(fs::read(dir.path().join("finally")).unwrap(), b"yes");
}

#[tokio::test]
async fn dropping_suspended_run_cancels_and_reaps_its_child() {
    let _serial = serial().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::default();
    let mut pending = Box::pin(engine.run_source_async("drop", BLOCK, options(dir.path())));
    let marker = dir.path().join("started");
    let pid = tokio::select! {
        result = &mut pending => panic!("process ended before drop: {:?}", result.result),
        pid = file_ready(&marker) => pid,
    };
    drop(pending);
    reaped(&pid).await;
}

#[test]
fn local_timeouts_are_noncatchable_and_cleanup_still_runs() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let source = format!("Try {{ {BLOCK} Options |{{\"timeout_ms\": 100}}| }} Catch |error| {{ Write File |\"caught\"| Text |\"bad\"| }} Finally {{ Write File |\"finally\"| Text |\"yes\"| }}");
    let result = run(dir.path(), &source);
    assert_eq!(result.result.unwrap_err().code(), Code::Timeout);
    assert!(!dir.path().join("caught").exists());
    assert_eq!(fs::read(dir.path().join("finally")).unwrap(), b"yes");
}

#[tokio::test]
async fn concurrent_process_calls_keep_run_environment_and_directory_separate() {
    let _serial = serial().lock().await;
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let engine = Engine::default();
    let source =
        r#"Run Process |"/bin/sh"| With Arguments |["-c", "printf '%s' \"$VALUE\" > value"]|"#;
    let (a_result, b_result) = tokio::join!(
        engine.run_source_async(
            "a",
            source,
            RunOptions {
                environment: BTreeMap::from([("VALUE".into(), Some("a".into()))]),
                ..options(a.path())
            }
        ),
        engine.run_source_async(
            "b",
            source,
            RunOptions {
                environment: BTreeMap::from([("VALUE".into(), Some("b".into()))]),
                ..options(b.path())
            }
        ),
    );
    assert!(a_result.result.is_ok(), "{:?}", a_result.result);
    assert!(b_result.result.is_ok(), "{:?}", b_result.result);
    assert_eq!(fs::read(a.path().join("value")).unwrap(), b"a");
    assert_eq!(fs::read(b.path().join("value")).unwrap(), b"b");
}

/// A statement whose worker's cleanup it did not see finish, here because its
/// cleanup allowance is zero, does not let the run end before the worker has.
#[test]
fn a_run_ends_only_after_the_process_its_statement_left_behind() {
    let _serial = serial().blocking_lock();
    let dir = tempfile::tempdir().unwrap();
    let source = format!("{BLOCK} Options |{{\"timeout_ms\": 100, \"cleanup_timeout_ms\": 0}}|");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    for attempt in 0..20 {
        let started = dir.path().join("started");
        let _ = fs::remove_file(&started);
        // Synchronous and asynchronous runs alike.
        let result = if attempt % 2 == 0 {
            run(dir.path(), &source)
        } else {
            runtime.block_on(Engine::default().run_source_async(
                "process.botwork",
                &source,
                options(dir.path()),
            ))
        };
        let error = result.result.unwrap_err();
        // The worker ended as the run did, so its failure names no workers.
        assert!(
            !error.to_string().contains("not cleaned up within"),
            "{error}"
        );
        let pid = fs::read_to_string(&started).unwrap();
        assert!(
            !Path::new(&format!("/proc/{}", pid.trim())).exists(),
            "process {} outlived its run: {error}",
            pid.trim()
        );
    }
}
