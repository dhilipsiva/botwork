use botwork::core::{
    ast::{Program, Statement, StatementKind},
    eval::{botwork, evaluate_program, execute_statement, Context},
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

fn extracted_return() -> Statement {
    let program = Program::parse("definition.botwork", "Holder {\n Return |7|\n}").unwrap();
    let StatementKind::Define(definition) = program.statements[0].kind() else {
        panic!("expected definition");
    };
    definition.body.statements[0].clone()
}

#[test]
fn assembled_programs_are_validated_before_any_assignment_or_definition() {
    let mut program =
        Program::parse("assembled.botwork", "Installed {}\n|sentinel| = |99|").unwrap();
    program.statements.push(extracted_return());
    let error = program.validate().unwrap_err();
    assert!(matches!(error, BWErr::ControlFlowError(message)
        if message == "definition.botwork:2:2: Return requires a custom-statement body"));

    let mut context = Context::default();
    let setup = Program::parse("setup.botwork", "|sentinel| = |3|").unwrap();
    evaluate_program(&setup, &mut context).unwrap();
    assert!(matches!(
        evaluate_program(&program, &mut context),
        Err(BWErr::ControlFlowError(_))
    ));
    let read = Program::parse("read.botwork", "|result| = |sentinel|").unwrap();
    assert!(matches!(
        evaluate_program(&read, &mut context),
        Ok(Literal::Int(3))
    ));
    let call = Program::parse("call.botwork", "Installed").unwrap();
    assert!(matches!(
        evaluate_program(&call, &mut context),
        Err(BWErr::StatementNotDefined(_))
    ));
}

#[test]
fn extracted_controls_are_revalidated_at_the_public_statement_boundary() {
    let mut context = Context::default();
    let error = execute_statement(&extracted_return(), &mut context).unwrap_err();
    assert!(matches!(error, BWErr::ControlFlowError(message)
        if message.starts_with("definition.botwork:2:2: Return")));

    let program = Program::parse("loop.botwork", "While |false| {\n Break\n Continue\n}").unwrap();
    let StatementKind::While { body, .. } = program.statements[0].kind() else {
        panic!("expected While");
    };
    for statement in &body.statements {
        assert!(matches!(
            execute_statement(statement, &mut context),
            Err(BWErr::ControlFlowError(_))
        ));
    }
}

#[test]
fn pair_validation_checks_unselected_blocks_before_their_side_effects() {
    for (rule, source, keyword, location) in [
        (Rule::stmt_if, "If |false| { Break }", "Break", "1:14"),
        (Rule::stmt_define, "Unused { Continue }", "Continue", "1:10"),
        (
            Rule::stmt_block,
            "{ |sentinel| = |99|\n If |false| { Return } }",
            "Return",
            "2:15",
        ),
        (
            Rule::stmt_else,
            "Else { If |false| { Break } }",
            "Break",
            "1:21",
        ),
        (
            Rule::stmt_catch,
            "Catch { If |false| { Continue } }",
            "Continue",
            "1:22",
        ),
    ] {
        let pair = BWParser::parse(rule, source).unwrap().next().unwrap();
        let mut context = Context::default();
        let setup = Program::parse("setup.botwork", "|sentinel| = |3|").unwrap();
        evaluate_program(&setup, &mut context).unwrap();
        let error = botwork(pair, &mut context).unwrap_err();
        assert!(
            matches!(&error, BWErr::ControlFlowError(message)
            if message.starts_with(&format!("<input>:{location}: {keyword}"))),
            "{source}: {error}"
        );
        let read = Program::parse("read.botwork", "|result| = |sentinel|").unwrap();
        assert!(matches!(
            evaluate_program(&read, &mut context),
            Ok(Literal::Int(3))
        ));
    }
}

#[test]
fn extracted_branch_is_validated_before_its_first_side_effect() {
    let program = Program::parse(
        "definition.botwork",
        "Holder {\n If |true| {\n |sentinel| = |99|\n Return\n}\n}",
    )
    .unwrap();
    let StatementKind::Define(definition) = program.statements[0].kind() else {
        panic!("expected definition");
    };
    let mut context = Context::default();
    let setup = Program::parse("setup.botwork", "|sentinel| = |3|").unwrap();
    evaluate_program(&setup, &mut context).unwrap();
    assert!(matches!(
        execute_statement(&definition.body.statements[0], &mut context),
        Err(BWErr::ControlFlowError(_))
    ));
    let read = Program::parse("read.botwork", "|result| = |sentinel|").unwrap();
    assert!(matches!(
        evaluate_program(&read, &mut context),
        Ok(Literal::Int(3))
    ));
}

#[test]
fn extracted_pair_control_reports_its_original_nonzero_offset() {
    let source = "# café\r\nHolder {\r\n\tReturn |7|\r\n}";
    let pair = BWParser::parse(Rule::botwork, source)
        .unwrap()
        .next()
        .unwrap()
        .into_inner()
        .nth(1)
        .unwrap()
        .into_inner()
        .next()
        .unwrap();
    assert_eq!(pair.as_rule(), Rule::stmt_return);
    let error = botwork(pair, &mut Context::default()).unwrap_err();
    assert!(matches!(error, BWErr::ControlFlowError(message)
        if message == "<input>:3:2: Return requires a custom-statement body"));
}
