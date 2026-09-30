//! Generates `docs/traceability.md`, the index from each language rule,
//! built-in statement, and runtime guarantee to its specification, examples,
//! tests, and review results, and fails when the page or a test it names falls
//! out of date. Regenerate with
//! `BOTWORK_UPDATE_TRACEABILITY=1 cargo test --test traceability`.
#[path = "conformance/cases.rs"]
#[allow(dead_code)]
mod corpus;

use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_owned()
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap()
}

/// Tests that were renamed after an evidence record named them: the recorded
/// name, the current name, and the commit that renamed it.
const RENAMED: &[(&str, &str, &str)] = &[(
    "aggregate_verdict_controls_cli_success_and_rejects_inconsistent_counts",
    "summary_and_exit_decision_share_one_verdict",
    "8cce433",
)];

/// Test files for rules that neither the specification's evidence nor a
/// same-named detail page ties to one. Every file must hold tests.
const RULE_FILES: &[(&str, &[&str])] = &[
    ("E5", &["tests/call_composition.rs"]),
    ("S1", &["tests/language_contract.rs"]),
    ("S2", &["tests/language_contract.rs"]),
    ("F4", &["tests/catch_contract.rs"]),
    ("F5", &["tests/native_registration.rs"]),
    ("F6", &["tests/signature_metadata.rs"]),
    ("F7", &["tests/async_operations.rs"]),
    ("M1", &["tests/local_imports.rs"]),
    ("R26", &["tests/typed_workers.rs"]),
    ("T1", &["tests/suite_cli.rs"]),
    (
        "T3",
        &["tests/suite_fixtures.rs", "tests/fixture_ownership.rs"],
    ),
];

fn current(test: &str) -> &str {
    RENAMED
        .iter()
        .find(|(old, ..)| *old == test)
        .map_or(test, |(_, new, _)| new)
}

/// The anchor GitHub generates for a heading.
fn anchor(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|character| match character {
            ' ' => Some('-'),
            '-' | '_' => Some(character),
            _ if character.is_alphanumeric() => Some(character),
            _ => None,
        })
        .collect()
}

/// Every function name in the Rust sources and tests, and every Python test
/// method in the helper tests.
fn functions() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut directories = vec![
        root().join("src"),
        root().join("tests"),
        root().join("benches"),
    ];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else if let Some(keyword) =
                match path.extension().and_then(|extension| extension.to_str()) {
                    Some("rs") => Some("fn "),
                    Some("py") => Some("def "),
                    _ => None,
                }
            {
                let text = fs::read_to_string(&path).unwrap();
                for (at, _) in text.match_indices(keyword) {
                    let name: String = text[at + keyword.len()..]
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

/// A test file path in the repository.
fn is_test_file(name: &str) -> bool {
    (name.starts_with("tests/") || name.starts_with("src/"))
        && name.ends_with(".rs")
        && root().join(name).is_file()
}

/// A test name: lowercase words joined by at least two underscores.
fn looks_like_a_test(name: &str) -> bool {
    name.matches('_').count() >= 2
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

struct Rule {
    id: String,
    title: String,
    section: String,
    /// The documentation pages its paragraph links to.
    pages: Vec<String>,
}

/// The specification's rules, in order, with the section each belongs to.
fn rules(specification: &str) -> Vec<Rule> {
    let mut section = String::new();
    let mut rules = Vec::new();
    for line in specification.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading.to_owned();
        } else if let Some((id, rest)) = line
            .strip_prefix("**")
            .and_then(|line| line.split_once(" — "))
        {
            let title = rest.split_once(".**").map_or(rest, |(title, _)| title);
            let mut pages: Vec<String> = Vec::new();
            for (at, _) in line.match_indices("](") {
                let target: String = line[at + 2..].chars().take_while(|c| *c != ')').collect();
                let page = target.split('#').next().unwrap_or_default();
                if page.ends_with(".md")
                    && !page.contains('/')
                    && !pages.iter().any(|known| known == page)
                {
                    pages.push(page.to_owned());
                }
            }
            rules.push(Rule {
                id: id.to_owned(),
                title: title.to_owned(),
                section: section.clone(),
                pages,
            });
        }
    }
    rules
}

/// Tests the specification's evidence table names for each rule.
fn rule_tests(specification: &str) -> BTreeMap<String, BTreeSet<String>> {
    let (_, table) = specification
        .split_once("## Evidence and Implementation Gaps")
        .unwrap();
    let mut tests: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in table.lines().filter(|line| line.starts_with("| ")) {
        let cells: Vec<&str> = line.split('|').collect();
        if cells.len() < 3 {
            continue;
        }
        let names: Vec<&str> = cells[2]
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|name| looks_like_a_test(name) || is_test_file(name))
            .collect();
        for rule in cells[1].split(',').map(str::trim) {
            tests
                .entry(rule.to_owned())
                .or_default()
                .extend(names.iter().map(|name| current(name).to_owned()));
        }
    }
    tests
}

/// A built-in statement's reference entry and executed example.
struct Statement {
    header: String,
    group: String,
    example: String,
}

fn statements(reference: &str) -> Vec<Statement> {
    let mut group = String::new();
    let mut found: Vec<Statement> = Vec::new();
    for line in reference.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            group = heading.to_owned();
        } else if let Some(header) = line
            .strip_prefix("### `")
            .and_then(|line| line.strip_suffix('`'))
        {
            found.push(Statement {
                header: header.to_owned(),
                group: group.clone(),
                example: String::new(),
            });
        } else if let Some(example) = line.strip_prefix("Example: ") {
            found.last_mut().unwrap().example = example.trim_end_matches('.').to_owned();
        }
    }
    found
}

/// The rules that specify each statement group.
fn group_rules(group: &str, header: &str) -> &'static str {
    match group {
        "Built-ins" if header.starts_with("Assert") => "B1, B8",
        "Built-ins" => "B1",
        "Collections" => "B2",
        "Strings" => "B3",
        "Dates and times" => "B4",
        "Files, environment, and paths" => "B5",
        "Processes" => "B6",
        "HTTP" => "B7",
        "JSON and CSV data" => "B9",
        other => panic!("no rule for statement group {other}"),
    }
}

/// A link from `docs/` to a repository path, with an optional anchor.
fn link(target: &str) -> String {
    let (path, fragment) = target.split_once('#').unwrap_or((target, ""));
    let relative = path
        .strip_prefix("docs/")
        .map_or_else(|| format!("../{path}"), str::to_owned);
    let fragment = if fragment.is_empty() {
        String::new()
    } else {
        format!("#{fragment}")
    };
    format!("[`{target}`]({relative}{fragment})")
}

fn tests_cell(tests: &[String]) -> String {
    tests
        .iter()
        .map(|test| {
            if is_test_file(test) {
                format!("[`{test}`](../{test})")
            } else if looks_like_a_test(test) {
                format!("`{}`", current(test))
            } else {
                test.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The tests a documentation page's validation evidence names.
fn page_tests(page: &str) -> Vec<String> {
    let evidence = root()
        .join("docs")
        .join(page.replace(".md", "-evidence.json"));
    let Ok(text) = fs::read_to_string(evidence) else {
        return Vec::new();
    };
    let record: Value = serde_json::from_str(&text).unwrap();
    record["requirements"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|requirement| requirement["tests"].as_array().cloned().unwrap_or_default())
        .filter_map(|test| test.as_str().map(str::to_owned))
        .filter(|test| looks_like_a_test(test))
        .collect()
}

/// Every evidence record's requirements: file, feature, and requirements.
fn guarantees() -> Vec<(String, String, Vec<Value>)> {
    let mut files: Vec<PathBuf> = fs::read_dir(root().join("docs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-evidence.json")
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let record: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let requirements = record["requirements"].as_array()?.clone();
            let feature = record["feature"].as_str().unwrap_or_default().to_owned();
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            Some((file, feature, requirements))
        })
        .collect()
}

const REVIEW: &str = "Pending";

fn render() -> String {
    let specification = read("docs/language-specification.md");
    let rules = rules(&specification);
    let named = rule_tests(&specification);
    let cases = corpus::cases();
    let mut page = String::from(
        "# Traceability

<!-- Generated by tests/traceability.rs. Regenerate with:
     BOTWORK_UPDATE_TRACEABILITY=1 cargo test --test traceability -->

This index traces each language rule, built-in statement, and runtime or tooling
guarantee to where it is specified, exemplified, tested, and reviewed. It is
generated from the [specification](language-specification.md), the
[conformance corpus](conformance-corpus.md), the
[statement reference](statements.md), and each feature's validation evidence,
and its test fails when a test it names no longer exists.

The **Review** column records the scored reviews of [D1](decisions.md#d1-reviewers).
They assess the release as a whole, so every entry is pending until they are
recorded. The [performance budgets](performance.md#registered-budgets) register the
representative workloads.

",
    );
    let _ = writeln!(page, "## Language rules\n");
    let _ = writeln!(
        page,
        "Every rule has conformance cases that exercise it as valid input, invalid input, or at a boundary; `conformance_inputs_match_status_stdout_and_error_contracts` runs them all. Its tests are the test files and tests that the specification's [evidence](language-specification.md#evidence-and-implementation-gaps) cites for it, the test file named after each page it links to, the tests that page's validation evidence names, and, for a few rules, test files listed in `tests/traceability.rs`.\n"
    );
    let _ = writeln!(
        page,
        "| Rule | Specification | Details | Conformance cases | Tests | Review |"
    );
    let _ = writeln!(page, "| --- | --- | --- | --- | --- | --- |");
    for rule in &rules {
        let count = |pick: fn(&corpus::Case) -> &[&str]| {
            cases
                .iter()
                .filter(|case| pick(case).contains(&rule.id.as_str()))
                .count()
        };
        let (positive, invalid, boundary) = (
            count(|case| case.positive),
            count(|case| case.invalid),
            count(|case| case.boundary),
        );
        let mut tests: BTreeSet<String> = named.get(&rule.id).cloned().unwrap_or_default();
        for linked in &rule.pages {
            tests.extend(
                page_tests(linked)
                    .iter()
                    .map(|test| current(test).to_owned()),
            );
            let file = format!(
                "tests/{}.rs",
                linked.trim_end_matches(".md").replace('-', "_")
            );
            if is_test_file(&file) {
                tests.insert(file);
            }
        }
        for (_, files) in RULE_FILES.iter().filter(|(id, _)| *id == rule.id) {
            tests.extend(files.iter().map(|file| (*file).to_owned()));
        }
        // Test files first, then test names.
        let (files, names): (Vec<String>, Vec<String>) =
            tests.into_iter().partition(|test| is_test_file(test));
        let tests: Vec<String> = files.into_iter().chain(names).collect();
        let details: Vec<String> = rule
            .pages
            .iter()
            .map(|linked| format!("[{linked}]({linked})"))
            .collect();
        let dash = |cell: String| {
            if cell.is_empty() {
                "—".to_owned()
            } else {
                cell
            }
        };
        let _ = writeln!(
            page,
            "| {} — {} | [{}](language-specification.md#{}) | {} | {positive} valid, {invalid} invalid, {boundary} boundary | {} | {REVIEW} |",
            rule.id,
            rule.title,
            rule.section,
            anchor(&rule.section),
            dash(details.join(", ")),
            dash(tests_cell(&tests)),
        );
    }
    let _ = writeln!(page, "\n## Built-in statements\n");
    let _ = writeln!(
        page,
        "Each statement's entry in the [statement reference](statements.md) lists its kinds and errors; `tests/statement_reference.rs` requires the executed example it links to call it.\n"
    );
    let _ = writeln!(
        page,
        "| Statement | Rules | Reference | Executed example | Review |"
    );
    let _ = writeln!(page, "| --- | --- | --- | --- | --- |");
    for statement in statements(&read("docs/statements.md")) {
        let _ = writeln!(
            page,
            "| `{}` | {} | [entry](statements.md#{}) | {} | {REVIEW} |",
            statement.header.replace('|', "\\|"),
            group_rules(&statement.group, &statement.header),
            anchor(&format!("`{}`", statement.header)),
            statement.example,
        );
    }
    let _ = writeln!(page, "\n## Runtime and tooling guarantees\n");
    let _ = writeln!(
        page,
        "Each guarantee comes from a feature's validation evidence: the contract that states it, and the tests that verify it.\n"
    );
    let _ = writeln!(
        page,
        "| Feature | Guarantee | Contract | Tests | Evidence | Review |"
    );
    let _ = writeln!(page, "| --- | --- | --- | --- | --- | --- |");
    for (file, feature, requirements) in guarantees() {
        for requirement in requirements {
            let tests: Vec<String> = requirement["tests"]
                .as_array()
                .map(|tests| {
                    tests
                        .iter()
                        .map(|test| test.as_str().unwrap().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            let _ = writeln!(
                page,
                "| {feature} | {} | {} | {} | [{file}]({file}) | {REVIEW} |",
                requirement["requirement"]
                    .as_str()
                    .unwrap()
                    .replace('|', "\\|"),
                requirement["contract"]
                    .as_str()
                    .map_or_else(|| "—".to_owned(), link),
                tests_cell(&tests),
            );
        }
    }
    page
}

#[test]
fn the_traceability_index_is_current() {
    let path = root().join("docs/traceability.md");
    let generated = render();
    if std::env::var_os("BOTWORK_UPDATE_TRACEABILITY").is_some() {
        fs::write(&path, &generated).unwrap();
    }
    assert!(
        fs::read_to_string(&path).unwrap() == generated,
        "docs/traceability.md is stale; run BOTWORK_UPDATE_TRACEABILITY=1 cargo test --test traceability"
    );
}

#[test]
fn every_test_the_index_names_exists() {
    let functions = functions();
    let page = read("docs/traceability.md");
    let named: BTreeSet<&str> = page
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|name| looks_like_a_test(name))
        .collect();
    let missing: Vec<&&str> = named
        .iter()
        .filter(|name| !functions.contains(**name))
        .collect();
    assert!(
        missing.is_empty(),
        "the index names missing tests: {missing:?}"
    );
    assert!(named.len() > 400, "{}", named.len());
    for (old, new, _) in RENAMED {
        assert!(
            !functions.contains(*old),
            "{old} exists again; drop its rename"
        );
        assert!(functions.contains(*new), "{new} is gone");
    }
}

#[test]
fn every_rule_statement_and_guarantee_is_indexed() {
    let page = read("docs/traceability.md");
    let specification = read("docs/language-specification.md");
    let rules = rules(&specification);
    assert!(rules.len() >= 79, "{}", rules.len());
    for rule in &rules {
        assert!(page.contains(&format!("| {} — ", rule.id)), "{}", rule.id);
        // Every rule has conformance cases, as tests/conformance.rs requires.
        assert!(
            corpus::cases().iter().any(|case| {
                case.positive.contains(&rule.id.as_str())
                    || case.invalid.contains(&rule.id.as_str())
                    || case.boundary.contains(&rule.id.as_str())
            }),
            "{} has no conformance case",
            rule.id
        );
    }
    // Every rule has tests, and each curated file holds tests.
    for line in page.lines().filter(|line| line.contains(" boundary | ")) {
        assert!(!line.contains(" boundary | — |"), "no tests: {line}");
    }
    for (rule, files) in RULE_FILES {
        assert!(rules.iter().any(|known| known.id == *rule), "{rule}");
        for file in *files {
            let text = read(file);
            assert!(
                text.contains("#[test]") || text.contains("#[tokio::test"),
                "{file} holds no tests"
            );
        }
    }
    let statements = statements(&read("docs/statements.md"));
    assert_eq!(statements.len(), 100);
    assert!(statements
        .iter()
        .all(|statement| !statement.example.is_empty()));
    let guarantees: usize = guarantees()
        .iter()
        .map(|(_, _, requirements)| requirements.len())
        .sum();
    assert!(guarantees > 150, "{guarantees}");
}
