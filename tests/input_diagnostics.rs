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
