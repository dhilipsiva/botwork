#[path = "support/cli_harness.rs"]
mod cli_harness;
use cli_harness::Harness;
use serde_json::{json, Value};
use std::{fs, process::Output, time::Duration};

#[cfg(target_os = "linux")]
#[path = "suite_cli/linux.rs"]
mod linux;

fn command(harness: &Harness, args: &[&str]) -> Output {
    harness
        .command("suite", args, Duration::from_secs(15))
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
fn ordinary_script_mode_keeps_its_output_and_custom_suite_word_names() {
    let harness = Harness::new();
    let output = harness
        .run(
            "ordinary",
            "Suite { Return |42| }\nLog |@{ Suite }|",
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(success(&output, "42\n"), "");
}

#[test]
fn cases_get_fresh_variables_definitions_and_module_caches_in_declaration_order() {
    let harness = Harness::new();
    fs::create_dir(harness.workspace.join("nested")).unwrap();
    fs::write(
        harness.workspace.join("nested/module.botwork"),
        "Log |\"module-init\"|\nValue { Return |42| }",
    )
    .unwrap();
    fs::write(
        harness.workspace.join("nested/suite.botwork"),
        r#"
Suite |"fresh"| Named |"Fresh contexts"| {
  Library {
    Import |"module.botwork"| As |module|
    Read { Return |input| }
  }
  Case |"second"| { Log |@{ Read }|\n|input| = |99| }
  Case |"first"| { Import |"module.botwork"| As |again|\nLog |@{ Read }| }
}
"#
        .replace("\\n", "\n"),
    )
    .unwrap();
    let output = command(
        &harness,
        &[
            "--suite",
            "nested/suite.botwork",
            "--jobs",
            "1",
            "--var",
            "input=7",
        ],
    );
    let stderr = success(&output, "module-init\n7\nmodule-init\n7\n");
    assert!(
        stderr.find("[case fresh/second] succeeded:").unwrap()
            < stderr.find("[case fresh/first] started:").unwrap()
    );
    assert!(stderr.ends_with("[cases] 2 selected: 2 succeeded, 0 failed\n"));
}

#[test]
fn listing_is_side_effect_free_and_filters_inherited_tags_without_reordering() {
    let harness = Harness::new();
    source(
        &harness,
        r#"Suite |"catalog"| Named |"கணிதம்"| Tags |["all"]| {
Library { Import |"missing.botwork"| As |missing| }
Case |"b"| Named |"Line\nName"| Tags |["fast"]| { Log |undefined| }
Case |"a"| Tags |["slow"]| { Log |undefined| }
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--list-cases",
            "--tag",
            "all",
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
    let rows = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [json!({"id":"catalog/b","suite":"கணிதம்","name":"Line\nName","tags":["all","fast"]})]
    );
}

#[test]
fn failed_case_reruns_follow_stable_ids_across_renames_and_reordering() {
    let harness = Harness::new();
    source(&harness, "Suite |\"stable\"| { Case |\"a\"| { Log |1| } Case |\"b\"| Named |\"Old\"| { Log |2|\nLog |missing|\nLog |999| } Case |\"c\"| { While |true| {} } }");
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--jobs",
            "1",
            "--max-steps",
            "100",
            "--failures",
            "failed.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"1\n2\n");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[case stable/b] failed: \"Old\""),
        "{stderr}"
    );
    assert!(
        stderr.contains("[case stable/c] limit exceeded:"),
        "{stderr}"
    );
    assert_eq!(
        state(&harness),
        json!({"format":"botwork-failed-cases","version":1,"complete":true,"failed":["stable/b","stable/c"]})
    );
    source(&harness, "Suite |\"stable\"| Named |\"Renamed suite\"| { Case |\"c\"| { Log |30| } Case |\"b\"| Named |\"New\"| { Log |20| } Case |\"a\"| { Log |10| } }");
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
    success(&output, "30\n20\n");
    assert_eq!(state(&harness)["failed"], json!([]));
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--rerun-failed", "failed.json"],
    );
    assert_eq!(
        success(&output, ""),
        "[cases] 0 selected: 0 succeeded, 0 failed\n"
    );
}

#[test]
fn discovery_and_selection_failures_prevent_all_case_effects_and_invalidate_old_results() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { Case |\"a\"| { Log |1| } }");
    success(
        &command(
            &harness,
            &["--suite", "suite.botwork", "--failures", "failed.json"],
        ),
        "1\n",
    );
    for rest in [
        vec!["--case", "s/removed"],
        vec!["--tag", "unmatched"],
        vec!["--suite", "suite.botwork"],
    ] {
        let mut args = vec!["--suite", "suite.botwork", "--failures", "failed.json"];
        args.extend(rest);
        let output = command(&harness, &args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("] started:"));
        assert_eq!(state(&harness)["complete"], false);
    }
    fs::write(
        harness.workspace.join("invalid.botwork"),
        "Suite |\"broken\"| { Case |\"bad\"| { |x| = |1 +| } }",
    )
    .unwrap();
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--suite", "invalid.botwork"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn malformed_incomplete_or_stale_failure_records_never_select_work() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { Case |\"a\"| { Log |1| } }");
    let valid =
        json!({"format":"botwork-failed-cases","version":1,"complete":true,"failed":["s/a"]});
    let mut invalid = vec![json!({}), json!([])];
    for (key, value) in [
        ("version", json!(2)),
        ("format", json!("other")),
        ("complete", json!(false)),
        ("failed", json!(["s/removed"])),
        ("failed", json!(["s/a", "s/a"])),
        ("failed", json!(["../a"])),
        ("extra", json!(true)),
    ] {
        let mut record = valid.clone();
        record[key] = value;
        invalid.push(record);
    }
    for record in invalid {
        fs::write(harness.workspace.join("failed.json"), record.to_string()).unwrap();
        let output = command(
            &harness,
            &["--suite", "suite.botwork", "--rerun-failed", "failed.json"],
        );
        assert_eq!(output.status.code(), Some(1), "{record}");
        assert!(output.stdout.is_empty(), "{record}");
    }
    fs::write(
        harness.workspace.join("failed.json"),
        vec![b' '; 2 * 1024 * 1024 + 1],
    )
    .unwrap();
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--rerun-failed", "failed.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("BW8001"));
}

#[test]
fn wrong_failed_field_type_explains_the_required_id_list() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { Case |\"a\"| { Log |1| } }");
    fs::write(
        harness.workspace.join("prior.json"),
        json!({"format":"botwork-failed-cases","version":1,"complete":true,"failed":true})
            .to_string(),
    )
    .unwrap();
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--rerun-failed", "prior.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("BW7002")
            && stderr.contains("bounded list")
            && stderr.contains("suite/case IDs"),
        "{stderr}"
    );
}

#[test]
fn failure_output_refuses_unrelated_files_and_bad_inputs_fail_each_admitted_case() {
    let harness = Harness::new();
    source(
        &harness,
        "Suite |\"s\"| { Case |\"a\"| { Log |1| } Case |\"b\"| { Log |2| } }",
    );
    fs::write(harness.workspace.join("failed.json"), "keep this file").unwrap();
    let output = command(
        &harness,
        &["--suite", "suite.botwork", "--failures", "failed.json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(harness.workspace.join("failed.json")).unwrap(),
        "keep this file"
    );
    fs::remove_file(harness.workspace.join("failed.json")).unwrap();
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--var",
            "bad",
            "--failures",
            "failed.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(state(&harness)["failed"], json!(["s/a", "s/b"]));
}

#[test]
fn output_records_are_validated_before_overwrite_including_inconsistent_incomplete_records() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { Case |\"a\"| { Log |1| } }");
    for record in [
        json!({"format":"botwork-failed-cases","version":1,"complete":false,"failed":["s/a"]}),
        json!({"format":"botwork-failed-cases","version":2,"complete":true,"failed":[]}),
        json!({"format":"unrelated","version":1,"complete":true,"failed":[]}),
        json!({"format":"botwork-failed-cases","version":1,"complete":true,"failed":["s/a","s/a"]}),
        json!({"format":"botwork-failed-cases","version":1,"complete":true,"failed":["bad"]}),
        json!({"format":"botwork-failed-cases","version":1,"complete":true,"failed":[],"extra":1}),
    ] {
        let text = record.to_string();
        fs::write(harness.workspace.join("failed.json"), &text).unwrap();
        let output = command(
            &harness,
            &["--suite", "suite.botwork", "--failures", "failed.json"],
        );
        assert_eq!(output.status.code(), Some(1), "{record}");
        assert!(output.stdout.is_empty(), "{record}");
        assert_eq!(
            fs::read_to_string(harness.workspace.join("failed.json")).unwrap(),
            text
        );
    }
}

#[test]
fn selected_cases_have_independent_output_budgets_and_listing_obeys_its_own_limit() {
    let harness = Harness::new();
    source(
        &harness,
        "Suite |\"s\"| { Case |\"a\"| { Log |123| } Case |\"b\"| { Log |123| } }",
    );
    success(
        &command(
            &harness,
            &["--suite", "suite.botwork", "--max-output-bytes", "4"],
        ),
        "123\n123\n",
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--list-cases",
            "--max-output-bytes",
            "0",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("BW8001"));
}

#[test]
fn selector_and_mode_conflicts_are_rejected_before_loading_files() {
    let harness = Harness::new();
    for args in [
        vec!["--case", "s/a"],
        vec!["--suite", "missing", "--file", "missing"],
        vec!["--suite", "missing", "--list-statements"],
        vec![
            "--suite",
            "missing",
            "--list-cases",
            "--failures",
            "failed.json",
        ],
        vec!["--suite", "missing", "--list-cases", "--var", "x=1"],
        vec!["--suite", "missing", "--list-cases", "--jobs", "2"],
    ] {
        let output = command(&harness, &args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
    assert!(!harness.workspace.join("failed.json").exists());
}

#[test]
fn case_errors_keep_original_suite_coordinates_and_custom_call_frames() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| {\nLibrary {\nExplode {\nLog |missing|\n}\n}\nCase |\"broken\"| { Explode }\n}");
    let output = command(&harness, &["--suite", "suite.botwork"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    for expected in [
        "[case s/broken] failed:",
        "suite.botwork:4:",
        "explode",
        "BW2001",
    ] {
        assert!(stderr.contains(expected), "missing {expected}: {stderr}");
    }
}

#[test]
fn multiple_suite_order_is_preserved_when_selector_order_and_duplicates_differ() {
    let harness = Harness::new();
    source(&harness, "Suite |\"first\"| { Case |\"b\"| Tags |[\"fast\"]| { Log |1| } Case |\"a\"| { Log |999| } }");
    fs::write(
        harness.workspace.join("second.botwork"),
        "Suite |\"second\"| Tags |[\"fast\"]| { Case |\"c\"| { Log |2| } }",
    )
    .unwrap();
    success(
        &command(
            &harness,
            &[
                "--suite",
                "suite.botwork",
                "--suite",
                "second.botwork",
                "--jobs",
                "1",
                "--case",
                "second/c",
                "--case",
                "first/b",
                "--case",
                "first/b",
                "--tag",
                "fast",
            ],
        ),
        "1\n2\n",
    );
}

#[test]
fn discovery_rejects_oversized_sources_before_utf8_and_never_lists_partial_results() {
    let harness = Harness::new();
    source(&harness, "Suite |\"s\"| { Case |\"a\"| {} }");
    fs::write(
        harness.workspace.join("large.botwork"),
        vec![0xff; 1024 * 1024 + 1],
    )
    .unwrap();
    let output = command(
        &harness,
        &[
            "--suite",
            "suite.botwork",
            "--suite",
            "large.botwork",
            "--list-cases",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("BW8001"));
    let args = std::iter::repeat_n(["--suite", "suite.botwork"], 65)
        .flatten()
        .chain(["--list-cases"])
        .collect::<Vec<_>>();
    let output = command(&harness, &args);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("suites (limit 64)"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rerun_codec_admits_exact_byte_and_id_limits_and_rejects_one_more() {
    let harness = Harness::new();
    let mut args = Vec::new();
    let paths = (0..4).map(|i| format!("s{i}.botwork")).collect::<Vec<_>>();
    let cases = (0..1024)
        .map(|i| format!("Case |\"c{i}\"| {{}}\n"))
        .collect::<String>();
    let mut ids = vec![];
    for (i, path) in paths.iter().enumerate() {
        fs::write(
            harness.workspace.join(path),
            format!("Suite |\"s{i}\"| {{ {cases} }}"),
        )
        .unwrap();
        args.extend(["--suite", path.as_str()]);
        ids.extend((0..1024).map(|c| format!("s{i}/c{c}")));
    }
    args.extend(["--list-cases", "--rerun-failed", "prior.json"]);
    let record = |ids: &[String]| {
        json!({"format":"botwork-failed-cases","version":1,"complete":true,"failed":ids})
            .to_string()
    };
    fs::write(harness.workspace.join("prior.json"), record(&ids)).unwrap();
    let output = command(&harness, &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().lines().count(),
        4096
    );
    ids.push("extra/c".into());
    fs::write(harness.workspace.join("prior.json"), record(&ids)).unwrap();
    let output = command(&harness, &args);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("excessive failed case IDs"));
    let mut padded = record(&[]);
    padded.extend(std::iter::repeat_n(' ', 2 * 1024 * 1024 - padded.len()));
    fs::write(harness.workspace.join("prior.json"), &padded).unwrap();
    success(&command(&harness, &args), "");
    padded.push(' ');
    fs::write(harness.workspace.join("prior.json"), &padded).unwrap();
    let output = command(&harness, &args);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("BW8001"));
}

#[test]
fn discovery_checks_aggregate_limits_before_reading_or_parsing_later_files() {
    let harness = Harness::new();
    let paths = (0..64).map(|i| format!("s{i}.botwork")).collect::<Vec<_>>();
    let mut args = vec![];
    for (i, path) in paths.iter().enumerate() {
        fs::write(
            harness.workspace.join(path),
            format!("Suite |\"s{i}\"| {{ Case |\"c\"| {{}} }}"),
        )
        .unwrap();
        args.extend(["--suite", path.as_str()]);
    }
    args.push("--list-cases");
    let output = command(&harness, &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().lines().count(),
        64
    );
    for count in [65, 66] {
        let args = std::iter::repeat_n(["--suite", "missing.botwork"], count)
            .flatten()
            .chain(["--list-cases"])
            .collect::<Vec<_>>();
        let output = command(&harness, &args);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("suites (limit 64)"));
    }
    let mut args = vec![];
    for (i, path) in paths[..8].iter().enumerate() {
        let mut source = format!("Suite |\"s{i}\"| {{ Case |\"c\"| {{}} }}\n#");
        source.extend(std::iter::repeat_n('x', 1024 * 1024 - source.len()));
        fs::write(harness.workspace.join(path), source).unwrap();
        args.extend(["--suite", path.as_str()]);
    }
    args.push("--list-cases");
    let output = command(&harness, &args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 8);
    fs::write(harness.workspace.join("over.botwork"), [0xff]).unwrap();
    args.extend(["--suite", "over.botwork"]);
    let output = command(&harness, &args);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("suite discovery source bytes (limit 8388608)"),
        "{stderr}"
    );

    let cases = (0..1024)
        .map(|i| format!("Case |\"c{i}\"| {{}}\n"))
        .collect::<String>();
    let mut args = vec![];
    for (i, path) in paths[..4].iter().enumerate() {
        fs::write(
            harness.workspace.join(path),
            format!("Suite |\"s{i}\"| {{ {cases} }}"),
        )
        .unwrap();
        args.extend(["--suite", path.as_str()]);
    }
    fs::write(
        harness.workspace.join("over.botwork"),
        "Suite |\"over\"| { Case |\"c\"| {} }",
    )
    .unwrap();
    args.extend([
        "--suite",
        "over.botwork",
        "--suite",
        "missing.botwork",
        "--list-cases",
    ]);
    let output = command(&harness, &args);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("discovered cases (limit 4096)"), "{stderr}");
}
