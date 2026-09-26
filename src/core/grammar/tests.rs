use super::{BWErr, BWParser, Literal, Operate, Rule};
use pest::Parser;

#[test]
fn identifier_recognition_matches_pest_for_reserved_names_ascii_and_unicode_boundaries() {
    let check = |name: &str| {
        let parsed = BWParser::parse(Rule::ident, name)
            .ok()
            .and_then(|mut pairs| pairs.next())
            .is_some_and(|pair| pair.as_span().start() == 0 && pair.as_span().end() == name.len());
        assert_eq!(super::is_identifier(name), parsed, "identifier {name:?}");
    };
    for name in [
        "",
        "_",
        "true",
        "false",
        "and",
        "or",
        "True",
        "FALSE",
        "And",
        "OR",
        "Return",
        "Import",
        "true_value",
        "false1",
        "and_then",
        "orElse",
        "a.b",
        "a b",
        " a",
        "a\n",
        "é",
        "e\u{301}",
        "தமிழ்",
        "变量",
    ] {
        check(name);
    }
    let characters = (0..=127)
        .map(char::from)
        .chain([
            'é',
            'λ',
            '中',
            'த',
            '\u{301}',
            '\u{b7}',
            '\u{200c}',
            '\u{200d}',
            '\u{feff}',
            '🦀',
            '\u{10400}',
            '\u{ff11}',
        ])
        .collect::<Vec<_>>();
    for first in &characters {
        check(&first.to_string());
        for reserved in ["true", "false", "and", "or"] {
            check(&format!("{reserved}{first}"));
        }
        for second in &characters {
            check(&format!("{first}{second}"));
        }
    }
}

#[test]
fn operator_error_formatting_occurs_only_after_value_checks_and_incompatibility() {
    use crate::core::{
        diagnostic::{Diagnostic, DiagnosticCode},
        value_limits::ValueLimits,
    };
    use std::cell::Cell;
    let calls = Cell::new(0);
    let fail = |message: std::fmt::Arguments<'_>| {
        calls.set(calls.get() + 1);
        Diagnostic::new(BWErr::OperationIncompatibleError(message.to_string()))
    };
    assert_eq!(
        Rule::plus
            .operate_binary_with_error(
                Literal::Int(1),
                Literal::Int(2),
                &ValueLimits::default(),
                fail
            )
            .unwrap()
            .to_string(),
        "3"
    );
    assert_eq!(
        Rule::equal
            .operate_binary_with_error(
                Literal::Array(vec![]),
                Literal::Bool(true),
                &ValueLimits::default(),
                fail
            )
            .unwrap()
            .to_string(),
        "false"
    );
    assert_eq!(
        Rule::minus
            .operate_unary_with_error(Literal::Int(1), &ValueLimits::default(), fail)
            .unwrap()
            .to_string(),
        "-1"
    );
    assert_eq!(
        Rule::plus
            .operate_binary_with_error(
                Literal::Float(f32::NAN),
                Literal::Int(1),
                &ValueLimits::default(),
                fail
            )
            .unwrap_err()
            .code(),
        DiagnosticCode::Arithmetic
    );
    assert_eq!(
        Rule::plus
            .operate_binary_with_error(
                Literal::None,
                Literal::None,
                &ValueLimits {
                    nodes: 0,
                    ..ValueLimits::default()
                },
                fail
            )
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(
        Rule::minus
            .operate_binary_with_error(
                Literal::Bool(true),
                Literal::Int(1),
                &ValueLimits::default(),
                fail
            )
            .unwrap_err()
            .code(),
        DiagnosticCode::IncompatibleType
    );
    assert_eq!(
        Rule::logical_not
            .operate_unary_with_error(Literal::Int(1), &ValueLimits::default(), fail)
            .unwrap_err()
            .code(),
        DiagnosticCode::IncompatibleType
    );
    assert_eq!(calls.get(), 2);
}

#[test]
fn unsupported_operator_rules_return_typed_errors_without_reaching_checked_branches() {
    for rule in [Rule::botwork, Rule::stmt_assign, Rule::literal] {
        assert!(matches!(
            rule.operate_binary(Literal::Int(1), Literal::Bool(true)),
            Err(BWErr::OperationIncompatibleError(_))
        ));
        assert!(matches!(
            rule.operate_unary(Literal::Int(1)),
            Err(BWErr::OperationIncompatibleError(_))
        ));
    }
}

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
fn expression_keywords_require_complete_identifier_tokens() {
    for name in ["true", "false", "and", "or"] {
        let source = format!("|{name}| = |7|");
        assert!(BWParser::parse(Rule::botwork, &source).is_err(), "{source}");
    }
    for expression in [
        "true andfalse",
        "false ortrue",
        "true and_false",
        "false or2",
        "true orδ",
        "true a### gap ###nd false",
    ] {
        let source = format!("|answer| = |{expression}|");
        assert!(BWParser::parse(Rule::botwork, &source).is_err(), "{source}");
    }
    for expression in [
        "true and false",
        "false or true",
        "(true)and(false)",
        "false or(true)",
        "true### gap ###and false",
    ] {
        let source = format!("|answer| = |{expression}|");
        assert!(BWParser::parse(Rule::botwork, &source).is_ok(), "{source}");
    }
}

#[test]
fn control_keyword_prefixes_are_single_custom_statements() {
    for name in [
        "Ifonly",
        "Elsewhere",
        "Format report",
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
        "F or report",
    ] {
        let call: Vec<_> = BWParser::parse(Rule::botwork, name).unwrap().collect();
        assert_eq!(call.len(), 2, "{name}");
        assert_eq!(call[0].as_rule(), Rule::stmt_invoke, "{name}");
        assert_eq!(call[0].as_str(), name, "{name}");
        let source = format!("{name} {{}}");
        let definition: Vec<_> = BWParser::parse(Rule::botwork, &source).unwrap().collect();
        assert_eq!(definition.len(), 2, "{source}");
        assert_eq!(definition[0].as_rule(), Rule::stmt_define, "{source}");
    }
}

#[test]
fn complete_control_keywords_preserve_their_statement_layout() {
    for (source, rule, children) in [
        (
            "iF|true|{}",
            Rule::stmt_if,
            vec![Rule::param_invoke, Rule::stmt_block],
        ),
        (
            "fOr|i|iN|[]|{}",
            Rule::stmt_for,
            vec![Rule::ident, Rule::param_invoke, Rule::stmt_block],
        ),
        (
            "wHiLe\t|false|{}",
            Rule::stmt_while,
            vec![Rule::param_invoke, Rule::stmt_block],
        ),
        ("bReAk", Rule::stmt_break, vec![]),
        ("Break# comment\n", Rule::stmt_break, vec![]),
        ("cOnTiNuE # comment\r\n", Rule::stmt_continue, vec![]),
        ("rEtUrN|1|", Rule::stmt_return, vec![Rule::param_invoke]),
        (
            "iF|false|{}eLsE iF|true|{}",
            Rule::stmt_if,
            vec![Rule::param_invoke, Rule::stmt_block, Rule::stmt_else],
        ),
        (
            "TrY### gap ###{}CaTcH{}",
            Rule::stmt_try,
            vec![Rule::stmt_block, Rule::stmt_catch],
        ),
    ] {
        let mut program = BWParser::parse(Rule::botwork, source).unwrap();
        let statement = program.next().unwrap();
        assert_eq!(statement.as_rule(), rule, "{source}");
        assert_eq!(
            statement
                .into_inner()
                .map(|pair| pair.as_rule())
                .collect::<Vec<_>>(),
            children,
            "{source}"
        );
        assert_eq!(program.next().unwrap().as_rule(), Rule::EOI, "{source}");
        assert!(program.next().is_none(), "{source}");
    }
    for source in [
        "For |i| Inside |[]| {}",
        "For |i| in_ |[]| {}",
        "For |i| in! |[]| {}",
        "For |i| i n |[]| {}",
        "Try {} Catchall {}",
    ] {
        assert!(BWParser::parse(Rule::botwork, source).is_err(), "{source}");
    }
    let mut program = BWParser::parse(Rule::botwork, "Example {Return}").unwrap();
    let block = program.next().unwrap().into_inner().nth(1).unwrap();
    assert_eq!(block.as_rule(), Rule::stmt_block);
    assert_eq!(
        block.into_inner().next().unwrap().as_rule(),
        Rule::stmt_return
    );
}

#[test]
fn value_level_boolean_operators_require_both_values_to_be_booleans() {
    for (operator, left) in [(Rule::logical_and, false), (Rule::logical_or, true)] {
        assert!(matches!(
            operator.operate_binary(Literal::Bool(left), Literal::Int(1)),
            Err(BWErr::OperationIncompatibleError(_))
        ));
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
            Rule::less_than,
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
fn mixed_numeric_comparisons_preserve_integer_precision_at_float_boundaries() {
    use std::cmp::Ordering::{Equal, Greater, Less};
    for (left, right, ordering) in [
        (Literal::Int(16777217), Literal::Float(16777216.0), Greater),
        (Literal::Int(-16777217), Literal::Float(-16777216.0), Less),
        (Literal::Int(i32::MAX), Literal::Float(2147483648.0), Less),
        (Literal::Int(i32::MIN), Literal::Float(-2147483648.0), Equal),
        (
            Literal::Int(i32::MIN + 1),
            Literal::Float(-2147483648.0),
            Greater,
        ),
        (Literal::Int(16777216), Literal::Float(16777216.0), Equal),
        (Literal::Int(0), Literal::Float(-0.0), Equal),
        (Literal::Int(0), Literal::Float(f32::from_bits(1)), Less),
        (Literal::Int(0), Literal::Float(-f32::from_bits(1)), Greater),
        (Literal::Float(f32::MAX), Literal::Int(i32::MAX), Greater),
        (Literal::Float(-f32::MAX), Literal::Int(i32::MIN), Less),
    ] {
        for (left, right, ordering) in [
            (left.clone(), right.clone(), ordering),
            (right, left, ordering.reverse()),
        ] {
            for (operator, expected) in [
                (Rule::less_than, ordering == Less),
                (Rule::less_than_or_equal, ordering != Greater),
                (Rule::greater_than, ordering == Greater),
                (Rule::greater_than_or_equal, ordering != Less),
                (Rule::equal, ordering == Equal),
                (Rule::not_equal, ordering != Equal),
            ] {
                let result = operator.operate_binary(left.clone(), right.clone());
                assert!(
                    matches!(result, Ok(Literal::Bool(value)) if value == expected),
                    "{left:?} {operator:?} {right:?}: {result:?}"
                );
            }
        }
    }
}

#[test]
fn equality_is_defined_for_every_finite_value_kind_without_coercion() {
    let values = [
        Literal::None,
        Literal::Bool(false),
        Literal::Int(0),
        Literal::Float(0.0),
        Literal::String("0".into()),
        Literal::Array(vec![]),
        Literal::Map(Default::default()),
    ];
    for (i, left) in values.iter().enumerate() {
        for (j, right) in values.iter().enumerate() {
            let expected = i == j || matches!((i, j), (2, 3) | (3, 2));
            for (operator, expected) in [(Rule::equal, expected), (Rule::not_equal, !expected)] {
                let result = operator.operate_binary(left.clone(), right.clone());
                assert!(
                    matches!(result, Ok(Literal::Bool(value)) if value == expected),
                    "{left:?} {operator:?} {right:?}: {result:?}"
                );
            }
        }
    }
}

#[test]
fn equality_rejects_nonfinite_values_even_in_unequal_or_nested_collections() {
    for number in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for invalid in [
            Literal::Float(number),
            Literal::Array(vec![Literal::Int(2), Literal::Float(number)]),
            Literal::Map(
                [(
                    "nested".into(),
                    Literal::Array(vec![Literal::Float(number)]),
                )]
                .into_iter()
                .collect(),
            ),
        ] {
            for other in [
                Literal::None,
                Literal::Array(vec![]),
                Literal::Array(vec![Literal::Int(1)]),
                invalid.clone(),
            ] {
                for (left, right) in [(invalid.clone(), other.clone()), (other, invalid.clone())] {
                    for operator in [Rule::equal, Rule::not_equal] {
                        assert!(
                            matches!(
                                operator.operate_binary(left.clone(), right.clone()),
                                Err(BWErr::ArithmeticError(_))
                            ),
                            "{left:?} {operator:?} {right:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn finite_value_equality_is_reflexive_symmetric_transitive_and_complementary() {
    let values = [
        Literal::None,
        Literal::Int(0),
        Literal::Float(-0.0),
        Literal::Float(0.0),
        Literal::Int(1),
        Literal::Float(1.0),
        Literal::Float(f32::from_bits(1)),
        Literal::Int(16777216),
        Literal::Float(16777216.0),
        Literal::Int(16777217),
        Literal::Bool(false),
        Literal::String("0".into()),
        Literal::Array(vec![Literal::Int(0)]),
        Literal::Array(vec![Literal::Float(-0.0)]),
        Literal::Map([("k".into(), Literal::Int(1))].into_iter().collect()),
        Literal::Map([("k".into(), Literal::Float(1.0))].into_iter().collect()),
    ];
    let mut equal = vec![vec![false; values.len()]; values.len()];
    for (i, left) in values.iter().enumerate() {
        for (j, right) in values.iter().enumerate() {
            let Literal::Bool(result) = Rule::equal
                .operate_binary(left.clone(), right.clone())
                .unwrap()
            else {
                panic!("equality must return a boolean")
            };
            equal[i][j] = result;
            assert!(
                matches!(Rule::not_equal.operate_binary(left.clone(), right.clone()),
                Ok(Literal::Bool(unequal)) if unequal != result)
            );
        }
    }
    for i in 0..values.len() {
        assert!(equal[i][i], "reflexivity at {i}");
        for j in 0..values.len() {
            assert_eq!(equal[i][j], equal[j][i], "symmetry at {i}, {j}");
            for k in 0..values.len() {
                if equal[i][j] && equal[j][k] {
                    assert!(equal[i][k], "transitivity at {i}, {j}, {k}");
                }
            }
        }
    }
}

#[test]
fn ordering_remains_numeric_only_and_rejects_nonfinite_numeric_operands() {
    for operator in [
        Rule::less_than,
        Rule::less_than_or_equal,
        Rule::greater_than,
        Rule::greater_than_or_equal,
    ] {
        for value in [
            Literal::None,
            Literal::Bool(false),
            Literal::String("0".into()),
            Literal::Array(vec![]),
            Literal::Map(Default::default()),
        ] {
            for (left, right) in [
                (value.clone(), Literal::Int(0)),
                (Literal::Int(0), value.clone()),
                (value.clone(), value),
            ] {
                assert!(matches!(
                    operator.operate_binary(left, right),
                    Err(BWErr::OperationIncompatibleError(_))
                ));
            }
        }
        for number in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for (left, right) in [
                (Literal::Float(number), Literal::Int(0)),
                (Literal::Int(0), Literal::Float(number)),
            ] {
                assert!(matches!(
                    operator.operate_binary(left, right),
                    Err(BWErr::ArithmeticError(_))
                ));
            }
        }
    }
}

#[test]
fn mixed_arithmetic_rounds_integer_operands_to_f32_before_the_operation() {
    for (operator, left, right, expected) in [
        (
            Rule::plus,
            Literal::Int(16777217),
            Literal::Float(1.0),
            16777216.0,
        ),
        (
            Rule::minus,
            Literal::Int(16777217),
            Literal::Float(1.0),
            16777215.0,
        ),
        (
            Rule::multiply,
            Literal::Int(16777217),
            Literal::Float(1.0),
            16777216.0,
        ),
        (
            Rule::divide,
            Literal::Int(16777217),
            Literal::Int(1),
            16777216.0,
        ),
        (
            Rule::modulus,
            Literal::Int(16777217),
            Literal::Float(2.0),
            0.0,
        ),
    ] {
        let result = operator.operate_binary(left.clone(), right.clone());
        assert!(
            matches!(result, Ok(Literal::Float(value)) if value == expected),
            "{left:?} {operator:?} {right:?}: {result:?}"
        );
    }
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
