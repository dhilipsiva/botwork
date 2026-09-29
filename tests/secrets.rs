#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};

const TOKEN: &str = "tok-9f8e7d\"q";
const USER: &str = "alice-7";
// Nine digits: longer than any timing digit run in the outputs.
const PIN: &str = "739173917";
const KEY: &str = "key-12345";

fn write(harness: &Harness, name: &str, source: &str) {
    fs::write(harness.workspace.join(name), source).unwrap();
}

/// Run the CLI with the environment variable that `--secret-env` reads.
fn command(harness: &Harness, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(arguments)
        .current_dir(&harness.workspace)
        .env("BOTWORK_TEST_API_KEY", KEY)
        .env_remove("BOTWORK_TEST_UNSET_VARIABLE")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// Every spelling of every secret that an output could contain.
fn spellings() -> Vec<String> {
    let mut all = Vec::new();
    for secret in [TOKEN, USER, PIN, KEY] {
        all.push(secret.to_owned());
        let json = serde_json::to_string(secret).unwrap();
        all.push(json[1..json.len() - 1].to_owned());
        all.push(format!("{secret:?}").trim_matches('"').to_owned());
    }
    all
}

/// No file under `root`, except the inputs, holds any secret spelling.
fn assert_masked(root: &Path, inputs: &[&str], stdout: &[u8], stderr: &[u8]) {
    let mut outputs = vec![
        ("stdout".to_owned(), stdout.to_vec()),
        ("stderr".to_owned(), stderr.to_vec()),
    ];
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if !inputs.iter().any(|input| path.ends_with(input)) {
                outputs.push((path.display().to_string(), fs::read(&path).unwrap()));
            }
        }
    }
    for (name, bytes) in outputs {
        let text = String::from_utf8_lossy(&bytes);
        for secret in spellings() {
            assert!(
                !text.contains(&secret),
                "{name} contains {secret:?}:\n{text}"
            );
        }
    }
}

fn vars(harness: &Harness) {
    let vars = serde_json::json!({
        "creds": {"user": USER, "token": TOKEN, "pins": [PIN.parse::<i32>().unwrap()]},
        "public": "hello",
    });
    write(harness, "vars.json", &vars.to_string());
}

#[test]
fn secret_inputs_are_masked_in_every_output() {
    let harness = Harness::new();
    vars(&harness);
    write(
        &harness,
        "leak.botwork",
        r#"Log |creds.token|
Log |creds|
Log |public + " " + api|
Log |@{ Format JSON |creds| }|
Assert |creds.token| Equals |"other"|
"#,
    );
    // A secret written into the source itself: its HTML excerpt is masked.
    write(
        &harness,
        "literal.botwork",
        "|copy| = |\"alice-7\"|\nFail |copy|",
    );
    write(&harness, "error.botwork", "Read File |\"/missing/\" + api|");
    let output = command(
        &harness,
        &[
            "--file",
            "leak.botwork",
            "--file",
            "error.botwork",
            "--file",
            "literal.botwork",
            "--jobs",
            "1",
            "--vars-file",
            "vars.json",
            "--secret",
            "creds",
            "--secret-env",
            "api=BOTWORK_TEST_API_KEY",
            "--debug",
            "--report-json",
            "report.json",
            "--report-html",
            "report.html",
            "--assertion-artifacts",
            "evidence",
            "--listener",
            "sh",
            "--listener-arg",
            "-c",
            "--listener-arg",
            "cat > events.jsonl",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "***",
            r#"{"pins": [***], "token": "***", "user": "***"}"#,
            "hello ***",
            r#"{"pins":[***],"token":"***","user":"***"}"#,
        ]
    );
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(stderr.contains(r#"got "***" (String)"#), "{stderr}");
    assert!(
        stderr.contains("got '*' (U+002A)"),
        "no character of a secret: {stderr}"
    );
    assert!(stderr.contains("/missing/***"), "{stderr}");
    assert!(stderr.contains("debug: leak.botwork:1:1: call"), "{stderr}");
    let report: Value =
        serde_json::from_slice(&fs::read(harness.workspace.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["runs"][0]["logs"][0]["text"], "***");
    assert!(report["runs"][1]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("/missing/***"));
    let artifact = report["runs"][0]["artifacts"][0]["path"].as_str().unwrap();
    let evidence: Value =
        serde_json::from_slice(&fs::read(harness.workspace.join(artifact)).unwrap()).unwrap();
    assert_eq!(evidence["secrets_masked"], true);
    assert_eq!(evidence["operands_complete"], false);
    assert!(evidence["actual_typed_json"].is_null());
    assert_eq!(
        evidence["actual_excerpt"],
        r#"{"kind":"String","value":"***"}"#
    );
    let page = fs::read_to_string(harness.workspace.join("report.html")).unwrap();
    assert!(page.contains(r#"<code class="source">|copy| = |&quot;***&quot;|</code>"#));
    assert_masked(
        &harness.workspace,
        &["vars.json", "literal.botwork"],
        &output.stdout,
        &output.stderr,
    );
}

#[test]
fn single_file_failures_mask_the_final_error() {
    let harness = Harness::new();
    write(&harness, "fail.botwork", "Fail |\"denied for \" + api|");
    let output = command(
        &harness,
        &[
            "--file",
            "fail.botwork",
            "--secret-env",
            "api=BOTWORK_TEST_API_KEY",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(stderr.contains("denied for ***"), "{stderr}");
    assert_masked(&harness.workspace, &[], &output.stdout, &output.stderr);
}

#[test]
fn suite_fixtures_and_cases_mask_their_inputs() {
    let harness = Harness::new();
    vars(&harness);
    write(
        &harness,
        "secret.suite.botwork",
        r#"Suite |"secret"| {
    SuiteSetup {
        Log |creds.user|
        Fail |"setup saw " + creds.token|
    }
    SuiteTeardown { Log |"closing " + creds.user| }
    Case |"blocked"| { Log |creds.token| }
}
"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "secret.suite.botwork",
            "--vars-file",
            "vars.json",
            "--secret",
            "creds",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(stdout, "***\nclosing ***\n");
    let report: Value =
        serde_json::from_slice(&fs::read(harness.workspace.join("report.json")).unwrap()).unwrap();
    assert!(report["fixtures"][0]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("setup saw ***"));
    assert_masked(
        &harness.workspace,
        &["vars.json"],
        &output.stdout,
        &output.stderr,
    );
}

#[test]
fn secret_marking_errors_stop_before_any_run() {
    let harness = Harness::new();
    vars(&harness);
    write(&harness, "effect.botwork", "Log |\"ran\"|");
    for (arguments, message) in [
        (
            &["--secret", "absent"][..],
            "--secret absent: no input variable has that name",
        ),
        (
            &["--secret-env", "key=BOTWORK_TEST_UNSET_VARIABLE"],
            "environment variable \"BOTWORK_TEST_UNSET_VARIABLE\" is not set",
        ),
        (
            &["--secret-env", "missing-equals"],
            "expected NAME=VARIABLE",
        ),
        (&["--secret-env", "1bad=BOTWORK_TEST_API_KEY"], "BW7001"),
    ] {
        let mut all = vec!["--file", "effect.botwork", "--vars-file", "vars.json"];
        all.extend(arguments);
        let output = command(&harness, &all);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert!(output.stdout.is_empty(), "no run starts: {arguments:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(message), "{stderr}");
    }
    let output = command(
        &harness,
        &[
            "--suite",
            "x.suite.botwork",
            "--list-cases",
            "--secret",
            "creds",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "listing takes no inputs");
}

#[test]
fn embedded_runs_mask_records_and_reasons_but_keep_full_operands_for_hosts() {
    use botwork::core::{
        grammar::{BWErr, Literal},
        report::RecordOptions,
        run::{Engine, RunOptions},
        secret::Secrets,
    };
    let secrets = Secrets::default();
    secrets.add(&Literal::String("hunter2-token".into()));
    let options = RunOptions {
        variables: [("token".to_owned(), Literal::String("hunter2-token".into()))].into(),
        record: Some(RecordOptions::default()),
        secrets,
        ..RunOptions::default()
    };
    let result = Engine::default().run_source(
        "embedded.botwork",
        "Log |\"t=\" + token|\nAssert |token| Equals |\"x\"|",
        options,
    );
    let record = result.record.unwrap();
    assert_eq!(record.logs[0].text, "t=***");
    let message = &record.error.as_ref().unwrap().message;
    assert!(
        message.contains(r#"got "***" (String)"#) && !message.contains("hunter2"),
        "{message}"
    );
    let error = result.result.unwrap_err();
    let BWErr::AssertionMismatch { reason, actual, .. } = &*error.error else {
        panic!("assertion mismatch");
    };
    assert!(!reason.contains("hunter2"), "{reason}");
    assert!(
        actual.contains("hunter2-token"),
        "hosts keep the exact operand: {actual}"
    );
}
