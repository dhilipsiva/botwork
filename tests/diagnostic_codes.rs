use botwork::core::{
    ast::Program,
    diagnostic::{Diagnostic, DiagnosticCode},
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
};
use std::collections::BTreeSet;

fn execute(source: &str) -> Result<Literal, Diagnostic> {
    let program = Program::parse_detailed("codes.botwork", source)?;
    evaluate_program_detailed(&program, &mut Context::default())
}

#[test]
fn every_existing_error_category_has_a_unique_pinned_code_and_repair_guidance() {
    use DiagnosticCode::*;
    let entries = [
        (BWErr::ParsingError("token".into()), Syntax, "BW1001"),
        (
            BWErr::ControlFlowError("placement".into()),
            InvalidControl,
            "BW1002",
        ),
        (
            BWErr::DuplicateParameter {
                name: "x".into(),
                original: "first".into(),
                duplicate: "second".into(),
            },
            DuplicateParameter,
            "BW1003",
        ),
        (
            BWErr::VariableNotDefined("x".into()),
            UndefinedVariable,
            "BW2001",
        ),
        (
            BWErr::StatementNotDefined("Read".into()),
            UndefinedStatement,
            "BW2002",
        ),
        (
            BWErr::DuplicateStatement {
                signature: "read".into(),
                original: "first".into(),
                duplicate: "second".into(),
            },
            DuplicateStatement,
            "BW2003",
        ),
        (
            BWErr::ParameterMissingError("count".into()),
            ParameterCount,
            "BW2004",
        ),
        (
            BWErr::ParsingIntegerError("range".into()),
            InvalidNumber,
            "BW3001",
        ),
        (
            BWErr::ArithmeticError("overflow".into()),
            Arithmetic,
            "BW3002",
        ),
        (
            BWErr::OperationIncompatibleError("types".into()),
            IncompatibleType,
            "BW3003",
        ),
        (
            BWErr::CollectionAccessError {
                path: "m.a".into(),
                segment: "a".into(),
                reason: "absent".into(),
            },
            CollectionAccess,
            "BW3004",
        ),
        (BWErr::OutputError("closed".into()), Output, "BW4001"),
        (BWErr::NativeError("offline".into()), Native, "BW4002"),
        (BWErr::NativePanic("fail".into()), NativePanic, "BW4003"),
        (BWErr::Cancelled("stopped".into()), Cancelled, "BW5001"),
        (BWErr::Timeout("deadline".into()), Timeout, "BW5002"),
        (
            BWErr::ImportRead("missing file".into()),
            ImportRead,
            "BW6001",
        ),
        (
            BWErr::ImportCycle("a -> b -> a".into()),
            ImportCycle,
            "BW6002",
        ),
        (
            BWErr::DuplicateNamespace {
                namespace: "math".into(),
                original: "first:1:1".into(),
                duplicate: "second:1:1".into(),
            },
            DuplicateNamespace,
            "BW6003",
        ),
        (
            BWErr::AsyncRuntime("runtime".into()),
            AsyncRuntime,
            "BW5003",
        ),
        (
            BWErr::SignatureError("unknown parameter".into()),
            Signature,
            "BW1004",
        ),
    ];
    let mut unique = BTreeSet::new();
    for (error, category, code) in entries {
        assert_eq!(error.code(), category);
        assert_eq!(error.code().as_str(), code);
        assert_eq!(category.to_string(), code);
        assert!(unique.insert(code));
        assert!(!error.help().is_empty());
        assert!(
            include_str!("../docs/diagnostics.md").contains(code),
            "document {code}"
        );
        let diagnostic = Diagnostic::new(error);
        let Literal::Map(metadata) = diagnostic.to_value() else {
            panic!("metadata map")
        };
        assert_eq!(metadata["code"].to_string(), code);
        assert_eq!(metadata["help"].to_string(), diagnostic.help());
        assert_eq!(
            metadata["message"].to_string(),
            diagnostic.error.to_string()
        );
        assert!(matches!(metadata["source"], Literal::None));
        let Literal::Map(details) = &metadata["details"] else {
            panic!("details map")
        };
        let expected_keys: &[&str] = match code {
            "BW1003" => &["duplicate", "name", "original"],
            "BW2001" => &["name"],
            "BW2002" => &["call"],
            "BW2003" => &["duplicate", "original", "signature"],
            "BW3004" => &["path", "reason", "segment"],
            "BW6003" => &["duplicate", "namespace", "original"],
            _ => &["reason"],
        };
        assert_eq!(
            details.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            expected_keys.iter().copied().collect()
        );
        assert_eq!(diagnostic.code(), category);
        assert!(diagnostic.to_string().starts_with(&format!("[{code}] ")));
        assert!(diagnostic.to_string().contains("\n  help: "));
        assert_eq!(diagnostic.into_error().code(), category);
    }
}

#[test]
fn common_errors_and_their_documented_repairs_have_expected_codes_and_results() {
    for (bad, fixed, code, hint, expected) in [
        ("|x| = |7", "|x| = |7|", "BW1001", "close every pipe", "7"),
        (
            "Return |7|",
            "Read { Return |7| }\n|x| = Read",
            "BW1002",
            "custom body",
            "7",
        ),
        (
            "Pair |x| with |x| {}",
            "Pair |x| with |y| { Return |y| }\n|x| = Pair |1| with |7|",
            "BW1003",
            "distinct",
            "7",
        ),
        (
            "|answer| = |missing|",
            "|missing| = |7|\n|answer| = |missing|",
            "BW2001",
            "Define `missing`",
            "7",
        ),
        (
            "Read { Return |7| }\nRead!",
            "Read { Return |7| }\nRead",
            "BW2002",
            "punctuation",
            "7",
        ),
        (
            "Read {}\nREAD {}",
            "Read {}\nOther { Return |7| }\nOther",
            "BW2003",
            "Rename",
            "7",
        ),
        (
            "|x| = |2147483648|",
            "|x| = |2147483647|",
            "BW3001",
            "2147483647",
            "2147483647",
        ),
        (
            "|x| = |1 / 0|",
            "|x| = |1 / 2|",
            "BW3002",
            "zero divisors",
            "0.5",
        ),
        ("If |1| {}", "If |true| {}", "BW3003", "booleans", "none"),
        (
            "|x| = |[7][1]|",
            "|x| = |[7][0]|",
            "BW3004",
            "in-bounds",
            "7",
        ),
    ] {
        let error = execute(bad).unwrap_err();
        assert_eq!(error.code().as_str(), code, "{bad}");
        assert!(error.help().contains(hint), "{bad}: {}", error.help());
        assert!(error.span.is_some());
        assert_eq!(execute(fixed).unwrap().to_string(), expected, "{fixed}");
    }
}

#[test]
fn unicode_multiline_and_eof_ranges_use_exclusive_scalar_end_positions() {
    let source = "# தமிழ்\r\n\t|answer| = |cafe\u{301}|\r\n";
    let error = execute(source).unwrap_err();
    let span = error.span.as_ref().unwrap();
    assert_eq!(span.line_column(), (2, 14));
    assert_eq!(span.end_line_column(), (2, 19));
    assert_eq!(span.end() - span.start(), "cafe\u{301}".len());
    assert!(error
        .to_string()
        .starts_with("codes.botwork:2:14-2:19: [BW2001]"));
    let source = "|answer| = |1 +\r\n \t2 / 0|";
    let error = execute(source).unwrap_err();
    let span = error.span.as_ref().unwrap();
    assert_eq!(span.line_column(), (2, 3));
    assert_eq!(span.end_line_column(), (2, 8));
    assert!(error
        .to_string()
        .contains("codes.botwork:2:3-2:8: [BW3002]"));
    let source = "|answer| = |1\r\n";
    let error = execute(source).unwrap_err();
    let span = error.span.as_ref().unwrap();
    assert_eq!(span.start(), source.len());
    assert_eq!(span.end(), source.len());
    assert_eq!(span.line_column(), span.end_line_column());
    assert!(error.to_string().starts_with("codes.botwork:2:1: [BW1001]"));
}

#[test]
fn handler_causes_keep_distinct_codes_and_hints_without_recategorization() {
    let error = execute("Try { |value| = |1 / 0| } Catch { |value| = |missing| }").unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
    assert!(error.help().contains("Define `missing`"));
    assert!(error.causes[0].help().contains("zero divisors"));
    let rendered = error.to_string();
    assert!(rendered.contains("[BW2001] Variable not defined"));
    assert!(rendered.contains("[BW3002] Arithmetic error"));
    assert!(rendered.find("[BW2001]").unwrap() < rendered.find("[BW3002]").unwrap());
}
