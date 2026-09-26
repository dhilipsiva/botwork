use super::*;

fn assigned_expression(source: &str) -> Expr {
    let program = Program::parse("expression.botwork", &format!("|answer| = |{source}|")).unwrap();
    let StatementKind::Assign {
        value: AssignmentValue::Expression(expression),
        ..
    } = &program.statements[0].kind
    else {
        panic!("expected expression assignment");
    };
    expression.clone()
}

fn shape(expression: &Expr) -> String {
    match &expression.kind {
        ExprKind::Integer(text) | ExprKind::Float(text) | ExprKind::Variable(text) => text.clone(),
        ExprKind::Bool(value) => value.to_string(),
        ExprKind::Unary {
            operator, operand, ..
        } => format!("({operator:?} {})", shape(operand)),
        ExprKind::Binary {
            operator,
            left,
            right,
            ..
        } => format!("({operator:?} {} {})", shape(left), shape(right)),
        kind => panic!("unexpected shape: {kind:?}"),
    }
}

#[test]
fn program_owns_source_and_cloned_definitions_share_their_body() {
    let program = {
        let source =
            String::from("Double |value| {\n Return |value * 2|\n}\n|answer| = Double |3|");
        Program::parse("owned.botwork", &source).unwrap()
    };
    assert_eq!(program.source.name(), "owned.botwork");
    assert!(program.source.text().ends_with("Double |3|"));
    let StatementKind::Define(definition) = &program.statements[0].kind else {
        panic!("expected definition");
    };
    let cloned = program.clone();
    let StatementKind::Define(cloned_definition) = &cloned.statements[0].kind else {
        panic!("expected cloned definition");
    };
    assert!(Arc::ptr_eq(definition, cloned_definition));
    assert!(Arc::ptr_eq(&program.source, definition.body.span.source()));
    assert_eq!(definition.body.span.text(), "{\n Return |value * 2|\n}");
    assert_eq!(definition.parameters[0].text, "value");
    assert_eq!(definition.parameters[0].span.text(), "value");
    assert_eq!(definition.span.text(), program.statements[0].span.text());
    assert_eq!(definition.body.statements[0].kind_name(), "return");
}

#[test]
fn spans_keep_original_bytes_unicode_columns_crlf_tabs_and_nested_locations() {
    let source = "# heading\r\n\t|café| = |\"é\" + \"🙂\"|\r\nOuter |value| {\r\n\tInner {\r\n\t\tLog |value|\r\n\t}\r\n}\r\n";
    let program = Program::parse("unicode.botwork", source).unwrap();
    let first = &program.statements[0];
    assert_eq!(first.span.line_column(), (2, 2));
    let StatementKind::Assign {
        name,
        value: AssignmentValue::Expression(expression),
    } = &first.kind
    else {
        panic!("expected assignment");
    };
    assert_eq!(name.span.line_column(), (2, 3));
    assert_eq!(name.span.start(), source.find("café").unwrap());
    assert_eq!(name.span.end() - name.span.start(), "café".len());
    assert_eq!(expression.span.line_column(), (2, 12));
    let ExprKind::Binary {
        operator_span,
        right,
        ..
    } = &expression.kind
    else {
        panic!("expected addition");
    };
    assert_eq!(operator_span.text(), "+");
    assert_eq!(operator_span.line_column(), (2, 16));
    assert_eq!(right.span.line_column(), (2, 18));
    assert_eq!(right.span.text(), "\"🙂\"");
    let StatementKind::Define(outer) = &program.statements[1].kind else {
        panic!("expected outer definition");
    };
    let StatementKind::Define(inner) = &outer.body.statements[0].kind else {
        panic!("expected inner definition");
    };
    assert_eq!(inner.span.line_column(), (4, 2));
    assert_eq!(inner.body.span.line_column(), (4, 8));
    let StatementKind::Invoke(call) = &inner.body.statements[0].kind else {
        panic!("expected nested call");
    };
    assert_eq!(call.span.line_column(), (5, 3));
    assert_eq!(call.arguments[0].span.line_column(), (5, 8));
    assert!(Arc::ptr_eq(
        &program.source,
        call.arguments[0].span.source()
    ));
}

#[test]
fn all_statement_forms_lower_without_evaluating_or_validating_control_placement() {
    let program = Program::parse(
        "statements.botwork",
        "|x| = |1|\n|y| = Do work |x|\nDo work |input| { Return |input| }\n\
         If |false| {} Else If |true| {} Else {}\n\
         For |item| In |[1]| { Break\n Continue }\nWhile |false| {}\n\
         Try {} Catch { Return }\nReturn |missing|\nBreak\nContinue\nDo work |2|",
    )
    .unwrap();
    assert_eq!(
        program
            .statements
            .iter()
            .map(Statement::kind_name)
            .collect::<Vec<_>>(),
        [
            "assignment",
            "assignment",
            "definition",
            "if",
            "for",
            "while",
            "try",
            "return",
            "break",
            "continue",
            "call"
        ]
    );
    assert!(matches!(
        &program.statements[1].kind,
        StatementKind::Assign {
            value: AssignmentValue::Call(_),
            ..
        }
    ));
    let StatementKind::If {
        else_branch: Some(ElseBranch::If(nested)),
        ..
    } = &program.statements[3].kind
    else {
        panic!("expected Else If");
    };
    assert!(matches!(
        &nested.kind,
        StatementKind::If {
            else_branch: Some(ElseBranch::Block(_)),
            ..
        }
    ));
    let StatementKind::For { binding, body, .. } = &program.statements[4].kind else {
        panic!("expected For");
    };
    assert_eq!(binding.text, "item");
    assert_eq!(binding.span.text(), "item");
    assert!(matches!(body.statements[0].kind, StatementKind::Break));
    assert!(matches!(body.statements[1].kind, StatementKind::Continue));
    let StatementKind::Try { body, handler } = &program.statements[6].kind else {
        panic!("expected Try");
    };
    assert!(body.statements.is_empty());
    assert_eq!(handler.span.text(), "{ Return }");
    assert!(matches!(
        handler.statements[0].kind,
        StatementKind::Return(None)
    ));
}

#[test]
fn expressions_preserve_precedence_association_and_unevaluated_operands() {
    for (source, expected) in [
        (
            "1 + 2 * 3 == 7 and true or false",
            "(Or (And (Equal (Add 1 (Multiply 2 3)) 7) true) false)",
        ),
        ("20 - 5 - 2", "(Subtract (Subtract 20 5) 2)"),
        ("2 ^ 3 ^ 2", "(Power 2 (Power 3 2))"),
        ("(2 ^ 3) ^ 2", "(Power (Power 2 3) 2)"),
        ("-2 ^ 2", "(Negate (Power 2 2))"),
        ("(-2) ^ 2", "(Power (Negate 2) 2)"),
        ("2 ^ -2 ^ 2", "(Power 2 (Negate (Power 2 2)))"),
        ("!!true", "(Not (Not true))"),
        ("false and missing", "(And false missing)"),
    ] {
        assert_eq!(shape(&assigned_expression(source)), expected, "{source}");
    }
    let expression = assigned_expression("-(1 + 2)");
    let ExprKind::Unary {
        operator_span,
        operand,
        ..
    } = &expression.kind
    else {
        panic!("expected unary expression");
    };
    assert_eq!(operator_span.text(), "-");
    assert_eq!(operand.span.text(), "(1 + 2)");
}

#[test]
fn binary_spans_include_parentheses_on_both_operands() {
    let text = "(2 + 3) * (4 - 1)";
    let expression = assigned_expression(text);
    assert_eq!(expression.span.text(), text);
    let ExprKind::Binary {
        left,
        right,
        operator_span,
        ..
    } = &expression.kind
    else {
        panic!("expected multiplication");
    };
    assert_eq!(left.span.text().trim_end(), "(2 + 3)");
    assert_eq!(right.span.text(), "(4 - 1)");
    assert_eq!(operator_span.text(), "*");
    assert_eq!(left.span.start(), expression.span.start());
    assert_eq!(right.span.end(), expression.span.end());
}

#[test]
fn numeric_conversion_remains_deferred_and_strings_decode_once() {
    let huge_integer = "9999999999999999999999999999999999999999999999";
    let huge_float = format!("{huge_integer}.0");
    assert!(
        matches!(assigned_expression(huge_integer).kind, ExprKind::Integer(text) if text == huge_integer)
    );
    assert!(
        matches!(assigned_expression(&huge_float).kind, ExprKind::Float(text) if text == huge_float)
    );
    let string = assigned_expression(r#""é\n\"\\n""#);
    assert!(matches!(string.kind, ExprKind::String(text) if text == "é\n\"\\n"));
    let access = assigned_expression("missing.items.9999999999999999999999999999");
    assert!(
        matches!(access.kind, ExprKind::Access(text) if text == "missing.items.9999999999999999999999999999")
    );
}

#[test]
fn collection_entries_keep_source_order_duplicate_keys_and_key_spans() {
    let expression = assigned_expression("{z: [missing, 2], café: true, z: 3}");
    let ExprKind::Map(entries) = &expression.kind else {
        panic!("expected map");
    };
    assert_eq!(
        entries
            .iter()
            .map(|(key, _)| key.text.as_str())
            .collect::<Vec<_>>(),
        ["z", "café", "z"]
    );
    assert_eq!(entries[1].0.span.text(), "café");
    let ExprKind::Array(items) = &entries[0].1.kind else {
        panic!("expected array");
    };
    assert_eq!(shape(&items[0]), "missing");
    assert_eq!(shape(&items[1]), "2");
}

#[test]
fn signatures_preserve_existing_space_case_and_tab_rules() {
    let program = Program::parse(
        "names.botwork",
        "Pair |x| WITH |y| {}\nPAIR   |1| with|2|\nTab\tname {}\nTab\tname\n",
    )
    .unwrap();
    let StatementKind::Define(definition) = &program.statements[0].kind else {
        panic!("expected definition");
    };
    let StatementKind::Invoke(call) = &program.statements[1].kind else {
        panic!("expected call");
    };
    assert_eq!(definition.signature, "pair|param|with|param|");
    assert_eq!(call.signature, definition.signature);
    assert_eq!(
        definition
            .parameters
            .iter()
            .map(|name| name.text.as_str())
            .collect::<Vec<_>>(),
        ["x", "y"]
    );
    assert_eq!(call.arguments.len(), 2);
    let StatementKind::Define(tab_definition) = &program.statements[2].kind else {
        panic!("expected tab definition");
    };
    let StatementKind::Invoke(tab_call) = &program.statements[3].kind else {
        panic!("expected tab call");
    };
    assert_eq!(tab_definition.signature, "tab\tname");
    assert_eq!(tab_call.signature, tab_definition.signature);
}

#[test]
fn lowering_a_nested_pair_keeps_full_original_input_and_offsets() {
    let source = "# prefix\nOuter {\n |value| = |12|\n}";
    let nested = BWParser::parse(Rule::botwork, source)
        .unwrap()
        .next()
        .unwrap()
        .into_inner()
        .nth(1)
        .unwrap()
        .into_inner()
        .next()
        .unwrap();
    let node = from_pair(nested).unwrap();
    let Node::Statement(statement) = node else {
        panic!("expected statement");
    };
    assert_eq!(statement.span.source().text(), source);
    assert_eq!(statement.span.source().name(), "<input>");
    assert_eq!(statement.span.line_column(), (3, 2));
    assert_eq!(statement.span.text(), "|value| = |12|");
}

#[test]
fn parser_rejects_syntax_errors_before_producing_a_program() {
    assert!(Program::parse("bad.botwork", "Log |1|\nTry {}").is_err());
    assert!(Program::parse("bad.botwork", "|answer| = |2 ^|").is_err());
    assert!(Program::parse("empty.botwork", "# empty\n")
        .unwrap()
        .statements
        .is_empty());
}

#[test]
fn operator_mapping_matches_the_existing_numeric_and_type_contract() {
    for operator in [
        BinaryOp::Add,
        BinaryOp::Subtract,
        BinaryOp::Multiply,
        BinaryOp::Divide,
        BinaryOp::Remainder,
        BinaryOp::Power,
        BinaryOp::Less,
        BinaryOp::LessEqual,
        BinaryOp::Greater,
        BinaryOp::GreaterEqual,
        BinaryOp::Equal,
        BinaryOp::NotEqual,
        BinaryOp::And,
        BinaryOp::Or,
    ] {
        assert_eq!(BinaryOp::from_rule(operator.to_rule()).unwrap(), operator);
    }
    for operator in [UnaryOp::Negate, UnaryOp::Not] {
        assert_eq!(UnaryOp::from_rule(operator.to_rule()).unwrap(), operator);
    }
}
