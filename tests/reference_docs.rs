//! The reference pages agree with what they describe: the CLI reference with
//! `botwork --help` and the exit statuses the CLI returns, the output formats
//! with the versions Botwork writes, the compatibility page with the CI
//! workflow and manifests, and the documentation index with `docs/`.
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_owned()
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap()
}

fn botwork(directory: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(arguments)
        .current_dir(directory)
        .output()
        .unwrap()
}

/// An option from `botwork --help`: its short form, value name, and default.
#[derive(Debug, PartialEq, Eq)]
struct CliOption {
    short: Option<String>,
    value: Option<String>,
    default: Option<String>,
}

fn help_options() -> BTreeMap<String, CliOption> {
    let output = botwork(&root(), &["--help"]);
    let help = String::from_utf8(output.stdout).unwrap();
    let mut options = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in help.lines() {
        let trimmed = line.trim_start();
        // Options are indented two to six spaces; descriptions, ten.
        if trimmed.starts_with('-') && line.len() - trimmed.len() <= 6 {
            let (short, rest) = match trimmed.split_once(", ") {
                Some((short, rest)) if !short.starts_with("--") => (Some(short.to_owned()), rest),
                _ => (None, trimmed),
            };
            let (long, value) = match rest.split_once(' ') {
                Some((long, value)) => (long, Some(value.trim().to_owned())),
                None => (rest, None),
            };
            options.insert(
                long.to_owned(),
                CliOption {
                    short,
                    value,
                    default: None,
                },
            );
            current = Some(long.to_owned());
        } else if let (Some(long), Some((_, default))) =
            (&current, trimmed.split_once("[default: "))
        {
            options.get_mut(long).unwrap().default = Some(default.trim_end_matches(']').to_owned());
        }
    }
    options
}

/// Option rows of the CLI reference: flag cell and default cell.
fn documented_options() -> BTreeMap<String, CliOption> {
    let reference = read("docs/cli.md");
    let (_, options) = reference.split_once("## Options").unwrap();
    let (options, _) = options.split_once("## Exit statuses").unwrap();
    let mut documented = BTreeMap::new();
    for line in options.lines().filter(|line| line.starts_with("| `-")) {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let flags: Vec<&str> = cells[1]
            .split(", ")
            .map(|flag| flag.trim_matches('`'))
            .collect();
        let (short, long) = match flags.as_slice() {
            [short, long] => (Some((*short).to_owned()), *long),
            [long] => (None, *long),
            other => panic!("{other:?}"),
        };
        let (long, value) = match long.split_once(' ') {
            Some((long, value)) => (long, Some(value.to_owned())),
            None => (long, None),
        };
        let default = Some(cells[2]).filter(|default| !default.is_empty());
        documented.insert(
            long.to_owned(),
            CliOption {
                short,
                value,
                default: default.map(str::to_owned),
            },
        );
    }
    documented
}

#[test]
fn the_cli_reference_lists_every_option_with_its_default() {
    let help = help_options();
    let documented = documented_options();
    assert_eq!(
        help.keys().collect::<Vec<_>>(),
        documented.keys().collect::<Vec<_>>(),
        "docs/cli.md and botwork --help list different options"
    );
    // Defaults that --help does not print, and where they are documented.
    let effective = [
        ("--listener-queue", "8192", "default 8,192"),
        ("--listener-timeout-ms", "10000", "default 10,000"),
    ];
    let listeners = read("docs/listeners.md");
    for (long, option) in &help {
        let entry = &documented[long];
        assert_eq!(
            (&entry.short, &entry.value),
            (&option.short, &option.value),
            "{long}"
        );
        match (&option.default, &entry.default) {
            (Some(default), documented) => {
                assert_eq!(documented.as_ref(), Some(default), "{long} default")
            }
            (None, Some(documented)) if documented == "none" => {}
            (None, Some(documented)) => {
                let (_, value, source) = effective
                    .iter()
                    .find(|(name, ..)| name == long)
                    .unwrap_or_else(|| panic!("{long}: {documented} is not a --help default"));
                assert_eq!(documented, value, "{long}");
                assert!(listeners.contains(source), "{long}: {source}");
            }
            (None, None) => {}
        }
    }
}

#[test]
fn exit_statuses_match_the_reference() {
    let reference = read("docs/cli.md");
    let (_, statuses) = reference.split_once("## Exit statuses").unwrap();
    let documented: BTreeSet<i32> = statuses
        .lines()
        .filter_map(|line| line.strip_prefix("| ")?.split(" |").next()?.parse().ok())
        .collect();
    assert_eq!(documented, BTreeSet::from([0, 1, 2, 130]));
    let directory = tempfile::tempdir().unwrap();
    let path = |name: &str, text: &str| {
        fs::write(directory.path().join(name), text).unwrap();
        name.to_owned()
    };
    let status = |arguments: &[&str]| botwork(directory.path(), arguments).status.code();
    let ok = path("ok.botwork", "Log |1|\n");
    let failing = path("failing.botwork", "Fail |\"no\"|\n");
    let broken = path("broken.botwork", "Log |1\n");
    let messy = path("messy.botwork", "log   |1|\n");
    assert_eq!(status(&["--file", &ok]), Some(0));
    assert_eq!(status(&["--file", &failing]), Some(1));
    assert_eq!(status(&["--jobs", "0", "--file", &ok]), Some(2));
    assert_eq!(status(&["--check", "--file", &ok]), Some(0));
    assert_eq!(status(&["--check", "--file", &broken]), Some(1));
    assert_eq!(status(&["--format-check", "--file", &messy]), Some(1));
    assert_eq!(status(&["--format", "--file", &broken]), Some(1));
    // The second-interrupt status, 130, is exercised in tests/terminal_outcomes.rs.
}

#[test]
fn output_formats_match_the_versions_botwork_writes() {
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path();
    fs::write(directory.join("ok.botwork"), "Log |1|\n").unwrap();
    fs::write(
        directory.join("s.suite.botwork"),
        "Suite |\"s\"| { Case |\"c\"| { Fail |\"no\"| } }\n",
    )
    .unwrap();
    let output = botwork(
        directory,
        &[
            "--suite",
            "s.suite.botwork",
            "--report-json",
            "report.json",
            "--failures",
            "failures.json",
            "--listener",
            "sh",
            "--listener-arg",
            "-c",
            "--listener-arg",
            "cat > events.jsonl",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let json = |text: &str| serde_json::from_str::<Value>(text).unwrap();
    let identity = |value: &Value| {
        (
            value["format"].as_str().unwrap().to_owned(),
            value["version"].as_u64().unwrap(),
        )
    };
    let events = fs::read_to_string(directory.join("events.jsonl")).unwrap();
    let written = BTreeMap::from([
        (
            "JSON report",
            identity(&json(
                &fs::read_to_string(directory.join("report.json")).unwrap(),
            )),
        ),
        (
            "Failed-case record",
            identity(&json(
                &fs::read_to_string(directory.join("failures.json")).unwrap(),
            )),
        ),
        (
            "Event stream",
            identity(&json(events.lines().next().unwrap())),
        ),
        (
            "Run record",
            (
                botwork::core::report::RECORD_FORMAT.to_owned(),
                u64::from(botwork::core::report::RECORD_VERSION),
            ),
        ),
    ]);
    let reporting = read("docs/reporting.md");
    let (_, formats) = reporting.split_once("## Versioned formats").unwrap();
    for (output, (format, version)) in written {
        let row = formats
            .lines()
            .find(|line| line.starts_with(&format!("| {output} |")))
            .unwrap_or_else(|| panic!("no row for {output}"));
        let cells: Vec<&str> = row.split(" | ").collect();
        assert_eq!(cells[1], format!("`{format}`"), "{output}");
        assert!(
            cells[2] == version.to_string() || cells[2].starts_with(&format!("{version}, ")),
            "{output}: documented {}, writes {version}",
            cells[2]
        );
    }
}

/// The value after `prefix` in `text`, up to the first character not in `allowed`.
fn value_after(text: &str, prefix: &str, allowed: impl Fn(char) -> bool) -> String {
    let (_, rest) = text
        .split_once(prefix)
        .unwrap_or_else(|| panic!("{prefix}"));
    rest.chars()
        .take_while(|character| allowed(*character))
        .collect()
}

#[test]
fn compatibility_versions_match_their_sources() {
    let compatibility = read("docs/compatibility.md");
    let ci = read(".github/workflows/ci.yml");
    let version = |character: char| character.is_ascii_digit() || character == '.';
    let tree_sitter = value_after(&ci, "tree-sitter-cli@", version);
    let helix = value_after(&ci, "helix/releases/download/", version);
    let node = value_after(&ci, "node-version: ", version);
    assert!(
        compatibility.contains(&format!("| {tree_sitter} in CI |")),
        "{tree_sitter}"
    );
    assert!(
        compatibility.contains(&format!("| {helix} in CI |")),
        "{helix}"
    );
    assert!(
        compatibility.contains(&format!("| {node} | {node} in CI |")),
        "{node}"
    );
    let manifest: Value = serde_json::from_str(&read("editors/vscode/package.json")).unwrap();
    let vscode = manifest["engines"]["vscode"].as_str().unwrap();
    let vscode = vscode.trim_start_matches('^').trim_end_matches(".0");
    assert!(
        compatibility.contains(&format!("| VS Code | {vscode} or later |")),
        "{vscode}"
    );
    // Latest stable only: CI installs stable, and Cargo.toml pins no older one.
    assert!(ci.contains("rustup toolchain install stable"));
    assert!(!read("Cargo.toml").contains("rust-version"));
    assert!(compatibility.contains("latest stable Rust release"));
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

/// Relative Markdown link targets in `text`.
fn links(text: &str) -> Vec<String> {
    text.match_indices("](")
        .filter_map(|(at, _)| {
            let target: String = text[at + 2..].chars().take_while(|c| *c != ')').collect();
            (!target.starts_with("http://") && !target.starts_with("https://")).then_some(target)
        })
        .collect()
}

#[test]
fn reference_pages_link_to_pages_and_headings_that_exist() {
    let docs = root().join("docs");
    let mut checked = 0;
    for page in [
        "README.md",
        "cli.md",
        "configuration.md",
        "reporting.md",
        "extending.md",
        "distribution.md",
        "compatibility.md",
    ] {
        let text = fs::read_to_string(docs.join(page)).unwrap();
        for target in links(&text) {
            let (file, fragment) = target.split_once('#').unwrap_or((&target, ""));
            let path = if file.is_empty() {
                docs.join(page)
            } else {
                docs.join(file)
            };
            assert!(path.exists(), "{page}: {target}");
            if !fragment.is_empty() {
                let headings: BTreeSet<String> = fs::read_to_string(&path)
                    .unwrap()
                    .lines()
                    .filter_map(|line| {
                        line.trim_start_matches('#')
                            .strip_prefix(' ')
                            .filter(|_| line.starts_with('#'))
                    })
                    .map(anchor)
                    .collect();
                assert!(headings.contains(fragment), "{page}: {target}");
            }
            checked += 1;
        }
    }
    assert!(checked > 150, "{checked}");
}

#[test]
fn the_documentation_index_lists_every_page() {
    let index = read("docs/README.md");
    let linked: BTreeSet<String> = links(&index).into_iter().collect();
    let pages: BTreeSet<String> = fs::read_dir(root().join("docs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".md") && name != "README.md")
        .collect();
    let missing: Vec<_> = pages.difference(&linked).collect();
    assert!(
        missing.is_empty(),
        "docs/README.md does not list {missing:?}"
    );
    assert!(read("README.md").contains("(docs/README.md)"));
}
