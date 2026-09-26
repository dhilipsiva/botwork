use super::{evaluate_program, BWErr, Context, Literal, LiteralResult, Program};

fn evaluate(source: &str, context: &mut Context) -> LiteralResult {
    let program = Program::parse("<test>", source).expect("valid test program");
    evaluate_program(&program, context)
}

fn variable(context: &Context, name: &str) -> Literal {
    context.get_variable(name).unwrap()
}

// Bypass public validation only to exercise defensive runtime completion guards.
fn evaluate_unvalidated_statement(source: &str, context: &mut Context) -> LiteralResult {
    use super::{ast, evaluate_statement, finish_script, Node, Rule};
    use crate::core::grammar::BWParser;
    use pest::Parser;

    let pair = BWParser::parse(Rule::botwork, source)
        .unwrap()
        .next()
        .unwrap();
    let Node::Statement(statement) = ast::from_pair(pair).unwrap() else {
        panic!("expected statement");
    };
    finish_script(evaluate_statement(&statement, context)?)
}

#[test]
fn normally_completed_controls_do_not_collect_body_results() {
    for source in [
        "If |true| { |value| = |7| }",
        "If |false| { |failure| = |missing| }",
        "If |false| {} Else { |value| = |7| }",
        "If |false| {} Else If |true| { |value| = |7| }",
        "For |item| In |[1, 2, 3]| { |value| = |item| }",
        "For |item| In |[]| { |failure| = |missing| }",
        "|i| = |0|\nWhile |i < 1000| { |i| = |i + 1| }",
        "While |false| { |failure| = |missing| }",
        "Try { |value| = |7| } Catch { |failure| = |missing| }",
        "Try { |value| = |missing| } Catch { |value| = |7| }",
    ] {
        let result = evaluate(source, &mut Context::default());
        assert!(matches!(result, Ok(Literal::None)), "{source}: {result:?}");
    }
}

#[test]
fn return_and_break_stop_before_another_while_condition() {
    for control in ["Return |7|", "Break"] {
        let mut context = Context::default();
        let source = format!(
            "Finish {{\n While |true| {{\n {control}\n |failure| = |missing|\n }}\n\
             }}\n|answer| = Finish"
        );
        let result = evaluate(&source, &mut context).unwrap();
        if control.starts_with("Return") {
            assert!(matches!(result, Literal::Int(7)));
            assert_eq!(context.expression_visits, ["true", "7"]);
        } else {
            assert!(matches!(result, Literal::None));
            assert_eq!(context.expression_visits, ["true"]);
        }
    }
}

#[test]
fn return_expression_is_evaluated_once_before_control_transfer() {
    let mut context = Context::default();
    let result = evaluate(
        "Answer {\n If |true| { Return |3 + 4| }\n |failure| = |missing|\n}\n\
         |answer| = Answer",
        &mut context,
    );
    assert!(matches!(result, Ok(Literal::Int(7))), "{result:?}");
    assert_eq!(context.expression_visits, ["true", "3 + 4", "3 ", "4"]);
}

#[test]
fn escaped_control_is_an_error_and_does_not_poison_the_context() {
    for control in ["Return |7|", "Return", "Break", "Continue"] {
        for source in [control.to_owned(), format!("If |true| {{ {control} }}")] {
            let mut context = Context::default();
            assert!(
                matches!(
                    evaluate_unvalidated_statement(&source, &mut context),
                    Err(BWErr::ControlFlowError(_))
                ),
                "{source}"
            );
            let result = evaluate("Fresh {\n Return |9|\n}\n|answer| = Fresh", &mut context);
            assert!(
                matches!(result, Ok(Literal::Int(9))),
                "{source}: {result:?}"
            );
        }
    }
}

#[test]
fn callee_loop_control_cannot_escape_to_a_callers_loop() {
    for control in ["Break", "Continue"] {
        let mut context = Context::default();
        evaluate_unvalidated_statement(&format!("Escape {{ {control} }}"), &mut context).unwrap();
        let error = evaluate("Escape", &mut context).unwrap_err();
        assert!(matches!(error, BWErr::ControlFlowError(message) if message.contains(control)));
        let source = "Fresh { Return |9| }\n|caught| = |0|\n|count| = |0|\n\
                      For |item| In |[1, 2, 3]| {\n\
                      Try { Escape } Catch { |caught| = |caught + 1| }\n\
                      |value| = Fresh\n|count| = |count + 1|\n}\n";
        evaluate(source, &mut context).unwrap();
        assert!(matches!(variable(&context, "caught"), Literal::Int(3)));
        assert!(matches!(variable(&context, "count"), Literal::Int(3)));
        assert!(matches!(variable(&context, "value"), Literal::Int(9)));
    }
}

#[test]
fn return_discards_unreachable_control_statements_in_the_same_loop() {
    for trailing in ["Break", "Continue", "Return |missing|"] {
        let source = format!(
            "Finish {{\n For |item| In |[1, 2]| {{\n Return |7|\n {trailing}\n}}\n\
             |failure| = |missing|\n}}\n|answer| = Finish"
        );
        let result = evaluate(&source, &mut Context::default());
        assert!(
            matches!(result, Ok(Literal::Int(7))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn returning_none_from_a_nested_call_still_stops_the_caller() {
    let result = evaluate(
        "Empty {}\nFinish {\n |value| = Empty\n Return |value|\n\
         |failure| = |missing|\n}\n|answer| = Finish",
        &mut Context::default(),
    );
    assert!(matches!(result, Ok(Literal::None)), "{result:?}");
}

#[test]
fn nested_for_and_while_loops_consume_only_their_own_controls() {
    for outer in [
        "For |outer| In |[1, 2, 3]| {",
        "While |outer < 3| {\n |outer| = |outer + 1|",
    ] {
        for inner in [
            "For |inner| In |[1, 2, 3, 4]| {",
            "While |inner < 4| {\n |inner| = |inner + 1|",
        ] {
            let source = format!(
                "|sum| = |0|\n|outer| = |0|\n{outer}\n |inner| = |0|\n{inner}\n\
                 If |inner == 2| {{ Continue }}\nIf |inner == 3| {{ Break }}\n\
                 |sum| = |sum + 1|\n}}\n|sum| = |sum + 10|\n}}\n|answer| = |sum|"
            );
            let result = evaluate(&source, &mut Context::default());
            assert!(
                matches!(result, Ok(Literal::Int(33))),
                "{source}: {result:?}"
            );
        }
    }
}

#[test]
fn boolean_operators_skip_irrelevant_values_and_failures() {
    let huge_integer = "9".repeat(50);
    let huge_float = format!("{huge_integer}.0");
    for (left, operator, expected) in [("false", "and", false), ("true", "or", true)] {
        for right in [
            "missing",
            "1 / 0",
            "2 ^ 31",
            "m.a",
            "1",
            "1.5",
            "\"text\"",
            "[missing]",
            "{a: missing}",
            "no_result",
            &huge_integer,
            &huge_float,
        ] {
            let mut context = Context::default();
            context.set_variable("no_result".into(), Literal::None);
            let source = format!("|answer| = |{left} {operator} {right}|");
            let result = evaluate(&source, &mut context);
            assert!(
                matches!(result, Ok(Literal::Bool(value)) if value == expected),
                "{source}: {result:?}"
            );
            assert_eq!(
                context.expression_visits.len(),
                2,
                "{source}: only root and left may be evaluated"
            );
            assert_eq!(context.expression_visits[1].trim(), left);
        }
    }
}

#[test]
fn boolean_operators_reject_the_left_type_before_visiting_the_right() {
    for operator in ["and", "or"] {
        for left in ["1", "1.5", "\"text\"", "[]", "{}", "no_result"] {
            let mut context = Context::default();
            context.set_variable("no_result".into(), Literal::None);
            let source = format!("|answer| = |{left} {operator} missing|");
            let result = evaluate(&source, &mut context);
            assert!(
                matches!(result, Err(BWErr::OperationIncompatibleError(_))),
                "{source}: {result:?}"
            );
            assert_eq!(
                context.expression_visits.len(),
                2,
                "{source}: invalid left must stop evaluation"
            );
        }
    }
}

#[test]
fn boolean_truth_tables_evaluate_required_operands_once_in_order() {
    for operator in ["and", "or"] {
        for left in [false, true] {
            for right in [false, true] {
                let mut context = Context::default();
                context.set_variable("left".into(), Literal::Bool(left));
                context.set_variable("right".into(), Literal::Bool(right));
                let source = format!("|answer| = |left {operator} right|");
                let expected = if operator == "and" {
                    left && right
                } else {
                    left || right
                };
                let result = evaluate(&source, &mut context);
                assert!(
                    matches!(result, Ok(Literal::Bool(value)) if value == expected),
                    "{source}: {result:?}"
                );
                let visits = context
                    .expression_visits
                    .iter()
                    .skip(1)
                    .map(|text| text.trim())
                    .collect::<Vec<_>>();
                if (operator == "and" && !left) || (operator == "or" && left) {
                    assert_eq!(visits, ["left"]);
                } else {
                    assert_eq!(visits, ["left", "right"]);
                }
            }
        }
    }
}

#[test]
fn required_boolean_operands_preserve_errors_and_type_requirements() {
    for expression in [
        "true and missing",
        "false or missing",
        "missing and false",
        "missing or true",
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Err(BWErr::VariableNotDefined(name)) if name == "missing"),
            "{expression}"
        );
    }
    for expression in [
        "true and (1 / 0)",
        "false or (1 / 0)",
        "(1 / 0) and false",
        "(1 / 0) or true",
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Err(BWErr::ArithmeticError(_))),
            "{expression}: {result:?}"
        );
    }
    for (left, operator) in [("true", "and"), ("false", "or")] {
        for right in ["1", "1.5", "\"text\"", "[]", "{}", "no_result"] {
            let mut context = Context::default();
            context.set_variable("no_result".into(), Literal::None);
            let source = format!("|answer| = |{left} {operator} {right}|");
            let result = evaluate(&source, &mut context);
            assert!(
                matches!(result, Err(BWErr::OperationIncompatibleError(_))),
                "{source}: {result:?}"
            );
            assert_eq!(
                context.expression_visits.len(),
                3,
                "{source}: both operands must be evaluated once"
            );
        }
    }
}

#[test]
fn short_circuiting_follows_precedence_parentheses_and_unary_grouping() {
    for (expression, expected) in [
        ("true or false and missing", true),
        ("false and missing or true", true),
        ("false and (missing or true)", false),
        ("!(false and missing)", true),
        ("(true or missing) == true", true),
        ("false or true and false", false),
        ("true and (false or true)", true),
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Bool(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
    assert!(
        matches!(evaluate("|answer| = |(true or false) and missing|", &mut Context::default()),
        Err(BWErr::VariableNotDefined(name)) if name == "missing")
    );
}

#[test]
fn lazy_conditions_and_catches_execute_only_required_paths() {
    let mut context = Context::default();
    evaluate(
        "|i| = |0|\n|answer| = |7|\n|caught| = |0|\n\
         Try {\n If |true or missing| { |answer| = |8| }\n\
         While |i < 3 and 6 / (3 - i) > 0| { |i| = |i + 1| }\n\
         } Catch { |caught| = |99| }\n\
         Try {\n |answer| = |true and missing|\n |i| = |99|\n\
         } Catch { |caught| = |caught + 1| }",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "i"), Literal::Int(3)));
    assert!(matches!(variable(&context, "answer"), Literal::Int(8)));
    assert!(matches!(variable(&context, "caught"), Literal::Int(1)));
}

#[test]
fn ordinary_binary_operators_still_evaluate_the_right_operand() {
    for expression in ["false == missing", "true != missing", "1 + missing"] {
        let mut context = Context::default();
        let result = evaluate(&format!("|answer| = |{expression}|"), &mut context);
        assert!(matches!(result, Err(BWErr::VariableNotDefined(name)) if name == "missing"));
        assert_eq!(context.expression_visits.len(), 3);
        assert_eq!(context.expression_visits[2].trim(), "missing");
    }
}

#[test]
fn short_circuiting_composes_in_collections_and_custom_arguments() {
    let result = evaluate(
        "|answer| = |[false and missing, {value: true or missing}]|",
        &mut Context::default(),
    )
    .unwrap();
    let Literal::Array(values) = result else {
        panic!("expected array")
    };
    assert!(matches!(values[0], Literal::Bool(false)));
    let Literal::Map(map) = &values[1] else {
        panic!("expected map")
    };
    assert!(matches!(map.get("value"), Some(Literal::Bool(true))));

    let result = evaluate(
        "Identity |value| {\n Return |value|\n}\n\
         |answer| = Identity |true or missing|",
        &mut Context::default(),
    );
    assert!(matches!(result, Ok(Literal::Bool(true))), "{result:?}");
}

#[test]
fn binary_evaluation_does_not_visit_the_right_operand_after_a_left_error() {
    let mut context = Context::default();
    let error = evaluate("|answer| = |missing_left + (1 / 0)|", &mut context).unwrap_err();
    assert!(matches!(error, BWErr::VariableNotDefined(name) if name == "missing_left"));
    assert_eq!(
        context.expression_visits.len(),
        2,
        "only binary root and left operand may be visited"
    );
    assert_eq!(context.expression_visits[1].trim(), "missing_left");
}

#[test]
fn custom_calls_reuse_the_same_definition_and_original_source_spans() {
    use super::{StatementKind, StmtType};
    use std::sync::Arc;

    let mut context = Context::default();
    let weak_definition = {
        let source = String::from(
            "# original\nDouble |value| {\n Return |value * 2|\n |unreachable| = |missing|\n}",
        );
        let program = Program::parse("original.botwork", &source).unwrap();
        let StatementKind::Define(definition) = &program.statements[0].kind else {
            panic!("definition")
        };
        let weak = Arc::downgrade(definition);
        evaluate_program(&program, &mut context).unwrap();
        let StmtType::UserDefined(stored) = &context.statements["double|param|"] else {
            panic!("stored definition")
        };
        assert!(Arc::ptr_eq(definition, stored));
        weak
    };

    for value in [2, 5, 9] {
        let result = evaluate(&format!("|answer| = Double |{value}|"), &mut context);
        assert!(matches!(result, Ok(Literal::Int(answer)) if answer == value * 2));
        let StmtType::UserDefined(stored) = &context.statements["double|param|"] else {
            panic!("stored definition")
        };
        assert!(Arc::ptr_eq(&weak_definition.upgrade().unwrap(), stored));
        assert_eq!(
            Arc::strong_count(stored),
            1,
            "calls must not retain cloned bodies"
        );
        assert_eq!(stored.span.source().name(), "original.botwork");
        assert_eq!(stored.span.line_column(), (2, 1));
        assert_eq!(stored.body.statements[0].span.line_column(), (3, 2));
        assert_eq!(stored.body.statements[0].span.text(), "Return |value * 2|");
    }
    drop(context);
    assert!(
        weak_definition.upgrade().is_none(),
        "stored syntax must be released with its context"
    );
}

#[test]
fn keyword_prefix_identifiers_preserve_their_complete_names() {
    for name in [
        "order",
        "android",
        "trueValue",
        "falsehood",
        "trueandfalse",
        "or2",
        "and_",
        "trueé",
        "false漢",
        "or٣",
        "True",
        "False",
        "And",
        "Or",
    ] {
        let source = format!("|{name}| = |7|\n|answer| = |{name} + 1|");
        let result = evaluate(&source, &mut Context::default());
        assert!(
            matches!(result, Ok(Literal::Int(8))),
            "{source}: {result:?}"
        );
    }
    let result = evaluate(
        "|answer| = |{order: 7, trueValue: 8, android: 9}|",
        &mut Context::default(),
    )
    .unwrap();
    let Literal::Map(values) = result else {
        panic!("expected map")
    };
    assert!(matches!(values.get("order"), Some(Literal::Int(7))));
    assert!(matches!(values.get("trueValue"), Some(Literal::Int(8))));
    assert!(matches!(values.get("android"), Some(Literal::Int(9))));
    assert!(
        matches!(evaluate("|answer| = |order.trueValue|", &mut Context::default()), Err(BWErr::UnsupportedAccessError(path)) if path == "order.trueValue")
    );
}

#[test]
fn custom_statements_starting_with_keyword_text_execute_normally() {
    for name in [
        "Format report",
        "Ifonly",
        "Elsewhere",
        "Breakdown",
        "Return2",
        "Continue_job",
        "Whileé",
        "If\u{301}",
        "Try漢",
        "Catch٣",
        "Break!",
        "Return-value",
        "I f",
        "In order",
    ] {
        let source = format!("{name} {{\n Return |7|\n}}\n|answer| = {name}");
        let result = evaluate(&source, &mut Context::default());
        assert!(
            matches!(result, Ok(Literal::Int(7))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn assignments_use_existing_variables_and_arithmetic_precedence() {
    let mut context = Context::default();
    evaluate("|a| = |2|\n|answer| = |a + 3 * 4|", &mut context).unwrap();
    assert!(matches!(variable(&context, "answer"), Literal::Int(14)));
}

#[test]
fn parentheses_override_arithmetic_precedence() {
    let mut context = Context::default();
    let result = evaluate("|answer| = |(2 + 3) * 4|", &mut context).unwrap();
    assert!(matches!(result, Literal::Int(20)));
}

#[test]
fn binary_precedence_orders_arithmetic_comparisons_equality_and_logic() {
    for expression in [
        "1 + 2 == 3",
        "3 == 1 + 2",
        "1 + 2 * 3 > 6",
        "6 < 1 + 2 * 3",
        "9 - 3 * 2 >= 3",
        "9 % 4 + 1 <= 2",
        "8 / 2 + 1 == 5",
        "2 ^ 3 * 2 == 16",
        "1 < 2 == 3 > 2",
        "false == 2 < 1",
        "true != 2 < 1",
        "true or true != true",
        "true or false and false",
        "1 + 2 == 3 and 4 > 3 or false",
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Bool(true))),
            "{expression}: {result:?}"
        );
    }
    let result = evaluate(
        "|answer| = |false and false == false|",
        &mut Context::default(),
    );
    assert!(matches!(result, Ok(Literal::Bool(false))), "{result:?}");
}

#[test]
fn additive_and_multiplicative_operators_associate_left() {
    for (expression, expected) in [("20 - 5 - 2", 13), ("11 % 7 % 5", 4), ("20 % 6 * 2", 4)] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Int(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
    let result = evaluate("|answer| = |12 / 3 / 2|", &mut Context::default());
    assert!(matches!(result, Ok(Literal::Float(2.0))), "{result:?}");
}

#[test]
fn parentheses_override_binary_precedence_and_association() {
    for (expression, expected) in [
        ("false and (false == false)", false),
        ("(false and false) == false", true),
        ("(true or false) and false", false),
        ("20 - (5 - 2) == 17", true),
        ("(1 + 2) * 3 == 9", true),
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Bool(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
}

#[test]
fn subtraction_and_unary_operators_work_in_the_same_expression() {
    for (expression, expected) in [("3 - -2", 5), ("-2 * 3", -6), ("-(2 + 3) - 1", -6)] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Int(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
    let result = evaluate("|answer| = |!false and !(1 > 2)|", &mut Context::default());
    assert!(matches!(result, Ok(Literal::Bool(true))), "{result:?}");
}

#[test]
fn mixed_precedence_works_in_conditions_and_collections() {
    let mut context = Context::default();
    evaluate(
        "If |1 + 2 == 3 and 9 > 3| {\n |picked| = |7|\n} Else {\n |picked| = |missing|\n}\n\
         |values| = |[1 + 2 == 3, 3 > 2 == true, true or false and false]|",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "picked"), Literal::Int(7)));
    assert!(
        matches!(variable(&context, "values"), Literal::Array(values)
        if values.len() == 3 && values.iter().all(|value| matches!(value, Literal::Bool(true))))
    );
}

#[test]
fn mixed_precedence_does_not_coerce_invalid_operand_types() {
    for expression in [
        "1 and 2",
        "true + false",
        "1 < true",
        "1 + 2 == true",
        "!1",
        "-true",
    ] {
        assert!(
            matches!(
                evaluate(
                    &format!("|answer| = |{expression}|"),
                    &mut Context::default()
                ),
                Err(BWErr::OperationIncompatibleError(_))
            ),
            "{expression}"
        );
    }
}

#[test]
fn arithmetic_failures_are_typed_and_catchable_without_assignment_changes() {
    for expression in [
        "1 % 0",
        "1 / 0",
        "1.0 % -0.0",
        "2147483647 + 1",
        "(-2147483647 - 1) - 1",
        "50000 * 50000",
        "-(-2147483647 - 1)",
        "2 ^ 31",
        "0 ^ -1",
        "2.0 ^ 128",
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Err(BWErr::ArithmeticError(_))),
            "{expression}: {result:?}"
        );
        let mut context = Context::default();
        evaluate(
            &format!(
                "|answer| = |7|\nTry {{\n |answer| = |{expression}|\n}} Catch {{\n\
             |caught| = |true|\n}}\n|after| = |answer|"
            ),
            &mut context,
        )
        .unwrap();
        assert!(matches!(variable(&context, "answer"), Literal::Int(7)));
        assert!(matches!(variable(&context, "caught"), Literal::Bool(true)));
        assert!(matches!(variable(&context, "after"), Literal::Int(7)));
    }
}

#[test]
fn float_literals_reject_nonfinite_overflow_and_allow_finite_underflow() {
    let source = format!("|number| = |{}.0|", "9".repeat(80));
    assert!(matches!(
        evaluate(&source, &mut Context::default()),
        Err(BWErr::ArithmeticError(_))
    ));
    let mut context = Context::default();
    evaluate(
        &format!("Try {{\n {source}\n}} Catch {{\n |caught| = |true|\n}}"),
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "caught"), Literal::Bool(true)));
    let source = format!("|number| = |0.{}1|", "0".repeat(60));
    assert!(matches!(
        evaluate(&source, &mut Context::default()),
        Ok(Literal::Float(0.0))
    ));
}

#[test]
fn arithmetic_boundaries_and_reciprocal_powers_work_in_source() {
    for (expression, expected) in [("(-2) ^ 31", i32::MIN), ("(-2147483647 - 1) % -1", 0)] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Int(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
    for (expression, expected) in [
        ("(-2147483647 - 1) / -1", 2147483648.0),
        ("1 ^ (-2147483647 - 1)", 1.0),
        ("(-1.0) ^ 16777217", -1.0),
        ("2 ^ -149", f32::from_bits(1)),
        ("-7.5 % 3", -1.5),
        ("7 % -3.0", 1.0),
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Float(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
}

#[test]
fn powers_associate_right_and_parentheses_override_them() {
    for (expression, expected) in [
        ("2 ^ 3 ^ 2", 512),
        ("(2 ^ 3) ^ 2", 64),
        ("2 ^ (3 ^ 2)", 512),
        ("2 ^ 2 ^ 3", 256),
        ("2 ^ 3 ^ 0", 2),
        ("4 * 2 ^ 3", 32),
        ("2 ^ 3 * 4", 32),
        ("2 ^ --3", 8),
        ("(-2) ^ 31", i32::MIN),
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Int(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
}

#[test]
fn unary_minus_binds_after_power_and_before_multiplication() {
    for (expression, expected) in [
        ("-2 ^ 2", -4),
        ("(-2) ^ 2", 4),
        ("-2 ^ 2 * 3", -12),
        ("--2 ^ 2", 4),
        ("- - -2", -2),
        ("3 - --2", 1),
        ("-2 ^ 0", -1),
        ("(-2) ^ 0", 1),
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Int(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
}

#[test]
fn powers_accept_negative_exponents_with_the_same_grouping_rules() {
    for (expression, expected) in [
        ("2 ^ -2", 0.25),
        ("2 ^ -2 ^ 2", 0.0625),
        ("-2 ^ -2", -0.25),
        ("(-2) ^ -2", 0.25),
        ("(-2) ^ -3", -0.125),
        ("2 ^ -(1 + 2)", 0.125),
        ("2 ^ -2 * 4", 1.0),
    ] {
        let result = evaluate(
            &format!("|answer| = |{expression}|"),
            &mut Context::default(),
        );
        assert!(
            matches!(result, Ok(Literal::Float(value)) if value == expected),
            "{expression}: {result:?}"
        );
    }
}

#[test]
fn nested_unary_expressions_keep_type_errors_and_controlled_failures() {
    for expression in ["!!true", "!!!false", "!!(1 < 2)", "!false == true"] {
        assert!(
            matches!(
                evaluate(
                    &format!("|answer| = |{expression}|"),
                    &mut Context::default()
                ),
                Ok(Literal::Bool(true))
            ),
            "{expression}"
        );
    }
    for expression in [
        "!-2",
        "-!true",
        "!!1",
        "--true",
        "2 ^ true",
        "2 ^ 0.5",
        "2 ^ 2.0",
        "2 ^ 2 ^ -1",
    ] {
        assert!(
            matches!(
                evaluate(
                    &format!("|answer| = |{expression}|"),
                    &mut Context::default()
                ),
                Err(BWErr::OperationIncompatibleError(_))
            ),
            "{expression}"
        );
    }
    assert!(
        matches!(evaluate("|answer| = |2 ^ -missing|", &mut Context::default()),
        Err(BWErr::VariableNotDefined(name)) if name == "missing")
    );
    assert!(
        matches!(evaluate("|answer| = |2 ^ m.a|", &mut Context::default()),
        Err(BWErr::UnsupportedAccessError(path)) if path == "m.a")
    );
    for expression in ["2 ^ 2 ^ 5", "-2 ^ 31", "2 ^ (1 / 0)", "(2 ^ 31) ^ 0"] {
        assert!(
            matches!(
                evaluate(
                    &format!("|answer| = |{expression}|"),
                    &mut Context::default()
                ),
                Err(BWErr::ArithmeticError(_))
            ),
            "{expression}"
        );
    }
}

#[test]
fn undefined_variables_and_statements_return_typed_errors() {
    let mut context = Context::default();
    assert!(matches!(
        evaluate("|x| = |missing|", &mut context),
        Err(BWErr::VariableNotDefined(_))
    ));
    assert!(matches!(
        evaluate("Unknown statement", &mut context),
        Err(BWErr::StatementNotDefined(_))
    ));
}

#[test]
fn failed_assignment_does_not_overwrite_a_value() {
    let mut context = Context::default();
    evaluate("|x| = |7|", &mut context).unwrap();
    assert!(evaluate("|x| = |missing|", &mut context).is_err());
    assert!(matches!(variable(&context, "x"), Literal::Int(7)));
}

#[test]
fn if_executes_only_the_selected_branch() {
    for (condition, expected) in [("true", 1), ("false", 2)] {
        let mut context = Context::default();
        let (when_true, when_false) = if condition == "true" {
            ("1", "missing")
        } else {
            ("missing", "2")
        };
        evaluate(
            &format!(
                "If |{condition}| {{\n |x| = |{when_true}|\n}} Else {{\n |x| = |{when_false}|\n}}"
            ),
            &mut context,
        )
        .unwrap();
        assert!(matches!(variable(&context, "x"), Literal::Int(value) if value == expected));
    }
}

#[test]
fn non_boolean_condition_is_an_error() {
    let mut context = Context::default();
    assert!(matches!(
        evaluate("If |1| {\n |x| = |2|\n}", &mut context),
        Err(BWErr::OperationIncompatibleError(_))
    ));
}

#[test]
fn for_accumulates_values_and_handles_continue_and_break() {
    let mut context = Context::default();
    evaluate(
        "|sum| = |0|\nFor |i| in |[1, 2, 3, 4, 5]| {\n\
         If |i == 2| { Continue }\n\
         If |i == 4| { Break }\n\
         |sum| = |sum + i|\n}",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "sum"), Literal::Int(4)));
}

#[test]
fn try_catch_recovers_from_an_undefined_variable() {
    let mut context = Context::default();
    evaluate(
        "Try {\n |x| = |missing|\n} Catch {\n |x| = |42|\n}",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "x"), Literal::Int(42)));
}

#[test]
fn successful_try_does_not_execute_catch() {
    let mut context = Context::default();
    evaluate(
        "Try {\n |x| = |7|\n} Catch {\n |x| = |missing|\n}",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "x"), Literal::Int(7)));
}

#[test]
fn a_failing_handler_propagates_its_error() {
    let mut context = Context::default();
    let result = evaluate(
        "Try { |x| = |body_missing| } Catch { |x| = |handler_missing| }",
        &mut context,
    );
    assert!(matches!(result, Err(BWErr::VariableNotDefined(name)) if name == "handler_missing"));
}

#[test]
fn outer_catch_handles_inner_handler_failure_once() {
    let mut context = Context::default();
    evaluate(
        "|count| = |0|\nTry {\n Try {\n |x| = |body_missing|\n } Catch {\n\
         |count| = |count + 1|\n |x| = |handler_missing|\n }\n\
         |count| = |999|\n} Catch {\n |count| = |count + 10|\n}\n\
         |after| = |true|",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "count"), Literal::Int(11)));
    assert!(matches!(variable(&context, "after"), Literal::Bool(true)));
}

#[test]
fn custom_statement_names_ignore_case_and_spaces() {
    let mut context = Context::default();
    evaluate(
        "Check value |value| {\n If |value != 42| {\n |failure| = |missing|\n }\n}\n\
         CHECK   Value |42|",
        &mut context,
    )
    .unwrap();
    assert!(matches!(
        evaluate("CHECK   Value |41|", &mut context),
        Err(BWErr::VariableNotDefined(_))
    ));
}

#[test]
fn collection_literals_evaluate_nested_expressions() {
    let mut context = Context::default();
    let value = evaluate("|data| = |{items: [1 + 2, true]}|", &mut context).unwrap();
    let Literal::Map(values) = value else {
        panic!("expected a map");
    };
    let Some(Literal::Array(items)) = values.get("items") else {
        panic!("expected items array");
    };
    assert!(matches!(
        items.as_slice(),
        [Literal::Int(3), Literal::Bool(true)]
    ));
}

#[test]
fn unsupported_access_reports_the_path_and_preserves_assignment_state() {
    for path in [
        "m.a",
        "m.a.0",
        "items.0",
        "missing.a",
        "δ.α",
        "items.999999999999999999999",
    ] {
        let mut context = Context::default();
        evaluate("|answer| = |9|", &mut context).unwrap();
        let error = evaluate(&format!("|answer| = |{path}|"), &mut context).unwrap_err();
        assert!(matches!(&error, BWErr::UnsupportedAccessError(found) if found == path));
        assert!(error.to_string().contains("unsupported"), "{error}");
        assert!(error.to_string().contains(path), "{error}");
        assert!(matches!(variable(&context, "answer"), Literal::Int(9)));
        evaluate("|answer| = |10|", &mut context).unwrap();
        assert!(matches!(variable(&context, "answer"), Literal::Int(10)));
    }
}

#[test]
fn unsupported_access_propagates_through_expression_contexts() {
    for source in [
        "|answer| = |1 + m.a|",
        "|answer| = |[m.a]|",
        "|answer| = |{value: m.a}|",
        "If |m.a| {}",
        "While |m.a| {}",
        "For |item| in |m.a| {}",
        "Log |m.a|",
        "Inspect |value| {}\nInspect |m.a|",
    ] {
        let mut context = Context::default();
        context.init_statements();
        evaluate("|m| = |{a: 7}|", &mut context).unwrap();
        let error = evaluate(source, &mut context).unwrap_err();
        assert!(matches!(&error, BWErr::UnsupportedAccessError(path) if path == "m.a"));
        assert!(
            error.to_string().contains("unsupported"),
            "{source}: {error}"
        );
        assert!(error.to_string().contains("m.a"), "{source}: {error}");
    }
}

#[test]
fn unsupported_access_is_catchable_and_skipped_branches_do_not_evaluate_it() {
    let mut context = Context::default();
    evaluate(
        "|answer| = |9|\nTry {\n |answer| = |m.a|\n} Catch {\n\
         |caught| = |true|\n}\nIf |false| {\n |answer| = |m.a|\n}\n\
         |after| = |answer + 1|",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "answer"), Literal::Int(9)));
    assert!(matches!(variable(&context, "caught"), Literal::Bool(true)));
    assert!(matches!(variable(&context, "after"), Literal::Int(10)));
}

#[test]
fn separate_contexts_do_not_share_variables() {
    let mut first = Context::default();
    let mut second = Context::default();
    evaluate("|x| = |1|", &mut first).unwrap();
    assert!(matches!(
        evaluate("|y| = |x|", &mut second),
        Err(BWErr::VariableNotDefined(_))
    ));
}

#[test]
fn strings_decode_supported_escapes_once_and_preserve_unicode() {
    for (source, expected) in [
        (r#"|s| = |""|"#, ""),
        (r#"|s| = |"hello"|"#, "hello"),
        (r#"|s| = |"  hello  "|"#, "  hello  "),
        ("|s| = |\"\t hello\t\"|", "\t hello\t"),
        ("|s| = |\"# literal\"|", "# literal"),
        ("|s| = |\"### literal\"|", "### literal"),
        (r#"|s| = |"தமிழ் café 🦀"|"#, "தமிழ் café 🦀"),
        (r#"|s| = |"a\nb\"c\\d"|"#, "a\nb\"c\\d"),
        (r#"|s| = |"\\n"|"#, "\\n"),
        (r#"|s| = |"| # { }"|"#, "| # { }"),
        ("|s| = |\"line one\nline two\"|", "line one\nline two"),
    ] {
        let result = evaluate(source, &mut Context::default()).unwrap();
        assert!(
            matches!(&result, Literal::String(value) if value == expected),
            "source {source:?}: expected {expected:?}, got {result:?}"
        );
    }
}

#[test]
fn decoded_strings_concatenate_without_source_delimiters() {
    let result = evaluate(r#"|s| = |"hello " + "world"|"#, &mut Context::default()).unwrap();
    assert!(matches!(result, Literal::String(value) if value == "hello world"));
}

#[test]
fn map_keys_remain_identifiers_while_string_values_are_decoded() {
    let result = evaluate(
        r#"|m| = |{label: "hello", nested: {value: "a\nb"}}|"#,
        &mut Context::default(),
    )
    .unwrap();
    let Literal::Map(values) = result else {
        panic!("expected map");
    };
    assert!(matches!(values.get("label"), Some(Literal::String(value)) if value == "hello"));
    let Some(Literal::Map(nested)) = values.get("nested") else {
        panic!("expected nested map");
    };
    assert!(matches!(nested.get("value"), Some(Literal::String(value)) if value == "a\nb"));
}

#[test]
fn log_output_is_a_value_followed_by_a_newline() {
    let mut output = Vec::new();
    super::write_log(&Literal::String("hello".into()), &mut output).unwrap();
    assert_eq!(output, b"hello\n");
}

#[test]
fn log_output_failure_returns_a_typed_error() {
    struct BrokenWriter;
    impl std::io::Write for BrokenWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "closed",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(matches!(
        super::write_log(&Literal::Int(7), &mut BrokenWriter),
        Err(BWErr::OutputError(message)) if message.contains("closed")
    ));
}

#[test]
fn while_rechecks_the_condition_after_every_iteration() {
    let mut context = Context::default();
    evaluate(
        "|i| = |0|\nWhile |i < 4| {\n |i| = |i + 1|\n}\n",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "i"), Literal::Int(4)));
}

#[test]
fn false_while_does_not_execute_its_body() {
    let mut context = Context::default();
    evaluate("While |false| {\n |x| = |missing|\n}", &mut context).unwrap();
}

#[test]
fn while_continue_skips_the_remaining_body_and_rechecks_condition() {
    let mut context = Context::default();
    evaluate(
        "|i| = |0|\n|sum| = |0|\nWhile |i < 5| {\n |i| = |i + 1|\n\
         If |i == 2| { Continue }\n |sum| = |sum + i|\n}",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "sum"), Literal::Int(13)));
    assert!(matches!(variable(&context, "i"), Literal::Int(5)));
}

#[test]
fn while_break_stops_the_loop_before_the_rest_of_its_body() {
    let mut context = Context::default();
    evaluate(
        "|i| = |0|\n|sum| = |0|\nWhile |i < 10| {\n |i| = |i + 1|\n\
         If |i == 4| { Break }\n |sum| = |sum + i|\n}",
        &mut context,
    )
    .unwrap();
    assert!(matches!(variable(&context, "sum"), Literal::Int(6)));
    assert!(matches!(variable(&context, "i"), Literal::Int(4)));
}

#[test]
fn while_reports_a_condition_that_becomes_non_boolean() {
    let mut context = Context::default();
    assert!(matches!(
        evaluate(
            "|condition| = |true|\nWhile |condition| {\n |condition| = |1|\n}",
            &mut context
        ),
        Err(BWErr::OperationIncompatibleError(_))
    ));
}
