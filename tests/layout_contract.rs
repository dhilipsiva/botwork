//! Whitespace, line boundaries, continuation, comments, and reserved delimiters.

use botwork::core::{
    ast::{AssignmentValue, ExprKind, Program, StatementKind},
    eval::{evaluate_program, Context},
    grammar::{BWErr, Literal},
};

fn evaluate(source: &str) -> Literal {
    let program = Program::parse("layout.botwork", source).expect("valid layout");
    evaluate_program(&program, &mut Context::default()).expect("successful execution")
}

#[test]
fn multiline_expressions_collections_and_access_work_with_lf_and_crlf() {
    let source = r#"|data| = |
    {
        "item list": [
            7,
            {value:
                2
                + 3 *
                4,
            },
        ],
        emptyArray: [
        ],
        emptyMap: {
        },
    }
|
|index| = |1|
|answer| = |
    data
    [
        "item list"
    ]
    [index]
    .
    value
    ^
    2
    + -
    1
|"#;
    for ending in ["\n", "\r\n"] {
        assert!(matches!(
            evaluate(&source.replace('\n', ending)),
            Literal::Int(195)
        ));
    }
}

#[test]
fn fixed_control_headers_braces_else_and_catch_allow_line_breaks() {
    let source = r#"Double |value|
{
    Return |
        value * 2
    |
}
|answer|
=
Double |3|
If
|false|
{
    |answer| = |missing|
}
# The nearest preceding If owns this Else.
Else
If |false| {}
Else
{
    For
    |item|
    In
    |[1, 2]|
    {
        |answer| = |answer + item|
    }
}
While
|answer < 10|
{
    |answer| = |answer + 1|
}
Try
{
    |answer| = |missing|
}
# The handler may start after blank lines or comments.

Catch
{
    |answer| = |answer + 1|
}
|result| = |answer|"#;
    for ending in ["\n", "\r\n"] {
        assert!(matches!(
            evaluate(&source.replace('\n', ending)),
            Literal::Int(11)
        ));
    }
}

#[test]
fn explicit_call_and_definition_continuations_preserve_signatures_and_arguments() {
    let source =
        "Combine |left| with \\\n    |right| \\\n    into a pair\n{ Return |[left, right]| }\n\
        |answer| = Combine |\n 1 +\n 2\n| with \\  \n\t|4| \\\n into a pair";
    for ending in ["\n", "\r\n"] {
        let source = source.replace('\n', ending);
        assert_eq!(evaluate(&source).to_string(), "[3, 4]");
        let program = Program::parse("continued.botwork", &source).unwrap();
        let StatementKind::Define(definition) = program.statements[0].kind() else {
            panic!("definition")
        };
        let StatementKind::Assign {
            value: AssignmentValue::Call(call),
            ..
        } = program.statements[1].kind()
        else {
            panic!("call assignment")
        };
        assert_eq!(definition.signature, "combine|param|with|param|intoapair");
        assert_eq!(definition.signature, call.signature);
        assert!(definition.span.text().contains('\\'));
        assert_eq!(call.arguments.len(), 2);
    }
}

#[test]
fn comments_separate_tokens_without_splitting_operators_or_escaping_strings() {
    let source = r####"Add ### sentence comment ### |value| { Return |value + 1| }
|answer| = Add ### ignored ### |
    1 # comment beside operand
    + ### block
comment ### 2
|
|text| = |"| # ### { } [ ] , : \\ \" é🙂"|
|answer| = |[answer, text, {"|#{}": "###"}["|#{}"]]|"####;
    assert_eq!(
        evaluate(source).to_string(),
        r####"[4, "| # ### { } [ ] , : \\ \" é🙂", "###"]"####
    );
    for expression in [
        "1 < # gap\n= 2",
        "true a### gap ###nd false",
        "1 =### gap ###= 1",
    ] {
        assert!(
            Program::parse("bad-token.botwork", &format!("|answer| = |{expression}|")).is_err(),
            "{expression}"
        );
    }
}

#[test]
fn empty_comment_only_and_inline_blocks_have_defined_layouts() {
    for source in [
        "",
        " \t ",
        "\n\n",
        "\r\n\t\r\n",
        "# end of file",
        "## line\n",
        "### block\ncomment ###",
        "\t# line\n### more ###\n",
    ] {
        let program = Program::parse("empty.botwork", source).unwrap();
        assert!(program.statements.is_empty(), "{source:?}");
        assert!(matches!(evaluate(source), Literal::None));
    }
    assert!(matches!(
        evaluate("Choose { |value| = |7| Return |value| }\n|answer| = Choose"),
        Literal::Int(7)
    ));
}

#[test]
fn line_boundaries_end_bare_calls_and_keep_return_values_on_their_own_statement() {
    let program = Program::parse("calls.botwork", "First\nSecond\n").unwrap();
    assert_eq!(program.statements.len(), 2);
    let program = Program::parse("calls.botwork", "First Second\n").unwrap();
    assert_eq!(program.statements.len(), 1);
    let result = evaluate("Bare {\nReturn\n|unused| = |missing|\n}\n|answer| = Bare");
    assert!(matches!(result, Literal::None));
    for separator in [" ", ""] {
        let source = format!("Explicit {{ Return{separator}\\\n |7| }}\n|answer| = Explicit");
        assert!(matches!(evaluate(&source), Literal::Int(7)));
    }
    let program = Program::parse("comment-call.botwork", "First # line ends here\nSecond").unwrap();
    assert_eq!(program.statements.len(), 2);
}

#[test]
fn malformed_layout_and_unterminated_block_comments_are_syntax_errors() {
    for source in [
        "### unterminated",
        "### unterminated\n|x| = |1|",
        "|x| = |1| ### unfinished",
        "|x| = |[1,,2]|",
        "|x| = |[,]|",
        "|x| = |{a: 1,,}|",
        "|x| = |{,}|",
        "|x| = |[1\n2]|",
        "|x| = |{a: 1\nb: 2}|",
        "|x| = |1 +\n|",
        "Call \\\n",
        "Call \\\n# no continued part",
        "Call \\\n|unfinished",
        "Call \\",
        "Call \\ # comment\n|7|",
        "Call \\word",
        "Return\\\n",
        "|x| = |1\r+2|",
        "First\rSecond",
        "|x| = |1\u{a0}+2|",
        "|x| = |1 <=\n= 2|",
        "|x| = |\"bad\\t\"|",
        "Else\n{}",
        "Catch\n{}",
    ] {
        assert!(
            matches!(
                Program::parse("invalid-layout.botwork", source),
                Err(BWErr::ParsingError(_))
            ),
            "{source:?}"
        );
    }
}

#[test]
fn literal_multiline_strings_preserve_their_exact_line_endings_and_delimiters() {
    for ending in ["\n", "\r\n", "\r"] {
        let text = format!("é|#{{}}{ending}🙂");
        let result = evaluate(&format!("|answer| = |\"{text}\"|"));
        assert!(matches!(result, Literal::String(found) if found == text));
    }
}

#[test]
fn multiline_spans_keep_original_bytes_unicode_and_crlf_positions() {
    let source = "# heading\r\n|café| = |\r\n\t[\r\n\t\t1,\r\n\t\t2 +\r\n\t\t3,\r\n\t]\r\n|";
    let program = Program::parse("locations.botwork", source).unwrap();
    let StatementKind::Assign {
        value: AssignmentValue::Expression(expression),
        ..
    } = program.statements[0].kind()
    else {
        panic!("assignment")
    };
    let ExprKind::Array(values) = &expression.kind else {
        panic!("array")
    };
    let ExprKind::Binary {
        operator_span,
        right,
        ..
    } = &values[1].kind
    else {
        panic!("binary")
    };
    assert_eq!(operator_span.line_column(), (5, 5));
    assert_eq!(operator_span.text(), "+");
    assert_eq!(right.span.line_column(), (6, 3));
    assert_eq!(right.span.text().trim(), "3");
    assert_eq!(
        &source[right.span.start()..right.span.end()],
        right.span.text()
    );
    assert_eq!(program.source.text(), source);
}
