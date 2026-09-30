//! The owner's roadmap decisions in `docs/decisions.md` stay consistent with the
//! roadmap that cites them, the documents that link to them, and the recorded
//! performance baseline their budgets derive from.
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
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

fn anchors() -> BTreeSet<String> {
    read("docs/decisions.md")
        .lines()
        .filter_map(|line| line.strip_prefix("### "))
        .map(anchor)
        .collect()
}

/// The decision numbers in the summary table, in order.
fn decisions() -> Vec<String> {
    read("docs/decisions.md")
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            (cells.len() == 4
                && cells[1]
                    .strip_prefix('D')
                    .is_some_and(|number| number.parse::<u32>().is_ok()))
            .then(|| cells[1].to_owned())
        })
        .collect()
}

#[test]
fn every_decision_has_a_section_and_is_cited_by_the_roadmap() {
    let numbers = decisions();
    let expected: Vec<String> = (1..=19).map(|number| format!("D{number}")).collect();
    assert_eq!(numbers, expected);
    let anchors = anchors();
    let todo = read("TODO.md");
    for number in numbers {
        let prefix = format!("{}-", number.to_lowercase());
        let section = anchors
            .iter()
            .find(|anchor| anchor.starts_with(&prefix))
            .unwrap_or_else(|| panic!("{number} has no section"));
        assert!(
            todo.contains(&format!("decisions.md#{section}")),
            "TODO.md does not cite {number}"
        );
    }
}

#[test]
fn links_into_the_decisions_resolve() {
    let anchors = anchors();
    let mut checked = 0;
    for file in [
        "TODO.md",
        "README.md",
        "docs/performance.md",
        "docs/quality-assessment.md",
    ] {
        let text = read(file);
        for (at, _) in text.match_indices("decisions.md#") {
            let target: String = text[at + "decisions.md#".len()..]
                .chars()
                .take_while(|character| *character != ')')
                .collect();
            assert!(
                anchors.contains(&target),
                "{file}: #{target} is not a heading"
            );
            checked += 1;
        }
    }
    assert!(checked >= 30, "{checked}");
}

#[test]
fn the_roadmap_states_the_revised_gates() {
    let todo = read("TODO.md");
    assert!(todo.contains("at least 1 CPU-hour per target before release"));
    assert!(!todo.contains("24 CPU-hours per target before release"));
    assert!(
        todo.contains("at least 95% line coverage of parser/evaluator/runtime correctness code.")
    );
    assert!(!todo.contains("plus 90% branch coverage"));
    // The registry moved past 1.0.
    let (before, after) = todo.split_once("\n## After 1.0\n").unwrap();
    assert!(after.contains("Build the trusted and verified package registry"));
    assert!(!before.contains("Build the trusted and verified package registry"));
}

/// `n` with thousands separators.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut result = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(digit);
    }
    result
}

#[test]
fn registered_budgets_follow_the_accepted_baseline() {
    let evidence: Value =
        serde_json::from_str(&read("docs/performance-baseline-evidence.json")).unwrap();
    assert_eq!(evidence["schema"], 2);
    let revision = evidence["base_revision"].as_str().unwrap();
    let decisions = read("docs/decisions.md");
    assert!(decisions.contains(&format!("revision `{}`", &revision[..7])));
    let performance = read("docs/performance.md");
    // The binary's budget: its size in bytes × 1.25, rounded up.
    let binary = evidence["binaries"]["botwork"]["bytes"].as_u64().unwrap();
    let size = format!("{} bytes", grouped((binary * 5).div_ceil(4)));
    for (file, text, heading) in [
        ("decisions", &decisions, "### D4: Budgets and baseline"),
        ("performance", &performance, "## Registered budgets"),
    ] {
        let (_, section) = text.split_once(heading).unwrap();
        let (section, _) = section.split_once("\n## ").unwrap_or((section, ""));
        assert!(
            section.contains(&size),
            "{file}: the binary should budget {size}"
        );
    }
    for (workload, label) in [
        ("cli-startup", "CLI startup"),
        ("parse", "Parse 10,000 statements"),
        ("calls", "100,000 custom calls"),
        ("loop", "1,000,000 loop iterations"),
        ("source-io", "Sixteen 256 KiB source loads"),
        ("waiting", "100 waiting runs"),
    ] {
        let statistics = &evidence["statistics"][workload];
        let p95 = statistics["workload_elapsed_ns"]["p95"].as_u64().unwrap();
        let heap = statistics["heap_kib"]["max"].as_u64().unwrap();
        // Hundredths of a millisecond, rounded; whole KiB, rounded up.
        let hundredths = (p95 as f64 * 1.25 / 10_000.0).round() as u64;
        let time = format!("{}.{:02} ms", grouped(hundredths / 100), hundredths % 100);
        let memory = format!("{} KiB", grouped((heap * 5).div_ceil(4)));
        // Each file's budget table, after its budget heading.
        for (file, text, heading) in [
            ("decisions", &decisions, "### D4: Budgets and baseline"),
            ("performance", &performance, "## Registered budgets"),
        ] {
            let (_, section) = text.split_once(heading).unwrap();
            let row = section
                .lines()
                .find(|line| line.starts_with(&format!("| {label} |")))
                .unwrap_or_else(|| panic!("{file}: no row for {label}"));
            assert!(
                row.contains(&format!("| {time} |")) && row.contains(&format!("| {memory} |")),
                "{file}: {label} should budget {time} and {memory}: {row}"
            );
        }
    }
}
