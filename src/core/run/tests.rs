use super::*;

#[test]
fn invalid_limit_configuration_uses_defaults_before_local_admission_or_effects() {
    type InvalidLimit = fn(&mut RunLimits);
    let cases: [(InvalidLimit, &str); 9] = [
        (
            |limits| limits.ast.depth = usize::MAX,
            "AST depth cannot exceed 128",
        ),
        (
            |limits| limits.values.depth = usize::MAX,
            "Value depth cannot exceed 64",
        ),
        (
            |limits| limits.diagnostic_values.values.depth = usize::MAX,
            "Value depth cannot exceed 64",
        ),
        (
            |limits| limits.diagnostics.depth = usize::MAX,
            "Diagnostic depth cannot exceed 64",
        ),
        (
            |limits| limits.imports.dependency_depth = usize::MAX,
            "Module dependency depth cannot exceed 32",
        ),
        (
            |limits| limits.import_depth = usize::MAX,
            "Import initialization depth cannot exceed 16",
        ),
        (
            |limits| limits.evaluation_depth = usize::MAX,
            "Evaluation depth cannot exceed 96",
        ),
        (
            |limits| limits.syntax.nesting = usize::MAX,
            "Syntax limits cannot exceed nesting 32 or operators 64",
        ),
        (
            |limits| limits.syntax.operators = usize::MAX,
            "Syntax limits cannot exceed nesting 32 or operators 64",
        ),
    ];
    for (invalid, expected) in cases {
        let mut limits = RunLimits::default();
        limits.diagnostics.text_bytes = 0;
        limits.diagnostics.source_bytes = 0;
        limits.diagnostics.diagnostics = 0;
        invalid(&mut limits);
        let context_error = Context::with_limits(limits.clone()).err().unwrap();
        let run = Engine::default().run_source(
            "invalid limits",
            "|effect| = |1|",
            RunOptions {
                limits,
                ..RunOptions::default()
            },
        );
        assert_eq!(run.steps, 0);
        assert!(run.variables.is_empty());
        let error = run.result.unwrap_err();
        for error in [context_error, error] {
            assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
            let BWErr::RunConfiguration(reason) = error.error.as_ref() else {
                panic!("configuration")
            };
            assert_eq!(reason, expected);
            assert!(error.span.is_none() && error.causes.is_empty() && error.omissions.is_none());
            DiagnosticLimits::default().check(&error).unwrap();
        }
    }
}

#[test]
fn setup_diagnostics_preserve_exact_default_text_boundaries_and_bounded_original_categories() {
    let maximum = DiagnosticLimits::default().text_bytes;
    let mut text = "x".repeat(maximum - "source".len());
    let error = RunEnvironment::configuration_error(format_args!("{text}"));
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    assert_eq!(
        DiagnosticLimits::default()
            .check(&error)
            .unwrap()
            .text_bytes,
        maximum
    );
    let BWErr::RunConfiguration(message) = error.error.as_ref() else {
        panic!("configuration")
    };
    assert_eq!(message.len(), text.len());
    assert!(error.span.is_none() && error.omissions.is_none());
    drop(error);
    text.push('x');
    let error = RunEnvironment::configuration_error(format_args!("{text}"));
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::RunConfiguration);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
    assert!(error.causes[0].span.is_none());
}

#[test]
fn setup_diagnostics_use_defaults_before_local_run_quotas_are_installed() {
    let options = RunOptions {
        inherit_environment: false,
        environment: BTreeMap::from([("bad=name".into(), Some("value".into()))]),
        limits: RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let error = RunEnvironment::prepare(&options, tokio::time::Instant::now())
        .err()
        .unwrap();
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    let BWErr::RunConfiguration(message) = error.error.as_ref() else {
        panic!("configuration")
    };
    assert_eq!(
        message,
        "Environment names must be nonempty and contain neither '=' nor NUL"
    );
    assert!(error.omissions.is_none());
}

#[test]
fn environment_names_ignore_case_only_where_the_platform_does() {
    let variables = BTreeMap::from([
        (OsString::from("Path"), OsString::from("bin")),
        (OsString::from("HOME"), OsString::from("home")),
    ]);
    let key = |name: &str, ignore_case| {
        environment_key(&variables, OsStr::new(name), ignore_case).and_then(|key| key.to_str())
    };
    assert_eq!(key("Path", false), Some("Path"));
    assert_eq!(key("PATH", false), None);
    assert_eq!(key("PATH", true), Some("Path"));
    assert_eq!(key("home", true), Some("HOME"));
    assert_eq!(key("PATHS", true), None);
    assert_eq!(NAMES_IGNORE_CASE, cfg!(windows));
}

#[test]
fn environment_overlays_replace_the_variable_their_name_names() {
    let host = BTreeMap::from([(OsString::from("Path"), OsString::from("bin"))]);
    let value = OsString::from("tools");
    // Where names ignore case, an overlay replaces the variable in its own spelling.
    let mut variables = host.clone();
    overlay(&mut variables, OsStr::new("PATH"), Some(&value), true);
    assert_eq!(
        variables,
        BTreeMap::from([(OsString::from("PATH"), value.clone())])
    );
    overlay(&mut variables, OsStr::new("path"), None, true);
    assert!(variables.is_empty());
    // Elsewhere differently cased names are different variables.
    let mut variables = host.clone();
    overlay(&mut variables, OsStr::new("PATH"), Some(&value), false);
    assert_eq!(variables.len(), 2);
    overlay(&mut variables, OsStr::new("path"), None, false);
    assert_eq!(variables.len(), 2);
    overlay(&mut variables, OsStr::new("Path"), None, false);
    assert_eq!(variables, BTreeMap::from([(OsString::from("PATH"), value)]));
}

/// A run waits as it ends, within its cleanup allowance, for the workers its
/// statements left unresolved, and fails naming them; a run that failed
/// already keeps its failure first, with theirs as a cause.
#[cfg(unix)]
#[test]
fn a_run_ends_failing_with_the_workers_it_left_unresolved() {
    use crate::core::{
        diagnostic::DiagnosticCode,
        operation::OperationControl,
        worker::{WorkerCommand, WorkerLimits, WorkerPool},
    };
    let pool = WorkerPool::new(WorkerLimits::default()).unwrap();
    let sleeper = || WorkerCommand {
        executable: "/bin/sleep".into(),
        arguments: vec!["30".into()],
        directory: std::env::temp_dir(),
        environment: Default::default(),
    };
    let options = || RunOptions {
        limits: RunLimits {
            cleanup: CleanupLimits {
                timeout: Duration::from_millis(150),
                ..CleanupLimits::default()
            },
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let engine = Engine::default();
    let failures = [
        None,
        Some(Diagnostic::new(BWErr::AssertionFailed(
            "the check failed".into(),
        ))),
    ];
    for failure in failures {
        let (active, prepared) =
            engine.prepare_run(asynchronous::PendingRun::new(options(), "main"), false);
        prepared.unwrap();
        let handle = pool
            .start(sleeper(), Vec::new(), OperationControl::default())
            .unwrap();
        let ledger = &active.context.environment.as_ref().unwrap().workers;
        ledger.record(&pool, handle.id());
        let before = Instant::now();
        let unsettled = active.settle_blocking();
        let waited = before.elapsed();
        assert!(waited >= Duration::from_millis(150), "{waited:?}");
        assert!(waited < Duration::from_secs(3), "{waited:?}");
        let failed = failure.is_some();
        let result = failure.map_or(Ok(Literal::None), |failure| {
            Err(RuntimeDiagnostic::constructed(failure, None))
        });
        let error = active.finish(result, unsettled).result.unwrap_err();
        let unresolved = if failed {
            assert_eq!(error.code(), DiagnosticCode::Assertion, "{error}");
            assert_eq!(error.causes.len(), 1, "{error}");
            &error.causes[0]
        } else {
            &error
        };
        assert_eq!(unresolved.code(), DiagnosticCode::AsyncRuntime, "{error}");
        let text = unresolved.error.to_string();
        assert!(
            text.contains(&format!(
                "The run ended with 1 worker process not cleaned up within its 150 ms cleanup allowance: worker {} (process ",
                handle.id()
            )),
            "{text}"
        );
        assert!(text.ends_with("): running"), "{text}");
        drop(handle);
    }
    // A run whose ledger is quiet ends at once, as it succeeded.
    let (active, prepared) =
        engine.prepare_run(asynchronous::PendingRun::new(options(), "main"), false);
    prepared.unwrap();
    let before = Instant::now();
    assert!(active.settle_blocking().is_none());
    assert!(before.elapsed() < Duration::from_millis(100));
    assert!(active.finish(Ok(Literal::None), None).result.is_ok());
}
