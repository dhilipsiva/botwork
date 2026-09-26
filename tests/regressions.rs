//! Expected language behavior for independently reproduced defects.
//! Remove each ignore when its corresponding TODO is implemented.

use botwork::core::{
    eval::{botwork, Context},
    grammar::{BWErr, BWParser, Literal, LiteralResult, Rule},
};
use pest::Parser;
use std::path::Path;
use std::process::Command;

fn evaluate(source: &str) -> LiteralResult {
    let mut context = Context::default();
    context.init_statements();
    let tree = BWParser::parse(Rule::botwork, source)
        .map_err(|err| BWErr::ParsingError(err.to_string()))?;
    let mut result = Literal::None;
    for pair in tree.filter(|pair| pair.as_rule() != Rule::EOI) {
        result = botwork(pair, &mut context)?;
    }
    Ok(result)
}

fn assert_int(source: &str, expected: i32) {
    let result = evaluate(source).unwrap();
    assert!(
        matches!(result, Literal::Int(value) if value == expected),
        "expected {expected}, got {result:?}"
    );
}

#[test]
#[ignore = "known defect: While returns after its first ordinary iteration"]
fn while_repeats_until_the_condition_is_false() {
    assert_int(
        "|i| = |0|\nWhile |i < 3| {\n |i| = |i + 1|\n}\n|answer| = |i|",
        3,
    );
}

#[test]
#[ignore = "known defect: a final Return yields an array and leaves a pending interrupt"]
fn final_return_produces_a_scalar() {
    assert_int("Get value {\n Return |7|\n}\n|answer| = Get value", 7);
}

#[test]
#[ignore = "known defect: nested blocks consume Return before the invocation boundary"]
fn nested_return_skips_remaining_outer_statements() {
    assert_int(
        "Get value {\n If |true| {\n Return |7|\n |unused| = |0|\n }\n\
         |failure| = |missing|\n}\n|answer| = Get value",
        7,
    );
}

#[test]
#[ignore = "known defect: binding the first parameter changes later argument evaluation"]
fn all_arguments_are_evaluated_in_caller_scope() {
    assert_int(
        "|x| = |10|\nPair |x| with |y| {\n Return |y|\n |unused| = |0|\n}\n\
         |answer| = Pair |1| with |x|",
        10,
    );
}

#[test]
#[ignore = "known defect: parameter bindings overwrite caller variables"]
fn invocation_preserves_caller_variables() {
    assert_int(
        "|x| = |10|\nUse value |x| {\n |local| = |x|\n}\n\
         Use value |1|\n|answer| = |x|",
        10,
    );
}

#[test]
#[ignore = "known defect: comparison binds more tightly than addition"]
fn arithmetic_binds_before_comparison() {
    let result = evaluate("|answer| = |1 + 2 == 3|").unwrap();
    assert!(matches!(result, Literal::Bool(true)), "{result:?}");
}

#[test]
#[ignore = "known defect: exponentiation is left associative"]
fn exponentiation_is_right_associative() {
    assert_int("|answer| = |2 ^ 3 ^ 2|", 512);
}

#[test]
#[ignore = "known defect: and eagerly evaluates its right operand"]
fn false_and_does_not_evaluate_the_right_operand() {
    let result = evaluate("|answer| = |false and missing|").unwrap();
    assert!(matches!(result, Literal::Bool(false)), "{result:?}");
}

#[test]
#[ignore = "known defect: or eagerly evaluates its right operand"]
fn true_or_does_not_evaluate_the_right_operand() {
    let result = evaluate("|answer| = |true or missing|").unwrap();
    assert!(matches!(result, Literal::Bool(true)), "{result:?}");
}

#[test]
#[ignore = "known defect: string values retain source quotation marks"]
fn string_concatenation_combines_decoded_values() {
    let result = evaluate("|answer| = |(\"a\" + \"b\") == \"ab\"|").unwrap();
    assert!(matches!(result, Literal::Bool(true)), "{result:?}");
}

#[test]
#[ignore = "known defect: string escape sequences remain encoded"]
fn string_escapes_are_decoded() {
    let result = evaluate(r#"|answer| = |"a\nb\"c\\d"|"#).unwrap();
    assert!(
        matches!(&result, Literal::String(value) if value == "a\nb\"c\\d"),
        "{result:?}"
    );
}

#[test]
#[ignore = "known defect: reserved-word prefixes reject otherwise valid identifiers"]
fn identifiers_can_begin_with_reserved_word_text() {
    assert_int("|order| = |7|\n|answer| = |order|", 7);
}

#[test]
#[ignore = "known defect: control-keyword prefixes reject natural statement names"]
fn statement_names_can_begin_with_control_keyword_text() {
    assert_int(
        "Format report {\n Return |7|\n |unused| = |0|\n}\n\
         |answer| = Format report",
        7,
    );
}

#[test]
#[ignore = "known defect: parsed dot access reaches unreachable code"]
fn collection_access_returns_a_value_or_an_explicit_unsupported_error() {
    let source = "|m| = |{a: 7}|\n|answer| = |m.a|";
    BWParser::parse(Rule::botwork, source).expect("collection access is accepted syntax");
    match evaluate(source) {
        Ok(Literal::Int(7)) => (),
        Err(error) => {
            let message = format!("{error:?}").to_lowercase();
            assert!(
                message.contains("access") && message.contains("unsupported"),
                "expected a specific unsupported-access diagnostic, got {error:?}"
            );
        }
        other => panic!("expected value 7 or unsupported access, got {other:?}"),
    }
}

#[test]
#[ignore = "known defect: integer remainder by zero panics instead of returning an error"]
fn arithmetic_failure_can_be_caught() {
    assert_int(
        "|answer| = |0|\nTry {\n |answer| = |1 % 0|\n} Catch {\n\
         |answer| = |42|\n}\n|result| = |answer|",
        42,
    );
}

#[test]
#[ignore = "known defect: grammar accepts Try without the Catch required by evaluation"]
fn try_requires_a_catch_clause() {
    assert!(BWParser::parse(Rule::botwork, "Try {\n |x| = |1|\n}").is_err());
}

fn assert_cli_failure(fixture: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--file")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "script failure must not exit successfully"
    );
    assert!(output.stdout.is_empty(), "diagnostics belong on stderr");
    assert!(!output.stderr.is_empty(), "failure needs a diagnostic");
}

#[test]
fn syntax_errors_fail_the_cli() {
    assert_cli_failure("syntax-error.botwork");
}

#[test]
fn runtime_errors_fail_the_cli() {
    assert_cli_failure("runtime-error.botwork");
}
