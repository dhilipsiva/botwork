use botwork::core::{
    ast::{Program, StatementKind},
    diagnostic::Diagnostic,
    eval::{
        botwork_detailed, evaluate_program, evaluate_program_detailed, execute_statement_detailed,
        Context,
    },
    grammar::{BWErr, BWParser, Literal, Rule},
};
use pest::Parser;
use std::error::Error;

fn execute(source: &str, context: &mut Context) -> Result<Literal, Diagnostic> {
    evaluate_program_detailed(
        &Program::parse_detailed("runtime.botwork", source)?,
        context,
    )
}

#[test]
fn runtime_errors_keep_the_most_specific_expression_and_original_error_kind() {
    for (source, expected, category) in [
        ("|answer| = |1 + missing|", "missing", "variable"),
        ("|answer| = |missing.items[0]|", "missing", "variable"),
        ("|answer| = |(1 + 2) / 0|", "(1 + 2) / 0", "arithmetic"),
        ("If |1| { |unreachable| = |0| }", "1", "type"),
        ("While |[]| {}", "[]", "type"),
        ("For |item| In |{}| {}", "{}", "type"),
        ("|answer| = |-2147483649|", "-2147483649", "number"),
        (
            "|answer| = |{a: 1}[\"missing\"]|",
            "[\"missing\"]",
            "access",
        ),
        ("|answer| = |{a: 1}.missing|", "missing", "access"),
    ] {
        let error = execute(source, &mut Context::default()).unwrap_err();
        let span = error.span.as_ref().unwrap();
        assert_eq!(span.text().trim(), expected, "{source}");
        assert_eq!(&source[span.start()..span.end()], span.text());
        assert_eq!(error.label, "expression");
        assert!(
            match category {
                "variable" => matches!(*error.error, BWErr::VariableNotDefined(_)),
                "arithmetic" => matches!(*error.error, BWErr::ArithmeticError(_)),
                "type" => matches!(*error.error, BWErr::OperationIncompatibleError(_)),
                "number" => matches!(*error.error, BWErr::ParsingIntegerError(_)),
                "access" => matches!(*error.error, BWErr::CollectionAccessError { .. }),
                _ => unreachable!(),
            },
            "{error}"
        );
        assert!(error.call_stack.is_empty());
        assert!(error.causes.is_empty());
    }
}

#[test]
fn syntax_errors_retain_unicode_and_eof_byte_ranges() {
    let source = "# தமிழ்\r\n\t|🙂| = |1|\r\n";
    let error = Program::parse_detailed("தமிழ்.botwork", source).unwrap_err();
    let span = error.span.as_ref().unwrap();
    assert_eq!(span.start(), source.find('🙂').unwrap());
    assert_eq!(span.text(), "🙂");
    assert_eq!(span.location(), "தமிழ்.botwork:2:3");
    assert!(matches!(*error.error, BWErr::ParsingError(_)));
    let source = "Log |1\r\n";
    let error = Program::parse_detailed("eof.botwork", source).unwrap_err();
    let span = error.span.unwrap();
    assert_eq!(span.start(), source.len());
    assert_eq!(span.end(), source.len());
    assert_eq!(span.location(), "eof.botwork:2:1");
}

#[test]
fn runtime_columns_count_scalars_and_tabs_without_changing_utf8_offsets() {
    let source = "# தமிழ்\r\n\t|மதிப்பு| = |1 + விடுபட்டது|\r\n";
    let error = execute(source, &mut Context::default()).unwrap_err();
    let span = error.span.unwrap();
    let offset = source.find("விடுபட்டது").unwrap();
    let prefix = source[..offset].rsplit('\n').next().unwrap();
    assert_eq!(span.start(), offset);
    assert_eq!(span.text(), "விடுபட்டது");
    assert_eq!(span.line_column(), (2, prefix.chars().count() + 1));
}

#[test]
fn validation_errors_retain_related_locations_and_prevent_all_state_changes() {
    let error = Program::parse_detailed("params.botwork", "Pair |x| with |x| {}").unwrap_err();
    assert!(matches!(*error.error, BWErr::DuplicateParameter { .. }));
    assert_eq!(
        error.span.as_ref().unwrap().location(),
        "params.botwork:1:16"
    );
    assert_eq!(error.related.len(), 1);
    assert_eq!(error.related[0].span.location(), "params.botwork:1:7");
    let mut program =
        Program::parse("extracted.botwork", "|early| = |7|\nHolder { Return |1| }").unwrap();
    let StatementKind::Define(definition) = program.statements[1].kind() else {
        panic!("definition")
    };
    let extracted = definition.body.statements[0].clone();
    let mut context = Context::default();
    let error = execute_statement_detailed(&extracted, &mut context).unwrap_err();
    assert!(matches!(*error.error, BWErr::ControlFlowError(_)));
    assert_eq!(error.span.unwrap(), extracted.span);
    program.statements.push(extracted);
    assert!(evaluate_program_detailed(&program, &mut context).is_err());
    assert!(matches!(
        *execute("|answer| = |early|", &mut context)
            .unwrap_err()
            .error,
        BWErr::VariableNotDefined(_)
    ));
}

#[test]
fn call_frames_keep_definition_and_call_sources_after_owners_are_dropped() {
    let error = {
        let mut context = Context::default();
        let definitions = Program::parse(
            "library.botwork",
            "Fail |value| { Return |value + missing| }",
        )
        .unwrap();
        evaluate_program_detailed(&definitions, &mut context).unwrap();
        drop(definitions);
        let caller =
            Program::parse("caller.botwork", "Outer {\n |result| = Fail |7|\n}\nOuter").unwrap();
        evaluate_program_detailed(&caller, &mut context).unwrap_err()
    };
    assert_eq!(
        error.span.as_ref().unwrap().source().name(),
        "library.botwork"
    );
    assert_eq!(error.span.as_ref().unwrap().text(), "missing");
    assert_eq!(
        error
            .call_stack
            .iter()
            .map(|frame| frame.signature.as_str())
            .collect::<Vec<_>>(),
        ["fail|param|", "outer"]
    );
    assert_eq!(
        error.call_stack[0].call_site.location(),
        "caller.botwork:2:13"
    );
    assert_eq!(
        error.call_stack[0]
            .definition_site
            .as_ref()
            .unwrap()
            .location(),
        "library.botwork:1:1"
    );
    assert_eq!(
        error.call_stack[1].call_site.location(),
        "caller.botwork:4:1"
    );
}

#[test]
fn argument_failure_excludes_the_unentered_custom_body_and_preserves_caller_state() {
    let mut context = Context::default();
    let error = execute(
        "|x| = |99|\nFail |x| {}\nOuter { Fail |missing| }\nOuter",
        &mut context,
    )
    .unwrap_err();
    assert_eq!(error.call_stack.len(), 1);
    assert_eq!(error.call_stack[0].signature, "outer");
    assert_eq!(error.span.as_ref().unwrap().text(), "missing");
    assert!(matches!(
        execute("|answer| = |x|", &mut context),
        Ok(Literal::Int(99))
    ));
    let next = execute("Unknown |missing_argument|", &mut context).unwrap_err();
    assert!(matches!(*next.error, BWErr::StatementNotDefined(_)));
    assert!(next.call_stack.is_empty());
}

#[test]
fn native_errors_record_the_call_site_without_a_dsl_definition() {
    let mut context = Context::default();
    context.init_statements();
    let error = execute("Log |missing|", &mut context).unwrap_err();
    assert!(error.call_stack.is_empty());
    assert_eq!(error.span.as_ref().unwrap().text(), "missing");
    context
        .register_native("Boom |value|", |_| {
            Err(BWErr::NativeError("offline".into()))
        })
        .unwrap();
    let error = execute("Boom |1|", &mut context).unwrap_err();
    assert_eq!(error.call_stack.len(), 1);
    assert_eq!(error.call_stack[0].signature, "boom|param|");
    assert!(error.call_stack[0].definition_site.is_none());
    assert_eq!(error.span.as_ref().unwrap().text(), "Boom |1|");
}

#[test]
fn recursive_errors_snapshot_every_entered_call_and_cleanup_all_frames() {
    let mut context = Context::default();
    let error = execute("|n| = |99|\nRecurse |n| {\n If |n == 0| { Return |missing| }\n Recurse |n - 1|\n}\nRecurse |3|", &mut context).unwrap_err();
    assert_eq!(error.call_stack.len(), 4);
    assert!(error
        .call_stack
        .iter()
        .all(|frame| frame.signature == "recurse|param|"));
    assert_eq!(
        error.call_stack.last().unwrap().call_site.line_column(),
        (6, 1)
    );
    assert!(matches!(
        execute("|answer| = |n|", &mut context),
        Ok(Literal::Int(99))
    ));
    let next = execute("|answer| = |missing_again|", &mut context).unwrap_err();
    assert!(next.call_stack.is_empty());
}

#[test]
fn failed_handlers_preserve_original_full_stacks_even_before_outer_calls_unwind() {
    let error = execute("Fail { Return |original| }\nHandler { Return |replacement| }\nOuter {\n Try { Fail } Catch { Handler }\n}\nOuter", &mut Context::default()).unwrap_err();
    assert!(
        matches!(error.error.as_ref(), BWErr::VariableNotDefined(name) if name == "replacement")
    );
    assert_eq!(
        error
            .call_stack
            .iter()
            .map(|frame| frame.signature.as_str())
            .collect::<Vec<_>>(),
        ["handler", "outer"]
    );
    assert_eq!(error.causes.len(), 1);
    let original = &error.causes[0];
    assert_eq!(original.span.as_ref().unwrap().text(), "original");
    assert_eq!(
        original
            .call_stack
            .iter()
            .map(|frame| frame.signature.as_str())
            .collect::<Vec<_>>(),
        ["fail", "outer"]
    );
    assert!(error.source().unwrap().to_string().contains("original"));
    assert!(error.to_string().contains("while handling:"));
}

#[test]
fn nested_handler_causes_keep_each_error_without_leaking_into_later_failures() {
    let mut context = Context::default();
    let source =
        "Try { |x| = |original| } Catch {\n Try { |x| = |intermediate| } Catch { |x| = |last| }\n}";
    let error = execute(source, &mut context).unwrap_err();
    assert_eq!(error.span.as_ref().unwrap().text(), "last");
    assert_eq!(
        error
            .causes
            .iter()
            .map(|cause| cause.span.as_ref().unwrap().text())
            .collect::<Vec<_>>(),
        ["intermediate", "original"]
    );
    execute("Try { |x| = |handled| } Catch { |x| = |7| }", &mut context).unwrap();
    let error = execute("|answer| = |fresh|", &mut context).unwrap_err();
    assert!(error.causes.is_empty());
    assert!(error.call_stack.is_empty());
}

#[test]
fn duplicate_definitions_keep_both_structured_source_locations() {
    let mut context = Context::default();
    let first = Program::parse("first.botwork", "Keep { Return |7| }").unwrap();
    evaluate_program_detailed(&first, &mut context).unwrap();
    let second = Program::parse("second.botwork", "KEEP {}").unwrap();
    let error = evaluate_program_detailed(&second, &mut context).unwrap_err();
    assert!(matches!(*error.error, BWErr::DuplicateStatement { .. }));
    assert_eq!(error.span.unwrap().location(), "second.botwork:1:1");
    assert_eq!(error.related[0].span.location(), "first.botwork:1:1");
    assert!(matches!(execute("Keep", &mut context), Ok(Literal::Int(7))));
}

#[test]
fn pair_compatibility_preserves_original_offsets_and_legacy_error_categories() {
    let source = "# prefix\n|answer| = |missing|";
    let pair = BWParser::parse(Rule::botwork, source)
        .unwrap()
        .next()
        .unwrap();
    let error = botwork_detailed(pair, &mut Context::default()).unwrap_err();
    assert_eq!(error.span.unwrap().location(), "<input>:2:13");
    let pair = BWParser::parse(Rule::part, "unsupported")
        .unwrap()
        .next()
        .unwrap();
    let error = botwork_detailed(pair, &mut Context::default()).unwrap_err();
    assert_eq!(error.span.unwrap().text(), "unsupported");
    let program = Program::parse("legacy.botwork", "|answer| = |missing|").unwrap();
    assert!(
        matches!(evaluate_program(&program, &mut Context::default()), Err(BWErr::VariableNotDefined(name)) if name == "missing")
    );
    assert!(matches!(
        Program::parse("legacy.botwork", "Return"),
        Err(BWErr::ControlFlowError(_))
    ));
}
