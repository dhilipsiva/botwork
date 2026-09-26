#[path = "support/cli_harness.rs"]
mod cli_harness;
use botwork::core::{
    diagnostic::DiagnosticCode,
    input::{
        load_variables_with_limits, parse_variable_with_limits, parse_variables_with_limits,
        InputLimits,
    },
    value_limits::ValueLimits,
};
use cli_harness::Harness;
use std::{fs, time::Duration};

#[test]
fn empty_input_object_and_exact_flag_budgets_have_defined_counts() {
    let empty = InputLimits {
        source_bytes: 2,
        total_bytes: 2,
        sources: 1,
        raw_nodes: 1,
        variables: 0,
        values: ValueLimits {
            nodes: 0,
            depth: 0,
            ..ValueLimits::default()
        },
    };
    assert!(parse_variables_with_limits("empty", "{}", &empty)
        .unwrap()
        .is_empty());
    let limits = InputLimits {
        source_bytes: 7,
        total_bytes: 7,
        sources: 1,
        raw_nodes: 3,
        variables: 1,
        values: ValueLimits {
            nodes: 3,
            depth: 2,
            entries: 2,
            payload_bytes: 8,
            ..ValueLimits::default()
        },
    };
    assert_eq!(
        parse_variable_with_limits("flag", "x=[1,2]", &limits)
            .unwrap()
            .1
            .to_string(),
        "[1, 2]"
    );
    for (limits, resource) in [
        (
            InputLimits {
                source_bytes: 6,
                ..limits.clone()
            },
            "input source bytes",
        ),
        (
            InputLimits {
                total_bytes: 6,
                ..limits.clone()
            },
            "total input bytes",
        ),
        (
            InputLimits {
                sources: 0,
                ..limits.clone()
            },
            "input sources",
        ),
        (
            InputLimits {
                raw_nodes: 2,
                ..limits.clone()
            },
            "input raw nodes",
        ),
        (
            InputLimits {
                variables: 0,
                ..limits
            },
            "input variables",
        ),
    ] {
        assert!(parse_variable_with_limits("flag", "x=[1,2]", &limits)
            .unwrap_err()
            .to_string()
            .contains(resource));
    }
}

#[test]
fn decoded_unicode_and_surrogate_pairs_are_sized_before_conversion() {
    for (setting, bytes, expected) in [
        (r#"x="\u0061""#, 1, "a"),
        (r#"x="\u00e9""#, 2, "é"),
        (r#"x="\u0b85""#, 3, "அ"),
        (r#"x="\uD83D\uDE42""#, 4, "🙂"),
        (r#"x="é\n\t\\\"\/""#, 7, "é\n\t\\\"/"),
    ] {
        let limits = InputLimits {
            values: ValueLimits {
                string_bytes: bytes,
                ..ValueLimits::default()
            },
            ..InputLimits::default()
        };
        assert_eq!(
            parse_variable_with_limits("unicode", setting, &limits)
                .unwrap()
                .1
                .to_string(),
            expected
        );
        let limits = InputLimits {
            values: ValueLimits {
                string_bytes: bytes - 1,
                ..limits.values
            },
            ..limits
        };
        assert!(parse_variable_with_limits("unicode", setting, &limits)
            .unwrap_err()
            .to_string()
            .contains("value string bytes"));
    }
    let limits = InputLimits {
        values: ValueLimits {
            key_bytes: 2,
            ..ValueLimits::default()
        },
        ..InputLimits::default()
    };
    assert!(
        parse_variables_with_limits("key", r#"{"\u00e9":1}"#, &limits)
            .unwrap()
            .contains_key("é")
    );
    assert!(
        parse_variables_with_limits("key", r#"{"x":{"\uD83D\uDE42":1}}"#, &limits)
            .unwrap_err()
            .to_string()
            .contains("value key bytes")
    );
}

#[test]
fn malformed_escapes_and_json_still_report_input_errors() {
    for text in [
        r#"x="\ud800""#,
        r#"x="\uDC00""#,
        r#"x="\uD800\uD800""#,
        r#"x="\uqqqq""#,
        r#"x="\ué""#,
        r#"x="\q""#,
        "x=[1,]",
        "x={\"k\":}",
        "x=[}",
        "x=1 2",
        "x=\"unfinished",
    ] {
        assert_eq!(
            parse_variable_with_limits("bad", text, &InputLimits::default())
                .unwrap_err()
                .code(),
            DiagnosticCode::Input,
            "{text}"
        );
    }
}

#[test]
fn root_names_and_nested_map_width_are_bounded_with_duplicate_keys_preserved() {
    let limits = InputLimits {
        variables: 1,
        values: ValueLimits {
            entries: 1,
            nodes: 2,
            payload_bytes: 5,
            ..ValueLimits::default()
        },
        ..InputLimits::default()
    };
    let result = parse_variables_with_limits(
        "duplicates",
        r#"{"x":2147483648,"x":{"k":1e999,"k":1}}"#,
        &limits,
    )
    .unwrap();
    assert_eq!(result["x"].to_string(), "{\"k\": 1}");
    assert!(
        parse_variables_with_limits("root", r#"{"x":1,"y":2}"#, &limits)
            .unwrap_err()
            .to_string()
            .contains("input variables")
    );
    assert!(
        parse_variables_with_limits("map", r#"{"x":{"a":1,"b":2}}"#, &limits)
            .unwrap_err()
            .to_string()
            .contains("value container entries")
    );
}

#[test]
fn discarded_raw_values_still_consume_resource_admission_budgets() {
    let text = r#"{"x":2147483648,"x":1}"#;
    let limits = InputLimits {
        raw_nodes: 5,
        ..InputLimits::default()
    };
    assert_eq!(
        parse_variables_with_limits("raw", text, &limits).unwrap()["x"].to_string(),
        "1"
    );
    assert!(parse_variables_with_limits(
        "raw",
        text,
        &InputLimits {
            raw_nodes: 4,
            ..limits
        }
    )
    .unwrap_err()
    .to_string()
    .contains("input raw nodes"));
    let limits = InputLimits {
        values: ValueLimits {
            string_bytes: 3,
            ..ValueLimits::default()
        },
        ..InputLimits::default()
    };
    assert!(
        parse_variables_with_limits("raw", r#"{"x":"discarded","x":1}"#, &limits)
            .unwrap_err()
            .to_string()
            .contains("value string bytes")
    );
}

#[test]
fn decoded_value_totals_and_nesting_are_checked_during_conversion() {
    for (setting, values, resource) in [
        (
            "x=[1,2]",
            ValueLimits {
                payload_bytes: 7,
                ..ValueLimits::default()
            },
            "value payload bytes",
        ),
        (
            r#"x={"a":1,"b":2}"#,
            ValueLimits {
                payload_bytes: 9,
                ..ValueLimits::default()
            },
            "value payload bytes",
        ),
        (
            "x=[[1],[2]]",
            ValueLimits {
                nodes: 4,
                ..ValueLimits::default()
            },
            "value nodes",
        ),
        (
            "x=[[[1]]]",
            ValueLimits {
                depth: 3,
                ..ValueLimits::default()
            },
            "value depth",
        ),
    ] {
        assert!(parse_variable_with_limits(
            "convert",
            setting,
            &InputLimits {
                values,
                ..InputLimits::default()
            }
        )
        .unwrap_err()
        .to_string()
        .contains(resource));
    }
}

#[test]
fn file_reads_stop_at_remaining_bytes_before_utf8_decoding() {
    let harness = Harness::new();
    let path = harness.workspace.join("data.json");
    fs::write(&path, [0xff; 9]).unwrap();
    let limits = InputLimits {
        source_bytes: 8,
        ..InputLimits::default()
    };
    let error = load_variables_with_limits(std::slice::from_ref(&path), &[], &limits).unwrap_err();
    assert!(error.to_string().contains("input source bytes"));
    assert_eq!(error.span.unwrap().source().text(), "");
    fs::write(&path, [0xff; 8]).unwrap();
    assert_eq!(
        load_variables_with_limits(std::slice::from_ref(&path), &[], &limits)
            .unwrap_err()
            .code(),
        DiagnosticCode::Input
    );
    fs::write(&path, "{\"x\":\"é\"}").unwrap();
    let error = load_variables_with_limits(
        &[path],
        &[],
        &InputLimits {
            source_bytes: 100,
            total_bytes: 6,
            ..InputLimits::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("total input bytes"));
}

#[test]
fn cumulative_file_flag_budgets_preserve_precedence_and_whole_value_replacement() {
    let harness = Harness::new();
    let files = [
        harness.workspace.join("a.json"),
        harness.workspace.join("b.json"),
    ];
    fs::write(&files[0], r#"{"x":1}"#).unwrap();
    fs::write(&files[1], r#"{"y":2}"#).unwrap();
    let settings = vec!["x=3".into()];
    let limits = InputLimits {
        sources: 3,
        total_bytes: 17,
        raw_nodes: 7,
        variables: 2,
        ..InputLimits::default()
    };
    let result = load_variables_with_limits(&files, &settings, &limits).unwrap();
    assert_eq!(result["x"].to_string(), "3");
    assert_eq!(result["y"].to_string(), "2");
    for (limits, resource) in [
        (
            InputLimits {
                sources: 2,
                ..limits.clone()
            },
            "input sources",
        ),
        (
            InputLimits {
                total_bytes: 16,
                ..limits.clone()
            },
            "total input bytes",
        ),
        (
            InputLimits {
                raw_nodes: 6,
                ..limits.clone()
            },
            "input raw nodes",
        ),
        (
            InputLimits {
                variables: 1,
                ..limits
            },
            "input variables",
        ),
    ] {
        assert!(load_variables_with_limits(&files, &settings, &limits)
            .unwrap_err()
            .to_string()
            .contains(resource));
    }
}

#[test]
fn overrides_do_not_add_variables_but_every_source_is_validated_in_order() {
    let limits = InputLimits {
        variables: 1,
        ..InputLimits::default()
    };
    let result = load_variables_with_limits(&[], &["x=1".into(), "x=[2]".into()], &limits).unwrap();
    assert_eq!(result["x"].to_string(), "[2]");
    assert!(
        load_variables_with_limits(&[], &["x=1".into(), "y=2".into()], &limits)
            .unwrap_err()
            .to_string()
            .contains("input variables")
    );
    let error = load_variables_with_limits(
        &[],
        &["x=2147483648".into(), "x=1".into()],
        &InputLimits {
            sources: 1,
            ..limits
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Input);
    assert!(error.to_string().contains("--var #1"));
}

#[test]
fn resource_errors_retain_origin_without_copying_the_payload() {
    let error = parse_variables_with_limits(
        "data.json",
        r#"{"secret":"private-payload"}"#,
        &InputLimits {
            source_bytes: 1,
            ..InputLimits::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("data.json:1:1"));
    assert!(!error.to_string().contains("private-payload"));
    assert!(error.span.unwrap().source().text().is_empty());
}

#[test]
fn long_collection_paths_are_abbreviated_without_changing_short_paths() {
    let key = "é".repeat(1000);
    let text = format!("x={{\"{key}\":2147483648}}");
    let error = parse_variable_with_limits("long", &text, &InputLimits::default()).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Input);
    assert!(error.to_string().contains('…'));
    assert!(error.to_string().len() < 1000);
    let error =
        parse_variable_with_limits("short", "x={\"key\":[2147483648]}", &InputLimits::default())
            .unwrap_err();
    assert!(error.to_string().contains("$[\"x\"][\"key\"][0]"));
}

#[test]
fn default_cli_input_width_failure_precedes_output_and_debug_traces() {
    let harness = Harness::new();
    let data = format!(
        "{{\"x\":[{}null]}}",
        "null,".repeat(ValueLimits::default().entries)
    );
    fs::write(harness.workspace.join("wide.json"), data).unwrap();
    let output = harness
        .run_with_args(
            "input-limit",
            "Log |\"unreachable\"|",
            &["--vars-file", "wide.json", "--debug"],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("[BW8001]"));
    assert!(diagnostic.contains("wide.json"));
    assert!(!diagnostic.contains("debug:"));
    let output = harness.run("recovery", "", Duration::from_secs(5)).unwrap();
    assert!(output.status.success());
}

#[test]
fn explicit_value_budgets_can_be_raised_without_changing_defaults() {
    let length = ValueLimits::default().string_bytes + 1;
    let setting = format!("x=\"{}\"", "a".repeat(length));
    assert!(parse_variable_with_limits("default", &setting, &InputLimits::default()).is_err());
    let limits = InputLimits {
        values: ValueLimits {
            string_bytes: length,
            ..ValueLimits::default()
        },
        ..InputLimits::default()
    };
    let value = parse_variable_with_limits("larger", &setting, &limits)
        .unwrap()
        .1;
    let botwork::core::grammar::Literal::String(value) = value else {
        panic!("string")
    };
    assert_eq!(value.len(), length);
}

#[test]
fn input_configuration_is_local_and_zero_sources_allows_an_empty_load() {
    let zero = InputLimits {
        sources: 0,
        total_bytes: 0,
        raw_nodes: 0,
        variables: 0,
        ..InputLimits::default()
    };
    assert!(load_variables_with_limits(&[], &[], &zero)
        .unwrap()
        .is_empty());
    let invalid = InputLimits {
        values: ValueLimits {
            depth: 65,
            ..ValueLimits::default()
        },
        ..InputLimits::default()
    };
    assert_eq!(
        parse_variable_with_limits("invalid", "x=0", &invalid)
            .unwrap_err()
            .code(),
        DiagnosticCode::RunConfiguration
    );
    let threads = (1..=2)
        .map(|entries| {
            std::thread::spawn(move || {
                let limits = InputLimits {
                    values: ValueLimits {
                        entries,
                        ..ValueLimits::default()
                    },
                    ..InputLimits::default()
                };
                let result = parse_variable_with_limits("thread", "x=[1,2]", &limits);
                assert_eq!(result.is_ok(), entries == 2);
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
}
