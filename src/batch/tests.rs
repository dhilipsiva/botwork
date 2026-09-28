use super::*;

#[tokio::test]
async fn invalid_admission_limits_fail_before_preparing_inputs_or_paths() {
    for jobs in [0, 65, usize::MAX] {
        let error = run(
            vec![PathBuf::from("unreachable.botwork")],
            jobs,
            Configuration {
                debug: false,
                files: vec![],
                settings: vec!["malformed input".into()],
                limits: RunLimits::default(),
                timeout_ms: None,
                suite_timeout_ms: None,
            },
        )
        .await
        .unwrap_err();
        let CliError::Script(error) = error else {
            panic!("structured configuration failure")
        };
        assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
        assert!(error.to_string().contains("Parallel jobs"));
    }
}
