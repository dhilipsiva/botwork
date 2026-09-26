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
fn try_catch_has_both_blocks_in_empty_mixed_case_and_nested_forms() {
    for source in [
        "Try {} Catch {}",
        "tRy\t{\r\n} cAtCh {\r\n}",
        "Try {\n |x| = |1|\n} Catch {\n |x| = |2|\n}",
        "Try { Try {} Catch {} } Catch {}",
    ] {
        let mut program = BWParser::parse(Rule::botwork, source).unwrap();
        let statement = program.next().unwrap();
        assert_eq!(statement.as_rule(), Rule::stmt_try, "{source}");
        let children: Vec<_> = statement.into_inner().map(|pair| pair.as_rule()).collect();
        assert_eq!(children, [Rule::stmt_block, Rule::stmt_catch], "{source}");
        assert_eq!(program.next().unwrap().as_rule(), Rule::EOI);
        assert!(program.next().is_none());
    }
}

#[test]
fn rejects_missing_or_malformed_catch_handlers() {
    for source in [
        "Try {}",
        "Try { |x| = |1| }",
        "Catch {}",
        "Try {} Catch",
        "Try {} Catch {",
        "Try {} Catch {} Catch {}",
        "Try { Try {} } Catch {}",
        "Try {} Catch { Try {} }",
    ] {
        assert!(BWParser::parse(Rule::botwork, source).is_err(), "{source}");
    }
}

#[test]
fn malformed_collection_paths_remain_syntax_errors() {
    for path in ["m.", "m..a", "items.-1", "m.[0]", "m.0a"] {
        let source = format!("|answer| = |{path}|");
        assert!(BWParser::parse(Rule::botwork, &source).is_err(), "{source}");
    }
}

#[test]
fn incomplete_power_and_unary_expressions_are_syntax_errors() {
    for expression in [
        "2 ^", "^ 2", "2 ^^ 3", "2 ^ -", "2 ^ +3", "2 ^ (3", "-", "!", "--", "!!", "2 ^ ()",
    ] {
        let source = format!("|answer| = |{expression}|");
        assert!(BWParser::parse(Rule::botwork, &source).is_err(), "{source}");
    }
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
fn integer_overflow_returns_arithmetic_errors() {
    for (operator, a, b) in [
        (Rule::plus, i32::MAX, 1),
        (Rule::minus, i32::MIN, 1),
        (Rule::multiply, i32::MAX, 2),
        (Rule::multiply, i32::MIN, -1),
        (Rule::exponent, 2, 31),
    ] {
        assert!(
            matches!(
                operator.operate_binary(Literal::Int(a), Literal::Int(b)),
                Err(BWErr::ArithmeticError(_))
            ),
            "{a} {operator:?} {b}"
        );
    }
    assert!(matches!(
        Rule::minus.operate_unary(Literal::Int(i32::MIN)),
        Err(BWErr::ArithmeticError(_))
    ));
}

#[test]
fn division_and_remainder_reject_integer_and_signed_float_zero() {
    for operator in [Rule::divide, Rule::modulus] {
        for numerator in [Literal::Int(7), Literal::Float(7.0)] {
            for divisor in [Literal::Int(0), Literal::Float(0.0), Literal::Float(-0.0)] {
                assert!(
                    matches!(
                        operator.operate_binary(numerator.clone(), divisor),
                        Err(BWErr::ArithmeticError(_))
                    ),
                    "{operator:?}"
                );
            }
        }
    }
}

#[test]
fn representable_integer_boundaries_and_large_exponents_succeed() {
    for (operator, a, b, expected) in [
        (Rule::plus, i32::MAX, 0, i32::MAX),
        (Rule::minus, i32::MIN, 0, i32::MIN),
        (Rule::multiply, i32::MIN, 1, i32::MIN),
        (Rule::modulus, i32::MIN, -1, 0),
        (Rule::modulus, -7, 3, -1),
        (Rule::modulus, 7, -3, 1),
        (Rule::exponent, -2, 31, i32::MIN),
        (Rule::exponent, 0, 0, 1),
        (Rule::exponent, 0, i32::MAX, 0),
        (Rule::exponent, 1, i32::MAX, 1),
        (Rule::exponent, -1, i32::MAX, -1),
    ] {
        let result = operator.operate_binary(Literal::Int(a), Literal::Int(b));
        assert!(
            matches!(result, Ok(Literal::Int(value)) if value == expected),
            "{a} {operator:?} {b}: {result:?}"
        );
    }
    assert!(matches!(
        Rule::divide.operate_binary(Literal::Int(i32::MIN), Literal::Int(-1)),
        Ok(Literal::Float(2147483648.0))
    ));
}

#[test]
fn nonfinite_float_operands_and_results_are_arithmetic_errors() {
    for value in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
        assert!(matches!(
            Rule::plus.operate_binary(Literal::Float(value), Literal::Int(1)),
            Err(BWErr::ArithmeticError(_))
        ));
        assert!(matches!(
            Rule::multiply.operate_binary(Literal::Int(1), Literal::Float(value)),
            Err(BWErr::ArithmeticError(_))
        ));
        assert!(matches!(
            Rule::minus.operate_unary(Literal::Float(value)),
            Err(BWErr::ArithmeticError(_))
        ));
    }
    for (operator, a, b) in [
        (Rule::plus, f32::MAX, f32::MAX),
        (Rule::minus, -f32::MAX, f32::MAX),
        (Rule::multiply, f32::MAX, 2.0),
        (Rule::divide, f32::MAX, 0.5),
    ] {
        assert!(
            matches!(
                operator.operate_binary(Literal::Float(a), Literal::Float(b)),
                Err(BWErr::ArithmeticError(_))
            ),
            "{operator:?}"
        );
    }
    assert!(matches!(
        Rule::exponent.operate_binary(Literal::Float(f32::MAX), Literal::Int(2)),
        Err(BWErr::ArithmeticError(_))
    ));
}

#[test]
fn incompatible_types_take_priority_over_nonfinite_host_values() {
    assert!(matches!(
        Rule::logical_not.operate_unary(Literal::Float(f32::NAN)),
        Err(BWErr::OperationIncompatibleError(_))
    ));
    for (operator, lhs, rhs) in [
        (
            Rule::plus,
            Literal::Bool(true),
            Literal::Float(f32::INFINITY),
        ),
        (Rule::exponent, Literal::Int(4), Literal::Float(f32::NAN)),
        (
            Rule::equal,
            Literal::String("x".into()),
            Literal::Float(f32::NAN),
        ),
    ] {
        assert!(matches!(
            operator.operate_binary(lhs, rhs),
            Err(BWErr::OperationIncompatibleError(_))
        ));
    }
    assert!(matches!(
        Rule::less_than.operate_binary(Literal::Float(f32::NAN), Literal::Int(1)),
        Err(BWErr::ArithmeticError(_))
    ));
}

#[test]
fn integer_exponents_preserve_parity_reciprocals_and_subnormal_results() {
    for (base, exponent, expected) in [
        (Literal::Int(2), -3, 0.125),
        (Literal::Float(-1.0), 16777217, -1.0),
        (Literal::Float(1.0), i32::MIN, 1.0),
        (Literal::Int(-1), i32::MIN, 1.0),
        (Literal::Int(2), -149, f32::from_bits(1)),
        (Literal::Int(2), -150, 0.0),
        (Literal::Float(0.0), 0, 1.0),
    ] {
        let result = Rule::exponent.operate_binary(base.clone(), Literal::Int(exponent));
        assert!(
            matches!(result, Ok(Literal::Float(value)) if value == expected),
            "{base:?} ^ {exponent}: {result:?}"
        );
    }
    for base in [Literal::Int(0), Literal::Float(-0.0)] {
        assert!(matches!(
            Rule::exponent.operate_binary(base, Literal::Int(-1)),
            Err(BWErr::ArithmeticError(_))
        ));
    }
    assert!(matches!(
        Rule::exponent.operate_binary(Literal::Int(4), Literal::Float(0.5)),
        Err(BWErr::OperationIncompatibleError(_))
    ));
}

#[test]
fn finite_float_rounding_and_underflow_remain_valid() {
    assert!(matches!(
        Rule::divide.operate_binary(Literal::Float(f32::from_bits(1)), Literal::Int(2)),
        Ok(Literal::Float(0.0))
    ));
    assert!(matches!(
        Rule::plus.operate_binary(Literal::Int(16777217), Literal::Float(0.0)),
        Ok(Literal::Float(16777216.0))
    ));
    assert!(
        matches!(Rule::plus.operate_binary(Literal::Float(f32::MAX), Literal::Int(1)),
        Ok(Literal::Float(value)) if value == f32::MAX)
    );
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
