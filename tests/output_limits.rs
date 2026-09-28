#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    eval::Context,
    grammar::Literal,
    run::{OutputLimits, RunLimits},
};
use cli_harness::Harness;
use std::{fs, process::Command, time::Duration};

#[test]
fn log_checks_newlines_and_cumulative_bytes_before_the_next_record() {
    let harness = Harness::new();
    let baseline = harness
        .run("ordinary-log", "Log |\"é\"|", Duration::from_secs(10))
        .unwrap();
    assert!(baseline.status.success());
    assert_eq!(baseline.stdout, "é\n".as_bytes());
    for (flag, limit, expected, resource) in [
        ("--max-output-record-bytes", "3", "é\né\n", None),
        (
            "--max-output-record-bytes",
            "2",
            "",
            Some("output record bytes"),
        ),
        ("--max-output-bytes", "6", "é\né\n", None),
        ("--max-output-bytes", "5", "é\n", Some("output total bytes")),
    ] {
        let result = harness
            .run_with_args(
                "log-limit",
                "Log |\"é\"|\nLog |\"é\"|",
                &[flag, limit],
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(result.stdout, expected.as_bytes());
        assert_eq!(result.status.success(), resource.is_none());
        if let Some(resource) = resource {
            let error = String::from_utf8(result.stderr).unwrap();
            assert!(error.contains("BW8001"), "{error}");
            assert!(error.contains(resource), "{error}");
            assert!(error.contains("Log |\"é\"|"), "{error}");
        }
    }
}

#[test]
fn arguments_finish_before_output_admission_and_limits_bypass_catch() {
    let harness = Harness::new();
    let source = "Produce {\nLog |\"arg\"|\nReturn |\"big\"|\n}\nTry { Log |@{Produce}| } Catch { Log |\"caught\"| }\nLog |\"after\"|";
    let result = harness
        .run_with_args(
            "argument-order",
            source,
            &["--max-output-bytes", "4"],
            Duration::from_secs(10),
        )
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(result.stdout, b"arg\n");
    assert!(String::from_utf8(result.stderr)
        .unwrap()
        .contains("output total bytes"));
}

#[test]
fn imported_modules_share_output_usage_and_preserve_the_originating_site() {
    let harness = Harness::new();
    fs::write(harness.workspace.join("module.botwork"), "Log |\"m\"|").unwrap();
    let source = "Log |\"r\"|\nImport |\"module.botwork\"| As |module|\nLog |\"after\"|";
    for total in [3, 4] {
        let result = harness
            .run_with_args(
                "module-output",
                source,
                &["--max-output-bytes", &total.to_string()],
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert_eq!(
            result.stdout,
            if total == 3 {
                &b"r\n"[..]
            } else {
                &b"r\nm\n"[..]
            }
        );
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(error.contains("BW8001"), "{error}");
        if total == 3 {
            assert!(error.contains("module.botwork:1:1"), "{error}");
        }
    }
}

#[test]
fn debug_traces_share_the_log_budget_and_failure_reporting_has_separate_capacity() {
    let harness = Harness::new();
    let trace = format!(
        "debug: {}:1:1: call\n",
        harness.workspace.join("trace-limit.botwork").display()
    );
    let result = harness
        .run_with_args(
            "trace-limit",
            "Log |\"ok\"|",
            &["--debug", "--max-output-bytes", &trace.len().to_string()],
            Duration::from_secs(10),
        )
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.starts_with(&trace), "{stderr}");
    assert!(stderr.contains("output total bytes"));
    let result = harness
        .run_with_args(
            "zero-trace",
            "Log |\"ok\"|",
            &["--debug", "--max-output-bytes", "0"],
            Duration::from_secs(10),
        )
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(!stderr.starts_with("debug:"));
    assert!(stderr.contains("BW8001"));
}

#[test]
fn statement_listing_and_streamed_help_obey_exact_output_limits() {
    for (args, total) in [
        (vec!["--list-statements"], true),
        (vec!["--list-statements"], false),
        (vec!["--statement-help", "Log |x|"], false),
    ] {
        let baseline = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(&args)
            .output()
            .unwrap();
        assert!(baseline.status.success());
        let maximum = if !total && args[0] == "--list-statements" {
            baseline
                .stdout
                .split_inclusive(|byte| *byte == b'\n')
                .map(<[u8]>::len)
                .max()
                .unwrap()
        } else {
            baseline.stdout.len()
        };
        for deficit in [0, 1] {
            let result = Command::new(env!("CARGO_BIN_EXE_botwork"))
                .args(&args)
                .args([
                    if total {
                        "--max-output-bytes"
                    } else {
                        "--max-output-record-bytes"
                    },
                    &(maximum - deficit).to_string(),
                ])
                .output()
                .unwrap();
            if deficit == 0 {
                assert!(result.status.success());
                assert_eq!(result.stdout, baseline.stdout);
            } else {
                assert_eq!(result.status.code(), Some(1));
                assert!(baseline.stdout.starts_with(&result.stdout));
                if args[0] != "--list-statements" {
                    assert!(result.stdout.is_empty());
                }
                assert!(String::from_utf8(result.stderr)
                    .unwrap()
                    .contains(if total {
                        "output total bytes"
                    } else {
                        "output record bytes"
                    }));
            }
        }
    }
}

#[test]
fn public_value_serialization_into_memory_is_bounded_and_fresh_contexts_start_empty() {
    let value = Literal::Array(vec![Literal::String("é\n".into())]);
    let expected = "[\"é\\n\"]";
    for _ in 0..2 {
        let context = Context::with_limits(RunLimits {
            output: OutputLimits {
                record_bytes: expected.len(),
                total_bytes: expected.len(),
            },
            ..Default::default()
        })
        .unwrap();
        let mut buffer = Vec::new();
        assert_eq!(
            context.write_value(&value, &mut buffer).unwrap(),
            expected.len()
        );
        assert_eq!(buffer, expected.as_bytes());
        assert!(context.write_value(&value, &mut buffer).is_err());
        assert_eq!(buffer, expected.as_bytes());
    }
}
