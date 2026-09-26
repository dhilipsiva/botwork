use super::{botwork, BWErr, BWParser, Context, Literal, LiteralResult, Rule};
use pest::Parser;

fn evaluate(source: &str, context: &mut Context) -> LiteralResult {
    let tree = BWParser::parse(Rule::botwork, source).expect("valid test program");
    let mut result = Literal::None;
    for pair in tree.filter(|pair| pair.as_rule() != Rule::EOI) {
        result = botwork(pair, context)?;
    }
    Ok(result)
}

fn variable(context: &Context, name: &str) -> Literal {
    context.get_variable(&name.to_owned()).unwrap()
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
