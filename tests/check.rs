//! `--check` parses, validates, and lints files and suites without running them.
use std::{fs, path::Path, process::Command};

fn check(directory: &Path, arguments: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--check")
        .args(arguments)
        .current_dir(directory)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn clean_files_pass_and_warnings_do_not_fail_the_check() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("clean.botwork"), "Log |\"hello\"|\n").unwrap();
    fs::write(directory.path().join("input.botwork"), "Log |name|\n").unwrap();
    assert_eq!(
        check(directory.path(), &["--file", "clean.botwork"]),
        (
            Some(0),
            String::new(),
            "[check] 1 file: 0 errors, 0 warnings\n".into()
        )
    );
    let (status, stdout, stderr) = check(
        directory.path(),
        &["--file", "clean.botwork", "--file", "input.botwork"],
    );
    assert_eq!((status, stdout.as_str()), (Some(0), ""));
    assert_eq!(
        stderr,
        "input.botwork:1:6-1:10: warning[undefined-variable]: [BW2001] `name` is never assigned in a scope that reaches this read\n  help: Assign it first, or supply it as an input variable with --var or --vars-file.\n[check] 2 files: 0 errors, 1 warning\n"
    );
}

#[test]
fn errors_fail_the_check_and_are_listed_in_source_order() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("broken.botwork"),
        "Log |1| |2|\nSleep |\"10\"|\nIf |1| { Log |1| }\n",
    )
    .unwrap();
    let (status, stdout, stderr) = check(directory.path(), &["--file", "broken.botwork"]);
    assert_eq!((status, stdout.as_str()), (Some(1), ""));
    let headlines: Vec<_> = stderr
        .lines()
        .filter(|line| !line.starts_with("  help:"))
        .collect();
    assert_eq!(
        headlines,
        [
            "broken.botwork:1:1-1:12: error[undefined-statement]: [BW2002] Statement not defined: Log |1| |2|",
            "broken.botwork:2:8-2:12: error[argument-kind]: [BW3003] Parameter `milliseconds` (argument 1) of `sleep|param|` requires Int; got String",
            "broken.botwork:3:5-3:6: error[condition-kind]: [BW3003] If requires a boolean condition",
            "[check] 1 file: 3 errors, 0 warnings",
        ]
    );
}

#[test]
fn syntax_placement_and_read_failures_count_as_errors() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("syntax.botwork"), "Log |1\n").unwrap();
    fs::write(directory.path().join("placement.botwork"), "Break\n").unwrap();
    let (status, _, stderr) = check(
        directory.path(),
        &[
            "--file",
            "syntax.botwork",
            "--file",
            "placement.botwork",
            "--file",
            "missing.botwork",
        ],
    );
    assert_eq!(status, Some(1));
    assert!(stderr.contains("[BW1001]"), "{stderr}");
    assert!(stderr.contains("[BW1002]"), "{stderr}");
    assert!(stderr.contains("missing.botwork: "), "{stderr}");
    assert!(
        stderr.ends_with("[check] 3 files: 3 errors, 0 warnings\n"),
        "{stderr}"
    );
}

#[test]
fn checking_runs_no_statement_process_or_request() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("effects.botwork"),
        r#"Write File |"created.txt"| Text |"ran"|
Log |"printed"|
Run Process |"/bin/sh"| With Arguments |["-c", "touch process.txt"]|
HTTP Request |"GET"| To |"http://127.0.0.1:9/"|
Import |"module.botwork"| As |module|
"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("module.botwork"),
        "Write File |\"module.txt\"| Text |\"ran\"|\n",
    )
    .unwrap();
    assert_eq!(
        check(directory.path(), &["--file", "effects.botwork"]),
        (
            Some(0),
            String::new(),
            "[check] 1 file: 0 errors, 0 warnings\n".into()
        )
    );
    for effect in ["created.txt", "process.txt", "module.txt"] {
        assert!(
            !directory.path().join(effect).exists(),
            "{effect} was created"
        );
    }
}

#[test]
fn suites_are_checked_with_their_setup_variables_and_rows() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("checkout.suite.botwork"),
        r#"Suite |"checkout"| {
    Dataset |"orders"| {
        Row |"one"| Values |{total: 5}|
    }
    SuiteSetup { |currency| = |"EUR"| }
    Case |"total"| Using |"orders"| As |order| {
        Log |currency|
        Assert |order.total| Equals |5|
    }
    Case |"typo"| {
        Log |currency|
        Asert |true|
    }
}
"#,
    )
    .unwrap();
    let (status, stdout, stderr) = check(directory.path(), &["--suite", "checkout.suite.botwork"]);
    assert_eq!((status, stdout.as_str()), (Some(1), ""));
    assert!(
        stderr.starts_with("checkout.suite.botwork:12:9-12:21: error[undefined-statement]: [BW2002] Statement not defined: Asert |true|\n"),
        "{stderr}"
    );
    assert!(
        stderr.ends_with("[check] 1 file: 1 error, 0 warnings\n"),
        "{stderr}"
    );
}

#[test]
fn check_cannot_be_combined_with_running_modes() {
    let directory = tempfile::tempdir().unwrap();
    for arguments in [
        &["--list-statements"][..],
        &["--file", "x.botwork", "--report-json", "report.json"],
    ] {
        let (status, _, stderr) = check(directory.path(), arguments);
        assert_eq!(status, Some(2), "{arguments:?}: {stderr}");
    }
}

#[test]
fn findings_predict_the_runtime_error_code_and_range() {
    let directory = tempfile::tempdir().unwrap();
    let scripts = [
        "Log |1| |2|\n",
        "Log |@{ Later }|\nLater { Return |1| }\n",
        "Log |1|\nDup { Return |1| }\nDup { Return |2| }\n",
        "Log |x| { Return |x| }\n",
        "Sleep |\"10\"|\n",
        "If |1| { Log |1| }\n",
        "For |x| In |\"abc\"| { Log |x| }\n",
        "Log |missing|\n",
    ];
    for (index, script) in scripts.iter().enumerate() {
        let name = format!("case{index}.botwork");
        fs::write(directory.path().join(&name), script).unwrap();
        let (_, _, checked) = check(directory.path(), &["--file", &name]);
        let run = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(["--file", &name])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(1), "{script}");
        let runtime = String::from_utf8(run.stderr).unwrap();
        // "file:range: [BWnnnn] ..." at runtime; "file:range: severity[rule]: [BWnnnn] ..." when checked.
        let (runtime_range, runtime_rest) = runtime.split_once(": ").unwrap();
        let (checked_range, checked_rest) = checked.split_once(": ").unwrap();
        let code = |text: &str| text[text.find("[BW").unwrap()..][..8].to_owned();
        assert_eq!(
            (checked_range, code(checked_rest)),
            (runtime_range, code(runtime_rest)),
            "{script}\nchecked: {checked}\nruntime: {runtime}"
        );
    }
}

#[test]
fn the_documented_example_prints_the_documented_findings() {
    let guide =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/check.md")).unwrap();
    let block = |fence: &str| {
        let start = guide.find(fence).unwrap() + fence.len();
        guide[start..start + guide[start..].find("```").unwrap()].to_owned()
    };
    let script = block("<!-- botwork-test: check-example -->\n```botwork\n");
    let shown = block("running the script:\n\n```text\n");
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("check-example.botwork"), script).unwrap();
    let (status, stdout, stderr) = check(directory.path(), &["--file", "check-example.botwork"]);
    assert_eq!(
        (status, stdout.as_str(), stderr.as_str()),
        (Some(1), "", shown.as_str())
    );
}
