//! Complete core value-kind/operator matrix and collection ownership contract.

use botwork::core::{
    ast::Program,
    eval::{evaluate_program, Context},
    grammar::{BWErr, Literal, LiteralResult, Operate, Rule},
};

fn evaluate(source: &str, context: &mut Context) -> LiteralResult {
    let program = Program::parse("values.botwork", source)?;
    evaluate_program(&program, context)
}

fn kind(value: &Literal) -> &str {
    match value {
        Literal::None => "none",
        Literal::Int(_) => "int",
        Literal::Float(_) => "float",
        Literal::Bool(_) => "bool",
        Literal::String(_) => "string",
        Literal::Array(_) => "array",
        Literal::Map(_) => "map",
    }
}

fn representatives() -> Vec<Literal> {
    vec![
        Literal::None,
        Literal::Int(2),
        Literal::Float(0.5),
        Literal::Bool(true),
        Literal::String("x".into()),
        Literal::Array(vec![Literal::Int(1)]),
        Literal::Map([("a".into(), Literal::Int(1))].into_iter().collect()),
    ]
}

#[test]
fn every_binary_operator_checks_every_value_kind_pair() {
    let values = representatives();
    for operator in [
        Rule::plus,
        Rule::minus,
        Rule::multiply,
        Rule::divide,
        Rule::modulus,
        Rule::exponent,
        Rule::less_than,
        Rule::less_than_or_equal,
        Rule::greater_than,
        Rule::greater_than_or_equal,
        Rule::equal,
        Rule::not_equal,
        Rule::logical_and,
        Rule::logical_or,
    ] {
        for left in &values {
            for right in &values {
                let left_kind = kind(left);
                let right_kind = kind(right);
                let numbers =
                    matches!(left_kind, "int" | "float") && matches!(right_kind, "int" | "float");
                let expected = match operator {
                    Rule::equal | Rule::not_equal => Some("bool"),
                    Rule::logical_and | Rule::logical_or
                        if left_kind == "bool" && right_kind == "bool" =>
                    {
                        Some("bool")
                    }
                    Rule::less_than
                    | Rule::less_than_or_equal
                    | Rule::greater_than
                    | Rule::greater_than_or_equal
                        if numbers =>
                    {
                        Some("bool")
                    }
                    Rule::exponent if numbers && right_kind == "int" => Some(left_kind),
                    Rule::divide if numbers => Some("float"),
                    Rule::plus | Rule::minus | Rule::multiply | Rule::modulus if numbers => {
                        Some(if left_kind == "int" && right_kind == "int" {
                            "int"
                        } else {
                            "float"
                        })
                    }
                    Rule::plus
                        if left_kind == right_kind && matches!(left_kind, "string" | "array") =>
                    {
                        Some(left_kind)
                    }
                    _ => None,
                };
                let result = operator.operate_binary(left.clone(), right.clone());
                match (expected, &result) {
                    (Some(expected), Ok(value)) => {
                        assert_eq!(kind(value), expected, "{left:?} {operator:?} {right:?}")
                    }
                    (None, Err(BWErr::OperationIncompatibleError(_))) => (),
                    _ => panic!(
                        "{left:?} {operator:?} {right:?}: expected {expected:?}, got {result:?}"
                    ),
                }
            }
        }
    }
}

#[test]
fn every_unary_operator_checks_every_value_kind() {
    for operator in [Rule::minus, Rule::logical_not] {
        for value in representatives() {
            let expected = match (operator, kind(&value)) {
                (Rule::minus, "int") => Some("-2"),
                (Rule::minus, "float") => Some("-0.5"),
                (Rule::logical_not, "bool") => Some("false"),
                _ => None,
            };
            let result = operator.operate_unary(value.clone());
            match (expected, &result) {
                (Some(expected), Ok(value)) => assert_eq!(value.to_string(), expected),
                (None, Err(BWErr::OperationIncompatibleError(_))) => (),
                _ => panic!("{operator:?} {value:?}: expected {expected:?}, got {result:?}"),
            }
        }
    }
}

#[test]
fn numeric_operations_check_values_for_every_numeric_kind_pair() {
    use Literal::{Float, Int};
    // Fixed, independently calculated answers, including exact binary fractions.
    for (left, right, expected) in [
        (
            Int(7),
            Int(2),
            [Int(9), Int(5), Int(14), Float(3.5), Int(1)],
        ),
        (
            Int(7),
            Float(2.0),
            [Float(9.0), Float(5.0), Float(14.0), Float(3.5), Float(1.0)],
        ),
        (
            Float(7.5),
            Int(2),
            [Float(9.5), Float(5.5), Float(15.0), Float(3.75), Float(1.5)],
        ),
        (
            Float(7.5),
            Float(2.0),
            [Float(9.5), Float(5.5), Float(15.0), Float(3.75), Float(1.5)],
        ),
    ] {
        for ((operator, spelling), expected) in [
            (Rule::plus, "+"),
            (Rule::minus, "-"),
            (Rule::multiply, "*"),
            (Rule::divide, "/"),
            (Rule::modulus, "%"),
        ]
        .into_iter()
        .zip(expected)
        {
            let literal_source = |value: &Literal| match value {
                Float(value) => format!("{value:.1}"),
                Int(value) => value.to_string(),
                _ => unreachable!(),
            };
            let source = format!(
                "|answer| = |{} {spelling} {}|",
                literal_source(&left),
                literal_source(&right)
            );
            for actual in [
                operator
                    .operate_binary(left.clone(), right.clone())
                    .unwrap(),
                evaluate(&source, &mut Context::default()).unwrap(),
            ] {
                match (&actual, &expected) {
                    (Int(actual), Int(expected)) => assert_eq!(actual, expected, "{source}"),
                    (Float(actual), Float(expected)) => {
                        assert_eq!(actual.to_bits(), expected.to_bits(), "{source}")
                    }
                    _ => panic!("{source}: expected {expected:?}, got {actual:?}"),
                }
            }
        }
    }
}

#[test]
fn invalid_left_boolean_operands_identify_the_operator_before_evaluating_the_right() {
    for operator in ["and", "or"] {
        let source = format!("|answer| = |1 {operator} missing|");
        let error = evaluate(&source, &mut Context::default()).unwrap_err();
        assert!(matches!(&error, BWErr::OperationIncompatibleError(message)
            if message == &format!("The left operand of `{operator}` must be a boolean")));
    }
}

#[test]
fn conditions_require_booleans_and_for_requires_an_array_for_every_value_kind() {
    for (value, value_kind) in [
        ("none", "none"),
        ("0", "int"),
        ("0.0", "float"),
        ("true", "bool"),
        ("false", "bool"),
        ("\"\"", "string"),
        ("[]", "array"),
        ("[1]", "array"),
        ("{}", "map"),
        ("{a: 1}", "map"),
    ] {
        for keyword in ["If", "While", "For"] {
            let statement = match keyword {
                "If" => format!("If |{value}| {{}}"),
                "While" => format!("While |{value}| {{ Break }}"),
                _ => format!("For |item| In |{value}| {{}}"),
            };
            let source = format!("Empty {{}}\n|none| = Empty\n{statement}");
            let result = evaluate(&source, &mut Context::default());
            let supported = if keyword == "For" {
                value_kind == "array"
            } else {
                value_kind == "bool"
            };
            if supported {
                assert!(
                    matches!(result, Ok(Literal::None)),
                    "{statement}: {result:?}"
                );
            } else {
                assert!(
                    matches!(result, Err(BWErr::OperationIncompatibleError(_))),
                    "{statement}: {result:?}"
                );
            }
        }
    }
}

#[test]
fn none_bindings_and_entries_are_present_while_absent_names_are_errors() {
    let mut context = Context::default();
    evaluate(
        "Empty {}\n|none| = Empty\n|map| = |{present: none}|",
        &mut context,
    )
    .unwrap();
    for expression in ["none", "map.present", "map[\"present\"]"] {
        assert!(matches!(
            evaluate(&format!("|answer| = |{expression}|"), &mut context),
            Ok(Literal::None)
        ));
    }
    assert!(matches!(
        evaluate("|answer| = |absent|", &mut context),
        Err(BWErr::VariableNotDefined(_))
    ));
    assert!(matches!(
        evaluate("|answer| = |map.absent|", &mut context),
        Err(BWErr::CollectionAccessError { .. })
    ));
}

#[test]
fn variable_argument_return_and_host_copies_do_not_alias_collections() {
    let mut context = Context::default();
    let result = evaluate(
        r#"|original| = |{items: [1]}|
|copy| = |original|
Append |value| { |value| = |{items: value.items + [2]}| Return |value| }
|changed| = Append |copy|
|copy| = |{items: [3]}|
|answer| = |[original, changed, copy]|"#,
        &mut context,
    )
    .unwrap();
    assert_eq!(
        result.to_string(),
        r#"[{"items": [1]}, {"items": [1, 2]}, {"items": [3]}]"#
    );

    let Literal::Map(mut exported) = evaluate("|export| = |original|", &mut context).unwrap()
    else {
        panic!("map value")
    };
    let Some(Literal::Array(items)) = exported.get_mut("items") else {
        panic!("items")
    };
    items.push(Literal::Int(99));
    exported.insert("new".into(), Literal::Bool(true));
    let result = evaluate(
        "|answer| = |original == export and original == {items: [1]}|",
        &mut context,
    )
    .unwrap();
    assert!(matches!(result, Literal::Bool(true)));
}

#[test]
fn duplicate_map_keys_use_decoded_names_and_evaluate_all_values_in_source_order() {
    let result = evaluate(
        "|answer| = |{a: 1, \"a\": 2, \"\\n\": 3, \"\n\": 4}|",
        &mut Context::default(),
    )
    .unwrap();
    assert_eq!(result.to_string(), r#"{"\n": 4, "a": 2}"#);
    for entries in [
        "a: missing_first, a: 2",
        "a: 1, a: missing_first",
        "z: missing_first, a: missing_second",
    ] {
        let mut context = Context::default();
        evaluate("|answer| = |7|", &mut context).unwrap();
        let result = evaluate(&format!("|answer| = |{{{entries}}}|"), &mut context);
        assert!(matches!(result, Err(BWErr::VariableNotDefined(name)) if name == "missing_first"));
        assert!(matches!(
            evaluate("|preserved| = |answer|", &mut context),
            Ok(Literal::Int(7))
        ));
    }
}

#[test]
fn map_display_is_sorted_and_explicit_key_arrays_define_iteration_order() {
    for entries in [
        "z: 2, a: 1, café: 3",
        "café: 3, z: 2, a: 1",
        "a: 1, café: 3, z: 2",
    ] {
        let result =
            evaluate(&format!("|map| = |{{{entries}}}|"), &mut Context::default()).unwrap();
        assert_eq!(result.to_string(), r#"{"a": 1, "café": 3, "z": 2}"#);
    }
    let result = evaluate(
        r#"|map| = |{z: 2, a: 1}|
|values| = |[]|
For |key| In |["z", "a"]| { |values| = |values + [map[key]]| }
|answer| = |values|"#,
        &mut Context::default(),
    )
    .unwrap();
    assert_eq!(result.to_string(), "[2, 1]");
}

#[test]
fn script_results_and_custom_fallthrough_have_explicit_value_rules() {
    for (source, expected) in [
        ("", "none"),
        ("|value| = |7|", "7"),
        ("|value| = |7|\nIf |true| {}", "none"),
        ("Trailing { |local| = |7| }\n|answer| = Trailing", "none"),
        ("Bare { Return }\n|answer| = Bare", "none"),
        ("Explicit { Return |[7]| }\n|answer| = Explicit", "[7]"),
    ] {
        let result = evaluate(source, &mut Context::default()).unwrap();
        assert_eq!(result.to_string(), expected, "{source}");
    }
}

#[test]
fn comparison_chains_follow_binary_grouping_for_numbers_and_collections() {
    for (expression, expected) in [
        ("1 < 2 == true", true),
        ("1 == 2 == false", true),
        ("(1 < 2) and (2 < 3)", true),
        ("[1] == [1] == true", true),
        ("1 == 1 == 1", false),
    ] {
        assert!(
            matches!(evaluate(&format!("|answer| = |{expression}|"), &mut Context::default()),
            Ok(Literal::Bool(value)) if value == expected),
            "{expression}"
        );
    }
    for expression in ["1 < 2 < 3", "1 <= 2 <= 3", "3 > 2 > 1", "3 >= 2 >= 1"] {
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
