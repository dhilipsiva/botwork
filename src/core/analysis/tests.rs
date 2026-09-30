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
            "condition-kind",
            "import-failure",
            "import-cycle",
            "duplicate-namespace"
        ]
    );
    assert_eq!(
        Rule::ALL.map(|rule| rule.code().map(|code| code.as_str())),
        [
            Some("BW2002"),
            Some("BW2002"),
            Some("BW2001"),
            Some("BW2003"),
            None,
            Some("BW3003"),
            Some("BW3003"),
            Some("BW6001"),
            Some("BW6002"),
            Some("BW6003")
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

/// A checked program named `main.botwork` in `directory`, with its modules.
fn report_in(directory: &std::path::Path, source: &str) -> Report {
    Analyzer::default()
        .with_modules(directory)
        .report_program(&Program::parse_detailed("main.botwork", source).unwrap())
}

fn write(directory: &std::path::Path, name: &str, text: &str) {
    let path = directory.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn file_name(finding: &Finding) -> String {
    let name = finding.span.source().name();
    std::path::Path::new(name)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

#[test]
fn modules_are_checked_and_their_exports_resolve_qualified_calls() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "lib/math.botwork",
        "Double |x| { Return |x * 2| }\n",
    );
    write(
        directory.path(),
        "lib/wrap.botwork",
        "Import |\"math.botwork\"| As |m|\nQuad |x| { Return |@{ m::Double |@{ m::Double |x| }| }| }\nLog |module_input|\n",
    );
    let report = report_in(
        directory.path(),
        "Import |\"lib/wrap.botwork\"| As |w|\nLog |@{ w::Quad |2| }|\nLog |@{ w::m::Double |2| }|\nLog |@{ w::quad |1| |2| }|\nLog |@{ w::Triple |1| }|\n",
    );
    let found: Vec<_> = report
        .findings
        .iter()
        .map(|finding| {
            (
                finding.rule,
                file_name(finding),
                finding.span.text().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                Rule::UndefinedStatement,
                "main.botwork".into(),
                "w::quad |1| |2|".into()
            ),
            (
                Rule::UndefinedStatement,
                "main.botwork".into(),
                "w::Triple |1|".into()
            ),
            (
                Rule::UndefinedVariable,
                "wrap.botwork".into(),
                "module_input".into()
            ),
        ]
    );
    assert!(
        report.findings[0]
            .help
            .contains("Did you mean `w::Quad |x|`?"),
        "{}",
        report.findings[0].help
    );
    // The module's own reads are not the checked file's input variables.
    assert!(report.inputs.is_empty());
    let modules: Vec<_> = report
        .modules
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(modules, ["math.botwork", "wrap.botwork"]);
    assert!(report.modules.iter().all(|path| path.is_absolute()));
    assert_eq!((report.errors(), report.warnings()), (2, 1));
}

#[test]
fn imports_that_cannot_load_cycle_or_collide_are_reported() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "a.botwork",
        "Import |\"b.botwork\"| As |b|\n",
    );
    write(
        directory.path(),
        "b.botwork",
        "Import |\"a.botwork\"| As |a|\n",
    );
    write(directory.path(), "broken.botwork", "Log |1\n");
    write(directory.path(), "one.botwork", "One { Return |1| }\n");
    let report = report_in(
        directory.path(),
        r#"Import |"missing.botwork"| As |missing|
Import |"http://x/y.botwork"| As |remote|
Import |"one.txt"| As |text|
Import |"a.botwork"| As |a|
Import |"broken.botwork"| As |broken|
Log |@{ early::One }|
Import |"one.botwork"| As |early|
Import |"one.botwork"| As |early|
"#,
    );
    let found: Vec<_> = report
        .findings
        .iter()
        .map(|finding| (finding.rule, finding.span.text().to_owned()))
        .collect();
    assert_eq!(
        found,
        [
            (Rule::ImportFailure, "\"missing.botwork\"".into()),
            (Rule::ImportFailure, "\"http://x/y.botwork\"".into()),
            (Rule::ImportFailure, "\"one.txt\"".into()),
            (Rule::StatementBeforeDefinition, "early::One ".into()),
            (Rule::DuplicateNamespace, "early".into()),
            (Rule::ImportCycle, "\"a.botwork\"".into()),
        ]
    );
    assert!(
        report.findings[0].message.contains("missing.botwork: "),
        "{}",
        report.findings[0].message
    );
    let cycle = &report.findings[5];
    assert_eq!(file_name(cycle), "b.botwork");
    assert!(
        cycle.message.starts_with("Import cycle: ") && cycle.message.matches(" -> ").count() == 2,
        "{}",
        cycle.message
    );
    // The module's syntax error is reported as the run would report it.
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].code(), DiagnosticCode::Syntax);
    assert_eq!(report.errors(), 6);
}

#[test]
fn without_module_analysis_qualified_calls_are_counted_as_unchecked() {
    let program = Program::parse_detailed(
        "main.botwork",
        "Import |\"helpers.botwork\"| As |h|\nLog |@{ h::Anything |1| }|\nLog |@{ h::Other }|\n",
    )
    .unwrap();
    let report = Analyzer::default().report_program(&program);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(report.unchecked_imports, 2);
    assert!(report.modules.is_empty());
}

#[test]
fn inputs_and_calls_that_depend_on_the_outside_world_are_reported() {
    let program = Program::parse_detailed(
        "main.botwork",
        r#"Log |@{ Read File |path| }|
Log |@{ Read File |"b.txt"| }|
Log |@{ HTTP Request |"GET"| To |url| }|
Log |@{ Current Date Time In |"UTC"| }|
Log |@{ Join Path |["a", "b"]| }|
Log |@{ Get Environment Variable |"HOME"| }|
"#,
    )
    .unwrap();
    let report = Analyzer::default().report_program(&program);
    assert_eq!(report.inputs.iter().collect::<Vec<_>>(), ["path", "url"]);
    assert_eq!(
        report.external.into_iter().collect::<Vec<_>>(),
        [
            (External::Files, 2),
            (External::Environment, 1),
            (External::Network, 1),
            (External::Clock, 1)
        ]
    );
}

#[test]
fn every_external_statement_is_a_built_in() {
    let mut context = Context::with_limits(RunLimits::default()).unwrap();
    context.init_statements();
    let catalogue: HashSet<String> = context
        .statement_signatures()
        .into_iter()
        .map(|signature| signature.normalized().to_owned())
        .collect();
    for (header, _) in EXTERNAL_STATEMENTS {
        let normalized = StatementSignature::native(header)
            .unwrap()
            .normalized()
            .to_owned();
        assert!(catalogue.contains(&normalized), "{header}");
    }
    assert_eq!(
        Analyzer::default().external.len(),
        EXTERNAL_STATEMENTS.len()
    );
    assert_eq!(
        [
            External::Files,
            External::Environment,
            External::Processes,
            External::Network,
            External::Clock
        ]
        .map(External::as_str),
        [
            "files",
            "the environment",
            "processes",
            "the network",
            "the clock"
        ]
    );
}

#[test]
fn undefined_variables_suggest_a_near_name_their_scope_reaches() {
    let help = |source: &str| {
        let findings = check(source);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule, Rule::UndefinedVariable);
        findings[0].help.clone()
    };
    assert_eq!(
        help("|discount| = |2|\nLog |10 - discont|\n"),
        "Did you mean `discount`? Otherwise, assign it first, or supply it as an input variable with --var or --vars-file."
    );
    // A definition's parameter, and a variable of the enclosing frame.
    assert!(
        help("Total |quantity| {\n    Return |quantity * prcie|\n}\n|price| = |4|\n")
            .starts_with("Did you mean `price`?")
    );
    // Another definition's parameter is out of reach.
    assert_eq!(
        help("Outer |discount| {\n    Return |discount|\n}\nInner |x| {\n    Return |x - discont|\n}\n"),
        "Assign it first, or supply it as an input variable with --var or --vars-file."
    );
}

#[test]
fn suite_inputs_are_suggested_for_misread_bindings() {
    let suite = Suite::parse(
        "suggest.suite.botwork",
        r#"Suite |"s"| {
    Dataset |"d"| {
        Row |"r"| Values |{total: 1}|
    }
    Case |"c"| Using |"d"| As |order| {
        Log |ordr.total|
    }
}"#,
    )
    .unwrap();
    let findings = Analyzer::default().check_suite(&suite);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0].help.starts_with("Did you mean `order`?"),
        "{}",
        findings[0].help
    );
}
