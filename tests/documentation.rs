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
            "http-statements",
            (
                "docs/http.md",
                include_bytes!("doc-examples/http-statements.stdout").as_slice(),
            ),
        ),
        (
            "html-report-example",
            (
                "docs/html-report.md",
                include_bytes!("doc-examples/html-report-example.stdout").as_slice(),
            ),
        ),
        (
            "listener-stream",
            (
                "docs/listeners.md",
                include_bytes!("doc-examples/listener-stream.stdout").as_slice(),
            ),
        ),
        (
            "structured-data",
            (
                "docs/structured-data.md",
                include_bytes!("doc-examples/structured-data.stdout").as_slice(),
            ),
        ),
        (
            "polling-statements",
            (
                "docs/polling.md",
                include_bytes!("doc-examples/polling-statements.stdout").as_slice(),
            ),
        ),
        (
            "process-statements",
            (
                "docs/processes.md",
                include_bytes!("doc-examples/process-statements.stdout").as_slice(),
            ),
        ),
        (
            "operating-system-statements",
            (
                "docs/operating-system.md",
                include_bytes!("doc-examples/operating-system-statements.stdout").as_slice(),
            ),
        ),
        (
            "datetime-statements",
            (
                "docs/datetime.md",
                include_bytes!("doc-examples/datetime-statements.stdout").as_slice(),
            ),
        ),
        (
            "string-statements",
            (
                "docs/strings.md",
                include_bytes!("doc-examples/string-statements.stdout").as_slice(),
            ),
        ),
        (
            "collection-statements",
            (
                "docs/collections.md",
                include_bytes!("doc-examples/collection-statements.stdout").as_slice(),
            ),
        ),
        (
            "assertion-details",
            (
                "docs/assertion-diagnostics.md",
                include_bytes!("doc-examples/assertion-details.stdout").as_slice(),
            ),
        ),
        (
            "builtins",
            (
                "docs/builtins.md",
                include_bytes!("doc-examples/builtins.stdout").as_slice(),
            ),
        ),
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
            "cleanup-example",
            (
                "docs/cleanup.md",
                include_bytes!("doc-examples/cleanup-example.stdout").as_slice(),
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
fn every_documented_suite_matches_its_cli_output() {
    struct Expected {
        document: &'static str,
        stdout: &'static str,
        status: i32,
        summary: &'static str,
        lines: Option<usize>,
        diagnostics: &'static [&'static str],
    }
    let expected = BTreeMap::from([
        ("named-suite", Expected {
            document: "docs/suites.md", stdout: "42\n8\n", status: 0,
            summary: "[cases] 2 selected: 2 succeeded, 0 failed\n", lines: Some(5), diagnostics: &[],
        }),
        ("fixture-suite", Expected {
            document: "docs/fixtures.md",
            stdout: "suite opened\n43\ncase closed\n42\ncase closed\nsuite closed\n", status: 0,
            summary: "[cases] 2 selected: 2 succeeded, 0 failed\n", lines: Some(8), diagnostics: &[],
        }),
        ("parameterized-suite", Expected {
            document: "docs/parameterized-cases.md", stdout: "42\n8\n0\n", status: 0,
            summary: "[cases] 3 selected: 3 succeeded, 0 failed\n", lines: Some(7), diagnostics: &[],
        }),
        ("json-report-suite", Expected {
            document: "docs/json-report.md", stdout: "1\n2\n", status: 1,
            summary: "[cases] 2 selected: 1 succeeded, 1 failed\n", lines: None,
            diagnostics: &["[case checkout/total/pair] failed:", "BW9001"],
        }),
        ("setup-failure-suite", Expected {
            document: "docs/setup-failure.md", stdout: "open demo\nclose demo\n", status: 1,
            summary: "[cases] 2 selected: 0 succeeded, 0 failed, 2 skipped; 1 suite fixtures failed\n",
            lines: None,
            diagnostics: &["BW2001", "BW2002", "[case environment/first] skipped:", "[case environment/second] skipped:"],
        }),
    ]);
    let mut seen = BTreeSet::new();
    let harness = Harness::new();
    for (document, blocks) in documents() {
        for block in blocks
            .into_iter()
            .filter(|block| block.language == "botwork-suite")
        {
            let id = block.id.as_deref().unwrap();
            assert!(seen.insert(id.to_owned()), "duplicate suite example: {id}");
            let expected = expected.get(id).unwrap_or_else(|| {
                panic!("{document}:{}: register suite output for {id}", block.line)
            });
            assert_eq!(document, expected.document);
            let filename = format!("{id}.suite.botwork");
            fs::write(harness.workspace.join(&filename), &block.source).unwrap();
            let output = harness
                .command(
                    id,
                    &["--suite", &filename, "--jobs", "1"],
                    Duration::from_secs(5),
                )
                .unwrap();
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected.status),
                "{document}:{}: {stderr}",
                block.line
            );
            assert_eq!(output.stdout, expected.stdout.as_bytes());
            if let Some(lines) = expected.lines {
                assert_eq!(stderr.lines().count(), lines);
            }
            assert!(stderr.ends_with(expected.summary), "{stderr}");
            let mut remainder = stderr.as_str();
            for evidence in expected.diagnostics {
                let position = remainder
                    .find(evidence)
                    .unwrap_or_else(|| panic!("missing ordered {evidence}: {stderr}"));
                remainder = &remainder[position + evidence.len()..];
            }
        }
    }
    assert_eq!(
        seen,
        expected.keys().map(|id| (*id).to_owned()).collect(),
        "stale or missing suite documentation expectations"
    );
}

#[test]
fn every_rust_documentation_example_is_included_in_crate_doctests() {
    assert!(include_str!("../src/lib.rs")
        .contains("#![doc = include_str!(\"../docs/acceptance-policy.md\")]"));
    assert!(
        include_str!("../src/lib.rs").contains("#![doc = include_str!(\"../docs/fixtures.md\")]")
    );
    assert!(include_str!("../src/lib.rs")
        .contains("#![doc = include_str!(\"../docs/parameterized-cases.md\")]"));
    let inclusion = "#![doc = include_str!(\"../docs/interpreter-architecture.md\")]";
    assert!(include_str!("../src/lib.rs").contains(inclusion));
    assert!(include_str!("../src/lib.rs")
        .contains("#![doc = include_str!(\"../docs/async-execution.md\")]"));
    assert!(include_str!("../src/lib.rs")
        .contains("#![doc = include_str!(\"../docs/nonblocking-io.md\")]"));
    assert!(include_str!("../src/lib.rs")
        .contains("#![doc = include_str!(\"../docs/run-records.md\")]"));
    assert!(
        include_str!("../src/lib.rs").contains("#![doc = include_str!(\"../docs/listeners.md\")]")
    );
    assert!(
        include_str!("../src/lib.rs").contains("#![doc = include_str!(\"../docs/secrets.md\")]")
    );
    assert!(
        include_str!("../src/lib.rs").contains("#![doc = include_str!(\"../docs/shutdown.md\")]")
    );
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
            "docs/acceptance-policy.md".to_owned(),
            "docs/interpreter-architecture.md".to_owned(),
            "docs/async-execution.md".to_owned(),
            "docs/nonblocking-io.md".to_owned(),
            "docs/parameterized-cases.md".to_owned(),
            "docs/cleanup.md".to_owned(),
            "docs/fixtures.md".to_owned(),
            "docs/isolated-workers.md".to_owned(),
            "docs/worker-protocol.md".to_owned(),
            "docs/run-records.md".to_owned(),
            "docs/listeners.md".to_owned(),
            "docs/secrets.md".to_owned(),
            "docs/shutdown.md".to_owned()
        ]),
        "include new Rust documentation examples in rustdoc before registering their files"
    );
}
