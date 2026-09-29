//! The Tree-sitter grammars in `editors/tree-sitter-botwork` accept exactly the
//! sources the interpreter parses. Every valid script, suite, and dataset in the
//! repository must parse without an error node, and every syntax error in the
//! conformance corpus and the invalid-input list below must produce one.
//!
//! The check runs the `tree-sitter` CLI (0.25). Without it the test is skipped
//! unless `BOTWORK_REQUIRE_TREE_SITTER` is set, as it is in continuous integration.
#[path = "support/sources.rs"]
mod sources;

use botwork::core::{
    ast::{
        suite::{Dataset, Suite},
        Program,
    },
    diagnostic::{Diagnostic, DiagnosticCode},
    format::SourceKind,
};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Sources the interpreter rejects with a syntax error, each covering one rule.
const INVALID: &[&str] = &[
    "Log |1\n",
    "Log |1 +|\n",
    "Log |+1|\n",
    "Log |(1|\n",
    "Log |[1, 2|\n",
    "Log |{a 1}|\n",
    "Log |\"unterminated|\n",
    "Log |\"\\t\"|\n",
    "### unclosed comment\n",
    "Log |1 <\n= 2|\n",
    "Log |true andfalse|\n",
    "|or| = |1|\n",
    "|x| = |and|\n",
    "Log |{true: 1}|\n",
    "Else { Log |1| }\n",
    "Catch { Log |1| }\n",
    "If x |y| { }\n",
    "If |x|\n",
    "For |x| |y| { }\n",
    "While { }\n",
    "Try { }\n",
    "Import |\"m.botwork\"|\n",
    "Import |m| As |x|\n",
    "|x| =\n",
    "|x + 1| = |2|\n",
    "Pair |a| \\\n# comment\n|b|\n",
    "Pair |a| \\\n\n|b|\n",
    "Pair |a| \\\n",
    "Log\r|1|\n",
    "Log |@{ Double\n|2| }|\n",
    "Log |@{ If |1| }|\n",
    "Log |a.|\n",
    "Log |a[]|\n",
    "Log |1.|\n",
    "Log |.5|\n",
    "{ Log |1| }\n",
    "}\n",
    "Log |x| {\n",
    "Double |x + 1| { }\n",
];

/// Suite and dataset sources the interpreter rejects with a syntax error.
const INVALID_SUITES: &[&str] = &[
    "Suite |\"s\"| { }\n",
    "Suite |s| { Case |\"c\"| { } }\n",
    "Suite |\"s\"| { Case |\"c\"| }\n",
    "Suite |\"s\"| { Case |\"c\"| { } Library { } }\n",
    "Suite |\"s\"| Tags |[\"a\" \"b\"]| { Case |\"c\"| { } }\n",
    "Suite |\"s\"| { Dataset |\"d\"| { } Case |\"c\"| { } }\n",
    "Suite |\"s\"| { Dataset |\"d\"| { Row |\"r\"| Values |x| } Case |\"c\"| { } }\n",
    "Suite |\"s\"| { Dataset |\"d\"| From YAML |\"x\"| Case |\"c\"| { } }\n",
    "Log |1|\n",
];

fn tree_sitter() -> Option<PathBuf> {
    let program = std::env::var_os("BOTWORK_TREE_SITTER").unwrap_or_else(|| "tree-sitter".into());
    let found = Command::new(&program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if found {
        return Some(program.into());
    }
    assert!(
        std::env::var_os("BOTWORK_REQUIRE_TREE_SITTER").is_none(),
        "BOTWORK_REQUIRE_TREE_SITTER is set but the tree-sitter CLI was not found"
    );
    None
}

/// The interpreter's verdict: Ok when valid, the error when not.
fn interpret(source: &str, kind: SourceKind) -> Result<(), Diagnostic> {
    match kind {
        SourceKind::Script => Program::parse_detailed("source.botwork", source).map(|_| ()),
        SourceKind::Suite => Suite::parse("source.suite.botwork", source).map(|_| ()),
        SourceKind::Dataset => Dataset::parse("source.dataset.botwork", source).map(|_| ()),
    }
}

/// Files Tree-sitter parses with an error, running the grammar in `directory`.
fn rejected(program: &Path, directory: &Path, files: &[PathBuf]) -> BTreeSet<PathBuf> {
    if files.is_empty() {
        return BTreeSet::new();
    }
    let list = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        list.path(),
        files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let output = Command::new(program)
        .args(["parse", "--quiet", "--paths"])
        .arg(list.path())
        .current_dir(directory)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() || !stdout.trim().is_empty(),
        "tree-sitter failed: {stderr}"
    );
    files
        .iter()
        .filter(|path| {
            let name = path.display().to_string();
            // Each file with an error prints its padded path, a tab, and details.
            stdout
                .lines()
                .any(|line| line.split('\t').next().map(str::trim) == Some(name.as_str()))
        })
        .cloned()
        .collect()
}

#[test]
fn the_tree_sitter_grammars_accept_exactly_what_the_interpreter_parses() {
    let Some(program) = tree_sitter() else {
        eprintln!("skipped: the tree-sitter CLI is not installed");
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("editors/tree-sitter-botwork");
    let workspace = tempfile::tempdir().unwrap();
    let mut cases: Vec<(String, String, SourceKind)> = sources::sources();
    cases.extend(INVALID.iter().enumerate().map(|(index, source)| {
        (
            format!("invalid script {index}"),
            (*source).to_owned(),
            SourceKind::Script,
        )
    }));
    cases.extend(INVALID_SUITES.iter().enumerate().map(|(index, source)| {
        (
            format!("invalid suite {index}"),
            (*source).to_owned(),
            SourceKind::Suite,
        )
    }));
    // (origin, file, whether the grammar must accept it)
    let mut expected = Vec::new();
    let (mut valid, mut syntax_errors, mut other_errors) = (0, 0, 0);
    for (index, (origin, source, kind)) in cases.iter().enumerate() {
        let accept = match interpret(source, *kind) {
            Ok(()) => {
                valid += 1;
                true
            }
            Err(error) if error.code() == DiagnosticCode::Syntax => {
                syntax_errors += 1;
                false
            }
            // Placement, limits, and suite configuration errors are grammatical.
            Err(_) => {
                other_errors += 1;
                true
            }
        };
        let extension = match kind {
            SourceKind::Script => "botwork",
            SourceKind::Suite => "suite.botwork",
            SourceKind::Dataset => "dataset.botwork",
        };
        let file = workspace.path().join(format!("case{index}.{extension}"));
        fs::write(&file, source).unwrap();
        expected.push((origin.clone(), file, *kind, accept, source.clone()));
    }
    for (index, source) in INVALID.iter().enumerate() {
        assert_eq!(
            interpret(source, SourceKind::Script)
                .err()
                .map(|error| error.code()),
            Some(DiagnosticCode::Syntax),
            "invalid script {index} must be a syntax error: {source:?}"
        );
    }
    for (index, source) in INVALID_SUITES.iter().enumerate() {
        assert!(
            interpret(source, SourceKind::Suite).is_err(),
            "invalid suite {index} must be rejected: {source:?}"
        );
    }
    let mut disagreements = Vec::new();
    for (grammar, kinds) in [
        ("botwork", &[SourceKind::Script][..]),
        ("suite", &[SourceKind::Suite, SourceKind::Dataset][..]),
    ] {
        let files: Vec<PathBuf> = expected
            .iter()
            .filter(|(_, _, kind, _, _)| kinds.contains(kind))
            .map(|(_, file, _, _, _)| file.clone())
            .collect();
        let errors = rejected(&program, &root.join(grammar), &files);
        for (origin, file, kind, accept, source) in &expected {
            if kinds.contains(kind) && errors.contains(file) == *accept {
                disagreements.push(format!(
                    "{origin}: the interpreter {} it, Tree-sitter does not\n{source}",
                    if *accept { "accepts" } else { "rejects" }
                ));
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "{}",
        disagreements.join("\n---\n")
    );
    assert!(
        valid > 150 && syntax_errors > 50 && other_errors > 5,
        "{valid} {syntax_errors} {other_errors}"
    );
}

#[test]
fn the_committed_parsers_match_their_grammar() {
    let Some(program) = tree_sitter() else {
        eprintln!("skipped: the tree-sitter CLI is not installed");
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("editors/tree-sitter-botwork");
    for grammar in ["botwork", "suite"] {
        let output = tempfile::tempdir().unwrap();
        let status = Command::new(&program)
            .arg("generate")
            .arg("--output")
            .arg(output.path())
            .current_dir(root.join(grammar))
            .status()
            .unwrap();
        assert!(status.success(), "{grammar}: generation failed");
        for file in ["parser.c", "grammar.json", "node-types.json"] {
            assert!(
                fs::read(output.path().join(file)).unwrap()
                    == fs::read(root.join(grammar).join("src").join(file)).unwrap(),
                "editors/tree-sitter-botwork/{grammar}/src/{file} is stale; run `tree-sitter generate` in that directory"
            );
        }
    }
}
