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
fn separate_contexts_do_not_share_variables() {
    let mut first = Context::default();
    let mut second = Context::default();
    evaluate("|x| = |1|", &mut first).unwrap();
    assert!(matches!(
        evaluate("|y| = |x|", &mut second),
        Err(BWErr::VariableNotDefined(_))
    ));
}
