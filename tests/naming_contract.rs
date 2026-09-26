//! Statement normalization, declaration collisions, and parameter-name validity.

use botwork::core::{
    ast::Program,
    eval::{evaluate_program, Context},
    grammar::{BWErr, Literal, LiteralResult},
};

fn evaluate(name: &str, source: &str, context: &mut Context) -> LiteralResult {
    let program = Program::parse(name, source)?;
    evaluate_program(&program, context)
}

#[test]
fn statement_names_ignore_ascii_spaces_tabs_and_case_but_keep_punctuation() {
    for call in [
        "Read value |7|",
        "READ\tVALUE |7|",
        "readvalue|7|",
        "r E a D\t v A l U e |7|",
    ] {
        let source = format!("Read\tvalue |x| {{ Return |x| }}\n|answer| = {call}");
        assert!(
            matches!(
                evaluate("matching.botwork", &source, &mut Context::default()),
                Ok(Literal::Int(7))
            ),
            "{call}"
        );
    }
    let source =
        "Read! { Return |1| }\nRead? { Return |2| }\n|a| = READ!\n|b| = read?\n|answer| = |[a, b]|";
    assert_eq!(
        evaluate("punctuation.botwork", source, &mut Context::default())
            .unwrap()
            .to_string(),
        "[1, 2]"
    );
}

#[test]
fn collisions_preserve_the_first_definition_and_report_both_source_locations() {
    for second_name in ["READ VALUE", "Read\tValue", "readvalue"] {
        let mut context = Context::default();
        evaluate(
            "first.botwork",
            "# original\r\n\tRead Value |x| { Return |x + 1| }",
            &mut context,
        )
        .unwrap();
        let source = format!("# newer\n\n\t{second_name} |renamed| {{ Return |99| }}");
        let error = evaluate("second.botwork", &source, &mut context).unwrap_err();
        assert!(matches!(&error, BWErr::DuplicateStatement { .. }));
        let message = error.to_string();
        assert!(message.contains("Duplicate statement"), "{message}");
        assert!(message.contains("readvalue|param|"), "{message}");
        assert!(message.contains("first.botwork:2:2"), "{message}");
        assert!(message.contains("second.botwork:3:2"), "{message}");
        assert!(matches!(
            evaluate("call.botwork", "|answer| = Read value |6|", &mut context),
            Ok(Literal::Int(7))
        ));
    }
}

#[test]
fn arity_and_parameter_positions_distinguish_signatures_but_parameter_labels_do_not() {
    let source = "Read { Return |1| }\nRead |x| { Return |x| }\nRead |x| with |y| { Return |x + y| }\nRead with |x| and |y| { Return |x * y| }\n\
        |a| = Read\n|b| = Read |2|\n|c| = Read |2| with |3|\n|d| = Read with |2| and |3|\n|answer| = |[a,b,c,d]|";
    let mut context = Context::default();
    assert_eq!(
        evaluate("arity.botwork", source, &mut context)
            .unwrap()
            .to_string(),
        "[1, 2, 5, 6]"
    );
    assert!(evaluate("collision.botwork", "Read |other| {}", &mut context).is_err());
}

#[test]
fn declaration_collisions_are_catchable_and_only_executed_definitions_register() {
    let source = "Choose { Return |7| }\n\
        If |false| { CHOOSE { Return |99| } }\n\
        Try { choose { Return |99| } } Catch { |caught| = |true| }\n\
        |value| = Choose\n|answer| = |[value, caught]|";
    assert_eq!(
        evaluate("runtime.botwork", source, &mut Context::default())
            .unwrap()
            .to_string(),
        "[7, true]"
    );
    let source =
        "If |true| { Choice { Return |1| } } Else { Choice { Return |2| } }\n|answer| = Choice";
    assert!(matches!(
        evaluate("branches.botwork", source, &mut Context::default()),
        Ok(Literal::Int(1))
    ));
}

#[test]
fn lexical_shadowing_and_fresh_invocations_allow_independent_definitions() {
    let source = "Helper { Return |1| }\nOuter {\n Helper { Return |2| }\n |local| = Helper\n Return |local|\n}\n\
        |first| = Outer\n|second| = Outer\n|root| = Helper\n|answer| = |[first, second, root]|";
    assert_eq!(
        evaluate("scopes.botwork", source, &mut Context::default())
            .unwrap()
            .to_string(),
        "[2, 2, 1]"
    );
}

#[test]
fn repeated_loop_declarations_collide_in_the_same_frame_and_preserve_the_binding() {
    let source = "|i| = |99|\nTry {\n For |i| In |[1, 2]| { Once { Return |7| } }\n}\nCatch { |caught| = |true| }\n\
        |value| = Once\n|answer| = |[value, caught, i]|";
    assert_eq!(
        evaluate("repeat.botwork", source, &mut Context::default())
            .unwrap()
            .to_string(),
        "[7, true, 99]"
    );
}

#[test]
fn duplicate_parameters_are_rejected_even_in_unused_or_unreachable_definitions() {
    for prefix in ["", "If |false| {", "Try {"] {
        let suffix = match prefix {
            "" => "",
            "Try {" => "} Catch {}",
            _ => "}",
        };
        let source = format!("{prefix}\nPair |café| with |café| {{ Return |1| }}\n{suffix}");
        let error = Program::parse("parameters.botwork", &source).unwrap_err();
        assert!(matches!(&error, BWErr::DuplicateParameter { .. }));
        let message = error.to_string();
        assert!(message.contains("Duplicate parameter `café`"), "{message}");
        assert!(message.contains("parameters.botwork:2:7"), "{message}");
        assert!(message.contains("parameters.botwork:2:19"), "{message}");
    }
    let source = "Pair |x| with |X| { Return |[x,X]| }\n|answer| = Pair |1| with |2|";
    assert_eq!(
        evaluate("case.botwork", source, &mut Context::default())
            .unwrap()
            .to_string(),
        "[1, 2]"
    );
}

#[test]
fn builtin_initialization_is_idempotent_and_never_overwrites_existing_definitions() {
    let mut context = Context::default();
    context.init_statements();
    context.init_statements();
    let error = evaluate(
        "builtin-collision.botwork",
        "Log |x| { Return |99| }",
        &mut context,
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("Duplicate statement"), "{message}");
    assert!(message.contains("<builtin Log>"), "{message}");
    assert!(
        message.contains("builtin-collision.botwork:1:1"),
        "{message}"
    );

    let mut context = Context::default();
    evaluate(
        "custom-log.botwork",
        "Log |x| { Return |x + 1| }",
        &mut context,
    )
    .unwrap();
    context.init_statements();
    assert!(matches!(
        evaluate("call.botwork", "|answer| = Log |6|", &mut context),
        Ok(Literal::Int(7))
    ));
}

#[test]
fn unicode_lowercase_mapping_is_per_character_and_other_spacing_is_literal() {
    let source = "ÉCHO { Return |7| }\nΟΣ { Return |8| }\nRead value { Return |1| }\nRead\u{a0}value { Return |2| }\n\
        |accent| = écho\n|greek| = Ο### ignored ###Σ\n|space| = readvalue\n|nbsp| = READ\u{a0}VALUE\n|answer| = |[accent, greek, space, nbsp]|";
    assert_eq!(
        evaluate("unicode.botwork", source, &mut Context::default())
            .unwrap()
            .to_string(),
        "[7, 8, 1, 2]"
    );
    for (name, call) in [("ΟΣ", "ος"), ("Straße", "STRASSE"), ("Écho", "E\u{301}cho")] {
        let source = format!("{name} {{ Return |7| }}\n|answer| = {call}");
        assert!(
            matches!(
                evaluate("distinct.botwork", &source, &mut Context::default()),
                Err(BWErr::StatementNotDefined(_))
            ),
            "{source}"
        );
    }
}

#[test]
fn a_collision_inside_an_invocation_discards_its_locals_and_keeps_the_caller() {
    let source = "|x| = |99|\nBroken |x| { Local {}\n LOCAL {} }\n\
        Try { Broken |1| } Catch { |caught| = |true| }\n|answer| = |[x, caught]|";
    let mut context = Context::default();
    assert_eq!(
        evaluate("cleanup.botwork", source, &mut context)
            .unwrap()
            .to_string(),
        "[99, true]"
    );
    assert!(matches!(
        evaluate("after.botwork", "Local", &mut context),
        Err(BWErr::StatementNotDefined(_))
    ));
}

#[test]
fn variables_and_parameters_remain_case_sensitive_without_statement_name_normalization() {
    let source = "|value| = |1|\n|Value| = |2|\nPair |x| with |X| { Return |[value, Value, x, X]| }\n|answer| = Pair |3| with |4|";
    assert_eq!(
        evaluate("variables.botwork", source, &mut Context::default())
            .unwrap()
            .to_string(),
        "[1, 2, 3, 4]"
    );
    assert!(matches!(
        evaluate(
            "missing.botwork",
            "|value| = |1|\n|answer| = |VALUE|",
            &mut Context::default()
        ),
        Err(BWErr::VariableNotDefined(_))
    ));
}
