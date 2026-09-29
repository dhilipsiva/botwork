use super::*;

fn offset(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} in {text}"))
}

fn slice(text: &str, location: &Location) -> String {
    text[location.start..location.end].to_owned()
}

#[test]
fn problems_come_from_the_parser_and_the_shared_checks() {
    let language = Language::new(".");
    let analysis = language.analyze(
        "a.botwork",
        "Log |1| |2|\nLog |missing|\n",
        SourceKind::Script,
    );
    assert!(analysis.parsed);
    let problems: Vec<_> = analysis
        .problems
        .iter()
        .map(|problem| {
            (
                problem.code.as_str(),
                problem.severity,
                problem.start,
                problem.end,
            )
        })
        .collect();
    assert_eq!(
        problems,
        [
            ("BW2002", Severity::Error, 0, 11),
            ("BW2001", Severity::Warning, 17, 24),
        ]
    );
    assert!(analysis.problems[0]
        .message
        .ends_with("(undefined-statement)"));
    assert!(analysis.problems[1].help.starts_with("Assign it first"));

    let broken = language.analyze("b.botwork", "Log |1\n", SourceKind::Script);
    assert!(!broken.parsed);
    assert_eq!(broken.problems.len(), 1);
    assert_eq!(broken.problems[0].code, "BW1001");
    assert!(broken.problems[0]
        .help
        .starts_with("Check the indicated token"));
    assert!(broken.problems[0].end > broken.problems[0].start);
}

#[test]
fn definitions_and_references_follow_runtime_resolution() {
    let text = "Double |x| {\n    Return |x * 2|\n}\n|value| = Double |3|\nLog |@{ Double |value| }|\nOuter {\n    Double |y| { Return |y| }\n    |value| = |0|\n    Return |@{ Double |1| }|\n}\n";
    let language = Language::new(".");
    let analysis = language.analyze("a.botwork", text, SourceKind::Script);
    // A root call reaches the root definition.
    let call = offset(text, "Double |3|");
    let definition = analysis.definition(call);
    assert_eq!(definition.len(), 1);
    assert_eq!(slice(text, &definition[0]), "Double |x|");
    assert_eq!(definition[0].file, "a.botwork");
    // The call inside Outer reaches Outer's own nested definition.
    let inner = offset(text, "Double |1|");
    assert_eq!(slice(text, &analysis.definition(inner)[0]), "Double |y|");
    // References to the root definition: its calls, plus the header on request.
    let references: Vec<_> = analysis
        .references(call, true)
        .iter()
        .map(|location| slice(text, location))
        .collect();
    assert_eq!(references, ["Double |x|", "Double |3|", "Double |value|"]);
    assert_eq!(analysis.references(call, false).len(), 2);
    // A variable read reaches its first binding in its frame.
    let read = offset(text, "value| }");
    assert_eq!(slice(text, &analysis.definition(read)[0]), "value");
    // Outer's own `value` is another variable.
    assert_eq!(analysis.references(read, true).len(), 2);
    assert_eq!(analysis.references(read, false).len(), 1);
    // Parameters bind in their definition's frame.
    let parameter = offset(text, "x * 2");
    let binding = analysis.definition(parameter);
    assert_eq!(binding[0].start, offset(text, "x| {"));
}

#[test]
fn hover_shows_built_in_help_definitions_and_input_variables() {
    let text = "Greet |name| { Return |name| }\nLog |@{ Greet |who| }|\n";
    let language = Language::new(".");
    let analysis = language.analyze("a.botwork", text, SourceKind::Script);
    let log = analysis.hover(&language, offset(text, "Log")).unwrap();
    assert!(log.markdown.contains("Log |value|"), "{}", log.markdown);
    assert!(log.markdown.contains("BW4001"), "{}", log.markdown);
    let greet = analysis
        .hover(&language, offset(text, "Greet |who|"))
        .unwrap();
    assert!(
        greet.markdown.contains("Greet |name|"),
        "{}",
        greet.markdown
    );
    let name = analysis.hover(&language, offset(text, "name| }")).unwrap();
    assert!(name.markdown.ends_with("\nVariable"), "{}", name.markdown);
    let who = analysis.hover(&language, offset(text, "who")).unwrap();
    assert!(who.markdown.contains("input variable"), "{}", who.markdown);
    assert_eq!(&text[who.start..who.end], "who");
    assert!(analysis.hover(&language, offset(text, "\n")).is_none());
}

#[test]
fn imports_resolve_to_module_files_and_their_definitions() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("lib")).unwrap();
    fs::write(
        directory.path().join("lib/math.botwork"),
        "Double |x| { Return |x * y| }\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("lib/wrap.botwork"),
        "Import |\"math.botwork\"| As |m|\n",
    )
    .unwrap();
    let text = "Import |\"lib/wrap.botwork\"| As |w|\nLog |@{ w::m::Double |2| }|\n";
    let language = Language::new(directory.path());
    let analysis = language.analyze("main.botwork", text, SourceKind::Script);
    // The module's own warning about `y` belongs to the module's file.
    assert!(analysis.problems.is_empty(), "{:?}", analysis.problems);
    let module = analysis.definition(offset(text, "lib/wrap"));
    assert!(module[0].file.ends_with("lib/wrap.botwork"), "{module:?}");
    let target = analysis.definition(offset(text, "w::m::Double"));
    assert!(target[0].file.ends_with("lib/math.botwork"), "{target:?}");
    assert_eq!(target[0].start, 0);
    let hover = analysis
        .hover(&language, offset(text, "w::m::Double"))
        .unwrap();
    assert!(hover.markdown.contains("Double |x|") && hover.markdown.contains("math.botwork"));
}

#[test]
fn completions_offer_variables_inside_parameters_and_statements_elsewhere() {
    let text = "|total| = |1|\nDouble |x| { Return |x| }\nLog |to\n";
    let language = Language::new(".");
    let analysis = language.analyze(
        "a.botwork",
        "|total| = |1|\nDouble |x| { Return |x| }\n",
        SourceKind::Script,
    );
    let inside = analysis.completions(&language, text, text.len() - 1);
    let labels: Vec<_> = inside
        .iter()
        .map(|completion| completion.label.as_str())
        .collect();
    assert!(labels.contains(&"total") && labels.contains(&"x") && labels.contains(&"true"));
    assert!(!labels.contains(&"Log |value|"));
    let outside = analysis.completions(&language, text, offset(text, "Log"));
    let statements: Vec<_> = outside
        .iter()
        .filter(|completion| completion.kind == CompletionKind::Statement)
        .map(|completion| completion.label.as_str())
        .collect();
    assert!(statements.contains(&"Log |value|") && statements.contains(&"Double |x|"));
    assert!(outside
        .iter()
        .any(|completion| completion.label == "If" && completion.kind == CompletionKind::Keyword));
    // Pipes in strings and comments do not open a parameter.
    assert!(!inside_parameter("Log |\"|\"| ", 11));
    assert!(!inside_parameter("# |\n", 3));
}

#[test]
fn suites_are_indexed_once_across_their_case_programs() {
    let text = "Suite |\"s\"| {\n    Library {\n        Helper { Return |1| }\n    }\n    Case |\"a\"| { Log |@{ Helper }| }\n    Case |\"b\"| { Log |@{ Helper }| }\n}\n";
    let language = Language::new(".");
    let analysis = language.analyze("a.suite.botwork", text, SourceKind::Suite);
    assert!(
        analysis.parsed && analysis.problems.is_empty(),
        "{:?}",
        analysis.problems
    );
    let header = offset(text, "Helper {");
    assert_eq!(analysis.references(header, false).len(), 2);
    let dataset = language.analyze(
        "d.dataset.botwork",
        "Dataset |\"d\"| { Row |\"r\"| Values |1| }",
        SourceKind::Dataset,
    );
    assert!(dataset.parsed && dataset.problems.is_empty());
}
