//! Every `botwork` command in the getting-started guide runs as documented. The
//! guide's scripts are written to the files it names, each command runs in that
//! directory, and the output the guide shows is compared with what it prints.
#[path = "support/markdown.rs"]
mod markdown;

use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    process::{Command, Output},
};

/// Split a documented command line into words, honouring single quotes.
fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    for character in line.chars() {
        match character {
            '\'' => {
                quoted = !quoted;
                started = true;
            }
            ' ' if !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            _ => {
                word.push(character);
                started = true;
            }
        }
    }
    assert!(!quoted, "unclosed quote in {line:?}");
    if started {
        words.push(word);
    }
    words
}

fn run(directory: &Path, line: &str) -> Output {
    let words = words(line);
    assert_eq!(words[0], "botwork", "{line}");
    Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(&words[1..])
        .current_dir(directory)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn getting_started_commands_run_as_documented() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let guide = fs::read_to_string(root.join("docs/getting-started.md")).unwrap();
    let blocks = markdown::blocks(&guide).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path();
    for block in &blocks {
        let name = match (block.language.as_str(), &block.id) {
            ("botwork", Some(id)) => format!("{id}.botwork"),
            ("botwork-suite", Some(id)) => format!("{id}.suite.botwork"),
            _ => continue,
        };
        assert!(
            guide.contains(&format!("`{name}`")),
            "the guide names {name}"
        );
        fs::write(workspace.join(name), &block.source).unwrap();
    }
    let shown: Vec<&str> = blocks
        .iter()
        .filter(|block| block.language == "text")
        .map(|block| block.source.as_str())
        .collect();
    let commands: Vec<&str> = blocks
        .iter()
        .filter(|block| block.language == "sh")
        .flat_map(|block| block.source.lines())
        .filter(|line| line.starts_with("botwork "))
        .collect();
    assert_eq!(commands.len(), 10, "{commands:#?}");
    let outputs: Vec<Output> = commands.iter().map(|line| run(workspace, line)).collect();
    let status = |index: usize| outputs[index].status.code();
    let stdout = |index: usize| text(&outputs[index].stdout);
    let stderr = |index: usize| text(&outputs[index].stderr);

    assert_eq!(status(0), Some(0));
    assert!(stdout(0).starts_with("botwork "), "{}", stdout(0));
    // The first script.
    assert_eq!((status(1), stdout(1)), (Some(0), "Hello, botwork!\n"));
    // Variables, statements, and an input override.
    assert_eq!((status(2), stdout(2)), (Some(0), shown[0]));
    assert_eq!(status(3), Some(0));
    assert_eq!(stdout(3), shown[0].replace("Ada", "Grace"));
    // The documented failure, exactly as shown.
    assert_eq!((status(4), stdout(4)), (Some(1), ""));
    assert_eq!(stderr(4), shown[1]);
    // The suite prints the documented lines, in any order, and logs twice.
    assert_eq!((status(5), stdout(5)), (Some(0), "EUR\nEUR\n"));
    let lines = |source: &str| source.lines().map(str::to_owned).collect::<BTreeSet<_>>();
    assert_eq!(lines(stderr(5)), lines(shown[2]));
    assert_eq!(stderr(5).lines().count(), shown[2].lines().count());
    assert!(stderr(5).ends_with("[cases] 3 selected: 3 succeeded, 0 failed\n"));
    // Listing and selecting cases by ID.
    assert_eq!(status(6), Some(0));
    let listed: Vec<serde_json::Value> = stdout(6)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let ids: Vec<_> = listed
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "checkout/empty",
            "checkout/total/single",
            "checkout/total/several"
        ]
    );
    assert!(listed
        .iter()
        .all(|case| case["tags"] == serde_json::json!(["smoke"])));
    assert_eq!(status(7), Some(0));
    assert!(stderr(7).ends_with("[cases] 1 selected: 1 succeeded, 0 failed\n"));
    // Continuous integration: parallel cases, both reports, and a failed-case record.
    assert_eq!(status(8), Some(0), "{}", stderr(8));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(workspace.join("report.json")).unwrap()).unwrap();
    assert_eq!(
        (report["complete"].as_bool(), report["exit_code"].as_i64()),
        (Some(true), Some(0))
    );
    assert!(fs::read_to_string(workspace.join("report.html"))
        .unwrap()
        .contains("checkout/total/several"));
    let failed: serde_json::Value =
        serde_json::from_slice(&fs::read(workspace.join("failed.json")).unwrap()).unwrap();
    assert_eq!(
        (
            failed["complete"].as_bool(),
            failed["failed"].as_array().map(Vec::len)
        ),
        (Some(true), Some(0))
    );
    // After a clean run, the rerun selects nothing and succeeds.
    assert_eq!(status(9), Some(0));
    assert!(stderr(9).ends_with("[cases] 0 selected: 0 succeeded, 0 failed\n"));

    // Commands mentioned inline run too; only the failing script fails.
    let inline: Vec<&str> = guide
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| span.starts_with("botwork "))
        .collect();
    assert_eq!(inline.len(), 3, "{inline:?}");
    for line in inline {
        let output = run(workspace, line);
        let expected = if line.contains("first-failure") { 1 } else { 0 };
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{line}: {}",
            text(&output.stderr)
        );
        assert!(
            !output.stdout.is_empty() || !output.stderr.is_empty(),
            "{line}"
        );
    }
}
