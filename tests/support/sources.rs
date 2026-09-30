//! Every script, suite, and dataset source the repository holds: conformance
//! corpus scripts, example and conformance files, and executed documentation.
#[path = "../conformance/cases.rs"]
#[allow(dead_code)]
mod corpus;
#[path = "markdown.rs"]
mod markdown;

use botwork::core::format::SourceKind;
use corpus::Input;
use std::{fs, path::Path};

/// Each source's origin, text, and kind, valid or not.
pub fn sources() -> Vec<(String, String, SourceKind)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = Vec::new();
    for case in corpus::cases() {
        if let Input::Script(source) = case.input {
            found.push((
                format!("corpus:{}", case.id),
                source.to_owned(),
                SourceKind::Script,
            ));
        }
    }
    for directory in [
        "examples",
        "examples/modules",
        "examples/datasets",
        "tests/conformance",
        "tests/fixtures",
        "editors/test",
    ] {
        let mut paths: Vec<_> = fs::read_dir(root.join(directory))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "botwork")
            })
            .collect();
        paths.sort();
        for path in paths {
            let text = fs::read_to_string(&path).unwrap();
            found.push((path.display().to_string(), text, SourceKind::of_path(&path)));
        }
    }
    let mut documents = vec![root.join("README.md")];
    documents.extend(
        fs::read_dir(root.join("docs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "md")),
    );
    documents.sort();
    for path in documents {
        for block in markdown::blocks(&fs::read_to_string(&path).unwrap()).unwrap() {
            let kind = match block.language.as_str() {
                "botwork" => SourceKind::Script,
                "botwork-suite" => SourceKind::Suite,
                _ => continue,
            };
            found.push((
                format!("{}:{}", path.display(), block.line),
                block.source,
                kind,
            ));
        }
    }
    found
}
