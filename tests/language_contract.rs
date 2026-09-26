//! Executable expectations for docs/language-specification.md.
//! Ignored cases are specified behavior awaiting their implementation TODO.

use botwork::core::{
    eval::{botwork, Context},
    grammar::{BWErr, BWParser, Literal, LiteralResult, Rule},
};
use pest::Parser;

fn evaluate(source: &str) -> LiteralResult {
    let tree = BWParser::parse(Rule::botwork, source)
        .map_err(|error| BWErr::ParsingError(error.to_string()))?;
    let mut context = Context::default();
    context.init_statements();
    let mut result = Literal::None;
    for pair in tree.filter(|pair| pair.as_rule() != Rule::EOI) {
        result = botwork(pair, &mut context)?;
    }
    Ok(result)
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
    // Keep lookup independent of the pending final-Return correction.
    let result = evaluate(
        "|x| = |10|\nRead value {\n Return |x|\n |unreachable| = |missing|\n}\n\
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
                Err(BWErr::VariableNotDefined(name)) if name == "missing_first"
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
#[ignore = "specified behavior: custom fallthrough must yield None instead of block-result arrays"]
fn custom_fallthrough_returns_none() {
    for body in ["", "|local| = |7|", "If |true| { |local| = |7| }"] {
        let source = format!("Do work {{\n {body}\n}}\n|answer| = Do work");
        let result = evaluate(&source);
        assert!(matches!(result, Ok(Literal::None)), "{source}: {result:?}");
    }
}

#[test]
#[ignore = "specified behavior: bare Return must yield None at the invocation boundary"]
fn bare_return_returns_none() {
    let result = evaluate("Do work {\n Return\n}\n|answer| = Do work");
    assert!(matches!(result, Ok(Literal::None)), "{result:?}");
}

#[test]
#[ignore = "specified behavior: free variables resolve through the defining environment"]
fn a_helper_reads_its_lexical_environment_not_its_callers_parameters() {
    // Trailing statements isolate lexical lookup from the recorded final-Return defect.
    let result = evaluate(
        "|x| = |10|\nRead value {\n Return |x|\n |unused| = |0|\n}\n\
         Call helper |x| {\n |value| = Read value\n Return |value|\n |unused| = |0|\n}\n\
         |answer| = Call helper |1|",
    );
    assert!(matches!(result, Ok(Literal::Int(10))), "{result:?}");
}

#[test]
#[ignore = "specified behavior: unrelated caller-local bindings must not be visible to callees"]
fn a_helper_cannot_read_an_unrelated_callers_local() {
    let result = evaluate(
        "Read private {\n |copy| = |private|\n}\n\
         Caller {\n |private| = |7|\n Read private\n}\nCaller",
    );
    assert!(matches!(result, Err(BWErr::VariableNotDefined(name)) if name == "private"));
}

#[test]
#[ignore = "specified behavior: failed argument evaluation must leave caller bindings intact"]
fn a_failed_argument_does_not_bind_earlier_parameters() {
    let result = evaluate(
        "|x| = |10|\nPair |x| with |y| {}\n\
         Try {\n Pair |1| with |missing|\n} Catch {}\n|answer| = |x|",
    );
    assert!(matches!(result, Ok(Literal::Int(10))), "{result:?}");
}

#[test]
#[ignore = "specified behavior: invocation-local assignments must not overwrite caller bindings"]
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
#[ignore = "specified behavior: For must restore its previous variable binding on every exit"]
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
#[ignore = "specified behavior: nested statement definitions must not escape their invocation"]
fn nested_definitions_disappear_when_the_defining_invocation_finishes() {
    let result = evaluate("Outer {\n Inner {}\n}\nOuter\nInner");
    assert!(matches!(result, Err(BWErr::StatementNotDefined(name)) if name == "Inner"));
}

#[test]
#[ignore = "specified behavior: Return propagates through loops and Try without invoking Catch"]
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
