//! Executable expectations for docs/language-specification.md.
//! Ignored cases are specified behavior awaiting their implementation TODO.

use botwork::core::{
    ast::Program,
    eval::{evaluate_program, Context},
    grammar::{BWErr, Literal, LiteralResult},
};

fn evaluate(source: &str) -> LiteralResult {
    let program = Program::parse("<contract>", source)?;
    let mut context = Context::default();
    context.init_statements();
    evaluate_program(&program, &mut context)
}

#[test]
fn resolve_a_call_before_evaluating_its_arguments() {
    assert!(matches!(
        evaluate("Unknown statement |missing|"),
        Err(BWErr::StatementNotDefined(name)) if name == "Unknown statement |missing|"
    ));
}

#[test]
fn definitions_become_available_when_executed() {
    assert!(matches!(
        evaluate("Later\nLater {}"),
        Err(BWErr::StatementNotDefined(name)) if name == "Later"
    ));
}

#[test]
fn a_definition_reads_updated_run_bindings() {
    let result = evaluate(
        "|x| = |10|\nRead value {\n Return |x|\n}\n\
         |x| = |11|\n|answer| = Read value",
    );
    assert!(matches!(result, Ok(Literal::Int(11))), "{result:?}");
}

#[test]
fn collections_report_the_first_source_order_error() {
    for expression in [
        "[missing_first, missing_second]",
        "{z: missing_first, a: missing_second}",
        "[1, {nested: missing_first}, missing_second]",
    ] {
        assert!(
            matches!(
                evaluate(&format!("|answer| = |{expression}|")),
                Err(BWErr::VariableNotDefined { name, .. }) if name == "missing_first"
            ),
            "{expression}"
        );
    }
}

#[test]
fn ordering_comparisons_do_not_form_mathematical_chains() {
    assert!(matches!(
        evaluate("|answer| = |1 < 2 < 3|"),
        Err(BWErr::OperationIncompatibleError(_))
    ));
    let result = evaluate("|answer| = |1 == 2 == false|");
    assert!(matches!(result, Ok(Literal::Bool(true))), "{result:?}");
}

#[test]
fn for_evaluates_its_array_once() {
    let result = evaluate(
        "|items| = |[1, 2, 3]|\n|sum| = |0|\n\
         For |item| In |items| {\n |sum| = |sum + item|\n |items| = |[]|\n}\n\
         |answer| = |sum|",
    );
    assert!(matches!(result, Ok(Literal::Int(6))), "{result:?}");
}

#[test]
fn conditions_require_booleans_without_truthiness_conversion() {
    for value in ["0", "1", "0.0", r#""""#, "[]", "{}"] {
        for keyword in ["If", "While"] {
            let source = format!("{keyword} |{value}| {{}}");
            assert!(
                matches!(evaluate(&source), Err(BWErr::OperationIncompatibleError(_))),
                "{source}"
            );
        }
    }
}

#[test]
fn catch_preserves_completed_work_and_a_failed_assignment_destination() {
    let result = evaluate(
        "|answer| = |7|\n|completed| = |0|\nTry {\n\
         |completed| = |2|\n |answer| = |missing|\n |completed| = |99|\n\
         } Catch {\n |completed| = |completed + 1|\n}\n\
         |result| = |answer + completed|",
    );
    assert!(matches!(result, Ok(Literal::Int(10))), "{result:?}");
}

#[test]
fn custom_fallthrough_returns_none() {
    for body in ["", "|local| = |7|", "If |true| { |local| = |7| }"] {
        let source = format!("Do work {{\n {body}\n}}\n|answer| = Do work");
        let result = evaluate(&source);
        assert!(matches!(result, Ok(Literal::None)), "{source}: {result:?}");
    }
}

#[test]
fn bare_return_returns_none() {
    let result = evaluate("Do work {\n Return\n}\n|answer| = Do work");
    assert!(matches!(result, Ok(Literal::None)), "{result:?}");
}

#[test]
fn a_helper_reads_its_lexical_environment_not_its_callers_parameters() {
    let result = evaluate(
        "|x| = |10|\nRead value {\n Return |x|\n}\n\
         Call helper |x| {\n |value| = Read value\n Return |value|\n}\n\
         |answer| = Call helper |1|",
    );
    assert!(matches!(result, Ok(Literal::Int(10))), "{result:?}");
}

#[test]
fn a_helper_cannot_read_an_unrelated_callers_local() {
    let result = evaluate(
        "Read private {\n |copy| = |private|\n}\n\
         Caller {\n |private| = |7|\n Read private\n}\nCaller",
    );
    assert!(matches!(result, Err(BWErr::VariableNotDefined { name, .. }) if name == "private"));
}

#[test]
fn a_failed_argument_does_not_bind_earlier_parameters() {
    let result = evaluate(
        "|x| = |10|\nPair |x| with |y| {}\n\
         Try {\n Pair |1| with |missing|\n} Catch {}\n|answer| = |x|",
    );
    assert!(matches!(result, Ok(Literal::Int(10))), "{result:?}");
}

#[test]
fn local_assignments_are_discarded_after_normal_completion_and_error() {
    for ending in ["", "|failure| = |missing|"] {
        let source = format!(
            "|x| = |10|\nDo work {{\n |x| = |1|\n {ending}\n}}\n\
             Try {{ Do work }} Catch {{}}\n|answer| = |x|"
        );
        let result = evaluate(&source);
        assert!(
            matches!(result, Ok(Literal::Int(10))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn for_restores_its_binding_after_completion_break_and_error() {
    for body in ["", "Break", "|failure| = |missing|"] {
        let source = format!(
            "|item| = |10|\nTry {{\n For |item| In |[1, 2]| {{\n {body}\n }}\n\
             }} Catch {{}}\n|answer| = |item|"
        );
        let result = evaluate(&source);
        assert!(
            matches!(result, Ok(Literal::Int(10))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn nested_definitions_disappear_when_the_defining_invocation_finishes() {
    let result = evaluate("Outer {\n Inner {}\n}\nOuter\nInner");
    assert!(matches!(result, Err(BWErr::StatementNotDefined(name)) if name == "Inner"));
}

#[test]
fn returns_cross_loops_and_try_blocks_without_running_handlers() {
    for nested in [
        "For |item| In |[1]| {\n Return |7|\n |failure| = |missing_inner|\n}",
        "While |again| {\n |again| = |false|\n Return |7|\n |failure| = |missing_inner|\n}",
        "Try {\n Return |7|\n |failure| = |missing_inner|\n} Catch { |failure| = |missing_handler| }",
    ] {
        let source = format!(
            "Get value {{\n |again| = |true|\n {nested}\n |failure| = |missing_after_return|\n}}\n\
             |answer| = Get value"
        );
        let result = evaluate(&source);
        assert!(
            matches!(result, Ok(Literal::Int(7))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn return_position_and_unreachable_statements_do_not_change_the_value() {
    for trailing in ["", "|unreachable| = |missing|"] {
        let source = format!("Get value {{\n Return |7|\n {trailing}\n}}\n|answer| = Get value");
        let result = evaluate(&source);
        assert!(
            matches!(result, Ok(Literal::Int(7))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn return_preserves_the_kind_of_each_value() {
    for (expression, expected) in [
        ("true", "bool"),
        ("7", "int"),
        ("7.5", "float"),
        (r#""answer""#, "string"),
        ("[1, 2]", "array"),
        ("{answer: 7}", "map"),
    ] {
        let source = format!("Get value {{ Return |{expression}| }}\n|answer| = Get value");
        let result = evaluate(&source).unwrap();
        let matches_value = match (&result, expected) {
            (Literal::Bool(value), "bool") => *value,
            (Literal::Int(value), "int") => *value == 7,
            (Literal::Float(value), "float") => *value == 7.5,
            (Literal::String(value), "string") => value == "answer",
            (Literal::Array(values), "array") => {
                matches!(values.as_slice(), [Literal::Int(1), Literal::Int(2)])
            }
            (Literal::Map(values), "map") => {
                values.len() == 1 && matches!(values.get("answer"), Some(Literal::Int(7)))
            }
            _ => false,
        };
        assert!(matches_value, "{source}: {result:?}");
    }
}

#[test]
fn repeated_calls_consume_return_outcomes_at_each_invocation_boundary() {
    let result = evaluate(
        "First { Return |1| }\nSecond { Return |2| }\nEmpty {}\n\
         Wrapper {\n |inner| = First\n |inner| = |inner + 10|\n Return |inner|\n}\n\
         |one| = First\nEmpty\n|two| = Second\n|eleven| = Wrapper\n\
         |again| = First\n|answer| = |[one, two, eleven, again]|",
    );
    assert!(
        matches!(&result, Ok(Literal::Array(values)) if matches!(values.as_slice(), [Literal::Int(1), Literal::Int(2), Literal::Int(11), Literal::Int(1)])),
        "{result:?}"
    );
}

#[test]
fn normal_control_constructs_return_none_without_collecting_body_values() {
    for source in [
        "Definition {}",
        "If |true| { |value| = |7| }",
        "If |false| { |value| = |missing| }",
        "If |false| {} Else { |value| = |7| }",
        "If |false| {} Else If |true| { |value| = |7| }",
        "For |item| In |[1, 2]| { |value| = |item| }",
        "For |item| In |[]| { |value| = |missing| }",
        "|count| = |0|\nWhile |count < 2| { |count| = |count + 1| }",
        "While |false| { |value| = |missing| }",
        "Try { |value| = |7| } Catch { |value| = |missing| }",
        "Try { |value| = |missing| } Catch { |value| = |7| }",
    ] {
        let result = evaluate(source);
        assert!(matches!(result, Ok(Literal::None)), "{source}: {result:?}");
    }
}

#[test]
fn nested_if_and_catch_returns_exit_the_containing_custom_statement() {
    for body in [
        "If |true| { Return |7| }",
        "If |false| {} Else { Return |7| }",
        "If |false| {} Else If |true| { Return |7| }",
        "Try { |failure| = |missing| } Catch { Return |7| }",
        "Try {\n Try { |failure| = |missing| } Catch { Return |7| }\n\
         } Catch { |failure| = |missing_outer_handler| }",
        "For |item| In |[1]| {\n Try {\n If |true| { Return |7| }\n\
         } Catch { |failure| = |missing_handler| }\n}",
    ] {
        let source = format!(
            "Get value {{\n {body}\n |failure| = |missing_after_return|\n}}\n|answer| = Get value"
        );
        let result = evaluate(&source);
        assert!(
            matches!(result, Ok(Literal::Int(7))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn failed_return_expression_is_catchable_before_a_return_outcome_exists() {
    let result = evaluate(
        "Get value {\n Try {\n Return |missing|\n\
         } Catch {\n |recovered| = |7|\n}\n Return |recovered|\n}\n|answer| = Get value",
    );
    assert!(matches!(result, Ok(Literal::Int(7))), "{result:?}");
    let result = evaluate(
        "Get value {\n Try { Return |missing_body| } Catch { Return |missing_handler| }\n}\nGet value",
    );
    assert!(
        matches!(result, Err(BWErr::VariableNotDefined { name, .. }) if name == "missing_handler")
    );
}

#[test]
fn break_and_continue_cross_if_try_and_catch_without_running_unused_handlers() {
    for controls in [
        "Try {\n If |item == 2| { Continue }\n If |item == 4| { Break }\n\
         } Catch { |failure| = |missing_handler| }",
        "Try { |failure| = |missing_body| } Catch {\n\
         If |item == 2| { Continue }\n If |item == 4| { Break }\n}",
    ] {
        for loop_source in [
            format!("For |item| In |[1, 2, 3, 4, 5]| {{\n {controls}\n |sum| = |sum + item|\n}}"),
            format!(
                "While |item < 5| {{\n |item| = |item + 1|\n {controls}\n |sum| = |sum + item|\n}}"
            ),
        ] {
            let source = format!("|item| = |0|\n|sum| = |0|\n{loop_source}\n|answer| = |sum|");
            let result = evaluate(&source);
            assert!(
                matches!(result, Ok(Literal::Int(4))),
                "{source}: {result:?}"
            );
        }
    }
}

#[test]
fn nested_loops_consume_only_their_own_break_and_continue() {
    let result = evaluate(
        "|sum| = |0|\nFor |outer| In |[1, 2, 3]| {\n\
         For |inner| In |[1, 2, 3]| {\n If |inner == 2| { Continue }\n\
         If |inner == 3| { Break }\n |sum| = |sum + 1|\n}\n\
         |sum| = |sum + 10|\n}\n|answer| = |sum|",
    );
    assert!(matches!(result, Ok(Literal::Int(33))), "{result:?}");
}

#[test]
fn a_callees_loops_and_return_do_not_interrupt_its_callers_loop() {
    let result = evaluate(
        "Get one {\n |count| = |0|\n For |inner| In |[1, 2, 3]| {\n\
         If |inner == 2| { Continue }\n If |inner == 3| { Break }\n\
         |count| = |count + 1|\n}\n Return |count|\n}\n\
         |sum| = |0|\nFor |outer| In |[1, 2, 3]| {\n |value| = Get one\n\
         |sum| = |sum + value + 10|\n}\n|answer| = |sum|",
    );
    assert!(matches!(result, Ok(Literal::Int(33))), "{result:?}");
}
