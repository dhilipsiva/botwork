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
