#[path = "support/markdown.rs"]
mod markdown;

#[path = "support/cli_harness.rs"]
mod cli_harness;

use cli_harness::Harness;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

fn markdown_files(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            markdown_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn documents() -> Vec<(String, Vec<markdown::Block>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("README.md")];
    markdown_files(&root.join("docs"), &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            let source = fs::read_to_string(&path).unwrap();
            let blocks =
                markdown::blocks(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
            (name, blocks)
        })
        .collect()
}

#[test]
fn every_documented_botwork_example_matches_its_cli_output() {
    let expected = BTreeMap::from([
        (
            "call-composition",
            (
                "docs/language.md",
                include_bytes!("doc-examples/call-composition.stdout").as_slice(),
            ),
        ),
        (
            "catch-inspection",
            (
                "docs/language.md",
                include_bytes!("doc-examples/catch-inspection.stdout").as_slice(),
            ),
        ),
        (
            "readme-sample",
            (
                "README.md",
                include_bytes!("doc-examples/readme-sample.stdout").as_slice(),
            ),
        ),
        (
            "multiline-call",
            (
                "docs/language.md",
                include_bytes!("doc-examples/multiline-call.stdout").as_slice(),
            ),
        ),
        (
            "multilingual-call",
            (
                "docs/language.md",
                include_bytes!("doc-examples/multilingual-call.stdout").as_slice(),
            ),
        ),
        (
            "catch-recovery",
            (
                "docs/language.md",
                include_bytes!("doc-examples/catch-recovery.stdout").as_slice(),
            ),
        ),
    ]);
    let mut seen = BTreeSet::new();
    let harness = Harness::new();
    for (document, blocks) in documents() {
        for block in blocks
            .into_iter()
            .filter(|block| block.language == "botwork")
        {
            let id = block.id.as_deref().unwrap();
            assert!(
                seen.insert(id.to_owned()),
                "duplicate documentation id: {id}"
            );
            let (expected_document, stdout) = expected
                .get(id)
                .unwrap_or_else(|| panic!("{document}:{}: register output for {id}", block.line));
            assert_eq!(&document, expected_document, "{id}: unexpected document");
            let output = harness
                .run(id, &block.source, Duration::from_secs(5))
                .unwrap_or_else(|error| panic!("{document}:{}: {error}", block.line));
            let diagnostic = output.stderr;
            let label = format!(
                "{document}:{} ({id}); {}; timeout=5s\n{}",
                block.line, harness.environment, block.source
            );
            assert_eq!(
                output.status.code(),
                Some(0),
                "{label}\n{}",
                String::from_utf8_lossy(&diagnostic)
            );
            assert!(
                diagnostic.is_empty(),
                "{label}: {}",
                String::from_utf8_lossy(&diagnostic)
            );
            assert_eq!(&output.stdout, stdout, "{label}");
        }
    }
    assert_eq!(
        seen,
        expected.keys().map(|key| (*key).to_owned()).collect(),
        "stale or missing documentation expectations"
    );
}

#[test]
fn every_rust_documentation_example_is_included_in_crate_doctests() {
    let inclusion = "#![doc = include_str!(\"../docs/interpreter-architecture.md\")]";
    assert!(include_str!("../src/lib.rs").contains(inclusion));
    assert!(include_str!("../src/core/worker.rs")
        .contains("#![doc = include_str!(\"../../docs/isolated-workers.md\")]"));
    assert!(include_str!("../src/core/worker/protocol.rs")
        .contains("#![doc = include_str!(\"../../../docs/worker-protocol.md\")]"));
    let rust_documents: BTreeSet<_> = documents()
        .into_iter()
        .filter(|(_, blocks)| blocks.iter().any(|block| block.language == "rust"))
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        rust_documents,
        BTreeSet::from([
            "docs/interpreter-architecture.md".to_owned(),
            "docs/isolated-workers.md".to_owned(),
            "docs/worker-protocol.md".to_owned()
        ]),
        "include new Rust documentation examples in rustdoc before registering their files"
    );
}
