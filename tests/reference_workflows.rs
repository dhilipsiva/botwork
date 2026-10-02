//! docs/reference-workflows.md names, for each representative workflow of the
//! quality assessment, its sources and the tests that check its success and
//! its failures. Every one must exist, and every workflow must have both.
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Every function name in the Rust tests and sources.
fn functions() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut directories = vec![root().join("src"), root().join("tests")];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = fs::read_to_string(&path).unwrap();
                for (at, _) in text.match_indices("fn ") {
                    let name: String = text[at + 3..]
                        .chars()
                        .take_while(|character| character.is_alphanumeric() || *character == '_')
                        .collect();
                    if !name.is_empty() {
                        found.insert(name);
                    }
                }
            }
        }
    }
    found
}

/// The backticked items of the bullet `label` in `section`.
fn items<'a>(section: &'a str, label: &str) -> Vec<&'a str> {
    let line = section
        .lines()
        .find(|line| line.starts_with(&format!("- **{label}:**")))
        .unwrap_or_else(|| panic!("no {label} in:\n{section}"));
    line.split('`').skip(1).step_by(2).collect()
}

#[test]
fn every_workflow_names_existing_sources_and_success_and_failure_checks() {
    let page = fs::read_to_string(root().join("docs/reference-workflows.md")).unwrap();
    let assessment = fs::read_to_string(root().join("docs/quality-assessment.md")).unwrap();
    let functions = functions();
    let sections: Vec<&str> = page.split("\n## ").skip(1).collect();
    let ids: Vec<&str> = sections.iter().map(|section| &section[..2]).collect();
    assert_eq!(ids, ["W1", "W2", "W3", "W4", "W5", "W6"]);
    for (id, section) in ids.iter().zip(&sections) {
        // The assessment protocol defines each workflow this page implements.
        assert!(
            assessment.contains(&format!("| {id} |")),
            "{id} is not in docs/quality-assessment.md"
        );
        // The summary table's source is among the workflow's sources.
        let row = page
            .lines()
            .find(|line| line.starts_with(&format!("| {id} |")))
            .unwrap_or_else(|| panic!("{id} has no row"));
        let listed = row.split('`').nth(1).unwrap();
        let sources = items(section, "Sources");
        assert!(
            sources.contains(&listed),
            "{id}: {listed} is not among its sources"
        );
        for source in sources {
            let path: PathBuf = root().join(source);
            assert!(path.is_file(), "{id}: {source} does not exist");
        }
        for label in ["Success", "Failure"] {
            let checks = items(section, label);
            assert!(!checks.is_empty(), "{id} has no {label} check");
            for check in checks {
                assert!(
                    functions.contains(check),
                    "{id}: the {label} check `{check}` is not a test"
                );
            }
        }
        for label in ["Setup and teardown", "Expected"] {
            assert!(
                section
                    .lines()
                    .any(|line| line.starts_with(&format!("- **{label}:**"))),
                "{id} says nothing under {label}"
            );
        }
    }
}
