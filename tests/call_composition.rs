use botwork::core::{
    ast::{AssignmentValue, ExprKind, Program, StatementKind},
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{botwork_detailed, evaluate_program_detailed, Context},
    grammar::{BWParser, Literal, Rule},
    signature::{StatementSignature, ValueKind},
};
use pest::Parser;
use std::sync::{Arc, Mutex};

fn run(source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(
        &Program::parse_detailed("composition.botwork", source)?,
        context,
    )
}

fn traced() -> (Context, Arc<Mutex<Vec<String>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let trace = Arc::clone(&events);
    let mut context = Context::default();
    context
        .register_native("Trace |name| returning |value|", move |args| {
            trace.lock().unwrap().push(args[0].to_string());
            Ok(args[1].clone())
        })
        .unwrap();
    (context, events)
}

#[test]
fn calls_compose_with_operators_grouping_and_return_precedence() {
    let mut context = Context::default();
    run("Value |x| { Return |x| }", &mut context).unwrap();
    for (source, expected) in [
        ("|answer| = |1 + @{Value |2|} * @{Value |3|}|", "7"),
        ("|answer| = |-@{Value |2|} ^ 2|", "-4"),
        ("|answer| = |@{Value |2|} ^ @{Value |3|} ^ 2|", "512"),
        ("|answer| = |(@{Value |1|} + @{Value |2|}) * 3|", "9"),
        ("|answer| = |!@{Value |false|} and @{Value |true|}|", "true"),
        ("|answer| = |@{Value |\"a\"|} + @{Value |\"b\"|}|", "ab"),
    ] {
        assert_eq!(
            run(source, &mut context).unwrap().to_string(),
            expected,
            "{source}"
        );
    }
}

#[test]
fn nested_calls_bind_in_caller_scope_and_restore_every_invocation() {
    let mut context = Context::default();
    let value = run("|x| = |99|\nIdentity |x| { Return |x| }\nPair |x| with |y| { Return |[x, y]| }\n|answer| = |@{Pair |1| with |@{Identity |x|}|}|\n|result| = |[answer, x]|", &mut context).unwrap();
    assert_eq!(value.to_string(), "[[1, 99], 99]");
    assert!(run("|answer| = |y|", &mut context).is_err());
}

#[test]
fn effectful_arguments_finish_left_to_right_before_the_callee_body() {
    let (mut context, events) = traced();
    let value = run("Pair |a| with |b| { Trace |\"body\"| returning |0|\nReturn |[a, b]| }\n|answer| = |@{Pair |@{Trace |\"first\"| returning |1|}| with |@{Trace |\"second\"| returning |2|}|}|", &mut context).unwrap();
    assert_eq!(value.to_string(), "[1, 2]");
    assert_eq!(*events.lock().unwrap(), ["first", "second", "body"]);
}

#[test]
fn unresolved_outer_calls_skip_all_nested_argument_effects() {
    let (mut context, events) = traced();
    let error = run(
        "|answer| = |@{Unknown |@{Trace |\"unreachable\"| returning |1|}|}|",
        &mut context,
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedStatement);
    assert!(error.call_stack.is_empty());
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn failing_arguments_keep_completed_effects_and_skip_later_arguments_and_body() {
    let (mut context, events) = traced();
    let error = run("Pair |a| with |b| { Trace |\"body\"| returning |0| }\n|answer| = |7|\n|answer| = |@{Pair |@{Trace |\"first\"| returning |1|} + missing| with |@{Trace |\"second\"| returning |2|}|}|", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert!(error.call_stack.is_empty());
    assert_eq!(*events.lock().unwrap(), ["first"]);
    assert!(matches!(
        run("|result| = |answer|", &mut context),
        Ok(Literal::Int(7))
    ));
}

#[test]
fn declared_parameter_kinds_skip_later_call_effects_before_native_entry() {
    let (mut context, events) = traced();
    let signature = StatementSignature::native("Require |first| then |second|")
        .unwrap()
        .parameter("first", ValueKind::String)
        .unwrap();
    context
        .register_native_with_signature(signature, |_| panic!("invalid args entered native"))
        .unwrap();
    let error = run(
        "|result| = |@{Require |1| then |@{Trace |\"later\"| returning |2|}|}|",
        &mut context,
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::IncompatibleType);
    assert!(events.lock().unwrap().is_empty());
    assert!(error.call_stack.is_empty());
}

#[test]
fn boolean_short_circuiting_skips_unselected_call_resolution_and_effects() {
    let (mut context, events) = traced();
    let value = run("|result| = |[false and @{Unknown}, true or @{Unknown}, true and @{Trace |\"and\"| returning |true|}, false or @{Trace |\"or\"| returning |false|}]|", &mut context).unwrap();
    assert_eq!(value.to_string(), "[false, true, true, false]");
    assert_eq!(*events.lock().unwrap(), ["and", "or"]);
    let error = run(
        "|result| = |1 and @{Trace |\"skipped\"| returning |true|}|",
        &mut context,
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::IncompatibleType);
    assert_eq!(*events.lock().unwrap(), ["and", "or"]);
}

#[test]
fn arrays_maps_and_duplicate_keys_preserve_call_effect_order() {
    let (mut context, events) = traced();
    let value = run("|result| = |[@{Trace |\"array\"| returning |1|}, {a: @{Trace |\"first-key\"| returning |2|}, a: @{Trace |\"last-key\"| returning |3|}}]|", &mut context).unwrap();
    assert_eq!(value.to_string(), "[1, {\"a\": 3}]");
    assert_eq!(*events.lock().unwrap(), ["array", "first-key", "last-key"]);
}

#[test]
fn collection_bases_and_computed_keys_run_once_in_order_and_stop_at_failure() {
    let (mut context, events) = traced();
    let value = run("|result| = |@{Trace |\"base\"| returning |{items: [4]}|}[@{Trace |\"key\"| returning |\"items\"|}][@{Trace |\"index\"| returning |0|}]|", &mut context).unwrap();
    assert!(matches!(value, Literal::Int(4)));
    assert_eq!(*events.lock().unwrap(), ["base", "key", "index"]);
    events.lock().unwrap().clear();
    let error = run("|result| = |@{Trace |\"base\"| returning |{}|}[@{Trace |\"missing\"| returning |\"missing\"|}][@{Trace |\"later\"| returning |0|}]|", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::CollectionAccess);
    assert_eq!(*events.lock().unwrap(), ["base", "missing"]);
}

#[test]
fn control_conditions_return_operands_and_loop_iterables_accept_calls() {
    let (mut context, events) = traced();
    let value = run("Outer {\nFor |item| In |@{Trace |\"iterable\"| returning |[1, 2]|}| {\nIf |@{Trace |\"condition\"| returning |item == 2|}| { Return |@{Trace |\"return\"| returning |item * 3|}| }\n}\n}\n|result| = |@{Outer}|", &mut context).unwrap();
    assert!(matches!(value, Literal::Int(6)));
    assert_eq!(
        *events.lock().unwrap(),
        ["iterable", "condition", "condition", "return"]
    );
    assert!(run("|result| = |item|", &mut context).is_err());
}

#[test]
fn while_rechecks_effectful_conditions_and_preserves_the_final_false_effect() {
    let (mut context, events) = traced();
    let value = run(
        "|i| = |0|\nWhile |@{Trace |i| returning |i < 3|}| { |i| = |i + 1| }\n|result| = |i|",
        &mut context,
    )
    .unwrap();
    assert!(matches!(value, Literal::Int(3)));
    assert_eq!(*events.lock().unwrap(), ["0", "1", "2", "3"]);
}

#[test]
fn recursive_expression_calls_preserve_results_and_diagnostic_frames() {
    let mut context = Context::default();
    run(
        "Factorial |n| { If |n == 0| { Return |1| }\nReturn |n * @{Factorial |n - 1|}| }",
        &mut context,
    )
    .unwrap();
    assert!(matches!(
        run("|result| = |@{Factorial |5|}|", &mut context),
        Ok(Literal::Int(120))
    ));
    let error = run("Fail |n| { If |n == 0| { Return |missing| }\nReturn |@{Fail |n - 1|}| }\n|result| = |@{Fail |3|}|", &mut context).unwrap_err();
    assert_eq!(error.call_stack.len(), 4);
    assert_eq!(error.span.as_ref().unwrap().text(), "missing");
    assert!(run("|result| = |@{Unknown}|", &mut context)
        .unwrap_err()
        .call_stack
        .is_empty());
}

#[test]
fn inline_failures_can_be_inspected_and_rethrown_without_self_causes() {
    let mut context = Context::default();
    let error = run("Fail { Return |1 / 0| }\nTry { |result| = |1 + @{Fail}| } Catch |error| { |code| = |error.code|\nRethrow }", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Arithmetic);
    assert_eq!(error.call_stack.len(), 1);
    assert_eq!(error.call_stack[0].signature, "fail");
    assert!(error.causes.is_empty());
    assert_eq!(
        run("|result| = |code|", &mut context).unwrap().to_string(),
        "BW3002"
    );
}

#[test]
fn no_result_calls_remain_none_and_do_not_leak_pending_returns() {
    let mut context = Context::default();
    let value = run(
        "Empty {}\nBare { Return }\n|result| = |[@{Empty}, @{Bare}, @{Empty} == @{Bare}]|",
        &mut context,
    )
    .unwrap();
    assert_eq!(value.to_string(), "[none, none, true]");
}

#[test]
fn multiline_unicode_calls_retain_sentence_continuation_rules() {
    let source = "கூட்டு |அ| உடன் |ஆ| { Return |அ + ஆ| }\n|result| = |@{\nகூட்டு |\n1\n| \\\nஉடன் |@{கூட்டு |2| உடன் |3|}|\n}|";
    for ending in ["\n", "\r\n"] {
        assert!(matches!(
            run(&source.replace('\n', ending), &mut Context::default()),
            Ok(Literal::Int(6))
        ));
    }
}

#[test]
fn malformed_or_statement_containing_calls_are_rejected_during_whole_program_parsing() {
    for expression in [
        "@{}",
        "@{ Value",
        "@{ Value {} }",
        "@{ |x| = |1| }",
        "@{ First\nSecond }",
        "@{ If |true| {} }",
        "@ { Value }",
        "@{ Value |@{}| }",
        "Value(1)",
        "Value |1|",
    ] {
        let source = format!("Log |\"must not execute\"|\n|result| = |{expression}|");
        assert_eq!(
            Program::parse_detailed("invalid.botwork", &source)
                .unwrap_err()
                .code(),
            DiagnosticCode::Syntax,
            "{source}"
        );
    }
}

#[test]
fn ast_call_spans_retain_both_delimiters_and_original_nested_call_text() {
    let program = Program::parse("spans.botwork", "|result| = |@{தமிழ் |1|}|").unwrap();
    let StatementKind::Assign {
        value: AssignmentValue::Expression(expression),
        ..
    } = program.statements[0].kind()
    else {
        panic!("assignment")
    };
    assert_eq!(expression.span.text(), "@{தமிழ் |1|}");
    let ExprKind::Call(call) = &expression.kind else {
        panic!("call expression")
    };
    assert_eq!(call.span.text(), "தமிழ் |1|");
    assert_eq!(call.signature, "தமிழ்|param|");
    assert_eq!(call.span.line_column(), (1, 15));
}

#[test]
fn pair_execution_accepts_call_expression_nodes_and_preserves_inner_error_locations() {
    let mut context = Context::default();
    run("Value |x| { Return |x| }", &mut context).unwrap();
    let pair = BWParser::parse(Rule::call_expression, "@{Value |7|}")
        .unwrap()
        .next()
        .unwrap();
    assert!(matches!(
        botwork_detailed(pair, &mut context),
        Ok(Literal::Int(7))
    ));
    let pair = BWParser::parse(Rule::call_expression, "@{Value |missing|}")
        .unwrap()
        .next()
        .unwrap();
    let error = botwork_detailed(pair, &mut context).unwrap_err();
    assert_eq!(error.span.as_ref().unwrap().source().name(), "<input>");
    assert_eq!(error.span.as_ref().unwrap().text(), "missing");
    assert!(error.call_stack.is_empty());
}
