use super::*;

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
