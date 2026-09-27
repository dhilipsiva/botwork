#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    grammar::{BWErr, Literal},
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome},
    signature::{StatementSignature, ValueKind},
};
use cli_harness::Harness;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

fn options(directory: &std::path::Path) -> RunOptions {
    RunOptions {
        working_directory: Some(directory.to_owned()),
        ..RunOptions::default()
    }
}

fn limited(steps: u64, depth: usize) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            steps,
            call_depth: depth,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

#[test]
fn engine_reuses_owned_programs_without_leaking_any_dsl_state() {
    let engine = Engine::default();
    let program = Program::parse(
        "reuse.botwork",
        "Double |value| { Return |value * 2| }\n|answer| = Double |input|",
    )
    .unwrap();
    for input in [2, 5, 9] {
        let run = engine.run_program(
            &program,
            RunOptions {
                variables: BTreeMap::from([("input".into(), Literal::Int(input))]),
                ..RunOptions::default()
            },
        );
        assert_eq!(run.outcome(), RunOutcome::Succeeded);
        assert_eq!(run.result.unwrap().to_string(), (input * 2).to_string());
        assert_eq!(run.variables.len(), 2);
        assert!(run.steps > 0);
    }
    let missing = engine.run_source("fresh", "Double |2|", RunOptions::default());
    assert_eq!(
        missing.result.unwrap_err().code(),
        DiagnosticCode::UndefinedStatement
    );
    let missing = engine.run_source("fresh", "|r| = |input|", RunOptions::default());
    assert_eq!(
        missing.result.unwrap_err().code(),
        DiagnosticCode::UndefinedVariable
    );
}

#[test]
fn results_retain_owned_values_diagnostics_and_only_completed_assignments() {
    let report = {
        let engine = Engine::default();
        let source = "|completed| = |7|\nFail { |private| = |9|\nReturn |missing| }\nFail\n|unreachable| = |8|".to_owned();
        engine.run_source("failure.botwork", &source, RunOptions::default())
    };
    assert_eq!(report.outcome(), RunOutcome::Failed);
    assert_eq!(report.variables.len(), 1);
    assert_eq!(report.variables["completed"].to_string(), "7");
    let error = report.result.unwrap_err();
    assert_eq!(error.span.as_ref().unwrap().text(), "missing");
    assert_eq!(error.call_stack.len(), 1);
    assert!(error.to_string().contains("failure.botwork:3:"));
}

#[test]
fn invalid_source_and_inputs_prevent_native_effects() {
    let count = Arc::new(AtomicUsize::new(0));
    let called = Arc::clone(&count);
    let mut engine = Engine::default();
    engine
        .register_native("Effect", move |_, _| {
            called.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let invalid = engine.run_source("invalid", "Effect\nBreak", RunOptions::default());
    assert_eq!(
        invalid.result.unwrap_err().code(),
        DiagnosticCode::InvalidControl
    );
    let invalid = engine.run_source(
        "invalid",
        "Effect",
        RunOptions {
            variables: BTreeMap::from([("bad name".into(), Literal::Int(1))]),
            ..RunOptions::default()
        },
    );
    assert_eq!(invalid.result.unwrap_err().code(), DiagnosticCode::Input);
    assert!(invalid.variables.is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[test]
fn native_metadata_kinds_collisions_and_shared_captures_use_existing_contracts() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let mut engine = Engine::default();
    engine
        .register_native_with_signature(
            StatementSignature::native("Count |value|")
                .unwrap()
                .parameter("value", ValueKind::Int)
                .unwrap()
                .returns(ValueKind::Int),
            move |_, _| Ok(Literal::Int(count.fetch_add(1, Ordering::SeqCst) as i32)),
        )
        .unwrap();
    assert!(engine
        .register_native("Count |other|", |_, _| Ok(Literal::None))
        .is_err());
    assert!(engine
        .register_native("If |x|", |_, _| Ok(Literal::None))
        .is_err());
    let invalid = engine.run_source("kind", "Count |true|", RunOptions::default());
    assert_eq!(
        invalid.result.unwrap_err().code(),
        DiagnosticCode::IncompatibleType
    );
    let cloned = engine.clone();
    for (index, engine) in [&engine, &cloned, &engine].into_iter().enumerate() {
        assert_eq!(
            engine
                .run_source("count", "Count |1|", RunOptions::default())
                .result
                .unwrap()
                .to_string(),
            index.to_string()
        );
    }
}

#[test]
fn environment_overlay_is_per_run_and_never_changes_the_host() {
    let before: BTreeMap<_, _> = std::env::vars_os().collect();
    let cwd = std::env::current_dir().unwrap();
    let inherited = before
        .keys()
        .find(|name| **name != "BOTWORK_RUN_TEST")
        .unwrap()
        .clone();
    let removed = inherited.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Environment", move |_, environment| {
            assert!(environment.get(&removed).is_none());
            assert_eq!(environment.get("BOTWORK_RUN_TEST").unwrap(), "local");
            assert_eq!(environment.working_directory(), cwd.canonicalize().unwrap());
            Ok(Literal::Int(environment.variables().len() as i32))
        })
        .unwrap();
    let config = RunOptions {
        environment: BTreeMap::from([
            (inherited, None),
            (OsString::from("BOTWORK_RUN_TEST"), Some("local".into())),
        ]),
        ..RunOptions::default()
    };
    assert_eq!(
        engine.run_source("env", "Environment", config).outcome(),
        RunOutcome::Succeeded
    );
    assert_eq!(before, std::env::vars_os().collect());
    let mut engine = Engine::default();
    engine
        .register_native("Environment", |_, environment| {
            Ok(Literal::Int(environment.variables().len() as i32))
        })
        .unwrap();
    let isolated = engine.run_source(
        "env",
        "Environment",
        RunOptions {
            inherit_environment: false,
            ..RunOptions::default()
        },
    );
    assert_eq!(isolated.result.unwrap().to_string(), "0");
}

#[test]
fn setup_failures_keep_default_diagnostic_quotas_and_exact_messages_before_input_installation() {
    use botwork::core::diagnostic::DiagnosticLimits;
    let harness = Harness::new();
    let file = harness.workspace.join("file");
    fs::write(&file, "").unwrap();
    let engine = Engine::default();
    for mut configured in [
        RunOptions {
            environment: BTreeMap::from([("bad=name".into(), Some("value".into()))]),
            ..RunOptions::default()
        },
        RunOptions {
            environment: BTreeMap::from([("name".into(), Some("bad\0value".into()))]),
            ..RunOptions::default()
        },
        RunOptions {
            timeout: Some(Duration::MAX),
            ..RunOptions::default()
        },
        options(&file),
        options(&harness.workspace.join("missing")),
    ] {
        configured.limits.diagnostics = DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        };
        configured.variables = BTreeMap::from([("seed".into(), Literal::Int(7))]);
        let run = engine.run_source("setup", "Unknown", configured);
        assert_eq!(run.outcome(), RunOutcome::Failed);
        assert!(run.variables.is_empty());
        assert_eq!(run.steps, 0);
        let error = run.result.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
        assert!(error.span.is_none() && error.causes.is_empty() && error.omissions.is_none());
        let BWErr::RunConfiguration(message) = error.error.as_ref() else {
            panic!("configuration")
        };
        assert!(
            message == "Environment names must be nonempty and contain neither '=' nor NUL"
                || message == "Environment values must not contain NUL"
                || message == "Timeout exceeds the monotonic clock range"
                || message == "Working directory must be a directory"
                || message.starts_with(&harness.workspace.join("missing").display().to_string())
        );
    }
    let run = engine.run_source(
        "input",
        "Unknown",
        RunOptions {
            variables: BTreeMap::from([("invalid name".into(), Literal::None)]),
            limits: RunLimits {
                diagnostics: DiagnosticLimits {
                    text_bytes: 0,
                    ..DiagnosticLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(
        run.result.unwrap_err().causes[0].code(),
        DiagnosticCode::Input
    );
}

#[test]
fn oversized_setup_messages_retain_bounded_configuration_evidence_and_release_deep_inputs() {
    use botwork::core::diagnostic::DiagnosticLimits;
    let path = PathBuf::from("é".repeat(DiagnosticLimits::default().text_bytes / 2));
    let deep = (0..20_000).fold(Literal::None, |value, _| Literal::Array(vec![value]));
    let effects = Arc::new(AtomicUsize::new(0));
    let seen = effects.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Effect", move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let run = engine.run_source(
        "setup",
        "Effect",
        RunOptions {
            working_directory: Some(path),
            inherit_environment: false,
            variables: BTreeMap::from([("deep".into(), deep)]),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run.variables.is_empty());
    assert_eq!(run.steps, 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    let cause = &error.causes[0];
    assert_eq!(cause.code(), DiagnosticCode::RunConfiguration);
    assert_eq!(cause.omissions.as_ref().unwrap().detail_fields, 1);
    assert!(cause.span.is_none());
    let BWErr::RunConfiguration(message) = cause.error.as_ref() else {
        panic!("configuration")
    };
    assert!(message.len() <= 256 && message.ends_with("…[truncated]"));
    assert_eq!(
        engine
            .run_source("fresh", "Effect", RunOptions::default())
            .outcome(),
        RunOutcome::Succeeded
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[test]
fn invalid_environment_names_values_directories_and_timeouts_fail_before_effects() {
    let engine = Engine::default();
    for name in ["", "bad=name", "bad\0name"] {
        let run = engine.run_source(
            "config",
            "Unknown",
            RunOptions {
                environment: BTreeMap::from([(name.into(), Some("v".into()))]),
                ..RunOptions::default()
            },
        );
        assert_eq!(
            run.result.unwrap_err().code(),
            DiagnosticCode::RunConfiguration
        );
    }
    let run = engine.run_source(
        "config",
        "Unknown",
        RunOptions {
            environment: BTreeMap::from([("name".into(), Some("v\0x".into()))]),
            ..RunOptions::default()
        },
    );
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::RunConfiguration
    );
    let harness = Harness::new();
    let file = harness.workspace.join("file");
    fs::write(&file, "").unwrap();
    for path in [file, harness.workspace.join("missing")] {
        let run = engine.run_source("config", "Unknown", options(&path));
        assert_eq!(
            run.result.unwrap_err().code(),
            DiagnosticCode::RunConfiguration
        );
    }
    let run = engine.run_source(
        "config",
        "Unknown",
        RunOptions {
            timeout: Some(Duration::MAX),
            ..RunOptions::default()
        },
    );
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::RunConfiguration
    );
}

#[test]
fn run_file_and_nested_imports_use_local_directory_and_fresh_module_caches() {
    let harness = Harness::new();
    let before = std::env::current_dir().unwrap();
    fs::create_dir(harness.workspace.join("scripts")).unwrap();
    fs::write(
        harness.workspace.join("scripts/main.botwork"),
        "Import |\"module.botwork\"| As |module|\nmodule::Read",
    )
    .unwrap();
    let module = harness.workspace.join("scripts/module.botwork");
    let engine = Engine::default();
    for value in [3, 9] {
        fs::write(&module, format!("Read {{ Return |{value}| }}")).unwrap();
        let run = engine.run_file("scripts/main.botwork", options(&harness.workspace));
        assert_eq!(run.result.unwrap().to_string(), value.to_string());
    }
    assert_eq!(std::env::current_dir().unwrap(), before);
    // The existing CLI remains usable independently of the embedding API.
    assert!(harness
        .run("empty", "", Duration::from_secs(5))
        .unwrap()
        .status
        .success());
}

#[test]
fn module_native_callbacks_receive_the_same_run_environment() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "|value| = Environment\nRead { Return |value| }",
    )
    .unwrap();
    let mut engine = Engine::default();
    engine
        .register_native("Environment", |_, environment| {
            Ok(Literal::String(
                environment
                    .get("RUN_VALUE")
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ))
        })
        .unwrap();
    let run = engine.run_source(
        "main.botwork",
        "Import |\"module.botwork\"| As |module|\nmodule::Read",
        RunOptions {
            environment: BTreeMap::from([("RUN_VALUE".into(), Some("private".into()))]),
            ..options(&harness.workspace)
        },
    );
    assert_eq!(run.result.unwrap().to_string(), "private");
}

#[test]
fn source_byte_limit_applies_before_parsing_and_uses_exact_utf8_bytes() {
    let engine = Engine::default();
    let source = "# é🙂";
    for limit in [source.len() - 1, source.len()] {
        let run = engine.run_source(
            "bytes",
            source,
            RunOptions {
                limits: RunLimits {
                    source_bytes: limit,
                    ..RunLimits::default()
                },
                ..RunOptions::default()
            },
        );
        assert_eq!(
            run.outcome(),
            if limit < source.len() {
                RunOutcome::LimitExceeded
            } else {
                RunOutcome::Succeeded
            }
        );
        assert_eq!(run.steps, 0);
    }
    let run = engine.run_source(
        "bytes",
        "invalid {{{",
        RunOptions {
            limits: RunLimits {
                source_bytes: 0,
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(
        run.result.unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn entry_file_errors_admit_exact_text_before_detail_formatting_and_preserve_installed_inputs() {
    use botwork::core::diagnostic::DiagnosticLimits;
    let harness = Harness::new();
    fs::write(harness.workspace.join("invalid.botwork"), [0xff]).unwrap();
    let engine = Engine::default();
    for name in ["missing-é.botwork", "invalid.botwork", ".", "nul\0.botwork"] {
        let baseline = engine
            .run_file(name, options(&harness.workspace))
            .result
            .unwrap_err();
        assert_eq!(baseline.code(), DiagnosticCode::SourceRead);
        assert!(baseline.span.is_none() && baseline.call_stack.is_empty());
        let bytes = DiagnosticLimits::default()
            .check(&baseline)
            .unwrap()
            .text_bytes;
        for shortage in [0, 1] {
            let run = engine.run_file(
                name,
                RunOptions {
                    limits: RunLimits {
                        diagnostics: DiagnosticLimits {
                            text_bytes: bytes - shortage,
                            ..DiagnosticLimits::default()
                        },
                        ..RunLimits::default()
                    },
                    variables: BTreeMap::from([("seed".into(), Literal::Int(7))]),
                    ..options(&harness.workspace)
                },
            );
            assert_eq!(run.steps, 0);
            assert_eq!(run.variables["seed"].to_string(), "7");
            assert_eq!(run.variables.len(), 1);
            assert!(run.snapshot_error.is_none());
            let error = run.result.unwrap_err();
            if shortage == 0 {
                assert_eq!(
                    error.to_value().to_string(),
                    baseline.to_value().to_string()
                );
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::SourceRead);
                assert!(error.causes[0].omissions.is_some());
                assert!(error.causes[0].span.is_none());
            }
        }
    }
}

#[test]
fn rejected_entry_file_errors_skip_callbacks_and_fresh_runs_need_no_diagnostic_text() {
    use botwork::core::diagnostic::DiagnosticLimits;
    let harness = Harness::new();
    fs::write(harness.workspace.join("valid.botwork"), "Effect").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Effect", move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let configured = || RunOptions {
        limits: RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        },
        ..options(&harness.workspace)
    };
    let failed = engine.run_file("missing.botwork", configured());
    assert_eq!(failed.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(
        failed.result.unwrap_err().causes[0].code(),
        DiagnosticCode::SourceRead
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        engine.run_file("valid.botwork", configured()).outcome(),
        RunOutcome::Succeeded
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn entry_file_construction_preserves_prior_cancellation_and_source_byte_limits() {
    use botwork::core::diagnostic::DiagnosticLimits;
    let harness = Harness::new();
    let control = OperationControl::default();
    control.cancel();
    let run = Engine::default().run_file(
        "missing.botwork",
        RunOptions {
            control,
            limits: RunLimits {
                diagnostics: DiagnosticLimits {
                    text_bytes: 0,
                    ..DiagnosticLimits::default()
                },
                ..RunLimits::default()
            },
            ..options(&harness.workspace)
        },
    );
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert!(run.result.unwrap_err().causes.is_empty());
    fs::write(harness.workspace.join("too-big.botwork"), [0xff]).unwrap();
    let run = Engine::default().run_file(
        "too-big.botwork",
        RunOptions {
            limits: RunLimits {
                source_bytes: 0,
                diagnostics: DiagnosticLimits {
                    text_bytes: 0,
                    ..DiagnosticLimits::default()
                },
                ..RunLimits::default()
            },
            ..options(&harness.workspace)
        },
    );
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert!(matches!(
        error.causes[0].error.as_ref(),
        BWErr::ResourceLimit {
            resource: "source bytes",
            limit: 0
        }
    ));
}

#[cfg(unix)]
#[test]
fn non_utf8_entry_paths_use_construction_admission_before_filesystem_access() {
    use botwork::core::diagnostic::DiagnosticLimits;
    use std::os::unix::ffi::OsStringExt;
    let path = OsString::from_vec(vec![0xff]);
    let baseline = Engine::default()
        .run_file(&path, RunOptions::default())
        .result
        .unwrap_err();
    assert_eq!(baseline.code(), DiagnosticCode::SourceRead);
    let BWErr::SourceRead(message) = baseline.error.as_ref() else {
        panic!("source read")
    };
    assert_eq!(message, "Source paths must be valid UTF-8");
    let exact = DiagnosticLimits::default()
        .check(&baseline)
        .unwrap()
        .text_bytes;
    for bytes in [exact, exact - 1] {
        let run = Engine::default().run_file(
            &path,
            RunOptions {
                limits: RunLimits {
                    diagnostics: DiagnosticLimits {
                        text_bytes: bytes,
                        ..DiagnosticLimits::default()
                    },
                    ..RunLimits::default()
                },
                ..RunOptions::default()
            },
        );
        let error = run.result.unwrap_err();
        if bytes == exact {
            assert_eq!(error.code(), DiagnosticCode::SourceRead);
        } else {
            assert_eq!(error.causes[0].code(), DiagnosticCode::SourceRead);
        }
    }
}

#[test]
fn file_limits_precede_truncated_utf8_and_import_failures_retain_sites() {
    let harness = Harness::new();
    let engine = Engine::default();
    fs::write(
        harness.workspace.join("big.botwork"),
        format!("#{}🙂", "x".repeat(64)),
    )
    .unwrap();
    let run = engine.run_file(
        "big.botwork",
        RunOptions {
            limits: RunLimits {
                source_bytes: 65,
                ..RunLimits::default()
            },
            ..options(&harness.workspace)
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    let run = engine.run_source(
        "main.botwork",
        "Import |\"big.botwork\"| As |big|",
        RunOptions {
            limits: RunLimits {
                source_bytes: 64,
                ..RunLimits::default()
            },
            ..options(&harness.workspace)
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    let error = run.result.unwrap_err();
    assert_eq!(error.span.as_ref().unwrap().text(), "\"big.botwork\"");
    assert_eq!(error.related.len(), 1);
    for name in ["missing.botwork", "invalid.botwork"] {
        if name.starts_with("invalid") {
            fs::write(harness.workspace.join(name), [0xff]).unwrap();
        }
        assert_eq!(
            engine
                .run_file(name, options(&harness.workspace))
                .result
                .unwrap_err()
                .code(),
            DiagnosticCode::SourceRead
        );
    }
}

#[test]
fn step_budget_counts_statements_expressions_and_empty_loop_iterations() {
    let engine = Engine::default();
    let run = engine.run_source("steps", "|x| = |1 + 2|", limited(4, 32));
    assert_eq!(run.result.unwrap().to_string(), "3");
    assert_eq!(run.steps, 4);
    let run = engine.run_source("steps", "|x| = |1 + 2|", limited(3, 32));
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.steps, 3);
    assert!(run.variables.is_empty());
    for source in ["While |true| {}", "For |item| In |[1, 2, 3]| {}"] {
        let run = engine.run_source("steps", source, limited(6, 32));
        assert_eq!(run.outcome(), RunOutcome::LimitExceeded, "{source}");
        assert_eq!(run.steps, 6);
        assert!(!run.variables.contains_key("item"));
    }
    let run = engine.run_source("steps", "|x| = |-1|", limited(3, 32));
    assert_eq!(run.steps, 3);
    assert!(run.result.is_ok());
}

#[test]
fn limits_skip_handlers_and_preserve_iterator_state_and_entered_call_frames() {
    let engine = Engine::default();
    let run = engine.run_source("limits", "|item| = |9|\nRecurse { Recurse }\nTry { For |item| In |[1]| { Recurse } } Catch { |handled| = |true| }", limited(1000, 3));
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.variables["item"].to_string(), "9");
    assert!(!run.variables.contains_key("handled"));
    let error = run.result.unwrap_err();
    assert_eq!(error.call_stack.len(), 3);
    assert_eq!(error.span.as_ref().unwrap().text(), "Recurse ");
    assert!(error.causes.is_empty());
    let run = engine.run_source("fresh", "|x| = |1|", limited(2, 0));
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
}

#[test]
fn call_depth_is_checked_before_argument_effects_and_budget_is_run_local() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let mut engine = Engine::default();
    engine
        .register_native("Effect", move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(1))
        })
        .unwrap();
    let run = engine.run_source("depth", "Take |x| {}\nTake |@{ Effect }|", limited(100, 0));
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let run = engine.run_source("fresh", "Effect", limited(1, 1));
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn default_call_depth_stops_recursive_source() {
    let run = Engine::default().run_source(
        "recursive",
        "Recurse { Recurse }\nRecurse",
        RunOptions::default(),
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(
        run.result.unwrap_err().call_stack.len(),
        RunLimits::default().call_depth
    );
}

#[test]
fn module_initialization_and_calls_share_the_run_step_budget() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "|x| = |1|\nRead { Return |x| }",
    )
    .unwrap();
    let engine = Engine::default();
    let run = engine.run_source(
        "main.botwork",
        "Import |\"module.botwork\"| As |module|\nmodule::Read",
        RunOptions {
            limits: RunLimits {
                steps: 6,
                ..RunLimits::default()
            },
            ..options(&harness.workspace)
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.steps, 6);
    assert_eq!(
        run.result.unwrap_err().call_stack[0].signature,
        "module::read"
    );
}

#[test]
fn cancelled_and_expired_runs_reject_entry_even_for_empty_or_invalid_programs() {
    let engine = Engine::default();
    let control = OperationControl::default();
    control.cancel();
    for source in ["", "invalid {{{", "Log |\"unreachable\"|"] {
        let cancelled = engine.run_source(
            "stopped",
            source,
            RunOptions {
                control: control.clone(),
                ..RunOptions::default()
            },
        );
        assert_eq!(cancelled.outcome(), RunOutcome::Cancelled);
        assert_eq!(cancelled.steps, 0);
        let expired = engine.run_source(
            "stopped",
            source,
            RunOptions {
                timeout: Some(Duration::ZERO),
                ..RunOptions::default()
            },
        );
        assert_eq!(expired.outcome(), RunOutcome::TimedOut);
        assert_eq!(expired.steps, 0);
    }
}

#[test]
fn callback_stop_requests_skip_assignment_and_handler_without_cancelling_parent() {
    let parent = OperationControl::default();
    let mut engine = Engine::default();
    engine
        .register_native("Stop", |_, environment| {
            environment.control().cancel();
            Ok(Literal::Int(2))
        })
        .unwrap();
    let run = engine.run_source(
        "stop",
        "|x| = |1|\nTry { |x| = Stop } Catch { |handled| = |true| }",
        RunOptions {
            control: parent.clone(),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert_eq!(run.variables["x"].to_string(), "1");
    assert!(!run.variables.contains_key("handled"));
    assert_eq!(run.result.unwrap_err().call_stack[0].signature, "stop");
    assert!(!parent.is_cancelled());
    assert!(engine
        .run_source(
            "fresh",
            "",
            RunOptions {
                control: parent,
                ..RunOptions::default()
            }
        )
        .result
        .is_ok());
}

#[test]
fn external_cancellation_stops_an_active_run_without_requiring_tokio() {
    let (entered, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let mut engine = Engine::default();
    engine
        .register_native("Ready", move |_, _| {
            entered.send(()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(Literal::None)
        })
        .unwrap();
    let control = OperationControl::default();
    let options = RunOptions {
        control: control.clone(),
        limits: RunLimits {
            steps: 1_000_000,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let (send, receive) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        send.send(engine.run_source("loop", "Ready\nWhile |true| {}", options))
            .unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    control.cancel();
    release.send(()).unwrap();
    let run = receive.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    worker.join().unwrap();
}

#[tokio::test(start_paused = true)]
async fn synchronous_native_return_observes_deadline_before_publishing_its_value() {
    let (entered, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let mut engine = Engine::default();
    engine
        .register_native("Slow", move |_, _| {
            entered.send(()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(Literal::Int(2))
        })
        .unwrap();
    let handle = tokio::runtime::Handle::current();
    let (send, receive) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _entered_runtime = handle.enter();
        let run = engine.run_source(
            "timeout",
            "|x| = |1|\n|x| = Slow",
            RunOptions {
                timeout: Some(Duration::from_secs(10)),
                ..RunOptions::default()
            },
        );
        send.send(run).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    tokio::time::advance(Duration::from_secs(11)).await;
    release.send(()).unwrap();
    let run = receive.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(run.outcome(), RunOutcome::TimedOut);
    assert_eq!(run.variables["x"].to_string(), "1");
    assert_eq!(run.result.unwrap_err().call_stack[0].signature, "slow");
    worker.join().unwrap();
}

#[test]
fn cancellation_keeps_callback_errors_as_causes_and_panics_remain_structured() {
    let mut engine = Engine::default();
    engine
        .register_native("Fail and stop", |_, environment| {
            environment.control().cancel();
            Err(BWErr::NativeError("cleanup failed".into()))
        })
        .unwrap();
    let run = engine.run_source("cancel", "Fail and stop", RunOptions::default());
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    let error = run.result.unwrap_err();
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    assert_eq!(error.causes[0].call_stack[0].signature, "failandstop");
    engine
        .register_native("Panic", |_, _| panic!("native fixture"))
        .unwrap();
    let run = engine.run_source("panic", "Panic", RunOptions::default());
    assert_eq!(run.result.unwrap_err().code(), DiagnosticCode::NativePanic);
    assert!(engine
        .run_source("fresh", "", RunOptions::default())
        .result
        .is_ok());
}

#[test]
fn ordinary_errors_remain_catchable_in_configured_runs() -> DiagnosticResult<()> {
    let mut engine = Engine::default();
    engine.register_native("Reported cancellation", |_, _| {
        Err(BWErr::Cancelled("callback report".into()))
    })?;
    let run = engine.run_source(
        "catch",
        "Try { Reported cancellation } Catch |error| { |code| = |error.code| }",
        RunOptions::default(),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert_eq!(run.variables["code"].to_string(), "BW5001");
    Ok(())
}

#[test]
fn concurrent_engine_runs_have_independent_directories_inputs_and_environments() {
    let first = Harness::new();
    let second = Harness::new();
    let mut engine = Engine::default();
    engine
        .register_native("Environment", |_, environment| {
            Ok(Literal::String(
                environment
                    .get("VALUE")
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ))
        })
        .unwrap();
    let engine = Arc::new(engine);
    let workers: Vec<_> = [&first, &second].into_iter().enumerate().map(|(index, harness)| {
        let engine = Arc::clone(&engine);
        fs::write(harness.workspace.join("module.botwork"), format!("Read {{ Return |{index}| }}")).unwrap();
        let options = RunOptions {
            variables: BTreeMap::from([("input".into(), Literal::Int(index as i32))]),
            environment: BTreeMap::from([("VALUE".into(), Some(index.to_string().into()))]),
            ..options(&harness.workspace)
        };
        std::thread::spawn(move || engine.run_source("main.botwork", "Import |\"module.botwork\"| As |module|\n|result| = |[input, @{ module::Read }, @{ Environment }]|", options))
    }).collect();
    for (index, worker) in workers.into_iter().enumerate() {
        assert_eq!(
            worker.join().unwrap().result.unwrap().to_string(),
            format!("[{index}, {index}, \"{index}\"]")
        );
    }
}

#[tokio::test(start_paused = true)]
async fn relative_timeout_uses_the_same_monotonic_clock_as_operation_control() {
    tokio::time::advance(Duration::from_secs(3600)).await;
    let run = Engine::default().run_source(
        "clock",
        "",
        RunOptions {
            timeout: Some(Duration::from_secs(1)),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
}

#[test]
fn cancellation_inside_a_handler_restores_its_binding_and_retains_the_original_cause() {
    let mut engine = Engine::default();
    engine
        .register_native("Stop", |_, environment| {
            environment.control().cancel();
            Ok(Literal::None)
        })
        .unwrap();
    let run = engine.run_source(
        "handler",
        "|error| = |7|\nTry { Unknown } Catch |error| { Stop }",
        RunOptions::default(),
    );
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert_eq!(run.variables["error"].to_string(), "7");
    let error = run.result.unwrap_err();
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
    assert_eq!(error.call_stack[0].signature, "stop");
}

#[test]
fn recursive_imported_calls_share_call_depth_and_the_original_module_location() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "Read { Read }").unwrap();
    let run = Engine::default().run_source(
        "main.botwork",
        "Import |\"module.botwork\"| As |module|\nmodule::Read",
        RunOptions {
            limits: RunLimits {
                call_depth: 3,
                ..RunLimits::default()
            },
            ..options(&harness.workspace)
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    let error = run.result.unwrap_err();
    assert!(error
        .span
        .as_ref()
        .unwrap()
        .source()
        .name()
        .ends_with("module.botwork"));
    assert_eq!(error.call_stack.len(), 3);
    assert_eq!(error.call_stack.last().unwrap().signature, "module::read");
    assert_eq!(error.related.len(), 1);
}

#[cfg(unix)]
#[test]
fn environment_snapshot_preserves_non_utf8_os_strings_without_global_mutation() {
    use std::os::unix::ffi::OsStringExt;
    let key = OsString::from_vec(vec![b'k', 0xff]);
    let value = OsString::from_vec(vec![b'v', 0xfe]);
    let (expected_key, expected_value) = (key.clone(), value.clone());
    let mut engine = Engine::default();
    engine
        .register_native("Inspect", move |_, environment| {
            assert_eq!(
                environment.get(&expected_key),
                Some(expected_value.as_os_str())
            );
            Ok(Literal::None)
        })
        .unwrap();
    let run = engine.run_source(
        "os-strings",
        "Inspect",
        RunOptions {
            inherit_environment: false,
            environment: BTreeMap::from([(key, Some(value))]),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
}
