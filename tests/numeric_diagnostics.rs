use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticLimits},
    eval::{botwork_detailed, evaluate_program_detailed, Context},
    grammar::{BWErr, BWParser, Literal, Rule},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
};
use pest::Parser;
use std::sync::{Arc, Mutex};

fn options(diagnostics: DiagnosticLimits) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            diagnostics,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

#[test]
fn numeric_failures_admit_exact_messages_and_complete_expression_call_context() {
    for (expression, expected) in [
        (
            "2147483648",
            BWErr::ParsingIntegerError("2147483648".parse::<i32>().unwrap_err().to_string()),
        ),
        (
            "-2147483649",
            BWErr::ParsingIntegerError("-2147483649".parse::<i32>().unwrap_err().to_string()),
        ),
        (
            "999999999999999999999999999999999999999.0",
            BWErr::ArithmeticError("Float literal produced a non-finite result".into()),
        ),
        (
            "2147483647 + 1",
            BWErr::ArithmeticError("Addition exceeds the i32 range".into()),
        ),
        (
            "-2147483648 - 1",
            BWErr::ArithmeticError("Subtraction exceeds the i32 range".into()),
        ),
        (
            "2147483647 * 2",
            BWErr::ArithmeticError("Multiplication exceeds the i32 range".into()),
        ),
        (
            "-(-2147483648)",
            BWErr::ArithmeticError("Negation exceeds the i32 range".into()),
        ),
        (
            "2 ^ 31",
            BWErr::ArithmeticError("Exponentiation exceeds the i32 range".into()),
        ),
        (
            "0 ^ -1",
            BWErr::ArithmeticError("Zero cannot have a negative exponent".into()),
        ),
        ("1 / 0", BWErr::ArithmeticError("divide by zero".into())),
        ("1 % 0.0", BWErr::ArithmeticError("modulus by zero".into())),
        (
            "340282300000000000000000000000000000000.0 * 2",
            BWErr::ArithmeticError("Multiplication produced a non-finite result".into()),
        ),
    ] {
        let source = format!("Compute {{ Return |{expression}| }}\nCompute");
        let engine = Engine::default();
        let baseline = engine
            .run_source("numeric-é", &source, RunOptions::default())
            .result
            .unwrap_err();
        assert_eq!(
            baseline.error.to_string(),
            expected.to_string(),
            "{expression}"
        );
        assert_eq!(baseline.label, "expression");
        assert_eq!(baseline.call_stack.len(), 1);
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        for dimension in 0..6 {
            let mut limits = DiagnosticLimits {
                diagnostics: size.diagnostics,
                depth: size.depth,
                call_frames: size.call_frames,
                related_locations: size.related_locations,
                text_bytes: size.text_bytes,
                source_bytes: size.source_bytes,
            };
            match dimension {
                0 => (),
                1 => limits.text_bytes -= 1,
                2 => limits.source_bytes -= 1,
                3 => limits.call_frames -= 1,
                4 => limits.diagnostics = 0,
                _ => limits.depth = 0,
            }
            let error = engine
                .run_source("numeric-é", &source, options(limits))
                .result
                .unwrap_err();
            if dimension == 0 {
                assert_eq!(
                    error.to_value().to_string(),
                    baseline.to_value().to_string()
                );
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                let original = &error.causes[0];
                assert_eq!(original.code(), baseline.code());
                assert_eq!(original.error.to_string(), expected.to_string());
                assert!(original.span.is_none());
                let omitted = original.omissions.as_ref().unwrap();
                assert_eq!(omitted.call_frames, 1);
                assert_eq!(omitted.detail_fields, 0);
                assert!(!omitted.prior_summary);
                assert_eq!(
                    omitted.source.as_ref().unwrap().start_byte,
                    baseline.span.as_ref().unwrap().start()
                );
            }
        }
    }
}

#[test]
fn arithmetic_admission_preserves_operand_effects_catch_bindings_and_stop_priority() {
    for reject in [false, true] {
        for cancel in [false, true] {
            let events = Arc::new(Mutex::new(vec![]));
            let seen = events.clone();
            let mut engine = Engine::default();
            engine
                .register_native("Left", move |_, _| {
                    seen.lock().unwrap().push("left");
                    Ok(Literal::Int(i32::MAX))
                })
                .unwrap();
            let seen = events.clone();
            engine
                .register_native("Right", move |_, environment| {
                    seen.lock().unwrap().push("right");
                    if cancel {
                        environment.control().cancel();
                    }
                    Ok(Literal::Int(1))
                })
                .unwrap();
            let run = engine.run_source(
                "effects", "|error| = |7|\n|before| = |1|\nTry { |value| = |(@{Left} + @{Right}) + @{Left}| } Catch |error| { |code| = |error.code| }\n|after| = |1|",
                options(DiagnosticLimits { text_bytes: if reject { 0 } else { DiagnosticLimits::default().text_bytes }, ..DiagnosticLimits::default() }),
            );
            assert_eq!(*events.lock().unwrap(), ["left", "right"]);
            assert_eq!(run.variables["error"].to_string(), "7");
            assert_eq!(run.variables["before"].to_string(), "1");
            assert!(!run.variables.contains_key("value"));
            if cancel {
                assert_eq!(run.outcome(), RunOutcome::Cancelled);
                assert_eq!(run.result.unwrap_err().code(), DiagnosticCode::Cancelled);
                assert!(
                    !run.variables.contains_key("after") && !run.variables.contains_key("code")
                );
            } else if reject {
                assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
                assert_eq!(
                    run.result.unwrap_err().causes[0].code(),
                    DiagnosticCode::Arithmetic
                );
                assert!(
                    !run.variables.contains_key("after") && !run.variables.contains_key("code")
                );
            } else {
                assert_eq!(run.outcome(), RunOutcome::Succeeded);
                assert_eq!(run.variables["code"].to_string(), "BW3002");
                assert_eq!(run.variables["after"].to_string(), "1");
            }
        }
    }
}

#[test]
fn pair_numeric_failures_latch_only_the_requester_and_valid_numeric_paths_need_no_error_budget() {
    let mut context = Context::with_limits(
        options(DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        })
        .limits,
    )
    .unwrap();
    let mut sibling = context.clone();
    let pair = BWParser::parse(Rule::botwork, "|value| = |2 ^ 31|")
        .unwrap()
        .next()
        .unwrap();
    let error = botwork_detailed(pair, &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
    assert!(context.checkpoint().is_err());
    let valid = Program::parse("valid", "|min| = |-2147483648|\n|rounded| = |16777217 + 0.0|\n|underflow| = |2 ^ -150|\n|skip| = |false and (1 / 0)|\n|result| = |[min, rounded, underflow, skip]|").unwrap();
    assert_eq!(
        evaluate_program_detailed(&valid, &mut sibling)
            .unwrap()
            .to_string(),
        "[-2147483648, 16777216, 0, false]"
    );
}
