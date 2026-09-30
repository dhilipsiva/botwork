#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;

use cli_harness::{http_fixture, Harness};
use serde_json::{json, Value};
use std::{path::Path, process::Output, time::Duration};

fn run(harness: &Harness, name: &str, inputs: &str, overrides: &[(&str, Value)]) -> Output {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut args = vec![
        "--file".to_owned(),
        root.join("examples")
            .join(name)
            .to_str()
            .unwrap()
            .to_owned(),
        "--vars-file".to_owned(),
        root.join("examples/inputs")
            .join(inputs)
            .to_str()
            .unwrap()
            .to_owned(),
    ];
    for (key, value) in overrides {
        args.extend(["--var".to_owned(), format!("{key}={value}")]);
    }
    harness
        .command(
            "automation",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            Duration::from_secs(30),
        )
        .unwrap()
}

fn assert_success(output: &Output, stdout: &str) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, stdout.as_bytes());
}

fn assert_failure(output: &Output, code: &str, source: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "no success report after failure");
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains(code), "{diagnostic}");
    // Sources are shown with the platform's separators.
    let source = source.replace('/', std::path::MAIN_SEPARATOR_STR);
    assert!(diagnostic.contains(&source), "{diagnostic}");
}

#[cfg(target_os = "linux")]
mod catalogue {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn workspace() -> Harness {
        let harness = Harness::new();
        fs::create_dir_all(harness.workspace.join("examples/inputs")).unwrap();
        fs::write(
            harness.workspace.join("examples/inputs/catalogue.txt"),
            include_bytes!("../examples/inputs/catalogue.txt"),
        )
        .unwrap();
        harness
    }

    fn execute(harness: &Harness, overrides: &[(&str, Value)]) -> Output {
        run(
            harness,
            "35-build-catalogue.botwork",
            "catalogue.json",
            overrides,
        )
    }

    fn assert_clean(harness: &Harness) {
        for entry in fs::read_dir(&harness.workspace).unwrap() {
            assert!(
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("botwork_catalogue_"),
                "Finally must remove the staging directory"
            );
        }
    }

    #[test]
    fn builds_verified_artifact_using_bundled_configuration_and_relative_paths() {
        let harness = workspace();
        let output = execute(&harness, &[]);
        assert_success(&output, "Published 3 entries\n");
        assert_eq!(
            fs::read(harness.workspace.join("catalogue.txt")).unwrap(),
            b"coffee\ntea\nwater\n"
        );
        assert_eq!(
            fs::read(harness.workspace.join("examples/inputs/catalogue.txt")).unwrap(),
            include_bytes!("../examples/inputs/catalogue.txt")
        );
        assert_clean(&harness);
    }

    #[test]
    fn accepts_unicode_crlf_and_literal_paths_through_variable_overrides() {
        let harness = workspace();
        let source = "source with spaces ; $(literal).txt";
        let destination = "result with spaces ; $(literal).txt";
        fs::write(harness.workspace.join(source), "  தேநீர்  \r\né\r\n\r\né\r\n").unwrap();
        let output = execute(
            &harness,
            &[
                ("source_file", json!(source)),
                ("destination_file", json!(destination)),
                ("expected_entries", json!(["é", "தேநீர்"])),
            ],
        );
        assert_success(&output, "Published 2 entries\n");
        assert_eq!(
            fs::read_to_string(harness.workspace.join(destination)).unwrap(),
            "é\nதேநீர்\n"
        );
        assert_clean(&harness);
    }

    #[test]
    fn mismatch_does_not_publish_and_still_cleans_staging() {
        let harness = workspace();
        let output = execute(&harness, &[("expected_entries", json!(["unexpected"]))]);
        assert_failure(&output, "BW9001", "35-build-catalogue.botwork");
        assert!(!harness.workspace.join("catalogue.txt").exists());
        assert_clean(&harness);
    }

    #[test]
    fn existing_destination_is_preserved_and_staging_is_removed() {
        let harness = workspace();
        fs::write(harness.workspace.join("catalogue.txt"), "keep me\n").unwrap();
        let output = execute(&harness, &[]);
        assert_failure(&output, "BW4002", "35-build-catalogue.botwork");
        assert_eq!(
            fs::read(harness.workspace.join("catalogue.txt")).unwrap(),
            b"keep me\n"
        );
        assert_clean(&harness);
    }

    #[test]
    fn empty_input_fails_in_imported_helper_and_still_cleans_staging() {
        let harness = workspace();
        fs::write(
            harness.workspace.join("examples/inputs/catalogue.txt"),
            " \n\t\n",
        )
        .unwrap();
        let output = execute(&harness, &[]);
        assert_failure(&output, "BW9001", "modules/catalogue.botwork");
        assert!(!harness.workspace.join("catalogue.txt").exists());
        assert_clean(&harness);
    }

    #[test]
    fn nonzero_process_exit_fails_before_publication_and_cleans_staging() {
        let harness = workspace();
        let process = harness.workspace.join("failing-sort");
        // Drain stdin so the fixture tests an exit result, not a broken input pipe.
        fs::write(&process, "#!/bin/sh\n/bin/cat >/dev/null\nexit 7\n").unwrap();
        fs::set_permissions(&process, fs::Permissions::from_mode(0o700)).unwrap();
        let output = execute(&harness, &[("sort_program", json!("./failing-sort"))]);
        assert_failure(&output, "BW9001", "modules/catalogue.botwork");
        assert!(!harness.workspace.join("catalogue.txt").exists());
        assert_clean(&harness);
    }

    #[test]
    fn process_launch_failure_also_cleans_staging() {
        let harness = workspace();
        let output = execute(&harness, &[("sort_program", json!("./absent-sort"))]);
        assert_failure(&output, "BW5003", "modules/catalogue.botwork");
        assert!(!harness.workspace.join("catalogue.txt").exists());
        assert_clean(&harness);
    }
}

#[test]
fn http_contract_uses_imported_helper_and_bundled_json_inputs() {
    let harness = Harness::new();
    let fixture = http_fixture::Fixture::new();
    let output = run(
        &harness,
        "36-http-contract.botwork",
        "http-contract.json",
        &[("url", json!(fixture.url))],
    );
    assert_success(&output, "HTTP contract passed (200)\n");
}

#[test]
fn http_contract_rejects_status_header_and_body_mismatches() {
    let harness = Harness::new();
    let fixture = http_fixture::Fixture::new();
    for (key, value) in [
        ("status", json!(201)),
        ("headers", json!({"content-type": ["text/plain"]})),
        ("headers", json!({"x-required": ["present"]})),
        ("body", json!("{\"message\":\"different\"}")),
    ] {
        let mut inputs: Value =
            serde_json::from_str(include_str!("../examples/inputs/http-contract.json")).unwrap();
        inputs["contract"][key] = value;
        let output = run(
            &harness,
            "36-http-contract.botwork",
            "http-contract.json",
            &[
                ("url", json!(fixture.url)),
                ("contract", inputs["contract"].clone()),
            ],
        );
        assert_failure(&output, "BW9001", "modules/http-contract.botwork");
    }
}

#[test]
fn http_contract_enforces_the_configured_deadline() {
    let harness = Harness::new();
    let fixture = http_fixture::Fixture::new();
    let mut inputs: Value =
        serde_json::from_str(include_str!("../examples/inputs/http-contract.json")).unwrap();
    inputs["contract"]["timeout_ms"] = json!(0);
    let output = run(
        &harness,
        "36-http-contract.botwork",
        "http-contract.json",
        &[
            ("url", json!(fixture.url)),
            ("contract", inputs["contract"].clone()),
        ],
    );
    assert_failure(&output, "BW5002", "modules/http-contract.botwork");
}
