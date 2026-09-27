use super::*;

#[test]
fn rejected_validation_evidence_never_scans_source_coordinates_and_releases_owners() {
    let owner = Arc::new(SourceFile {
        name: "é".into(),
        text: String::new(),
    });
    let weak = Arc::downgrade(&owner);
    // Invalid private offsets are a sentinel: coordinate formatting would panic.
    let span = Span {
        source: owner.clone(),
        start: usize::MAX,
        end: usize::MAX,
    };
    let limits = DiagnosticLimits {
        source_bytes: 0,
        ..DiagnosticLimits::default()
    };
    let control = ValidationFailure::Control {
        span: &span,
        message: "invalid placement",
    }
    .diagnostic(&limits, std::iter::empty());
    let duplicate = ValidationFailure::DuplicateParameter {
        name: "é",
        original: &span,
        duplicate: &span,
    }
    .diagnostic(&limits, std::iter::empty());
    assert_eq!(
        control.causes[0].omissions.as_ref().unwrap().detail_fields,
        1
    );
    assert_eq!(
        duplicate.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .detail_fields,
        2
    );
    let BWErr::ControlFlowError(message) = control.causes[0].error.as_ref() else {
        panic!("control")
    };
    assert_eq!(
        message,
        &format!(
            "é:[byte {}; coordinates omitted]: invalid placement",
            usize::MAX
        )
    );
    drop(span);
    drop(owner);
    assert!(weak.upgrade().is_none());
}

#[test]
fn program_and_native_parameter_validation_use_exact_default_text_quotas() {
    let maximum = DiagnosticLimits::default().text_bytes;
    let fixed = "source".len() + "first parameter".len() + "x".len() + ":1:4".len() + ":1:8".len();
    assert_eq!((maximum - fixed) % 2, 0);
    let name = "n".repeat((maximum - fixed) / 2);
    for native in [false, true] {
        let error = if native {
            native_signature(&name, "R |x| |x|").err().unwrap()
        } else {
            Program::parse_detailed(&name, "R |x| |x| {}").unwrap_err()
        };
        assert_eq!(
            error.code(),
            super::super::diagnostic::DiagnosticCode::DuplicateParameter
        );
        assert_eq!(
            DiagnosticLimits::default()
                .check(&error)
                .unwrap()
                .text_bytes,
            maximum
        );
        let longer = format!("{name}é");
        let error = if native {
            native_signature(&longer, "R |x| |x|").err().unwrap()
        } else {
            Program::parse_detailed(&longer, "R |x| |x| {}").unwrap_err()
        };
        assert_eq!(
            error.code(),
            super::super::diagnostic::DiagnosticCode::ResourceLimit
        );
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 2);
        assert!(
            error.causes[0]
                .omissions
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap()
                .file_truncated
        );
        assert!(error.causes[0].span.is_none());
    }
}

#[test]
fn deferred_locations_preserve_coordinates_and_byte_summaries_never_index_source_text() {
    let program = Program::parse("é.botwork", "#🦀\r\n\tRead {}").unwrap();
    let span = &program.statements[0].span;
    assert_eq!(format!("{}", span.location_display()), span.location());
    assert_eq!(
        format!("{:#}", span.location_display()),
        format!("é.botwork:[byte {}; coordinates omitted]", span.start())
    );
    // Deliberately invalid private offsets prove alternate formatting never
    // indexes source contents. Public parsers cannot create this span.
    let sentinel = Span {
        source: Arc::new(SourceFile {
            name: "file".into(),
            text: String::new(),
        }),
        start: usize::MAX,
        end: usize::MAX,
    };
    assert_eq!(
        format!("{:#}", sentinel.location_display()),
        format!("file:[byte {}; coordinates omitted]", usize::MAX)
    );
}

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
    // Exercise raw lowering independently of the public program validator.
    let source = Arc::new(SourceFile {
        name: "statements.botwork".into(),
        text: "|x| = |1|\n|y| = Do work |x|\nDo work |input| { Return |input| }\n\
         If |false| {} Else If |true| {} Else {}\n\
         For |item| In |[1]| { Break\n Continue }\nWhile |false| {}\n\
         Try {} Catch { Return }\nReturn |missing|\nBreak\nContinue\nDo work |2|"
            .into(),
    });
    let statements = BWParser::parse(Rule::botwork, &source.text)
        .unwrap()
        .filter(|pair| pair.as_rule() != Rule::EOI)
        .map(|pair| statement(pair, &source).unwrap())
        .collect();
    let program = Program { source, statements };
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
    let StatementKind::Try { body, handler, .. } = &program.statements[6].kind else {
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
fn control_validation_rejects_invalid_placement_even_when_unreachable() {
    for control in ["Return", "Return |missing|", "Break", "Continue"] {
        for source in [
            control.to_owned(),
            format!("If |false| {{ {control} }}"),
            format!("If |true| {{}} Else {{ {control} }}"),
            format!("If |true| {{}} Else If |false| {{ {control} }}"),
            format!("Try {{}} Catch {{ {control} }}"),
            format!("Try {{ {control} }} Catch {{}}"),
        ] {
            let result = Program::parse("invalid.botwork", &source);
            assert!(
                matches!(result, Err(BWErr::ControlFlowError(_))),
                "{source}: {result:?}"
            );
        }
    }
    for source in [
        "For |item| In |[]| { Return }",
        "While |false| { Return }",
        "Unused { Break }",
        "Unused { Continue }",
        "Unused {\n Return |1|\n Break\n}",
        "Unused {\n Return |1|\n Continue\n}",
        "For |item| In |[]| { Unused { Break } }",
        "While |false| { Unused { Continue } }",
        "Outer { While |false| { Inner { Break } } }",
        "For |item| In |[]| {}\nBreak",
        "While |false| {}\nContinue",
        "Unused { Return }\nReturn",
    ] {
        let result = Program::parse("invalid.botwork", source);
        assert!(
            matches!(result, Err(BWErr::ControlFlowError(_))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn control_validation_accepts_only_lexically_enclosing_targets() {
    for source in [
        "Valid { Return }",
        "Valid {\n Return |1|\n Return |missing|\n}",
        "For |item| In |[]| { Break\nContinue }",
        "While |false| { Continue\nBreak }",
        "Valid { For |item| In |[]| { Return } }",
        "Valid { While |false| { Return } }",
        "Valid { If |false| { Return } Else If |false| { Return } Else { Return } }",
        "Valid { Try { Return } Catch { Return } }",
        "While |false| { Try { Break } Catch { Continue } }",
        "For |item| In |[]| { If |false| { Continue } Else { Break } }",
        "While |false| { Inner { Return }\nBreak }",
        "Outer {\n Inner { While |false| { Break }\nReturn }\nReturn\n}",
        "While |false| { Inner {}\nContinue }",
        "While |false| { While |false| {}\nBreak }",
    ] {
        let result = Program::parse("valid.botwork", source);
        assert!(result.is_ok(), "{source}: {result:?}");
    }
}

#[test]
fn control_validation_reports_the_first_invalid_statement_with_its_location() {
    let source = "# café\r\nIf |false| {\r\n\t|é| = |\"🙂\"|\r\n\tContinue\r\n}\r\nBreak";
    let error = Program::parse("unicode.botwork", source).unwrap_err();
    assert!(matches!(error, BWErr::ControlFlowError(message)
        if message == "unicode.botwork:4:2: Continue requires an enclosing loop in the same invocation"));
    let error = Program::parse("inline.botwork", "|é| = |\"🙂\"| Return").unwrap_err();
    assert!(matches!(error, BWErr::ControlFlowError(message)
        if message == "inline.botwork:1:13: Return requires a custom-statement body"));
}

#[test]
fn control_validation_does_not_evaluate_expressions_or_replace_syntax_errors() {
    for source in ["Return\n|broken| = |2 ^|", "Unused { Break }\nTry {}"] {
        assert!(matches!(
            Program::parse("bad.botwork", source),
            Err(BWErr::ParsingError(_))
        ));
    }
    for source in [
        "Valid { Return |99999999999999999999999999999999| }",
        "While |missing| { Break }",
        "For |item| In |missing| { Continue }",
        "Unknown |missing|",
    ] {
        assert!(Program::parse("valid.botwork", source).is_ok(), "{source}");
    }
    let error = Program::parse("invalid.botwork", "|value| = |missing|\nReturn").unwrap_err();
    assert!(matches!(error, BWErr::ControlFlowError(_)));
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
    assert!(matches!(access.kind, ExprKind::Access { base, segments }
            if matches!(&base.kind, ExprKind::Variable(name) if name == "missing")
            && matches!(&segments[0], AccessSegment::Literal(name) if name.text == "items")
            && matches!(&segments[1], AccessSegment::Literal(name) if name.text == "9999999999999999999999999999")));
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
fn collection_paths_retain_parsed_segments_and_their_original_spans() {
    let expression = assigned_expression("data ### ignored.dot ### . café . 00");
    let ExprKind::Access { base, segments } = &expression.kind else {
        panic!("access expression");
    };
    assert!(matches!(&base.kind, ExprKind::Variable(name) if name == "data"));
    assert_eq!(base.span.text(), "data");
    assert_eq!(segments.len(), 2);
    for (segment, expected) in segments.iter().zip(["café", "00"]) {
        let AccessSegment::Literal(name) = segment else {
            panic!("literal segment")
        };
        assert_eq!(name.text, expected);
        assert_eq!(name.span.text(), expected);
    }
    assert!(expression.span.text().contains("ignored.dot"));
}

#[test]
fn computed_access_preserves_base_bracket_and_nested_expression_spans() {
    let expression = assigned_expression("(data) ### outside ### [ café + 1 ].items[positions[0]]");
    let ExprKind::Access { base, segments } = &expression.kind else {
        panic!("access")
    };
    assert_eq!(base.span.text(), "(data)");
    assert!(matches!(&base.kind, ExprKind::Variable(name) if name == "data"));
    assert_eq!(segments.len(), 3);
    let AccessSegment::Computed { span, index } = &segments[0] else {
        panic!("computed")
    };
    assert_eq!(span.text(), "[ café + 1 ]");
    assert_eq!(index.span.text().trim(), "café + 1");
    assert!(Arc::ptr_eq(span.source(), expression.span.source()));
    let ExprKind::Binary { left, .. } = &index.kind else {
        panic!("binary index")
    };
    assert_eq!(left.span.text().trim(), "café");
    assert_eq!(
        &span.source().text()[left.span.start()..left.span.end()],
        left.span.text()
    );
    assert!(matches!(&segments[1], AccessSegment::Literal(name) if name.text == "items"));
    let AccessSegment::Computed { span, index } = &segments[2] else {
        panic!("computed")
    };
    assert_eq!(span.text(), "[positions[0]]");
    assert!(matches!(&index.kind, ExprKind::Access { .. }));
}

#[test]
fn quoted_map_key_spans_retain_source_while_text_is_decoded_once() {
    let expression = assigned_expression(r#"{"é\n": 1, "\\n": 2, "": 3, plain: 4}"#);
    let ExprKind::Map(entries) = expression.kind else {
        panic!("map")
    };
    assert_eq!(
        entries
            .iter()
            .map(|(key, _)| key.text.as_str())
            .collect::<Vec<_>>(),
        ["é\n", "\\n", "", "plain"]
    );
    assert_eq!(entries[0].0.span.text(), r#""é\n""#);
    assert_eq!(entries[1].0.span.text(), r#""\\n""#);
    assert_eq!(entries[2].0.span.text(), "\"\"");
}

#[test]
fn malformed_computed_reads_and_all_indexed_assignments_are_syntax_errors() {
    for source in [
        "|answer| = |data[]|",
        "|answer| = |data[0|",
        "|answer| = |data[0, 1]|",
        "|answer| = |data[1 + ]|",
        "|answer| = |data.[0]|",
        "|answer| = |data[0].|",
        "|answer| = |false and data[]|",
        "|answer| = |true or data[]|",
        "|data[0]| = |7|",
        "|data.item[0]| = |7|",
        r#"|data["key"]| = |7|"#,
        r#"|answer| = |{"bad\t": 1}|"#,
        "|answer| = |{[key]: 1}|",
    ] {
        assert!(
            matches!(
                Program::parse("invalid.botwork", source),
                Err(BWErr::ParsingError(_))
            ),
            "{source}"
        );
    }
}

#[test]
fn signatures_normalize_spaces_tabs_and_case_with_positional_parameters() {
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
    assert_eq!(tab_definition.signature, "tabname");
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
