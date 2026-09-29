use super::*;
use crate::core::format::SourceKind;

/// Labels offered at the end of `typed`, after the definitions in `defined`,
/// and the active parameter.
fn help(defined: &str, typed: &str) -> Option<(Vec<String>, Option<usize>)> {
    let language = Language::new(".");
    let analysis = language.analyze("a.botwork", defined, SourceKind::Script);
    let text = format!("{defined}{typed}");
    let help = analysis.signature_help(&language, &text, text.len())?;
    Some((
        help.signatures
            .into_iter()
            .map(|signature| signature.label)
            .collect(),
        help.active_parameter,
    ))
}

const DEFINED: &str = "Add |a| To |b| { Return |a + b| }\n";

#[test]
fn the_parameter_being_typed_is_active() {
    assert_eq!(
        help("", "Log |"),
        Some((vec!["Log |value|".into()], Some(0)))
    );
    assert_eq!(
        help(DEFINED, "Add |1| To "),
        Some((vec!["Add |a| To |b|".into()], Some(1)))
    );
    assert_eq!(
        help(DEFINED, "    add |1| to |2 + "),
        Some((vec!["Add |a| To |b|".into()], Some(1)))
    );
    // After the last parameter, no parameter is active.
    assert_eq!(
        help(DEFINED, "Add |1| To |2| "),
        Some((vec!["Add |a| To |b|".into()], None))
    );
}

#[test]
fn nested_calls_and_assignments_are_followed() {
    assert_eq!(
        help(DEFINED, "Log |@{ Add |1| To |"),
        Some((vec!["Add |a| To |b|".into()], Some(1)))
    );
    assert_eq!(
        help(DEFINED, "|x| = Add |@{ Length Of |[1]| }| To |"),
        Some((vec!["Add |a| To |b|".into()], Some(1)))
    );
    // A finished nested call leaves the outer one active.
    assert_eq!(
        help("", "Log |@{ Type Of |1| }| "),
        Some((vec!["Log |value|".into()], None))
    );
    assert_eq!(
        help("", "Log |{\"a\": @{ Type Of |"),
        Some((vec!["Type Of |value|".into()], Some(0)))
    );
}

#[test]
fn partial_names_offer_every_statement_they_can_become() {
    let (labels, active) = help(DEFINED, "Ad").unwrap();
    assert_eq!(active, Some(0));
    assert!(labels.contains(&"Add |a| To |b|".to_owned()), "{labels:?}");
    assert!(labels
        .iter()
        .all(|label| label.to_lowercase().starts_with("ad")));
    // Exact words come before longer statements that start with them.
    let (labels, _) = help("Get |x| { Return |x| }\n", "Get ").unwrap();
    assert_eq!(labels[0], "Get |x|");
    assert!(
        labels.contains(&"Get Variable |name|".to_owned()),
        "{labels:?}"
    );
}

#[test]
fn imported_modules_offer_their_exports() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("math.botwork"),
        "Double |x| { Return |x * 2| }\nImport |\"inner.botwork\"| As |In|\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("inner.botwork"),
        "Triple |x| { Return |x * 3| }\n",
    )
    .unwrap();
    let language = Language::new(directory.path());
    let name = directory.path().join("main.botwork");
    let name = name.to_str().unwrap();
    let defined = "Import |\"math.botwork\"| As |M|\n";
    let analysis = language.analyze(name, defined, SourceKind::Script);
    for (typed, label, active) in [
        ("M::Dou", "M::Double |x|", Some(0)),
        ("m::double |", "M::Double |x|", Some(0)),
        ("M::In::Triple |", "M::In::Triple |x|", Some(0)),
    ] {
        let text = format!("{defined}{typed}");
        let help = analysis
            .signature_help(&language, &text, text.len())
            .unwrap();
        assert_eq!(help.signatures[0].label, label, "{typed}");
        assert!(help.signatures[0].documentation.contains("botwork`"));
        assert_eq!(help.active_parameter, active);
    }
}

#[test]
fn parameter_ranges_cover_each_parameter_with_its_pipes() {
    let language = Language::new(".");
    let analysis = language.analyze("a.botwork", DEFINED, SourceKind::Script);
    let text = format!("{DEFINED}Add |");
    let help = analysis
        .signature_help(&language, &text, text.len())
        .unwrap();
    let signature = &help.signatures[0];
    let ranges: Vec<_> = signature
        .parameters
        .iter()
        .map(|&(start, end)| &signature.label[start..end])
        .collect();
    assert_eq!(ranges, ["|a|", "|b|"]);
    assert_eq!(signature.documentation, "Defined in this file");
    let log = help_for("Log |");
    assert!(log.signatures[0].documentation.contains("BW4001"));
}

fn help_for(text: &str) -> SignatureHelp {
    let language = Language::new(".");
    let analysis = language.analyze("a.botwork", "", SourceKind::Script);
    analysis
        .signature_help(&language, text, text.len())
        .unwrap()
}

#[test]
fn strings_comments_control_statements_and_blocks_get_no_help() {
    for typed in [
        "Log |\"abc",
        "Log |1| # Add |",
        "If |",
        "For |x| In |",
        "While |",
        "Return |",
        "Double |x| {",
        "}",
        "",
        "   ",
    ] {
        assert_eq!(help(DEFINED, typed), None, "{typed:?}");
    }
    // A call inside a control statement's parameter gets help.
    assert_eq!(
        help(DEFINED, "If |@{ Add |1| To |"),
        Some((vec!["Add |a| To |b|".into()], Some(1)))
    );
}
