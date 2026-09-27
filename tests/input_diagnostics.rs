use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticLimits},
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
    input::{parse_variable, parse_variables},
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn limits(text_bytes: usize) -> RunLimits {
    RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    }
}

#[test]
fn input_file_source_limits_admit_exact_displayed_origin_bytes_before_opening() {
    use botwork::core::input::{load_variables_with_limits, InputLimits};
    use std::path::PathBuf;
    let maximum = DiagnosticLimits::default().source_bytes;
    let mut filename = "é".repeat(maximum / 2);
    for rejected in [false, true] {
        if rejected {
            filename.push('x');
        }
        let file = PathBuf::from(&filename);
        let error = load_variables_with_limits(
            &[file],
            &["invalid setting".into()],
            &InputLimits {
                sources: 0,
                ..InputLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        let original = if rejected {
            assert!(matches!(
                error.error.as_ref(),
                BWErr::ResourceLimit {
                    resource: "diagnostic source bytes",
                    ..
                }
            ));
            let original = &error.causes[0];
            let evidence = original
                .omissions
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap();
            assert!(evidence.file_truncated && evidence.file.starts_with('é'));
            assert_eq!((evidence.start_byte, evidence.end_byte), (0, 0));
            assert!(original.span.is_none());
            original
        } else {
            assert_eq!(error.span.as_ref().unwrap().source().name(), filename);
            assert!(error.span.as_ref().unwrap().source().text().is_empty());
            assert!(error.causes.is_empty());
            &error
        };
        assert!(matches!(
            original.error.as_ref(),
            BWErr::ResourceLimit {
                resource: "input sources",
                limit: 0
            }
        ));
    }
}

#[cfg(unix)]
#[test]
fn file_failures_preserve_lossy_native_origins_and_precede_settings() {
    use botwork::core::input::{load_variables_with_limits, InputLimits};
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};
    let path = PathBuf::from(OsString::from_vec(vec![b'x', 0xff, 0, b'.', b'j']));
    let expected = format!(
        "{}: $: {}",
        path.display(),
        std::fs::File::open(&path).unwrap_err()
    );
    let error = load_variables_with_limits(
        std::slice::from_ref(&path),
        &["invalid setting".into()],
        &InputLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(error.error.as_ref(), BWErr::InputError(message) if message == &expected));
    let error = load_variables_with_limits(
        std::slice::from_ref(&path),
        &[],
        &InputLimits {
            sources: 0,
            ..InputLimits::default()
        },
    )
    .unwrap_err();
    assert!(matches!(
        error.error.as_ref(),
        BWErr::ResourceLimit {
            resource: "input sources",
            limit: 0
        }
    ));
    assert_eq!(
        error.span.as_ref().unwrap().source().name(),
        path.display().to_string()
    );
}

fn invalid_input(invalid_name: bool) -> (String, Literal, String) {
    if invalid_name {
        let name = "é invalid\n";
        (
            name.into(),
            Literal::None,
            format!("host variables: variable {name:?}: expected an exact DSL identifier"),
        )
    } else {
        let name = "é";
        (
            name.into(),
            Literal::Array(vec![Literal::Float(f32::INFINITY)]),
            format!("host variables: {name:?}: values must contain only finite floats"),
        )
    }
}

#[test]
fn host_input_error_text_has_exact_admission_and_ordinary_errors_allow_retry() {
    for invalid_name in [false, true] {
        let (name, value, message) = invalid_input(invalid_name);
        let bytes = "source".len() + message.len();
        let mut context = Context::with_limits(limits(bytes)).unwrap();
        let error = context
            .set_input_variables(BTreeMap::from([(name.clone(), value.clone())]))
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Input);
        let BWErr::InputError(detail) = error.error.as_ref() else {
            panic!("input category")
        };
        assert_eq!(detail, &message);
        assert!(
            error.span.is_none()
                && error.call_stack.is_empty()
                && error.causes.is_empty()
                && error.omissions.is_none()
        );
        assert_eq!(
            DiagnosticLimits::default()
                .check(&error)
                .unwrap()
                .text_bytes,
            bytes
        );
        context
            .set_input_variables(BTreeMap::from([("good".into(), Literal::Int(7))]))
            .unwrap();
        let program = Program::parse("retry", "|out| = |good|").unwrap();
        assert_eq!(
            evaluate_program_detailed(&program, &mut context)
                .unwrap()
                .to_string(),
            "7"
        );

        let mut rejected = Context::with_limits(limits(bytes - 1)).unwrap();
        let mut sibling = rejected.clone();
        let error = rejected
            .set_input_variables(BTreeMap::from([(name, value)]))
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::Input);
        assert!(error.causes[0].omissions.is_some());
        assert!(error.causes[0].span.is_none());
        assert!(rejected.set_input_variables(BTreeMap::new()).is_err());
        sibling
            .set_input_variables(BTreeMap::from([("good".into(), Literal::Int(7))]))
            .unwrap();
        evaluate_program_detailed(&program, &mut sibling).unwrap();
    }
}

#[test]
fn invalid_input_rejects_the_whole_engine_batch_before_effects_and_fresh_runs_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Effect", move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    for invalid_name in [false, true] {
        for text_bytes in [0, DiagnosticLimits::default().text_bytes] {
            let (name, value, _) = invalid_input(invalid_name);
            let run = engine.run_source(
                "input",
                "Effect",
                RunOptions {
                    variables: BTreeMap::from([("a".into(), Literal::Int(1)), (name, value)]),
                    limits: limits(text_bytes),
                    ..RunOptions::default()
                },
            );
            assert!(run.variables.is_empty());
            assert!(run.snapshot_error.is_none());
            assert_eq!(run.steps, 0);
            let error = run.result.unwrap_err();
            assert_eq!(
                error.code(),
                if text_bytes == 0 {
                    DiagnosticCode::ResourceLimit
                } else {
                    DiagnosticCode::Input
                }
            );
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let run = engine.run_source(
        "valid",
        "Effect",
        RunOptions {
            limits: limits(0),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn ordinary_invalid_name_preserves_existing_bindings_and_skips_earlier_valid_changes() {
    let mut context = Context::default();
    context
        .set_input_variables(BTreeMap::from([("a".into(), Literal::Int(1))]))
        .unwrap();
    let error = context
        .set_input_variables(BTreeMap::from([
            ("a".into(), Literal::Int(2)),
            ("b".into(), Literal::Int(3)),
            ("z invalid".into(), Literal::None),
        ]))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Input);
    let program = Program::parse("read", "|out| = |a|").unwrap();
    assert_eq!(
        evaluate_program_detailed(&program, &mut context)
            .unwrap()
            .to_string(),
        "1"
    );
    let absent = Program::parse("absent", "|out| = |b|").unwrap();
    assert_eq!(
        evaluate_program_detailed(&absent, &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::UndefinedVariable
    );
}

#[test]
fn name_and_value_limits_keep_priority_and_deep_invalid_inputs_drop_iteratively() {
    let mut configuration = limits(0);
    configuration.retained_names.name_bytes = 2;
    let mut context = Context::with_limits(configuration).unwrap();
    let error = context
        .set_input_variables(BTreeMap::from([("bad name".into(), Literal::None)]))
        .unwrap_err();
    assert!(error.to_string().contains("name bytes"));
    assert!(error.causes.is_empty());

    let mut configuration = limits(0);
    configuration.values.nodes = 0;
    let mut context = Context::with_limits(configuration).unwrap();
    let error = context
        .set_input_variables(BTreeMap::from([(
            "value".into(),
            Literal::Float(f32::INFINITY),
        )]))
        .unwrap_err();
    assert!(error.to_string().contains("value nodes"));
    assert!(error.causes.is_empty());

    for text_bytes in [0, DiagnosticLimits::default().text_bytes] {
        let deep = (0..20_000).fold(Literal::None, |value, _| Literal::Array(vec![value]));
        let mut context = Context::with_limits(limits(text_bytes)).unwrap();
        let error = context
            .set_input_variables(BTreeMap::from([("invalid name".into(), deep)]))
            .unwrap_err();
        assert_eq!(
            error.code(),
            if text_bytes == 0 {
                DiagnosticCode::ResourceLimit
            } else {
                DiagnosticCode::Input
            }
        );
        if text_bytes == 0 {
            assert_eq!(error.causes[0].code(), DiagnosticCode::Input);
        }
    }
}

#[test]
fn prior_cancellation_wins_before_name_validation_and_deep_input_cleanup() {
    let control = OperationControl::default();
    let mut context = Context::with_control(limits(0), control.clone()).unwrap();
    let deep = (0..20_000).fold(Literal::None, |value, _| Literal::Array(vec![value]));
    control.cancel();
    let error = context
        .set_input_variables(BTreeMap::from([("invalid name".into(), deep)]))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert!(error.causes.is_empty());
}

#[test]
fn host_json_and_flags_share_exact_unicode_and_reserved_word_rules() {
    for name in [
        "_",
        "True",
        "FALSE",
        "And",
        "OR",
        "Return",
        "Import",
        "true1",
        "and_then",
        "é",
        "e\u{301}",
        "தமிழ்",
        "变量",
    ] {
        let mut context = Context::with_limits(limits(0)).unwrap();
        context
            .set_input_variables(BTreeMap::from([(name.into(), Literal::Int(7))]))
            .unwrap();
        let json = serde_json::to_string(&BTreeMap::from([(name, 7)])).unwrap();
        assert!(parse_variables("json", &json).unwrap().contains_key(name));
        assert_eq!(
            parse_variable("flag", &format!("{name}=7")).unwrap().0,
            name
        );
        let program = Program::parse("read", &format!("|out| = |{name}|")).unwrap();
        assert_eq!(
            evaluate_program_detailed(&program, &mut context)
                .unwrap()
                .to_string(),
            "7"
        );
    }
    for name in [
        "", "true", "false", "and", "or", " x", "x ", "x\n", "a.b", "a[0]", "\u{301}x", "🦀",
    ] {
        let mut context = Context::default();
        assert_eq!(
            context
                .set_input_variables(BTreeMap::from([(name.into(), Literal::None)]))
                .unwrap_err()
                .code(),
            DiagnosticCode::Input
        );
        let json = serde_json::to_string(&BTreeMap::from([(name, 7)])).unwrap();
        assert_eq!(
            parse_variables("json", &json).unwrap_err().code(),
            DiagnosticCode::Input
        );
        assert_eq!(
            parse_variable("flag", &format!("{name}=7"))
                .unwrap_err()
                .code(),
            DiagnosticCode::Input
        );
    }
}

#[test]
fn input_helper_failures_preserve_messages_and_bound_oversized_origins() {
    let origin = "é".repeat(DiagnosticLimits::default().text_bytes / 2);
    let too_deep = format!("x={}0{}", "[".repeat(129), "]".repeat(129));
    for (json, text, reason) in [
        (false, "missing", "expected NAME=JSON"),
        (false, "x=", "EOF while parsing a value"),
        (
            false,
            "x=2147483648",
            "integer is outside -2147483648..2147483647",
        ),
        (false, "x=1e9999", "decimal exceeds the finite f32 range"),
        (
            false,
            too_deep.as_str(),
            "JSON exceeds 128 nested containers",
        ),
        (
            true,
            "[]",
            "expected a JSON object of variable names and values",
        ),
        (true, r#"{"x":"#, "EOF while parsing"),
        (true, r#"{"x":"\uD800"}"#, "unexpected end of hex escape"),
        (
            true,
            r#"{"x":[{"key":2147483648}]}"#,
            "$[\"x\"][0][\"key\"]",
        ),
    ] {
        let parse = |origin| {
            if json {
                parse_variables(origin, text).map(|_| ())
            } else {
                parse_variable(origin, text).map(|_| ())
            }
        };
        let ordinary = parse("config").unwrap_err();
        assert_eq!(ordinary.code(), DiagnosticCode::Input, "{text}");
        assert!(ordinary.to_string().contains(reason), "{ordinary}");
        assert!(ordinary.span.is_none() && ordinary.omissions.is_none());
        let rejected = parse(&origin).unwrap_err();
        assert_eq!(rejected.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(rejected.causes[0].code(), DiagnosticCode::Input);
        assert_eq!(
            rejected.causes[0].omissions.as_ref().unwrap().detail_fields,
            1
        );
        assert!(rejected.causes[0].span.is_none());
    }
    assert_eq!(parse_variable("fresh", "x=7").unwrap().1.to_string(), "7");
}

#[test]
fn standalone_input_helper_messages_accept_exact_default_text_bytes_and_reject_one_more() {
    let limits = DiagnosticLimits::default();
    let suffix = ": $: expected NAME=JSON";
    let mut origin = "x".repeat(limits.text_bytes - "source".len() - suffix.len());
    let error = parse_variable(&origin, "missing").unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Input);
    assert_eq!(limits.check(&error).unwrap().text_bytes, limits.text_bytes);
    let BWErr::InputError(detail) = error.error.as_ref() else {
        panic!("input")
    };
    assert!(detail.starts_with(&origin) && detail.ends_with(suffix));
    drop(error);
    origin.push('x');
    let error = parse_variable(&origin, "missing").unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Input);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
}

#[test]
fn input_resource_origin_has_exact_source_budget_and_retains_original_limit_on_rejection() {
    use botwork::core::input::{parse_variables_with_limits, InputLimits};
    let diagnostics = DiagnosticLimits::default();
    let input = InputLimits {
        source_bytes: 0,
        ..InputLimits::default()
    };
    let mut origin = "x".repeat(diagnostics.source_bytes);
    let error = parse_variables_with_limits(&origin, "{}", &input).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(matches!(
        error.error.as_ref(),
        BWErr::ResourceLimit {
            resource: "input source bytes",
            limit: 0
        }
    ));
    assert!(error.causes.is_empty());
    assert_eq!(
        diagnostics.check(&error).unwrap().source_bytes,
        diagnostics.source_bytes
    );
    let span = error.span.as_ref().unwrap();
    assert_eq!(span.source().name(), origin);
    assert_eq!(span.source().text(), "");
    assert_eq!(span.line_column(), (1, 1));
    drop(error);
    origin.push('x');
    let error = parse_variables_with_limits(&origin, "{}", &input).unwrap_err();
    assert!(matches!(
        error.error.as_ref(),
        BWErr::ResourceLimit {
            resource: "diagnostic source bytes",
            ..
        }
    ));
    assert!(matches!(
        error.causes[0].error.as_ref(),
        BWErr::ResourceLimit {
            resource: "input source bytes",
            limit: 0
        }
    ));
    assert!(error.span.is_none() && error.causes[0].span.is_none());
    let evidence = error.causes[0]
        .omissions
        .as_ref()
        .unwrap()
        .source
        .as_ref()
        .unwrap();
    assert!(evidence.file_truncated && evidence.file.len() <= 256);
    assert_eq!((evidence.start_byte, evidence.end_byte), (0, 0));
    assert_eq!(diagnostics.check(&error).unwrap().source_bytes, 0);
}

#[test]
fn input_limits_precede_later_conversion_failures_even_when_origin_is_rejected() {
    use botwork::core::input::{parse_variables_with_limits, InputLimits};
    let origin = "é".repeat(DiagnosticLimits::default().source_bytes / 2 + 1);
    for (limits, expected) in [
        (
            InputLimits {
                sources: 0,
                ..InputLimits::default()
            },
            "input sources",
        ),
        (
            InputLimits {
                source_bytes: 0,
                ..InputLimits::default()
            },
            "input source bytes",
        ),
        (
            InputLimits {
                total_bytes: 0,
                ..InputLimits::default()
            },
            "total input bytes",
        ),
        (
            InputLimits {
                raw_nodes: 0,
                ..InputLimits::default()
            },
            "input raw nodes",
        ),
        (
            InputLimits {
                variables: 0,
                ..InputLimits::default()
            },
            "input variables",
        ),
    ] {
        let error =
            parse_variables_with_limits(&origin, r#"{"x":2147483648}"#, &limits).unwrap_err();
        assert!(matches!(
            error.error.as_ref(),
            BWErr::ResourceLimit {
                resource: "diagnostic source bytes",
                ..
            }
        ));
        assert!(
            matches!(error.causes[0].error.as_ref(), BWErr::ResourceLimit { resource, limit: 0 } if *resource == expected)
        );
        assert!(
            error.causes[0]
                .omissions
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap()
                .file_truncated
        );
    }
}
