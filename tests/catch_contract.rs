use botwork::core::{
    ast::Program,
    diagnostic::{Diagnostic, DiagnosticCode},
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, BWParser, Literal, Rule},
};
use pest::Parser;

fn execute(source: &str, context: &mut Context) -> Result<Literal, Diagnostic> {
    let program = Program::parse_detailed("catch.botwork", source)?;
    evaluate_program_detailed(&program, context)
}

#[test]
fn catch_can_inspect_the_original_code_and_details_through_a_temporary_binding() {
    let source = "|error| = |99|\nTry { |value| = |missing| } Catch |error| {\n\
        |observed| = |[error.code, error.details.name, error.source.file]|\n}\n\
        |answer| = |[observed, error]|";
    let result = execute(source, &mut Context::default()).unwrap();
    assert_eq!(
        result.to_string(),
        "[[\"BW2001\", \"missing\", \"catch.botwork\"], 99]"
    );
}

#[test]
fn bare_rethrow_preserves_the_original_failure_instead_of_wrapping_it_again() {
    for keyword in ["Rethrow", "rethrow", "rEtHrOw"] {
        let error = execute(
            &format!("Try {{ |value| = |1 / 0| }} Catch {{ {keyword} }}"),
            &mut Context::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Arithmetic);
        assert_eq!(error.span.as_ref().unwrap().text().trim(), "1 / 0");
        assert!(error.causes.is_empty());
    }
}

#[test]
fn changing_or_copying_the_error_binding_cannot_change_what_rethrow_propagates() {
    let mut context = Context::default();
    let error = execute(
        "Try { |value| = |1 / 0| } Catch |failure| {\n\
        |saved| = |failure|\n|failure| = |{code: \"changed\"}|\nRethrow\n}",
        &mut context,
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Arithmetic);
    assert_eq!(error.related.last().unwrap().message, "rethrow");
    assert!(error.causes.is_empty());
    assert_eq!(
        execute("|answer| = |saved.code|", &mut context)
            .unwrap()
            .to_string(),
        "BW3002"
    );
    assert_eq!(
        execute("|answer| = |failure|", &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::UndefinedVariable
    );
}

#[test]
fn nested_handler_bindings_restore_outer_metadata_and_then_remove_the_local() {
    let mut context = Context::default();
    let result = execute(
        "Try { |x| = |missing_outer| } Catch |error| {\n\
        |first| = |error.details.name|\n\
        Try { |x| = |1 / 0| } Catch |error| { |second| = |error.code| }\n\
        |third| = |error.details.name|\n}\n|answer| = |[first, second, third]|",
        &mut context,
    )
    .unwrap();
    assert_eq!(
        result.to_string(),
        "[\"missing_outer\", \"BW3002\", \"missing_outer\"]"
    );
    assert_eq!(
        execute("|answer| = |error|", &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::UndefinedVariable
    );
}

#[test]
fn an_inner_try_can_catch_rethrow_from_its_surrounding_handler() {
    let result = execute("Try { |x| = |missing| } Catch |outer| {\n\
        Try { Rethrow } Catch |inner| {\n\
        |answer| = |[outer.code == inner.code, outer.source == inner.source, inner.causes == [], inner.related[0].message]|\n}\n}\n\
        |result| = |answer|", &mut Context::default()).unwrap();
    assert_eq!(result.to_string(), "[true, true, true, \"rethrow\"]");
}

#[test]
fn rethrowing_a_different_inner_error_retains_the_original_outer_cause() {
    let error = execute(
        "Try { |x| = |original| } Catch {\n\
        Try { |x| = |1 / 0| } Catch { Rethrow }\n}",
        &mut Context::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Arithmetic);
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(error.causes[0].span.as_ref().unwrap().text(), "original");
}

#[test]
fn a_fresh_failure_at_the_same_source_is_preserved_as_a_distinct_handling_failure() {
    let error = execute(
        "Fail { Return |missing| }\nTry { Fail } Catch { Fail }",
        &mut Context::default(),
    )
    .unwrap_err();
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.span, error.causes[0].span);
    assert!(!std::sync::Arc::ptr_eq(
        &error.error,
        &error.causes[0].error
    ));
    assert_ne!(
        error.call_stack[0].call_site,
        error.causes[0].call_stack[0].call_site
    );
}

#[test]
fn metadata_exposes_original_causes_calls_positions_and_specific_details() {
    let source = "Failure { Return |missing_original| }\n\
        Try {\nTry { Failure } Catch { |x| = |1 / 0| }\n\
        } Catch |error| {\n\
        |answer| = |[error.code, error.causes[0].code, error.causes[0].details.name, error.causes[0].call_stack[0].signature, error.causes[0].source.line, error.causes[0].source.column]|\n\
        }\n|result| = |answer|";
    assert_eq!(
        execute(source, &mut Context::default())
            .unwrap()
            .to_string(),
        "[\"BW3002\", \"BW2001\", \"missing_original\", \"failure\", \"1\", \"19\"]"
    );
    let error = execute(
        "Try { |x| = |[1][2]| } Catch |error| { Return |error| }",
        &mut Context::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::InvalidControl);
    let result = execute(
        "Inspect { Try { |x| = |[1][2]| } Catch |error| { Return |error.details| } }\nInspect",
        &mut Context::default(),
    )
    .unwrap();
    let Literal::Map(details) = result else {
        panic!("details map")
    };
    assert_eq!(details["path"].to_string(), "[1][2]");
    assert_eq!(details["segment"].to_string(), "[2]");
    assert!(details["reason"].to_string().contains("bounds"));
}

#[test]
fn rethrow_placement_is_lexical_and_invalid_even_in_unreachable_helpers() {
    for source in [
        "Rethrow",
        "Try { Rethrow } Catch {}",
        "If |false| { Rethrow }",
        "Unused { Rethrow }",
        "Try {} Catch { Unused { Rethrow } }",
        "Try {} Catch { Unused { Try { Rethrow } Catch {} } }",
    ] {
        let error = Program::parse_detailed("placement.botwork", source).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::InvalidControl, "{source}");
        assert_eq!(error.span.as_ref().unwrap().text().trim(), "Rethrow");
    }
    for source in [
        "Try {} Catch { Rethrow }",
        "Try {} Catch { Try { Rethrow } Catch {} }",
        "Unused { Try {} Catch { Rethrow } }",
    ] {
        Program::parse_detailed("valid.botwork", source).unwrap();
    }
}

#[test]
fn malformed_headers_and_rethrow_values_fail_while_prefix_names_and_unicode_work() {
    for source in [
        "Try {} Catch || {}",
        "Try {} Catch |a + b| {}",
        "Try {} Catch |true| {}",
        "Try {} Catch |a| |b| {}",
        "Try {} Catch { Rethrow |7| }",
        "Catch |error| {}",
    ] {
        assert_eq!(
            Program::parse_detailed("invalid.botwork", source)
                .unwrap_err()
                .code(),
            DiagnosticCode::Syntax,
            "{source}"
        );
    }
    let result = execute(
        "Rethrow! { Return |7| }\nRethrowing { Return |8| }\n\
        |one| = Rethrow!\n|two| = Rethrowing\n\
        Try { |x| = |missing| }\nCaTcH\n|பிழை|\n{ |code| = |பிழை.code| }\n\
        |answer| = |[one, two, code]|",
        &mut Context::default(),
    )
    .unwrap();
    assert_eq!(result.to_string(), "[7, 8, \"BW2001\"]");
}

#[test]
fn skipped_handlers_create_no_binding_and_successful_handlers_leave_no_pending_rethrow() {
    let mut context = Context::default();
    assert!(matches!(
        execute(
            "|error| = |99|\nTry {} Catch |error| { Rethrow }\n|answer| = |error|",
            &mut context
        ),
        Ok(Literal::Int(99))
    ));
    execute("Try { |x| = |missing| } Catch |error| {}", &mut context).unwrap();
    let next = execute("|answer| = |fresh|", &mut context).unwrap_err();
    assert!(next.causes.is_empty());
    assert!(next.related.is_empty());
    assert!(next.call_stack.is_empty());
}

#[test]
fn parser_pair_execution_requires_the_enclosing_try_for_bindings_and_rethrow() {
    use botwork::core::eval::botwork_detailed;
    for (rule, source) in [
        (Rule::stmt_catch, "Catch |error| {}"),
        (Rule::stmt_rethrow, "Rethrow"),
    ] {
        let pair = BWParser::parse(rule, source).unwrap().next().unwrap();
        let error = botwork_detailed(pair, &mut Context::default()).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::InvalidControl);
    }
    let source = "Try { |x| = |missing| } Catch |error| { |answer| = |error.code| }";
    let pair = BWParser::parse(Rule::stmt_try, source)
        .unwrap()
        .next()
        .unwrap();
    let mut context = Context::default();
    botwork_detailed(pair, &mut context).unwrap();
    assert_eq!(
        execute("|result| = |answer|", &mut context)
            .unwrap()
            .to_string(),
        "BW2001"
    );
}

#[test]
fn source_free_metadata_preserves_none_and_shared_error_identity_on_clone() {
    let original = Diagnostic::new(BWErr::OutputError("closed".into()));
    let cloned = original.clone();
    assert!(std::sync::Arc::ptr_eq(&original.error, &cloned.error));
    let Literal::Map(mut value) = original.to_value() else {
        panic!("metadata map")
    };
    assert!(matches!(value["source"], Literal::None));
    assert_eq!(value["code"].to_string(), "BW4001");
    value.insert("code".into(), Literal::String("changed".into()));
    assert_eq!(cloned.into_error().code(), DiagnosticCode::Output);
    assert_eq!(original.code(), DiagnosticCode::Output);
}

#[test]
fn unicode_catch_metadata_preserves_exact_byte_and_scalar_coordinate_strings() {
    let source = "# தமிழ்\r\n\tTry { |x| = |விடுபட்டது| } Catch |பிழை| { |answer| = |பிழை.source| }\r\n|result| = |answer|";
    let Literal::Map(location) = execute(source, &mut Context::default()).unwrap() else {
        panic!("source map")
    };
    let start = source.find("விடுபட்டது").unwrap();
    let column = source[..start].rsplit('\n').next().unwrap().chars().count() + 1;
    for (key, expected) in [
        ("start_byte", start),
        ("end_byte", start + "விடுபட்டது".len()),
        ("line", 2),
        ("end_line", 2),
        ("column", column),
        ("end_column", column + "விடுபட்டது".chars().count()),
    ] {
        assert!(
            matches!(&location[key], Literal::String(value) if *value == expected.to_string()),
            "{key}"
        );
    }
    assert_eq!(location["text"].to_string(), "விடுபட்டது");
}

#[test]
fn native_call_metadata_uses_none_for_a_missing_dsl_definition_site() {
    let mut context = Context::default();
    context.init_statements();
    let value = execute("Try { Log |missing| } Catch |error| {\n\
        |answer| = |[error.call_stack[0].signature, error.call_stack[0].definition_site, error.call_stack[0].call_site.line]|\n\
        }\n|result| = |answer|", &mut context).unwrap();
    assert_eq!(value.to_string(), "[\"log|param|\", none, \"1\"]");
}

#[test]
fn declaration_collision_metadata_identifies_the_retained_original_definition() {
    let value = execute(
        "Keep { Return |7| }\n\
        Try { KEEP { Return |8| } } Catch |error| {\n\
        |observed| = |[error.code, error.details.signature, error.related[0].source.line]|\n}\n\
        |retained| = Keep\n|answer| = |[observed, retained]|",
        &mut Context::default(),
    )
    .unwrap();
    assert_eq!(value.to_string(), "[[\"BW2003\", \"keep\", \"1\"], 7]");
}
