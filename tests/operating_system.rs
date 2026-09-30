use botwork::core::{
    diagnostic::DiagnosticCode as Code,
    grammar::Literal,
    run::{Engine, RunLimits, RunOptions, RunOutcome, SnapshotLimits, TemporaryLimits},
    value_limits::ValueLimits,
};
use std::{collections::BTreeMap, fs, path::Path};

fn options(directory: &Path) -> RunOptions {
    RunOptions {
        working_directory: Some(directory.into()),
        inherit_environment: false,
        ..Default::default()
    }
}
fn run(directory: &Path, source: &str) -> botwork::core::run::RunResult {
    Engine::default().run_source("os.botwork", source, options(directory))
}
fn ok(directory: &Path, source: &str) {
    let result = run(directory, source);
    assert!(result.result.is_ok(), "{source}: {:?}", result.result);
}

const FILES: &str = r#"
Create Directory |"work/nested"|
Create Directory |"work/nested"|
Assert |@{ Directory Exists |"work/nested"| }| Equals |true|
Assert |@{ Path Kind |"work/nested"| }| Equals |"directory"|
Assert |@{ List Directory |"work/nested"| }| Equals |[]|
Create File |"work/nested/text"| Text |"hello"|
Append To File |"work/nested/text"| Text |"🙂"|
Assert |@{ Read File |"work/nested/text"| }| Equals |"hello🙂"|
Assert |@{ File Size |"work/nested/text"| }| Equals |"9"|
Write File |"work/nested/text"| Text |"é"|
Assert |@{ Read Binary File |"work/nested/text"| }| Equals |[195, 169]|
Write Binary File |"work/nested/bytes"| Bytes |[0, 255, 128, 10, 13]|
Assert |@{ Read Binary File |"work/nested/bytes"| }| Equals |[0, 255, 128, 10, 13]|
Copy File |"work/nested/bytes"| To |"work/nested/copy"|
Assert |@{ Read Binary File |"work/nested/copy"| }| Equals |[0, 255, 128, 10, 13]|
Move Path |"work/nested/copy"| To |"work/nested/moved"|
Assert |@{ Path Exists |"work/nested/copy"| }| Equals |false|
Assert |@{ File Exists |"work/nested/moved"| }| Equals |true|
Assert |@{ Directory Exists |"work/nested/moved"| }| Equals |false|
Assert |@{ Path Kind |"work/nested/moved"| }| Equals |"file"|
Assert |@{ Path Kind |"missing"| }| Equals |"missing"|
Assert |@{ List Directory |"work/nested"| }| Equals |["bytes", "moved", "text"]|
Remove File |"work/nested/moved"|
Remove File |"work/nested/moved"|
Write File |"work/nested/empty"| Text |""|
Assert |@{ Read File |"work/nested/empty"| }| Equals |""|
Assert |@{ Read Binary File |"work/nested/empty"| }| Equals |[]|
Remove Directory |"work"| Recursively |true|
Remove Directory |"work"| Recursively |true|
Assert |@{ Directory Exists |"work"| }| Equals |false|
Create Directory |"empty"|
Remove Directory |"empty"| Recursively |false|
"#;

#[test]
fn files_directories_copy_move_and_cleanup_preserve_exact_contents() {
    let directory = tempfile::tempdir().unwrap();
    ok(directory.path(), FILES);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn filesystem_statements_have_same_async_effects_and_results() {
    let directory = tempfile::tempdir().unwrap();
    let result = Engine::default()
        .run_source_async("async-os", FILES, options(directory.path()))
        .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn paths_use_native_syntax_and_the_run_directory_without_global_changes() {
    let directory = tempfile::tempdir().unwrap();
    let global = std::env::current_dir().unwrap();
    let expected = botwork::core::paths::canonicalize(directory.path()).unwrap();
    fs::create_dir(directory.path().join("child")).unwrap();
    let result = Engine::default().run_source(
        "paths",
        r#"
Assert |@{ Working Directory }| Equals |expected|
Assert |@{ Absolute Path |""| }| Equals |expected|
Assert |@{ Canonical Path |"child/.."| }| Equals |expected|
Assert |@{ Path Is Absolute |expected| }| Equals |true|
Assert |@{ Path Is Absolute |"relative"| }| Equals |false|
Assert |@{ File Name |"a/b.txt"| }| Equals |"b.txt"|
Assert |@{ Parent Path |"name"| }| Equals |""|
Assert |@{ File Extension |"archive.tar.gz"| }| Equals |"gz"|
Assert |@{ File Extension |"name."| }| Equals |""|
Assert |@{ File Extension |".hidden"| }| Equals |@{ No Operation }|
Assert |@{ Join Path |[]| }| Equals |""|
Assert |@{ Join Path |["ignored", expected]| }| Equals |expected|
Assert |@{ Path Components |"a/./b/../c"| }| Equals |["a", "b", "..", "c"]|
Assert |@{ Operating System }| Equals |platform|
Assert |@{ Path Separator }| Equals |separator|
"#,
        RunOptions {
            variables: BTreeMap::from([
                (
                    "expected".into(),
                    Literal::String(expected.to_str().unwrap().into()),
                ),
                (
                    "platform".into(),
                    Literal::String(std::env::consts::OS.into()),
                ),
                (
                    "separator".into(),
                    Literal::String(std::path::MAIN_SEPARATOR_STR.into()),
                ),
            ]),
            ..options(directory.path())
        },
    );
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(std::env::current_dir().unwrap(), global);
}

#[test]
fn environment_reads_use_exact_immutable_run_overlays_and_preserve_empty_values() {
    let directory = tempfile::tempdir().unwrap();
    // Windows names the same variable in any case, as the system does.
    let source = r#"
Assert |@{ Environment Variable Exists |"Name"| }| Equals |true|
Assert |@{ Environment Variable Exists |"NAME"| }| Equals |WINDOWS|
Assert |@{ Get Environment Variable |"Name"| }| Equals |"é"|
Assert |@{ Get Environment Variable |"empty"| }| Equals |""|
Assert |@{ Get Environment Variable |"removed"| }| Equals |@{ No Operation }|
Assert |@{ Environment Variables }| Equals |{"Name": "é", "empty": ""}|
"#
    .replace("WINDOWS", &cfg!(windows).to_string());
    let result = Engine::default().run_source(
        "environment",
        &source,
        RunOptions {
            environment: BTreeMap::from([
                ("Name".into(), Some("é".into())),
                ("empty".into(), Some("".into())),
                ("removed".into(), None),
            ]),
            ..options(directory.path())
        },
    );
    assert!(result.result.is_ok(), "{:?}", result.result);
    ok(
        directory.path(),
        "Assert |@{ Environment Variables }| Equals |{}|",
    );
}

#[test]
fn invalid_arguments_and_bytes_fail_before_mutating_destinations() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("kept"), b"old").unwrap();
    for source in [
        "Write Binary File |\"kept\"| Bytes |[0, -1]|",
        "Write Binary File |\"kept\"| Bytes |[0, 256]|",
        "Write Binary File |\"kept\"| Bytes |[1.0]|",
        "Write Binary File |\"kept\"| Bytes |[true]|",
        "Read File |1|",
        "Read File |\"\"|",
        "Join Path |[\"a\", 1]|",
        "Environment Variable Exists |\"\"|",
        "Get Environment Variable |\"a=b\"|",
        "Remove Directory |\"kept\"| Recursively |1|",
        "Create Temporary Directory In |\".\"| Prefix |\"../escape\"|",
        "Create Temporary Directory In |\".\"| Prefix |\"é\"|",
    ] {
        let error = run(directory.path(), source).result.unwrap_err();
        assert_eq!(error.code(), Code::IncompatibleType, "{source}: {error}");
        assert_eq!(fs::read(directory.path().join("kept")).unwrap(), b"old");
    }
    for source in [
        "Write File |bad| Text |\"new\"|",
        "Get Environment Variable |bad|",
    ] {
        let result = Engine::default().run_source(
            "nul",
            source,
            RunOptions {
                variables: BTreeMap::from([("bad".into(), Literal::String("a\0b".into()))]),
                ..options(directory.path())
            },
        );
        assert_eq!(result.result.unwrap_err().code(), Code::IncompatibleType);
    }
}

#[test]
fn filesystem_failures_are_catchable_and_never_hide_wrong_parent_types_or_overwrite_copies() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("kept"), b"old").unwrap();
    fs::create_dir(directory.path().join("nonempty")).unwrap();
    fs::write(directory.path().join("nonempty/child"), b"child").unwrap();
    for source in [
        "Read File |\"missing\"|",
        "Read File |\"nonempty\"|",
        "File Size |\"nonempty\"|",
        "Write File |\"missing/child\"| Text |\"new\"|",
        "Create File |\"kept\"| Text |\"new\"|",
        "Copy File |\"kept\"| To |\"kept\"|",
        "Copy File |\"missing\"| To |\"new\"|",
        "Move Path |\"missing\"| To |\"new\"|",
        "Remove File |\"nonempty\"|",
        "Create Directory |\"kept\"|",
        "Remove Directory |\"nonempty\"| Recursively |false|",
        "Path Exists |\"kept/child\"|",
        "File Exists |\"kept/child\"|",
        "Directory Exists |\"kept/child\"|",
        "Canonical Path |\"missing\"|",
        "List Directory |\"kept\"|",
    ] {
        let error = run(directory.path(), source).result.unwrap_err();
        assert_eq!(error.code(), Code::Native, "{source}: {error}");
        assert_eq!(error.call_stack.len(), 1);
        assert_eq!(error.span.as_ref().unwrap().source().name(), "os.botwork");
        assert_eq!(fs::read(directory.path().join("kept")).unwrap(), b"old");
    }
    assert!(!directory.path().join("new").exists());
    ok(
        directory.path(),
        r#"
|value| = |"old"|
Try { |value| = Read File |"missing"| } Catch |error| { Assert |error.code| Equals |"BW4002"| }
Assert |value| Equals |"old"|
"#,
    );
    fs::write(directory.path().join("invalid"), [255]).unwrap();
    assert_eq!(
        run(directory.path(), "Read File |\"invalid\"|")
            .result
            .unwrap_err()
            .code(),
        Code::IncompatibleType
    );
}

#[test]
fn temporary_directories_are_unique_admitted_before_creation_and_explicitly_cleaned() {
    let directory = tempfile::tempdir().unwrap();
    ok(
        directory.path(),
        r#"
|a| = Create Temporary Directory In |"."| Prefix |"test_"|
|b| = Create Temporary Directory In |"."| Prefix |"test_"|
Assert |a != b|
Assert |@{ Path Is Absolute |a| }|
Try {
    Write File |@{ Join Path |[a, "value"]| }| Text |"content"|
    Fail |"test cleanup"|
} Catch {} Finally {
    Remove Directory |a| Recursively |true|
    Remove Directory |b| Recursively |true|
}
"#,
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    let result = Engine::default().run_source(
        "admission",
        "Create Temporary Directory In |\".\"| Prefix |\"x\"|",
        RunOptions {
            limits: RunLimits {
                values: ValueLimits {
                    string_bytes: directory.path().as_os_str().len(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..options(directory.path())
        },
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn file_listing_and_environment_values_obey_exact_output_limits() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a"), "é").unwrap();
    fs::write(directory.path().join("b"), b"b").unwrap();
    for (source, field, boundary) in [
        ("Read File |\"a\"|", 0, 2),
        ("Read Binary File |\"a\"|", 1, 2),
        ("List Directory |\".\"|", 1, 2),
        ("Environment Variables", 1, 2),
    ] {
        for allowed in [false, true] {
            let mut limits = RunLimits::default();
            if field == 0 {
                limits.values.string_bytes = boundary - usize::from(!allowed);
            } else {
                limits.values.entries = boundary - usize::from(!allowed);
            }
            let result = Engine::default().run_source(
                "limit",
                source,
                RunOptions {
                    limits,
                    environment: BTreeMap::from([
                        ("one".into(), Some("1".into())),
                        ("two".into(), Some("2".into())),
                    ]),
                    ..options(directory.path())
                },
            );
            assert_eq!(
                result.result.is_ok(),
                allowed,
                "{source}: {:?}",
                result.result
            );
            if !allowed {
                assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
            }
        }
    }
    for allowed in [false, true] {
        let result = Engine::default().run_source(
            "overlap",
            "Read File |\"a\"|",
            RunOptions {
                limits: RunLimits {
                    temporaries: TemporaryLimits {
                        payload_bytes: 3 - usize::from(!allowed),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..options(directory.path())
            },
        );
        assert_eq!(result.result.is_ok(), allowed, "{:?}", result.result);
    }
    for allowed in [false, true] {
        let result = Engine::default().run_source(
            "environment-overlap",
            "Environment Variables",
            RunOptions {
                environment: BTreeMap::from([("A".into(), Some("xy".into()))]),
                limits: RunLimits {
                    temporaries: TemporaryLimits {
                        payload_bytes: 3 - usize::from(!allowed),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..options(directory.path())
            },
        );
        assert_eq!(result.result.is_ok(), allowed, "{:?}", result.result);
    }
}

#[tokio::test]
async fn filesystem_workers_require_snapshot_admission_before_any_effect() {
    let directory = tempfile::tempdir().unwrap();
    for (source, entries, expected) in [
        ("Write File |\"new\"| Text |\"data\"|", 100, false),
        ("Write File |\"new\"| Text |\"data\"|", 101, true),
        ("Path Is Absolute |\"relative\"|", 100, true),
    ] {
        let result = Engine::default()
            .run_source_async(
                "worker",
                source,
                RunOptions {
                    limits: RunLimits {
                        snapshots: SnapshotLimits {
                            entries,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..options(directory.path())
                },
            )
            .await;
        assert_eq!(
            result.result.is_ok(),
            expected,
            "{source}: {:?}",
            result.result
        );
        if !expected {
            assert!(!directory.path().join("new").exists());
        }
    }
}

#[tokio::test]
async fn concurrent_runs_keep_environment_and_directory_snapshots_separate() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let engine = Engine::default();
    let source = "Write File |\"value\"| Text |@{ Get Environment Variable |\"VALUE\"| }|";
    let (first, second) = tokio::join!(
        engine.run_source_async(
            "first",
            source,
            RunOptions {
                environment: BTreeMap::from([("VALUE".into(), Some("a".into()))]),
                ..options(a.path())
            }
        ),
        engine.run_source_async(
            "second",
            source,
            RunOptions {
                environment: BTreeMap::from([("VALUE".into(), Some("b".into()))]),
                ..options(b.path())
            }
        )
    );
    assert!(first.result.is_ok(), "{:?}", first.result);
    assert!(second.result.is_ok(), "{:?}", second.result);
    assert_eq!(fs::read(a.path().join("value")).unwrap(), b"a");
    assert_eq!(fs::read(b.path().join("value")).unwrap(), b"b");
}

#[test]
fn metadata_is_typed_and_preserves_host_registrations() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let mut context = Context::default();
    context
        .register_native("Operating System", |_| Ok(Literal::Int(42)))
        .unwrap();
    context.init_statements();
    context.init_statements();
    assert_eq!(context.statement_signatures().len(), 100);
    let help = context
        .statement_signature("Write Binary File |p| Bytes |b|")
        .unwrap()
        .unwrap()
        .help();
    for fragment in [
        "path: String",
        "bytes: Array",
        "returns: None",
        "BW3003",
        "BW4002",
    ] {
        assert!(help.contains(fragment), "{help}");
    }
    let value = evaluate_program_detailed(
        &Program::parse("override", "Operating System").unwrap(),
        &mut context,
    )
    .unwrap();
    assert!(matches!(value, Literal::Int(42)));
    let error = evaluate_program_detailed(
        &Program::parse("environment", "Environment Variables").unwrap(),
        &mut context,
    )
    .unwrap_err();
    assert_eq!(error.code(), Code::RunConfiguration);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::{
        ffi::OsStringExt,
        fs::{symlink, PermissionsExt},
    };

    #[test]
    fn symlinks_are_distinguished_and_cleanup_does_not_follow_directory_targets() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("outside")).unwrap();
        fs::write(directory.path().join("outside/kept"), b"kept").unwrap();
        fs::create_dir(directory.path().join("tree")).unwrap();
        symlink("../outside", directory.path().join("tree/link")).unwrap();
        symlink("missing", directory.path().join("dangling")).unwrap();
        symlink("outside/kept", directory.path().join("file-link")).unwrap();
        ok(
            directory.path(),
            r#"
Assert |@{ Path Kind |"tree/link"| }| Equals |"symlink"|
Assert |@{ Directory Exists |"tree/link"| }|
Assert |@{ File Exists |"file-link"| }|
Assert |@{ Read File |"file-link"| }| Equals |"kept"|
Assert |@{ Path Exists |"dangling"| }|
Assert |@{ File Exists |"dangling"| }| Equals |false|
Remove Directory |"tree"| Recursively |true|
Remove File |"dangling"|
"#,
        );
        assert_eq!(
            fs::read(directory.path().join("outside/kept")).unwrap(),
            b"kept"
        );
        let error = run(
            directory.path(),
            "Copy File |\"file-link\"| To |\"outside/kept\"|",
        )
        .result
        .unwrap_err();
        assert_eq!(error.code(), Code::Native);
        symlink("missing", directory.path().join("new-link")).unwrap();
        assert_eq!(
            run(
                directory.path(),
                "Create File |\"new-link\"| Text |\"new\"|"
            )
            .result
            .unwrap_err()
            .code(),
            Code::Native
        );
        assert!(!directory.path().join("missing").exists());
    }

    #[test]
    fn trailing_separators_and_dots_cannot_redirect_recursive_cleanup_through_a_symlink() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("outside")).unwrap();
        fs::write(directory.path().join("outside/kept"), b"kept").unwrap();
        for suffix in ["", "/", "/.", "/./"] {
            symlink("outside", directory.path().join("link")).unwrap();
            let source = format!("Assert |@{{ Path Kind |\"link{suffix}\"| }}| Equals |\"symlink\"|\nRemove Directory |\"link{suffix}\"| Recursively |true|");
            ok(directory.path(), &source);
            assert_eq!(
                fs::read(directory.path().join("outside/kept")).unwrap(),
                b"kept"
            );
            assert!(fs::symlink_metadata(directory.path().join("link")).is_err());
        }
    }

    #[test]
    fn native_non_utf8_values_fail_without_lossy_names_and_temporary_directories_are_private() {
        let directory = tempfile::tempdir().unwrap();
        // macOS file systems store names as UTF-8 and refuse other bytes.
        #[cfg(not(target_os = "macos"))]
        {
            fs::write(
                directory
                    .path()
                    .join(std::ffi::OsString::from_vec(vec![255])),
                b"data",
            )
            .unwrap();
            assert_eq!(
                run(directory.path(), "List Directory |\".\"|")
                    .result
                    .unwrap_err()
                    .code(),
                Code::IncompatibleType
            );
        }
        for source in [
            "Get Environment Variable |\"bad\"|",
            "Environment Variables",
        ] {
            let result = Engine::default().run_source(
                "utf8",
                source,
                RunOptions {
                    environment: BTreeMap::from([(
                        "bad".into(),
                        Some(std::ffi::OsString::from_vec(vec![255])),
                    )]),
                    ..options(directory.path())
                },
            );
            assert_eq!(result.result.unwrap_err().code(), Code::IncompatibleType);
        }
        let result = run(
            directory.path(),
            "|temporary| = Create Temporary Directory In |\".\"| Prefix |\"private_\"|",
        );
        assert!(result.result.is_ok(), "{:?}", result.result);
        let Literal::String(path) = &result.variables["temporary"] else {
            panic!()
        };
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn file_size_is_exact_beyond_int_range_and_text_preserves_control_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let file = fs::File::create(directory.path().join("sparse")).unwrap();
        file.set_len(4_294_967_297).unwrap();
        ok(
            directory.path(),
            "Assert |@{ File Size |\"sparse\"| }| Equals |\"4294967297\"|",
        );
        let expected = "a\r\n\t\0🙂";
        fs::write(directory.path().join("controls"), expected).unwrap();
        let result = Engine::default().run_source(
            "controls",
            "Assert |@{ Read File |\"controls\"| }| Equals |expected|",
            RunOptions {
                variables: BTreeMap::from([("expected".into(), Literal::String(expected.into()))]),
                ..options(directory.path())
            },
        );
        assert!(result.result.is_ok(), "{:?}", result.result);
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_stalled_file_open_keeps_the_single_thread_executor_available() {
    use std::{
        os::unix::fs::OpenOptionsExt,
        process::Command,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let fifo = directory.path().join("fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let (release, wait) = mpsc::channel();
    let writer = thread::spawn(move || {
        // A watchdog releases an accidentally inline open, turning a hang into a
        // failed assertion. Nonblocking writer retries also have a finite bound.
        let timely = wait.recv_timeout(Duration::from_secs(3)).is_ok();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
            {
                Ok(file) => {
                    drop(file);
                    return timely;
                }
                Err(error)
                    if error.raw_os_error() == Some(libc::ENXIO) && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(1))
                }
                Err(error) => panic!("FIFO writer: {error}"),
            }
        }
    });
    let engine = Engine::default();
    let (result, sibling) = tokio::join!(
        engine.run_source_async(
            "stalled-open",
            "Read File |\"fifo\"|",
            options(directory.path())
        ),
        async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let result = engine
                .run_source_async("sibling", "No Operation", options(directory.path()))
                .await;
            let _ = release.send(());
            result
        }
    );
    assert!(sibling.result.is_ok());
    assert_eq!(
        result.result.unwrap_err().code(),
        Code::Native,
        "an opened FIFO is not a regular file"
    );
    assert!(
        writer.join().unwrap(),
        "filesystem open blocked the executor until the watchdog fired"
    );
}
