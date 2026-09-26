//! Unicode identifier, naming, normalization, and fixed-syntax expectations.

use botwork::core::{
    ast::{Program, StatementKind},
    eval::{evaluate_program, Context},
    grammar::{BWErr, Literal},
};

fn evaluate(source: &str) -> Literal {
    let program = Program::parse("unicode.botwork", source).expect("valid Unicode program");
    evaluate_program(&program, &mut Context::default()).expect("successful execution")
}

#[test]
fn unicode_identifiers_support_combining_marks_scripts_and_underscore() {
    for name in [
        "தமிழ்",
        "முதல்",
        "हिंदी",
        "नाम",
        "العربية",
        "变量",
        "名前",
        "한글",
        "Δείγμα",
        "café",
        "cafe\u{301}",
        "_private",
        "_",
        "value٣",
    ] {
        let source = format!("|{name}| = |7|\n|answer| = |{name}|");
        assert!(matches!(evaluate(&source), Literal::Int(7)), "{name}");
    }
}

#[test]
fn tamil_parameters_variables_map_keys_access_and_loops_work_together() {
    let source = "கூட்டு |முதல்| உடன் |இரண்டாம்| { Return |முதல் + இரண்டாம்| }\n\
        |எண்| = கூட்டு |2| உடன் |3|\n|தரவு| = |{பெயர்: \"தமிழ்\", மதிப்பு: எண்}|\n\
        |மொத்தம்| = |0|\nFor |உருப்பு| In |[1, 2]| { |மொத்தம்| = |மொத்தம் + உருப்பு| }\n\
        |answer| = |[தரவு.பெயர், தரவு[\"மதிப்பு\"], மொத்தம்]|";
    assert_eq!(evaluate(source).to_string(), "[\"தமிழ்\", 5, 3]");
}

#[test]
fn composed_and_decomposed_spellings_are_distinct_case_sensitive_bindings_and_keys() {
    let source = "|café| = |1|\n|cafe\u{301}| = |2|\n|Café| = |3|\n\
        |map| = |{café: café, cafe\u{301}: cafe\u{301}, Café: Café}|\n\
        |answer| = |[map.café, map.cafe\u{301}, map.Café]|";
    assert_eq!(evaluate(source).to_string(), "[1, 2, 3]");
}

#[test]
fn combining_marks_are_part_of_keyword_boundaries() {
    for name in ["true\u{301}", "false\u{301}", "and\u{301}", "or\u{301}"] {
        let source = format!("|{name}| = |7|\n|answer| = |{name}|");
        assert!(matches!(evaluate(&source), Literal::Int(7)), "{name}");
    }
    for expression in ["true and\u{301} false", "false or\u{301} true"] {
        assert!(Program::parse(
            "invalid-keyword.botwork",
            &format!("|answer| = |{expression}|")
        )
        .is_err());
    }
}

#[test]
fn identifier_starts_and_punctuation_follow_unicode_identifier_properties() {
    for name in [
        "3value",
        "٣value",
        "\u{301}name",
        "🙂",
        "some-name",
        "some name",
        "some\u{a0}name",
        "name\u{200b}",
        "name\u{202e}",
    ] {
        let source = format!("|{name}| = |7|");
        assert!(
            matches!(
                Program::parse("invalid-name.botwork", &source),
                Err(BWErr::ParsingError(_))
            ),
            "{name}"
        );
    }
}

#[test]
fn numeric_literals_and_operators_use_ascii_while_numeric_map_segments_keep_spelling() {
    for expression in ["٣", "١.٥", "１２", "−1", "1 ＋ 2", "உண்மை and true"] {
        if expression.starts_with("உண்மை") {
            // A translated boolean word is an ordinary variable, not a keyword.
            let program =
                Program::parse("word.botwork", &format!("|answer| = |{expression}|")).unwrap();
            assert!(
                matches!(evaluate_program(&program, &mut Context::default()), Err(BWErr::VariableNotDefined(name)) if name == "உண்மை")
            );
        } else {
            assert!(
                Program::parse(
                    "ascii-syntax.botwork",
                    &format!("|answer| = |{expression}|")
                )
                .is_err(),
                "{expression}"
            );
        }
    }
    assert!(matches!(
        evaluate("|answer| = |{\"٣\": 7}.٣|"),
        Literal::Int(7)
    ));
    assert!(matches!(
        evaluate("|உண்மை| = |true|\n|answer| = |உண்மை and true|"),
        Literal::Bool(true)
    ));
}

#[test]
fn unicode_source_spans_preserve_utf8_bytes_and_count_scalar_columns() {
    let source = "# தமிழ்\r\n\t|மதிப்பு| = |7|\r\n";
    let program = Program::parse("தமிழ்.botwork", source).unwrap();
    let StatementKind::Assign { name, .. } = program.statements[0].kind() else {
        panic!("assignment")
    };
    assert_eq!(name.text, "மதிப்பு");
    assert_eq!(name.span.text(), "மதிப்பு");
    assert_eq!(name.span.end() - name.span.start(), "மதிப்பு".len());
    assert_eq!(name.span.location(), "தமிழ்.botwork:2:3");
    assert_eq!(
        &source[name.span.start()..name.span.end()],
        name.span.text()
    );
}

#[test]
fn collection_display_preserves_combining_marks_and_escapes_controls_and_quotes() {
    let value = Literal::Map(
        [
            ("தமிழ்".into(), Literal::String("cafe\u{301}".into())),
            (
                "controls".into(),
                Literal::String("'\n\t\r\0\"\\\u{202e}".into()),
            ),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(
        value.to_string(),
        "{\"controls\": \"'\\n\\t\\r\\0\\\"\\\\\\u{202e}\", \"தமிழ்\": \"cafe\u{301}\"}"
    );
    let values = Literal::Array(vec![Literal::String("தமிழ் हिंदी cafe\u{301}".into())]);
    assert_eq!(values.to_string(), "[\"தமிழ் हिंदी cafe\u{301}\"]");
}

#[test]
fn unicode_duplicate_parameters_and_statement_collisions_follow_the_same_rules() {
    let source = "கூட்டு |எண்| உடன் |எண்| {}";
    let error = Program::parse("parameters.botwork", source).unwrap_err();
    assert!(
        matches!(error, BWErr::DuplicateParameter { name, original, duplicate }
        if name == "எண்" && original == "parameters.botwork:1:9" && duplicate == "parameters.botwork:1:20")
    );
    let source = "Écho |entrée| { Return |entrée| }\n\
        Try { ÉCHO |autre| {} } Catch { |caught| = |true| }\n\
        |value| = éCHO |7|\n|answer| = |[value, caught]|";
    assert_eq!(evaluate(source).to_string(), "[7, true]");
}
