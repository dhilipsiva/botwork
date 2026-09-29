use super::*;

fn check(source: &str) -> Vec<Finding> {
    Analyzer::default().check_program(&Program::parse_detailed("test.botwork", source).unwrap())
}

fn rules(source: &str) -> Vec<(Rule, String)> {
    check(source)
        .into_iter()
        .map(|finding| (finding.rule, finding.span.text().to_owned()))
        .collect()
}

#[test]
fn clean_programs_have_no_findings() {
    let source = r#"If |!@{ Variable Exists |"name"| }| {
    |name| = |"Ada"|
}
Greet |person| with |greeting| {
    Return |greeting + ", " + person + "!"|
}
Log |@{ greet |name| WITH |"Welcome"| }|
|total| = |0|
For |price| In |[3, 4, 5]| {
    |total| = |total + price|
}
Try { Fail |"no"| } Catch |error| { Log |error.code| }
Import |"helpers.botwork"| As |helpers|
Log |@{ helpers::Double |2| }|
"#;
    assert!(check(source).is_empty(), "{:?}", check(source));
}

#[test]
fn undefined_statements_name_the_call_and_suggest_matching_words() {
    let findings = check("Log |1| |2|\nLog |@{ Missing words }|\nLog |@{ nope::Thing }|\n");
    let found: Vec<_> = findings
        .iter()
        .map(|finding| (finding.rule, finding.span.text(), finding.help.as_str()))
        .collect();
    assert_eq!(found.len(), 3, "{findings:?}");
    assert_eq!(found[0].0, Rule::UndefinedStatement);
    assert_eq!(found[0].1, "Log |1| |2|");
    assert!(
        found[0].2.contains("Did you mean `Log |value|`?"),
        "{}",
        found[0].2
    );
    assert_eq!(found[1].1, "Missing words ");
    assert!(!found[1].2.contains("Did you mean"));
    assert_eq!(found[2].1, "nope::Thing ");
    assert_eq!(findings[2].message, "No module is imported as `nope`");
    assert!(findings
        .iter()
        .all(|finding| finding.severity() == Severity::Error));
}

#[test]
fn definitions_register_in_order_but_bodies_resolve_when_called() {
    assert_eq!(
        rules("Log |@{ Later }|\nLater { Return |1| }\n"),
        [(Rule::StatementBeforeDefinition, "Later ".into())]
    );
    // Called from a body, a root definition registered later still resolves.
    assert!(
        rules("Outer { Return |@{ Inner }| }\nInner { Return |1| }\nLog |@{ Outer }|\n").is_empty()
    );
    // A built-in answers calls made before a (colliding) root redefinition.
    assert_eq!(
        rules("Log |1|\nLog |x| { Return |x| }\n"),
        [(Rule::DuplicateStatement, "Log |x| { Return |x| }".into())]
    );
    // Inside a body, its own nested definitions also register in order.
    assert_eq!(
        rules("Outer {\n    Inner\n    Inner { Return |1| }\n}\n"),
        [(Rule::StatementBeforeDefinition, "Inner".into())]
    );
}

#[test]
fn duplicates_are_scoped_and_only_the_root_cannot_shadow_built_ins() {
    let findings = check("Dup { Return |1| }\nIf |true| { dup { Return |2| } }\nOuter {\n    Log |x| { Return |x| }\n    Log |1|\n}\nOther {\n    Twice { Return |1| }\n    Twice { Return |2| }\n}\n");
    let found: Vec<_> = findings
        .iter()
        .map(|finding| (finding.rule, finding.span.text()))
        .collect();
    assert_eq!(
        found,
        [
            (Rule::DuplicateStatement, "dup { Return |2| }"),
            (Rule::DuplicateStatement, "Twice { Return |2| }"),
        ]
    );
    assert!(
        findings[0]
            .message
            .contains("first defined at test.botwork:1:1"),
        "{}",
        findings[0].message
    );
    assert_eq!(
        findings[0].rule.code(),
        Some(DiagnosticCode::DuplicateStatement)
    );
}

#[test]
fn undefined_variables_are_reported_once_and_bindings_count() {
    let source = r#"Log |missing|
Log |missing|
Use |parameter| {
    Log |parameter + outer + nested|
    |nested| = |1|
}
|outer| = |2|
For |item| In |[1]| { Log |item| }
Try { Fail |"x"| } Catch |error| { Log |error| }
While |later < 1| { |later| = |2| }
"#;
    assert_eq!(rules(source), [(Rule::UndefinedVariable, "missing".into())]);
    let finding = &check(source)[0];
    assert_eq!(finding.severity(), Severity::Warning);
    assert!(finding.help.contains("--var"), "{}", finding.help);
}

#[test]
fn statements_after_a_block_exit_are_unreachable() {
    let source = r#"Exits |flag| {
    If |flag| { Return |1| } Else { Return |2| }
    Log |"never"|
    Log |"also never, reported once"|
}
Loop {
    For |item| In |[1]| {
        Continue
        Log |item|
    }
    While |true| { Break
        Log |"no"| }
    Try { Return |1| } Catch { Return |2| }
    Log |"after try"|
}
Maybe |flag| {
    If |flag| { Return |1| }
    Log |"reachable"|
    Try { Return |1| } Catch { Log |"handled"| }
    Log |"reachable after a handler"|
}
Fail |"stop"|
Log |"after fail"|
"#;
    let found: Vec<_> = rules(source)
        .into_iter()
        .map(|(rule, text)| {
            assert_eq!(rule, Rule::UnreachableCode);
            text
        })
        .collect();
    assert_eq!(
        found,
        [
            "Log |\"never\"|",
            "Log |item|",
            "Log |\"no\"|",
            "Log |\"after try\"|",
            "Log |\"after fail\"|",
        ]
    );
    assert_eq!(Rule::UnreachableCode.code(), None);
}

#[test]
fn only_the_first_unreachable_statement_of_a_block_is_reported() {
    let findings = check("Twice {\n    Return |1|\n    Return |2|\n    Log |3|\n}\n");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].span.text(), "Return |2|");
    assert!(
        findings[0].help.contains("test.botwork:2:5"),
        "{}",
        findings[0].help
    );
}

#[test]
fn a_redefined_fail_does_not_leave_its_block() {
    assert!(
        rules("Outer {\n    Fail |x| { Return |x| }\n    Fail |1|\n    Log |\"runs\"|\n}\n")
            .is_empty()
    );
}

#[test]
fn literal_arguments_and_conditions_must_have_accepted_kinds() {
    let source = r#"Sleep |"10"|
Sleep |-5|
Log |@{ String Length |-5| }|
Wrap {
    Sleep |value| { Return |value| }
    Sleep |"shadowed"|
}
If |1| { Log |1| }
While |"x"| { Break }
For |x| In |"abc"| { Log |x| }
For |x| In |{a: 1}| { Log |x| }
If |1 > 0| { Log |1| }
"#;
    let findings = check(source);
    let found: Vec<_> = findings
        .iter()
        .map(|finding| (finding.rule, finding.span.text()))
        .collect();
    assert_eq!(
        found,
        [
            (Rule::ArgumentKind, "\"10\""),
            (Rule::ArgumentKind, "-5"),
            (Rule::ConditionKind, "1"),
            (Rule::ConditionKind, "\"x\""),
            (Rule::ConditionKind, "\"abc\""),
            (Rule::ConditionKind, "{a: 1}"),
        ]
    );
    assert_eq!(
        findings[0].message,
        "Parameter `milliseconds` (argument 1) of `sleep|param|` requires Int; got String"
    );
    assert!(
        findings[1].message.ends_with("requires String; got Int"),
        "{}",
        findings[1].message
    );
    assert_eq!(findings[2].message, "If requires a boolean condition");
    assert_eq!(findings[3].message, "While requires a boolean condition");
    assert_eq!(findings[4].message, "For requires an array to iterate over");
}

#[test]
fn findings_render_their_range_rule_code_and_help() {
    let finding = &check("Log |1|\nLog |1| |2|\n")[0];
    assert_eq!(
        finding.to_string(),
        "test.botwork:2:1-2:12: error[undefined-statement]: [BW2002] Statement not defined: Log |1| |2|\n  help: Did you mean `Log |value|`? Calls must match a definition's words and parameter positions."
    );
    let warning = &check("Log |x|\n")[0];
    assert!(warning
        .to_string()
        .starts_with("test.botwork:1:6-1:7: warning[undefined-variable]: [BW2001] "));
}

#[test]
fn every_rule_has_a_distinct_name_severity_and_code() {
    let names: BTreeSet<_> = Rule::ALL.iter().map(|rule| rule.as_str()).collect();
    assert_eq!(names.len(), Rule::ALL.len());
    let errors: Vec<_> = Rule::ALL
        .iter()
        .filter(|rule| rule.severity() == Severity::Error)
        .map(|rule| rule.as_str())
        .collect();
    assert_eq!(
        errors,
        [
            "undefined-statement",
            "duplicate-statement",
            "argument-kind",
            "condition-kind"
        ]
    );
    assert_eq!(Severity::Error.as_str(), "error");
    assert_eq!(Severity::Warning.as_str(), "warning");
}

#[test]
fn host_statements_resolve_calls() {
    let program = Program::parse_detailed("test.botwork", "Fetch |1|\n").unwrap();
    assert_eq!(Analyzer::default().check_program(&program).len(), 1);
    let host = StatementSignature::native("Fetch |id|").unwrap();
    assert!(Analyzer::with_statements([&host])
        .check_program(&program)
        .is_empty());
}

#[test]
fn suites_share_setup_variables_and_row_bindings_and_report_library_findings_once() {
    let suite = Suite::parse(
        "test.suite.botwork",
        r#"Suite |"s"| {
    Dataset |"rows"| {
        Row |"one"| Values |1|
    }
    Library {
        Helper { Return |@{ Undefined helper }| }
    }
    SuiteSetup { |shared| = |40| }
    SuiteTeardown { Log |shared| }
    Case |"first"| Using |"rows"| As |row| { Log |shared + row| }
    Case |"second"| { Log |row| }
}"#,
    )
    .unwrap();
    let findings = Analyzer::default().check_suite(&suite);
    let found: Vec<_> = findings
        .iter()
        .map(|finding| (finding.rule, finding.span.text()))
        .collect();
    assert_eq!(
        found,
        [
            (Rule::UndefinedStatement, "Undefined helper "),
            (Rule::UndefinedVariable, "row"),
        ]
    );
}
