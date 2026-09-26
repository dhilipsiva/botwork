use botwork::core::{
    ast::Program,
    eval::{botwork, evaluate_program, Context},
    grammar::{BWErr, BWParser, Literal, Rule},
};
use pest::Parser;

#[test]
fn an_owned_program_executes_after_the_input_string_is_dropped() {
    let program = {
        let source = String::from("|answer| = |2 ^ 3 ^ 2|");
        Program::parse("owned.botwork", &source).unwrap()
    };
    let result = evaluate_program(&program, &mut Context::default());
    assert!(matches!(result, Ok(Literal::Int(512))), "{result:?}");
    assert_eq!(program.source.name(), "owned.botwork");
    assert_eq!(program.statements[0].span.text(), "|answer| = |2 ^ 3 ^ 2|");
}

#[test]
fn a_stored_definition_survives_its_program_and_source() {
    let mut context = Context::default();
    {
        let source =
            String::from("Double |value| {\n Return |value * 2|\n |unreachable| = |missing|\n}");
        let program = Program::parse("definition.botwork", &source).unwrap();
        evaluate_program(&program, &mut context).unwrap();
    }
    for value in [1, 3, 7] {
        let call =
            Program::parse("caller.botwork", &format!("|answer| = Double |{value}|")).unwrap();
        let result = evaluate_program(&call, &mut context);
        assert!(
            matches!(result, Ok(Literal::Int(answer)) if answer == value * 2),
            "{result:?}"
        );
    }
}

#[test]
fn numeric_conversion_stays_at_runtime_after_parsing() {
    for number in ["9".repeat(50), format!("{}.0", "9".repeat(50))] {
        let source = format!(
            "Never called {{\n |value| = |{number}|\n}}\n\
             If |false| {{\n |value| = |{number}|\n}}\n\
             |answer| = |1|\nTry {{\n |answer| = |{number}|\n\
             }} Catch {{\n |answer| = |7|\n}}\n|result| = |answer|"
        );
        let program = Program::parse("numbers.botwork", &source).expect("syntax is valid");
        let result = evaluate_program(&program, &mut Context::default());
        assert!(matches!(result, Ok(Literal::Int(7))), "{result:?}");
    }
}

#[test]
fn pair_compatibility_uses_original_offsets_and_retains_definitions() {
    let source = String::from(
        "# café\r\n\tDouble |value| {\n Return |value * 2|\n |unreachable| = |missing|\n}",
    );
    let mut context = Context::default();
    {
        let pair = BWParser::parse(Rule::botwork, &source)
            .unwrap()
            .next()
            .unwrap();
        assert!(pair.as_span().start() > 0);
        botwork(pair, &mut context).unwrap();
    }
    drop(source);
    let call_source = "# call\n|answer| = Double |4|";
    let pair = BWParser::parse(Rule::botwork, call_source)
        .unwrap()
        .next()
        .unwrap();
    assert!(matches!(botwork(pair, &mut context), Ok(Literal::Int(8))));
}

#[test]
fn pair_compatibility_supports_expressions_and_rejects_non_executable_rules() {
    let expression = BWParser::parse(Rule::expression, "2 + 3 * 4")
        .unwrap()
        .next()
        .unwrap();
    assert!(matches!(
        botwork(expression, &mut Context::default()),
        Ok(Literal::Int(14))
    ));
    let header = BWParser::parse(Rule::stmt_header, "Named |value|")
        .unwrap()
        .next()
        .unwrap();
    assert!(matches!(
        botwork(header, &mut Context::default()),
        Err(BWErr::ParsingError(_))
    ));
}

#[test]
fn pair_blocks_discard_normal_values_and_reject_escaping_control() {
    for source in [
        "{}",
        "{ |value| = |7| }",
        "{ For |item| In |[1]| { Break } }",
    ] {
        let pair = BWParser::parse(Rule::stmt_block, source)
            .unwrap()
            .next()
            .unwrap();
        let result = botwork(pair, &mut Context::default());
        assert!(matches!(result, Ok(Literal::None)), "{source}: {result:?}");
    }
    for source in ["{ Return |7| }", "{ Break }", "{ Continue }"] {
        let pair = BWParser::parse(Rule::stmt_block, source)
            .unwrap()
            .next()
            .unwrap();
        assert!(matches!(
            botwork(pair, &mut Context::default()),
            Err(BWErr::ControlFlowError(_))
        ));
    }
}
