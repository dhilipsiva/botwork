#[path = "support/cli_harness.rs"]
mod cli_harness;
use cli_harness::Harness;
use serde_json::{json, Value};
use std::{fs, process::Output, time::Duration};

fn command(harness: &Harness, args: &[&str]) -> Output {
    harness
        .command("rows", args, Duration::from_secs(15))
        .unwrap()
}

fn source(harness: &Harness, text: &str) {
    fs::write(harness.workspace.join("suite.botwork"), text).unwrap();
}

fn state(harness: &Harness) -> Value {
    serde_json::from_slice(&fs::read(harness.workspace.join("failed.json")).unwrap()).unwrap()
}

fn success(output: &Output, stdout: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(output.stdout, stdout.as_bytes());
    stderr
}

#[test]
fn dataset_declaration_words_remain_ordinary_custom_names_in_scripts() {
    let harness = Harness::new();
    let output = harness.run("names", "Dataset { Return |1| }\nRow { Return |2| }\nValues { Return |3| }\nUsing { Return |4| }\nLog |@{ Dataset } + @{ Row } + @{ Values } + @{ Using }|", Duration::from_secs(5)).unwrap();
    assert_eq!(success(&output, "10\n"), "");
}

#[test]
fn rows_get_fresh_inputs_libraries_and_module_state_and_common_inputs_are_preserved() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Log |\"init\"|\nRead { Return |42| }",
    )
    .unwrap();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| {
Row |"first"| Values |{n: 21}|
Row |"second"| Values |{n: 4}|
}
Library {
Import |"module.botwork"| As |module|
Double { Return |data.n * 2| }
}
Case |"double"| Using |"d"| As |data| {
Log |@{ Double }|
Log |common|
|data| = |99|
|common| = |0|
}
Case |"again"| Using |"d"| As |data| { Log |data.n| }
Case |"plain"| { Log |data| }
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "1",
            "--var",
            "data=8",
            "--var",
            "common=7",
        ],
    );
    let stderr = success(
        &output,
        "init\n42\n7\ninit\n8\n7\ninit\n21\ninit\n4\ninit\n8\n",
    );
    for id in [
        "s/double/first",
        "s/double/second",
        "s/again/first",
        "s/again/second",
        "s/plain",
    ] {
        assert_eq!(stderr.matches(&format!("[case {id}] started:")).count(), 1);
        assert_eq!(
            stderr.matches(&format!("[case {id}] succeeded:")).count(),
            1
        );
    }
    assert!(stderr.ends_with("[cases] 5 selected: 5 succeeded, 0 failed\n"));
}

#[test]
fn each_row_has_an_outcome_and_only_failed_rows_rerun_after_reordering_and_repair() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| {
Row |"first"| Values |0|
Row |"middle"| Values |2|
Row |"last"| Values |0|
}
Case |"divide"| Using |"d"| As |n| { Log |8 / n| }
Case |"unrelated"| { Log |42| }
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "1",
            "--failures",
            "failed.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"4\n42\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    for id in ["s/divide/first", "s/divide/last"] {
        assert_eq!(
            stderr.matches(&format!("[case {id}] failed:")).count(),
            1,
            "{stderr}"
        );
    }
    assert!(stderr.ends_with("[cases] 4 selected: 2 succeeded, 2 failed\n"));
    assert_eq!(
        state(&harness),
        json!({"format":"botwork-failed-cases","version":2,"complete":true,"failed":["s/divide/first","s/divide/last"]})
    );
    source(
        &harness,
        r#"Suite |"s"| Named |"Renamed suite"| {
Dataset |"renamed-data"| {
Row |"last"| Named |"Fixed last"| Values |4|
Row |"middle"| Values |0|
Row |"first"| Values |1|
}
Case |"divide"| Named |"Renamed case"| Using |"renamed-data"| As |n| { Log |8 / n| }
Case |"unrelated"| { Log |undefined| }
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "1",
            "--rerun-failed",
            "failed.json",
            "--failures",
            "failed.json",
        ],
    );
    let stderr = success(&output, "2\n8\n");
    assert!(stderr.contains("[case s/divide/last] succeeded:"));
    assert!(stderr.contains("[case s/divide/first] succeeded:"));
    assert!(!stderr.contains("[case s/divide/middle]"));
    assert_eq!(state(&harness)["failed"], json!([]));
}

#[test]
fn listing_selects_parent_or_exact_row_and_inherits_all_tag_levels_without_effects() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| Named |"Suite name"| Tags |["suite"]| {
Dataset |"d"| Tags |["data"]| {
Row |"z"| Named |"தமிழ்\nrow"| Tags |["fast"]| Values |none|
Row |"a"| Tags |["slow"]| Values |[true, false, -2, 1.5]|
}
Library { Import |"missing.botwork"| As |missing| }
Case |"c"| Named |"Case"| Tags |["case"]| Using |"d"| As |value| { Log |undefined| }
Case |"plain"| {}
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--list-cases",
            "--case",
            "s/c",
            "--tag",
            "data",
            "--exclude-tag",
            "slow",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({
            "id":"s/c/z", "case":"s/c", "dataset":"d", "suite":"Suite name",
            "name":"Case / தமிழ்\nrow", "row":{"id":"z","name":"தமிழ்\nrow"},
            "tags":["case","data","fast","suite"]
        })
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--list-cases",
            "--case",
            "s/c/a",
        ],
    );
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["id"],
        "s/c/a"
    );
    for id in ["s/c/missing", "s/plain/z", "s/c/a/extra"] {
        let output = command(&harness, &["--suite", "suite.botwork", "--case", id]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let output = command(
            &harness,
            &[
                "--suite",
                "suite.botwork",
                "--list-cases",
                "--case",
                "s/c/z",
                "--case",
                id,
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn external_files_resolve_relative_to_each_suite_and_reuse_typed_rows_with_local_aliases() {
    let harness = Harness::new();
    fs::create_dir(harness.workspace.join("nested")).unwrap();
    fs::write(
        harness.workspace.join("shared.dataset.botwork"),
        r#"Dataset |"original"| Tags |["shared"]| {
Row |"edge"| Values |{min: -2147483648, text: "é", values: [none, true, 1.5]}|
}"#,
    )
    .unwrap();
    for (path, id, reference) in [
        ("suite.botwork", "root", "shared.dataset.botwork"),
        (
            "nested/suite.botwork",
            "nested",
            "../shared.dataset.botwork",
        ),
    ] {
        fs::write(
            harness.workspace.join(path),
            format!(
                r#"Suite |"{id}"| {{
Dataset |"alias"| From |"{reference}"|
Case |"c"| Using |"alias"| As |data| {{
Log |data.min|
Log |data.text|
Log |data.values|
}}
}}"#
            ),
        )
        .unwrap();
    }
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--suite",
            "nested/suite.botwork",
            "--jobs",
            "1",
            "--tag",
            "shared",
        ],
    );
    let stderr = success(
        &output,
        "-2147483648\né\n[none, true, 1.5]\n-2147483648\né\n[none, true, 1.5]\n",
    );
    assert!(stderr.contains("[case root/c/edge] succeeded:"));
    assert!(stderr.contains("[case nested/c/edge] succeeded:"));
}

#[test]
fn invalid_unselected_data_blocks_all_effects_and_keeps_failure_record_incomplete() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| From |"data.dataset.botwork"|
Case |"ordinary"| { Log |"must not execute"| }
Case |"rows"| Using |"d"| As |data| {}
}"#,
    );
    for data in [
        r#"Dataset |"d"| { Row |"r"| Values |1 + 2| }"#,
        r#"Dataset |"d"| { Row |"r"| Values |1| Row |"r"| Values |2| }"#,
        r#"Dataset |"d"| From |"recursive.dataset.botwork"|"#,
    ] {
        fs::write(harness.workspace.join("data.dataset.botwork"), data).unwrap();
        let output = command(
            &harness,
            &[
                "--suite",
                "suite.botwork",
                "--case",
                "s/ordinary",
                "--failures",
                "failed.json",
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("[case "));
        assert_eq!(state(&harness)["complete"], false);
        let output = command(
            &harness,
            &[
                "--suite",
                "suite.botwork",
                "--case",
                "s/ordinary",
                "--list-cases",
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }
    fs::write(harness.workspace.join("data.dataset.botwork"), [0xff]).unwrap();
    let output = command(&harness, &["--suite", "suite.botwork"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("utf-8"));
    fs::remove_file(harness.workspace.join("data.dataset.botwork")).unwrap();
    let output = command(&harness, &["--suite", "suite.botwork"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("suite.botwork:2:"), "{stderr}");
    assert!(stderr.contains("data.dataset.botwork"), "{stderr}");
}

#[test]
fn failure_record_migration_rejects_template_expansion_and_accepts_exact_leaf_ids() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| { Row |"r"| Values |42| }
Case |"c"| Using |"d"| As |data| { Log |data| }
Case |"plain"| { Log |7| }
}"#,
    );
    for (version, id, expected) in [
        (1, "s/plain", Some("7\n")),
        (2, "s/plain", Some("7\n")),
        (2, "s/c/r", Some("42\n")),
        (1, "s/c/r", None),
        (1, "s/c", None),
        (2, "s/c", None),
        (2, "s/c/missing", None),
        (3, "s/c/r", None),
    ] {
        fs::write(harness.workspace.join("failed.json"), json!({"format":"botwork-failed-cases","version":version,"complete":true,"failed":[id]}).to_string()).unwrap();
        let output = command(
            &harness,
            &["--suite", "suite.botwork", "--rerun-failed", "failed.json"],
        );
        if let Some(stdout) = expected {
            success(&output, stdout);
        } else {
            assert_eq!(output.status.code(), Some(1), "{version}: {id}");
            assert!(output.stdout.is_empty());
        }
    }
    // A valid leaf beside a stale parent must not turn the record into a
    // partial rerun that silently ignores one of its saved failures.
    fs::write(
        harness.workspace.join("failed.json"),
        json!({
            "format":"botwork-failed-cases","version":2,"complete":true,
            "failed":["s/c", "s/c/r"]
        })
        .to_string(),
    )
    .unwrap();
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--rerun-failed", "failed.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unknown case selector"));
}

#[test]
fn discovery_accepts_exact_dataset_row_and_node_capacity() {
    let harness = Harness::new();
    let rows: String = (0..1024)
        .map(|i| format!("Row |\"r{i}\"| Values |0|"))
        .collect();
    let value = format!("{{{}}}", vec!["same:0"; 16_383].join(","));
    let nodes = format!("Row |\"a\"| Values |{value}| Row |\"b\"| Values |{value}|");
    for (body, count) in [
        (rows.as_str(), 4),
        (nodes.as_str(), 2),
        ("Row |\"r\"| Values |0|", 64),
    ] {
        let mut declarations = String::new();
        for i in 0..count {
            let path = format!("data{i}");
            fs::write(
                harness.workspace.join(&path),
                format!("Dataset |\"d\"| {{ {body} }}"),
            )
            .unwrap();
            declarations.push_str(&format!("Dataset |\"d{i}\"| From |\"{path}\"|"));
        }
        source(
            &harness,
            &format!("Suite |\"s\"| {{ {declarations} Case |\"c\"| {{ Log |42| }} }}"),
        );
        success(&command(&harness, &["--suite", "suite.botwork"]), "42\n");
    }
}

#[test]
fn external_source_bytes_are_admitted_at_exact_combined_capacity() {
    let harness = Harness::new();
    let declarations: String = (0..8)
        .map(|i| format!("Dataset |\"d{i}\"| From |\"data{i}\"|"))
        .collect();
    let suite = format!("Suite |\"s\"| {{ {declarations} Case |\"c\"| {{ Log |42| }} }}");
    source(&harness, &suite);
    let mut last = String::new();
    for i in 0..8 {
        let mut data = "Dataset |\"d\"| { Row |\"r\"| Values |0| }".to_owned();
        let size = 1024 * 1024 - if i == 7 { suite.len() } else { 0 };
        data.push_str(&" ".repeat(size - data.len()));
        fs::write(harness.workspace.join(format!("data{i}")), &data).unwrap();
        last = data;
    }
    success(&command(&harness, &["--suite", "suite.botwork"]), "42\n");
    last.push(' ');
    fs::write(harness.workspace.join("data7"), last).unwrap();
    let output = command(&harness, &["--suite", "suite.botwork"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("suite discovery source bytes"));
}

#[test]
fn per_row_output_limits_fail_only_the_oversized_row_and_reset_for_siblings() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "Log |7|").unwrap();
    source(
        &harness,
        r#"Suite |"s"| {
Dataset |"d"| {
Row |"too-large"| Values |[1, 2, 3]|
Row |"small"| Values |42|
}
Library { Import |"module.botwork"| As |module| }
Case |"c"| Using |"d"| As |data| { Log |data| }
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "1",
            "--max-output-record-bytes",
            "3",
            "--failures",
            "failed.json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"7\n7\n42\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("output record bytes"), "{stderr}");
    assert!(stderr.ends_with("[cases] 2 selected: 1 succeeded, 0 failed, 1 limit exceeded\n"));
    assert_eq!(state(&harness)["failed"], json!(["s/c/too-large"]));
}

#[test]
fn external_source_and_row_expansion_bounds_apply_before_selection() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"s"| { Dataset |"d"| From |"data"| Case |"c"| {} }"#,
    );
    fs::write(harness.workspace.join("data"), " ".repeat(1024 * 1024 + 1)).unwrap();
    let output = command(&harness, &["--suite", "suite.botwork", "--list-cases"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("dataset source bytes"));
    let rows: String = (0..1024)
        .map(|i| format!("Row |\"r{i}\"| Values |0|"))
        .collect();
    fs::write(
        harness.workspace.join("data"),
        format!("Dataset |\"d\"| {{ {rows} }}"),
    )
    .unwrap();
    let cases: String = (0..5)
        .map(|i| format!("Case |\"c{i}\"| Using |\"d\"| As |data| {{}}"))
        .collect();
    source(
        &harness,
        &format!("Suite |\"s\"| {{ Dataset |\"d\"| From |\"data\"| {cases} }}"),
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--case",
            "s/c0/r0",
            "--list-cases",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("discovered case rows"));
}

#[test]
fn discovery_rejects_aggregate_external_data_limits_even_when_unused() {
    let harness = Harness::new();
    let rows: String = (0..1024)
        .map(|i| format!("Row |\"r{i}\"| Values |0|"))
        .collect();
    let value = format!("{{{}}}", vec!["same:0"; 16_383].join(","));
    let nodes = format!("Row |\"a\"| Values |{value}| Row |\"b\"| Values |{value}|");
    for (body, count, padding, resource) in [
        (rows.as_str(), 5, false, "defined data rows"),
        (nodes.as_str(), 3, false, "dataset literal nodes"),
        (
            "Row |\"r\"| Values |0|",
            8,
            true,
            "suite discovery source bytes",
        ),
    ] {
        let mut declarations = String::new();
        for i in 0..count {
            let path = format!("data{i}");
            let mut data = format!("Dataset |\"d\"| {{ {body} }}");
            if padding {
                data.push_str(&" ".repeat(1024 * 1024 - data.len()));
            }
            fs::write(harness.workspace.join(&path), data).unwrap();
            declarations.push_str(&format!("Dataset |\"d{i}\"| From |\"{path}\"|"));
        }
        // Admission must stop before even trying this later file. Deferring
        // all accounting until selection would report a missing file instead.
        declarations.push_str("Dataset |\"later\"| From |\"missing-after-limit\"|");
        source(
            &harness,
            &format!("Suite |\"s\"| {{ {declarations} Case |\"c\"| {{ Log |42| }} }}"),
        );
        let output = command(&harness, &["--suite", "suite.botwork", "--list-cases"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(resource),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut paths = Vec::new();
    for (suite, count) in [(0, 32), (1, 33)] {
        let mut declarations = String::new();
        for i in 0..count {
            let path = format!("unique{suite}-{i}");
            fs::write(
                harness.workspace.join(&path),
                "Dataset |\"d\"| { Row |\"r\"| Values |0| }",
            )
            .unwrap();
            declarations.push_str(&format!("Dataset |\"d{i}\"| From |\"{path}\"|"));
        }
        if suite == 1 {
            declarations.push_str("Dataset |\"later\"| From |\"missing-after-limit\"|");
        }
        let name = format!("unique{suite}.suite");
        fs::write(
            harness.workspace.join(&name),
            format!("Suite |\"s{suite}\"| {{ {declarations} Case |\"c\"| {{}} }}"),
        )
        .unwrap();
        paths.push(name);
    }
    let output = command(
        &harness,
        &["--suite", &paths[0], "--suite", &paths[1], "--list-cases"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("discovered datasets"));
}

#[cfg(target_os = "linux")]
#[test]
fn external_data_fifos_and_directories_are_rejected_without_waiting() {
    let harness = Harness::new();
    assert!(std::process::Command::new("mkfifo")
        .arg(harness.workspace.join("pipe"))
        .status()
        .unwrap()
        .success());
    fs::create_dir(harness.workspace.join("directory")).unwrap();
    for path in ["pipe", "directory"] {
        source(
            &harness,
            &format!("Suite |\"s\"| {{ Dataset |\"d\"| From |\"{path}\"| Case |\"c\"| {{}} }}"),
        );
        let output = command(&harness, &["--suite", "suite.botwork", "--list-cases"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("ordinary file"));
    }
}
