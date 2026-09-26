use super::{BWErr, BWParser, Literal, Operate, Rule};
use pest::Parser;

#[test]
fn parses_assignments_collections_and_custom_statements() {
    let source = r#"
        |values| = |[1, 2.5, true, {name: "sample"}]|
        Double |number| {
            Return |number * 2|
        }
        |answer| = Double |3|
    "#;
    let rules: Vec<_> = BWParser::parse(Rule::botwork, source)
        .unwrap()
        .map(|pair| pair.as_rule())
        .collect();
    assert_eq!(
        rules,
        [
            Rule::stmt_assign,
            Rule::stmt_define,
            Rule::stmt_assign,
            Rule::EOI
        ]
    );
}

#[test]
fn parses_comments_and_control_flow_with_crlf() {
    let source = "# comment\r\nIf |true| {\r\n |x| = |1|\r\n} Else {\r\n |x| = |2|\r\n}\r\n";
    let rules: Vec<_> = BWParser::parse(Rule::botwork, source)
        .unwrap()
        .map(|pair| pair.as_rule())
        .collect();
    assert_eq!(rules, [Rule::stmt_if, Rule::EOI]);
}

#[test]
fn rejects_unclosed_parameters_blocks_and_strings() {
    for source in ["|x| = |1", "If |true| {", "Log |\"unterminated|"] {
        assert!(BWParser::parse(Rule::botwork, source).is_err(), "{source}");
    }
}

#[test]
fn parses_empty_program() {
    let rules: Vec<_> = BWParser::parse(Rule::botwork, "")
        .unwrap()
        .map(|pair| pair.as_rule())
        .collect();
    assert_eq!(rules, [Rule::EOI]);
}

#[test]
fn integer_operators_produce_expected_values() {
    for (operator, lhs, rhs, expected) in [
        (Rule::plus, 5, 3, 8),
        (Rule::minus, 5, 3, 2),
        (Rule::multiply, 5, 3, 15),
        (Rule::modulus, 5, 3, 2),
        (Rule::exponent, 5, 3, 125),
    ] {
        let result = operator
            .operate_binary(Literal::Int(lhs), Literal::Int(rhs))
            .unwrap();
        assert!(matches!(result, Literal::Int(value) if value == expected));
    }
}

#[test]
fn division_and_mixed_arithmetic_preserve_fractional_results() {
    let division = Rule::divide
        .operate_binary(Literal::Int(7), Literal::Int(2))
        .unwrap();
    assert!(matches!(division, Literal::Float(3.5)));
    let sum = Rule::plus
        .operate_binary(Literal::Float(1.5), Literal::Int(2))
        .unwrap();
    assert!(matches!(sum, Literal::Float(3.5)));
}

#[test]
fn comparisons_and_boolean_operators_produce_booleans() {
    for (operator, expected) in [
        (Rule::less_than, true),
        (Rule::less_than_or_equal, true),
        (Rule::greater_than, false),
        (Rule::greater_than_or_equal, false),
        (Rule::equal, false),
        (Rule::not_equal, true),
    ] {
        let result = operator
            .operate_binary(Literal::Int(2), Literal::Int(3))
            .unwrap();
        assert!(matches!(result, Literal::Bool(value) if value == expected));
    }
    for (operator, expected) in [(Rule::logical_and, false), (Rule::logical_or, true)] {
        let result = operator
            .operate_binary(Literal::Bool(true), Literal::Bool(false))
            .unwrap();
        assert!(matches!(result, Literal::Bool(value) if value == expected));
    }
}

#[test]
fn unary_operators_check_operand_types() {
    assert!(matches!(
        Rule::minus.operate_unary(Literal::Int(3)),
        Ok(Literal::Int(-3))
    ));
    assert!(matches!(
        Rule::logical_not.operate_unary(Literal::Bool(true)),
        Ok(Literal::Bool(false))
    ));
    assert!(matches!(
        Rule::logical_not.operate_unary(Literal::Int(1)),
        Err(BWErr::OperationIncompatibleError(_))
    ));
}

#[test]
fn incompatible_binary_operands_return_an_error() {
    assert!(matches!(
        Rule::plus.operate_binary(Literal::Int(1), Literal::Bool(true)),
        Err(BWErr::OperationIncompatibleError(_))
    ));
}

#[test]
fn array_concatenation_preserves_order() {
    let result = Rule::plus
        .operate_binary(
            Literal::Array(vec![Literal::Int(1)]),
            Literal::Array(vec![Literal::Int(2)]),
        )
        .unwrap();
    assert!(matches!(result, Literal::Array(values)
        if matches!(values.as_slice(), [Literal::Int(1), Literal::Int(2)])));
}

#[test]
fn unsupported_string_escapes_are_rejected() {
    for source in [r#"|s| = |"\t"|"#, r#"|s| = |"\q"|"#, r#"|s| = |"\u1234"|"#] {
        assert!(BWParser::parse(Rule::botwork, source).is_err(), "{source}");
    }
}

#[test]
fn display_values_without_rust_type_wrappers() {
    for (value, expected) in [
        (Literal::None, "none"),
        (Literal::Int(-3), "-3"),
        (Literal::Float(1.5), "1.5"),
        (Literal::Bool(false), "false"),
        (Literal::String("a\nb".into()), "a\nb"),
        (Literal::Array(vec![]), "[]"),
        (Literal::Map(Default::default()), "{}"),
    ] {
        assert_eq!(value.to_string(), expected);
    }
}

#[test]
fn display_nested_collections_with_escaped_strings_and_sorted_keys() {
    let value = Literal::Map(
        [
            ("z".into(), Literal::None),
            (
                "a".into(),
                Literal::Array(vec![
                    Literal::String("line\n\"quoted\"\\".into()),
                    Literal::Map([("key".into(), Literal::Int(7))].into()),
                ]),
            ),
        ]
        .into(),
    );
    assert_eq!(
        value.to_string(),
        r#"{"a": ["line\n\"quoted\"\\", {"key": 7}], "z": none}"#
    );
}
