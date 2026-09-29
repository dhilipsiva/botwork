use super::*;
use std::collections::BTreeMap;

fn at(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} in {text}"))
}

/// Rename in a single document and return its new text, or the refusal.
fn rename(text: &str, needle: &str, new_name: &str) -> Result<String, String> {
    let language = Language::new(".");
    let documents = BTreeMap::from([("a.botwork".to_owned(), text.to_owned())]);
    let edits = language.rename(&documents, "a.botwork", at(text, needle), new_name)?;
    assert!(edits.keys().all(|file| file == "a.botwork"), "{edits:?}");
    Ok(apply(
        text,
        edits.get("a.botwork").map_or(&[], Vec::as_slice),
    ))
}

#[test]
fn definitions_are_renamed_with_the_calls_that_reach_them() {
    let text = "Add |a| To |b| {\n    Return |a + b|\n}\nLog |@{ add |1| to \\\n  |2| }|\nOuter {\n    Add |a| To |b| { Return |0| }\n    Log |@{ Add |3| To |4| }|\n}\n";
    // From a call: the header and the root call change, keeping the call's
    // line continuation; Outer's own definition and the call that reaches it
    // do not.
    assert_eq!(
        rename(text, "add |1|", "Sum Of |a| And |b|").unwrap(),
        "Sum Of |a| And |b| {\n    Return |a + b|\n}\nLog |@{ Sum Of |1| And \\\n  |2| }|\nOuter {\n    Add |a| To |b| { Return |0| }\n    Log |@{ Add |3| To |4| }|\n}\n"
    );
    // From the inner header, only Outer's pair changes.
    let renamed = rename(text, "Add |a| To |b| { Return", "Plus |a| |b|").unwrap();
    assert!(renamed.contains("    Plus |a| |b| { Return |0| }\n    Log |@{ Plus |3| |4| }|"));
    assert!(renamed.starts_with("Add |a| To |b| {"));
}

#[test]
fn words_move_around_parameters_as_the_new_header_places_them() {
    let pair = "Add |a| To |b| { Return |a + b| }\nLog |@{ Add |1| To |2| }|\n";
    assert_eq!(
        rename(pair, "Add |1|", "Add |a| |b| Together").unwrap(),
        "Add |a| |b| Together { Return |a + b| }\nLog |@{ Add |1| |2| Together }|\n"
    );
    let join = "Join |a| |b| { Return |a| }\nLog |@{ Join |1| |2| }|\n";
    assert_eq!(
        rename(join, "Join |1|", "Join |a| With |b|").unwrap(),
        "Join |a| With |b| { Return |a| }\nLog |@{ Join |1| With |2| }|\n"
    );
    let text = "Double |x| { Return |x * 2| }\nLog |@{ Double |2| }|\n";
    assert_eq!(
        rename(text, "Double |x|", "Twice |x| Over").unwrap(),
        "Twice |x| Over { Return |x * 2| }\nLog |@{ Twice |2| Over }|\n"
    );
    // Changing only the spelling keeps the signature.
    assert_eq!(
        rename(text, "Double |x|", "DOUBLE |x|").unwrap(),
        "DOUBLE |x| { Return |x * 2| }\nLog |@{ DOUBLE |2| }|\n"
    );
}

#[test]
fn definition_renames_that_could_change_resolution_are_refused() {
    let text = "Double |x| { Return |x * 2| }\nHalf |x| { Return |x / 2| }\nLog |@{ Double |2| }|\nLog |@{ Triple |2| }|\nIf |true| {\n    Maybe { Return |1| }\n}\nTwin {}\nTwin {}\n";
    let refused = |needle: &str, new_name: &str| rename(text, needle, new_name).unwrap_err();
    assert!(refused("Double |x|", "Log |x|").contains("built-in"));
    assert!(refused("Double |x|", "Half |x|").contains("already defined"));
    assert!(refused("Double |x|", "Triple |x|").contains("already called"));
    assert!(refused("Double |x|", "Double |y|").contains("Keep the parameters |x|"));
    assert!(refused("Double |x|", "Double |x| |y|").contains("Keep the parameters"));
    assert!(refused("Double |x|", "If |x|").contains("not a statement header"));
    assert!(refused("Double |x|", "m::Double |x|").contains("`::`"));
    assert!(refused("Double |x|", "Double |x|").contains("already"));
    assert!(refused("Maybe {", "Perhaps").contains("inside a block"));
    assert!(refused("Twin {}", "Pair").contains("same scope"));
    assert!(refused("Log |@{ Double", "Print |value|").contains("Built-in"));
    assert!(refused("Triple |2|", "Treble |x|").contains("no definition"));
    assert!(refused("\n", "X").contains("nothing") || refused("\n", "X").contains("no statement"));
}

#[test]
fn variables_are_renamed_in_their_scope_only() {
    let text = "|total| = |1|\nAdd |x| {\n    |total| = |x|\n    Return |total|\n}\nFor |item| In |[1, 2]| {\n    |total| = |total + item|\n}\nShow |value| { Log |value| }\n";
    // Add assigns its own `total`, which shadows the root's: refused both ways.
    assert!(rename(text, "total| = |1|", "sum")
        .unwrap_err()
        .contains("enclosing or nested scope"));
    assert!(rename(text, "total| = |x|", "sum")
        .unwrap_err()
        .contains("enclosing or nested scope"));
    assert_eq!(
        rename(text, "item|", "entry").unwrap(),
        "|total| = |1|\nAdd |x| {\n    |total| = |x|\n    Return |total|\n}\nFor |entry| In |[1, 2]| {\n    |total| = |total + entry|\n}\nShow |value| { Log |value| }\n"
    );
    // Sibling definitions' parameters are separate variables.
    let siblings = "First |x| { Return |x| }\nSecond |x| { Return |x| }\n";
    assert_eq!(
        rename(siblings, "x| { Return |x| }\nSecond", "y").unwrap(),
        "First |y| { Return |y| }\nSecond |x| { Return |x| }\n"
    );
    // A parameter is renamed in its header and body; callers pass it by position.
    assert_eq!(
        rename(text, "value| {", "shown").unwrap(),
        "|total| = |1|\nAdd |x| {\n    |total| = |x|\n    Return |total|\n}\nFor |item| In |[1, 2]| {\n    |total| = |total + item|\n}\nShow |shown| { Log |shown| }\n"
    );
}

#[test]
fn variable_renames_that_could_change_resolution_are_refused() {
    let text = "|a| = |1|\n|b| = |2|\nLog |input|\nLog |@{ Get Variable |\"a\"| }|\n";
    assert!(rename(text, "b|", "a")
        .unwrap_err()
        .contains("already a variable"));
    assert!(rename(text, "b|", "and")
        .unwrap_err()
        .contains("not a variable name"));
    assert!(rename(text, "b|", "1b")
        .unwrap_err()
        .contains("not a variable name"));
    assert!(rename(text, "b|", "b")
        .unwrap_err()
        .contains("already named"));
    assert!(rename(text, "input", "given")
        .unwrap_err()
        .contains("input variable"));
    // `a` is looked up by name; `b` is safe while the lookup names neither.
    assert!(rename(text, "a|", "c")
        .unwrap_err()
        .contains("looked up by name"));
    assert!(rename(text, "b|", "c").is_ok());
    assert!(rename(text, "b|", "a2").is_ok());
    let dynamic = "|b| = |2|\n|name| = |\"b\"|\nLog |@{ Variable Exists |name| }|\n";
    assert!(rename(dynamic, "b|", "c")
        .unwrap_err()
        .contains("looked up by name"));
    // A file that does not parse cannot be renamed in.
    assert!(rename("|b| = |2|\nLog |b\n", "b|", "c")
        .unwrap_err()
        .contains("does not parse"));
}

#[test]
fn module_definitions_are_renamed_across_their_importers() {
    let directory = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    fs::create_dir(root.join("lib")).unwrap();
    let files = [
        (
            "lib/math.botwork",
            "Double |x| { Return |x * 2| }\nQuad |x| { Return |@{ Double |@{ Double |x| }| }| }\n",
        ),
        ("lib/wrap.botwork", "Import |\"math.botwork\"| As |m|\n"),
        (
            "main.botwork",
            "Import |\"lib/math.botwork\"| As |m|\nLog |@{ m::Double |2| }|\n",
        ),
        (
            "other.botwork",
            "Import |\"lib/wrap.botwork\"| As |w|\nLog |@{ w::m::double |3| }|\n",
        ),
    ];
    let mut documents = BTreeMap::new();
    for (name, text) in files {
        fs::write(root.join(name), text).unwrap();
        documents.insert(
            root.join(name).to_string_lossy().into_owned(),
            text.to_owned(),
        );
    }
    let name = |file: &str| root.join(file).to_string_lossy().into_owned();
    let language = Language::new(&root);
    let main = &documents[&name("main.botwork")];
    let edits = language
        .rename(
            &documents,
            &name("main.botwork"),
            at(main, "m::Double"),
            "Twice |x|",
        )
        .unwrap();
    let renamed: BTreeMap<_, _> = edits
        .iter()
        .map(|(file, edits)| (file.clone(), apply(&documents[file], edits)))
        .collect();
    assert_eq!(
        renamed,
        BTreeMap::from([
            (
                name("lib/math.botwork"),
                "Twice |x| { Return |x * 2| }\nQuad |x| { Return |@{ Twice |@{ Twice |x| }| }| }\n"
                    .to_owned()
            ),
            (
                name("main.botwork"),
                "Import |\"lib/math.botwork\"| As |m|\nLog |@{ m::Twice |2| }|\n".to_owned()
            ),
            (
                name("other.botwork"),
                "Import |\"lib/wrap.botwork\"| As |w|\nLog |@{ w::m::Twice |3| }|\n".to_owned()
            ),
        ])
    );
    // An importer that already calls the new name through the module.
    let mut colliding = documents.clone();
    colliding.insert(
        name("third.botwork"),
        "Import |\"lib/math.botwork\"| As |k|\nLog |@{ k::Twice |1| }|\n".to_owned(),
    );
    let refused = language
        .rename(
            &colliding,
            &name("main.botwork"),
            at(main, "m::Double"),
            "Twice |x|",
        )
        .unwrap_err();
    assert!(
        refused.contains("third.botwork") && refused.contains("already calls"),
        "{refused}"
    );
    // A module whose open text differs from the file importers read.
    let mut unsaved = documents.clone();
    unsaved.insert(name("lib/math.botwork"), format!("{}\n", files[0].1));
    let refused = language
        .rename(
            &unsaved,
            &name("main.botwork"),
            at(main, "m::Double"),
            "Twice |x|",
        )
        .unwrap_err();
    assert!(refused.contains("Save"), "{refused}");
    // An importer that does not parse may call it.
    let mut broken = documents.clone();
    broken.insert(
        name("broken.botwork"),
        "Import |\"lib/math.botwork\"| As |m|\nLog |\n".to_owned(),
    );
    let refused = language
        .rename(
            &broken,
            &name("main.botwork"),
            at(main, "m::Double"),
            "Twice |x|",
        )
        .unwrap_err();
    assert!(refused.contains("broken.botwork"), "{refused}");
}

#[test]
fn prepare_rename_names_the_symbol_or_why_it_cannot_be_renamed() {
    let text = "Double |x| { Return |x * 2| }\nLog |@{ Double |value| }|\n";
    let language = Language::new(".");
    let analysis = language.analyze("a.botwork", text, SourceKind::Script);
    let call = analysis.prepare_rename(at(text, "Double |value|")).unwrap();
    assert_eq!(
        (&text[call.start..call.end], call.placeholder.as_str()),
        ("Double |value|", "Double |x|")
    );
    let parameter = analysis.prepare_rename(at(text, "x * 2")).unwrap();
    assert_eq!(parameter.placeholder, "x");
    assert!(analysis
        .prepare_rename(at(text, "value"))
        .unwrap_err()
        .contains("input variable"));
    assert!(analysis
        .prepare_rename(at(text, "Log"))
        .unwrap_err()
        .contains("Built-in"));
}
