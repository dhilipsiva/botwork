use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
};

fn run(source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(&Program::parse_detailed("native.botwork", source)?, context)
}

#[test]
fn registers_valid_headers_with_dsl_normalization_and_argument_positions() {
    let mut context = Context::default();
    context
        .register_native("Build |First| with |first|", |args| {
            Ok(Literal::Array(args.to_vec()))
        })
        .unwrap();
    let result = run(
        "|First| = |3|\n|first| = |4|\nb U I L D |First|\tWITH |first|",
        &mut context,
    )
    .unwrap();
    assert_eq!(result.to_string(), "[3, 4]");
    context
        .register_native("வணக்கம் |பெயர்|", |args| {
            Ok(args[0].clone())
        })
        .unwrap();
    assert_eq!(
        run("வணக்கம் |\"உலகம்\"|", &mut context).unwrap().to_string(),
        "உலகம்"
    );
    context
        .register_native("Pair |a| \\\n with |b|", |args| {
            Ok(Literal::Array(args.to_vec()))
        })
        .unwrap();
    assert_eq!(
        run("Pair |1| with |2|", &mut context).unwrap().to_string(),
        "[1, 2]"
    );
}

#[test]
fn registration_rejects_invalid_headers_without_running_callbacks() {
    for header in [
        "",
        " \t ",
        "If |x|",
        "Return",
        "Read |true|",
        "Read |1|",
        "Read |a + b|",
        "Read |a-b|",
        "Read |a",
        "Read {}",
        "Read\nOther",
        "Read {}\nOther {}",
    ] {
        let mut context = Context::default();
        let error = context
            .register_native(header, |_| panic!("registration invoked callback"))
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Syntax, "{header:?}: {error}");
        assert_eq!(error.span.unwrap().source().name(), "<native>");
        context
            .register_native("Read |value|", |_| Ok(Literal::Int(42)))
            .unwrap();
        assert!(matches!(
            run("Read |0|", &mut context),
            Ok(Literal::Int(42))
        ));
    }
}

#[test]
fn duplicate_parameters_retain_both_owned_source_locations() {
    let mut context = Context::default();
    let error = context
        .register_native("Pair |பேர்| with |பேர்|", |_| {
            Ok(Literal::None)
        })
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::DuplicateParameter);
    assert_eq!(error.span.as_ref().unwrap().text(), "பேர்");
    assert_eq!(error.related[0].span.text(), "பேர்");
    assert!(error.span.as_ref().unwrap().start() > error.related[0].span.start());
}

#[test]
fn collisions_preserve_native_and_dsl_registrations_in_both_directions() {
    let mut context = Context::default();
    context
        .register_native("Read |value|", |_| Ok(Literal::Int(1)))
        .unwrap();
    let error = context
        .register_native("r e a d |other|", |_| Ok(Literal::Int(2)))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::DuplicateStatement);
    assert_eq!(error.related[0].span.text(), "Read |value|");
    let error = run("READ |x| { Return |3| }", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::DuplicateStatement);
    assert_eq!(error.related[0].span.source().name(), "<native>");
    assert!(matches!(run("Read |0|", &mut context), Ok(Literal::Int(1))));
    run("Keep { Return |4| }", &mut context).unwrap();
    let error = context
        .register_native("KEEP", |_| Ok(Literal::Int(5)))
        .unwrap_err();
    assert_eq!(error.related[0].span.source().name(), "native.botwork");
    assert!(matches!(run("Keep", &mut context), Ok(Literal::Int(4))));
}

#[test]
fn initialization_is_idempotent_and_preserves_registered_log() {
    let mut context = Context::default();
    context
        .register_native("Log |input|", |_| Ok(Literal::Int(77)))
        .unwrap();
    context.init_statements();
    context.init_statements();
    assert!(matches!(run("Log |1|", &mut context), Ok(Literal::Int(77))));
    let mut builtin = Context::default();
    builtin.init_statements();
    let error = builtin
        .register_native("Log |input|", |_| Ok(Literal::None))
        .unwrap_err();
    assert!(error.to_string().contains("<builtin Log>"));
}

#[test]
fn callback_gets_every_argument_once_in_caller_order_after_success() {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let callback_record = Arc::clone(&recorded);
    let mut context = Context::default();
    context
        .register_native("Record |a| and |b| and |c|", move |args| {
            callback_record
                .lock()
                .unwrap()
                .push(Literal::Array(args.to_vec()).to_string());
            Ok(args[2].clone())
        })
        .unwrap();
    run("|a| = |1|\n|b| = |2|\n|c| = |3|", &mut context).unwrap();
    for (source, expected) in [
        (
            "Record |missing| and |1 / 0| and |c|",
            DiagnosticCode::UndefinedVariable,
        ),
        (
            "Record |a| and |1 / 0| and |missing|",
            DiagnosticCode::Arithmetic,
        ),
        (
            "Record |a| and |b| and |missing|",
            DiagnosticCode::UndefinedVariable,
        ),
        ("Record |missing|", DiagnosticCode::UndefinedStatement),
    ] {
        let error = run(source, &mut context).unwrap_err();
        assert_eq!(error.code(), expected);
        assert!(error.call_stack.is_empty());
        assert!(recorded.lock().unwrap().is_empty());
    }
    assert!(matches!(
        run("Record |a| and |b| and |c|", &mut context),
        Ok(Literal::Int(3))
    ));
    assert_eq!(*recorded.lock().unwrap(), ["[1, 2, 3]"]);
}

#[test]
fn every_finite_return_kind_can_be_assigned_and_passed_back_to_native_code() {
    let mut context = Context::default();
    context
        .register_native("Echo |value|", |args| Ok(args[0].clone()))
        .unwrap();
    for value in [
        Literal::None,
        Literal::Int(i32::MIN),
        Literal::Float(f32::MAX),
        Literal::Bool(false),
        Literal::String("தமிழ்\n\"".into()),
        Literal::Array(vec![Literal::None]),
        Literal::Map([("x".into(), Literal::Int(2))].into()),
    ] {
        let expected = value.to_string();
        let mut context = context.clone();
        context
            .register_native("Value", move |_| Ok(value.clone()))
            .unwrap();
        assert_eq!(
            run("|value| = Value\nEcho |value|", &mut context)
                .unwrap()
                .to_string(),
            expected
        );
    }
}

#[test]
fn non_finite_native_returns_are_rejected_before_replacing_a_binding() {
    for number in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for value in [
            Literal::Float(number),
            Literal::Array(vec![Literal::Float(number)]),
            Literal::Map([("x".into(), Literal::Array(vec![Literal::Float(number)]))].into()),
        ] {
            let mut context = Context::default();
            context
                .register_native("Invalid", move |_| Ok(value.clone()))
                .unwrap();
            let error = run("|answer| = |7|\n|answer| = Invalid", &mut context).unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::Arithmetic);
            assert_eq!(error.call_stack[0].signature, "invalid");
            assert!(matches!(
                run("|result| = |answer|", &mut context),
                Ok(Literal::Int(7))
            ));
        }
    }
}

#[test]
fn native_errors_keep_codes_callers_causes_and_metadata() {
    let mut context = Context::default();
    context
        .register_native("Fail", |_| Err(BWErr::NativeError("offline".into())))
        .unwrap();
    let error = run(
        "Outer { Try { |x| = |1 / 0| } Catch { Fail } }\nOuter",
        &mut context,
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Native);
    assert_eq!(error.span.as_ref().unwrap().text(), "Fail ");
    assert_eq!(
        error
            .call_stack
            .iter()
            .map(|frame| frame.signature.as_str())
            .collect::<Vec<_>>(),
        ["fail", "outer"]
    );
    assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
    let value = run("Try { Fail } Catch |error| { |seen| = |[error.code, error.details.reason]| }\n|result| = |seen|", &mut context).unwrap();
    assert_eq!(value.to_string(), "[\"BW4002\", \"offline\"]");
}

#[test]
fn callback_panics_are_catchable_and_restore_calls_loops_and_bindings() {
    let mut context = Context::default();
    context
        .register_native("Panic", |_| panic!("callback bug"))
        .unwrap();
    run(
        "|item| = |99|\n|error| = |88|\nOuter |item| { For |item| In |[1]| { Panic } }",
        &mut context,
    )
    .unwrap();
    let error = run("Outer |7|", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::NativePanic);
    assert_eq!(error.call_stack.len(), 2);
    let result = run("Try { For |item| In |[1]| { Panic } } Catch |error| { |seen| = |error.code| }\n|result| = |[item, error, seen]|", &mut context).unwrap();
    assert_eq!(result.to_string(), "[99, 88, \"BW4003\"]");
    assert!(run("Unknown", &mut context)
        .unwrap_err()
        .call_stack
        .is_empty());
}

#[test]
fn context_clones_share_explicit_callback_state_and_isolate_dsl_bindings() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut original = Context::default();
    original
        .register_native("Next", move |_| {
            Ok(Literal::Int(counter.fetch_add(1, Ordering::SeqCst) as i32))
        })
        .unwrap();
    run("|x| = |10|", &mut original).unwrap();
    let mut cloned = original.clone();
    assert!(matches!(
        run("|x| = Next", &mut cloned),
        Ok(Literal::Int(0))
    ));
    assert!(matches!(run("Next", &mut original), Ok(Literal::Int(1))));
    assert!(matches!(
        run("|result| = |x|", &mut original),
        Ok(Literal::Int(10))
    ));
}

#[test]
fn lexical_dsl_shadowing_of_a_native_does_not_replace_the_parent() {
    let mut context = Context::default();
    context
        .register_native("Value", |_| Ok(Literal::Int(1)))
        .unwrap();
    assert!(matches!(
        run(
            "Outer { Value { Return |2| }\n |value| = Value\n Return |value| }\nOuter",
            &mut context
        ),
        Ok(Literal::Int(2))
    ));
    assert!(matches!(run("Value", &mut context), Ok(Literal::Int(1))));
}
