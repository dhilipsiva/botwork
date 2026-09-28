use std::path::PathBuf;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(args)
        .output()
        .expect("run botwork CLI")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn statement_listing_and_help_use_registered_metadata_without_a_file() {
    use botwork::core::eval::Context;
    let output = run(&["--list-statements"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stdout,
        concat!(
            "Absolute Path |path|\n",
            "Add Durations |left| And |right|\n",
            "Add Duration |duration| To Date Time |datetime|\n",
            "Append To File |path| Text |text|\n",
            "Append To |array| Value |value|\n",
            "Assert |condition|\n",
            "Assert |actual| Equals |expected|\n",
            "Canonical Path |path|\n",
            "Capture From |text| Regex |pattern|\n",
            "Collection Contains |collection| Item |item|\n",
            "Collections Equal |left| And |right|\n",
            "Compare Date Times |left| And |right|\n",
            "Compare Durations |left| And |right|\n",
            "Convert Date Time |datetime| To |zone|\n",
            "Copy File |source| To |destination|\n",
            "Create Array\n",
            "Create Directory |path|\n",
            "Create Duration |amount| In |unit|\n",
            "Create File |path| Text |text|\n",
            "Create Map\n",
            "Create Map From |entries|\n",
            "Create Temporary Directory In |directory| Prefix |prefix|\n",
            "Current Date Time In |zone|\n",
            "Difference Between Date Times |left| And |right|\n",
            "Directory Exists |path|\n",
            "Duration Nanoseconds |duration|\n",
            "Duration Seconds |duration|\n",
            "Enumerate |array|\n",
            "Environment Variable Exists |name|\n",
            "Environment Variables\n",
            "Fail |message|\n",
            "File Exists |path|\n",
            "File Extension |path|\n",
            "File Name |path|\n",
            "File Size |path|\n",
            "Find Matches In |text| Regex |pattern|\n",
            "Format Date Time |datetime| Using |format| In |zone|\n",
            "Format String |template| With |values|\n",
            "Get Environment Variable |name|\n",
            "Get From |collection| At |key|\n",
            "Get Variable |name|\n",
            "Join Path |parts|\n",
            "Join Strings |strings| With |separator|\n",
            "Length Of |collection|\n",
            "List Directory |path|\n",
            "Log |value|\n",
            "Lowercase String |text|\n",
            "Map Entries |map|\n",
            "Map Keys |map|\n",
            "Map Values |map|\n",
            "Move Path |source| To |destination|\n",
            "No Operation\n",
            "Operating System\n",
            "Parent Path |path|\n",
            "Parse Date Time |text|\n",
            "Parse Date Time |text| Using |format| In |zone| Choosing |ambiguity|\n",
            "Parse Duration |text|\n",
            "Path Components |path|\n",
            "Path Exists |path|\n",
            "Path Is Absolute |path|\n",
            "Path Kind |path|\n",
            "Path Separator\n",
            "Read Binary File |path|\n",
            "Read File |path|\n",
            "Remove Directory |path| Recursively |recursive|\n",
            "Remove File |path|\n",
            "Remove From |collection| At |key|\n",
            "Repeat |value| Times |count|\n",
            "Replace String |text| Find |needle| With |replacement|\n",
            "Run Binary Process |executable| With Arguments |arguments|\n",
            "Run Binary Process |executable| With Arguments |arguments| Options |options|\n",
            "Run Process |executable| With Arguments |arguments|\n",
            "Run Process |executable| With Arguments |arguments| Options |options|\n",
            "Set In |collection| At |key| To |value|\n",
            "Sleep |milliseconds|\n",
            "Slice String |text| From |start| To |end|\n",
            "Slice |array| From |start| To |end|\n",
            "Split Lines |text|\n",
            "Split String |text| On |separator|\n",
            "String Contains |text| Text |needle|\n",
            "String Ends With |text| Suffix |suffix|\n",
            "String Length |text|\n",
            "String Matches |text| Regex |pattern|\n",
            "String Starts With |text| Prefix |prefix|\n",
            "Subtract Durations |left| Minus |right|\n",
            "Subtract Duration |duration| From Date Time |datetime|\n",
            "Trim String |text|\n",
            "Type Of |value|\n",
            "Uppercase String |text|\n",
            "Variable Exists |name|\n",
            "Working Directory\n",
            "Write Binary File |path| Bytes |bytes|\n",
            "Write File |path| Text |text|\n",
        )
        .as_bytes()
    );
    assert!(output.stderr.is_empty());
    let output = run(&["--statement-help", "l O g |input|"]);
    assert_eq!(output.status.code(), Some(0));
    let mut context = Context::default();
    context.init_statements();
    let expected = format!(
        "{}\n",
        context
            .statement_signature("Log |x|")
            .unwrap()
            .unwrap()
            .help()
    );
    assert_eq!(output.stdout, expected.as_bytes());
    assert!(output.stderr.is_empty());
    assert!(expected.contains("value: Any\n  returns: Any\n  BW4001:"));
}

#[test]
fn statement_help_rejects_invalid_unknown_and_conflicting_requests() {
    for (header, code) in [("Unknown", "BW2002"), ("Log |1|", "BW1001")] {
        let output = run(&["--statement-help", header]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr).unwrap().contains(code));
    }
    for args in [
        vec!["--list-statements", "--file", "missing.botwork"],
        vec!["--statement-help", "Log |x|", "--debug"],
        vec!["--list-statements", "--statement-help", "Log |x|"],
        vec![],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn invalid_call_composition_fails_before_output_and_valid_call_effects_stop_at_first_error() {
    let path = fixture("invalid-call-composition.botwork");
    let output = run(&["--file", path.to_str().unwrap(), "--debug"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("BW1001"));
    assert!(!diagnostic.contains("debug:"));
    let path = fixture("call-composition-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"completed first\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("BW2001"));
    assert!(diagnostic.contains("expression: missing"));
}

fn assert_control_placement_failure(name: &str, line: usize, column: usize, keyword: &str) {
    let path = fixture(name);
    // Validation also precedes debug tracing: no statement is executed or traced.
    for debug in [false, true] {
        let mut arguments = vec!["--file", path.to_str().unwrap()];
        if debug {
            arguments.push("--debug");
        }
        let output = run(&arguments);
        assert_eq!(output.status.code(), Some(1), "{name}, debug={debug}");
        assert!(
            output.stdout.is_empty(),
            "{name}: validation must prevent earlier output: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("Invalid control flow:"), "{diagnostic}");
        assert!(
            diagnostic.contains(&format!("{name}:{line}:{column}: {keyword} requires ")),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("panicked"), "{diagnostic}");
        assert!(!diagnostic.contains("debug:"), "{diagnostic}");
    }
}

#[test]
fn execution_trace_preserves_recursive_order_skips_failed_calls_and_restores_scopes() {
    let path = fixture("execution-order.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "[\"enter\", 3]\n[\"enter\", 2]\n[\"enter\", 1]\n[\"enter\", 0]\n",
            "[\"leave\", 1]\n[\"leave\", 2]\n[\"leave\", 3]\n[6, 99]\n",
            "arguments failed before body\npair body\n[1, 99]\n",
            "[\"inner\", 10]\n[\"restored outer\", 1]\n",
            "[\"inner\", 10]\n[\"restored outer\", 2]\n[\"restored root\", 99]\n",
            "[\"caller resumes\", 1, 7]\n[\"caller resumes\", 2, 7]\n"
        )
    );
}

#[test]
fn native_argument_failure_preserves_prior_output_and_skips_the_entire_call_tail() {
    let path = fixture("execution-order-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\ncallee entered\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("execution-order-error.botwork"));
    assert!(diagnostic.contains("missing_first"));
    assert!(!diagnostic.contains("missing_second"));
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn structured_runtime_diagnostics_report_the_expression_and_entered_call_stack() {
    let path = fixture("diagnostic-stack.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("diagnostic-stack.botwork:2:17"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("expression: missing"), "{diagnostic}");
    assert!(
        diagnostic.contains("diagnostic-stack.botwork:5:16"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("diagnostic-stack.botwork:8:1"),
        "{diagnostic}"
    );
}

#[test]
fn a_failed_handler_reports_both_failures_at_their_original_locations() {
    let path = fixture("diagnostic-handler.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("missing_handler"), "{diagnostic}");
    assert!(diagnostic.contains("missing_original"), "{diagnostic}");
    assert!(
        diagnostic.contains("diagnostic-handler.botwork:4:16"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("diagnostic-handler.botwork:2:16"),
        "{diagnostic}"
    );
}

#[test]
fn cli_diagnostics_expose_stable_codes_and_actionable_guidance() {
    for (name, code, hint) in [
        ("syntax-error.botwork", "BW1001", "close every pipe"),
        ("runtime-error.botwork", "BW2001", "check spelling and case"),
        ("arithmetic-failure.botwork", "BW3002", "zero divisors"),
        ("boolean-left-type-error.botwork", "BW3003", "operand kinds"),
        (
            "computed-access-out-of-bounds.botwork",
            "BW3004",
            "in-bounds",
        ),
        ("duplicate-parameter.botwork", "BW1003", "distinct"),
    ] {
        let path = fixture(name);
        let output = run(&["--file", path.to_str().unwrap()]);
        assert_eq!(output.status.code(), Some(1));
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(
            diagnostic.contains(&format!("[{code}]")),
            "{name}: {diagnostic}"
        );
        assert!(diagnostic.contains(hint), "{name}: {diagnostic}");
        assert!(diagnostic.contains("help:"), "{name}: {diagnostic}");
    }
}

#[test]
fn rethrow_placement_fails_before_output_and_valid_rethrow_preserves_the_original_diagnostic() {
    assert_control_placement_failure("rethrow-outside-catch.botwork", 2, 1, "Rethrow");
    let path = fixture("rethrow-original.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"BW3002\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("[BW3002] Arithmetic error:"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("rethrow-original.botwork:2:10"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("rethrow-original.botwork:6:5"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("while handling:"), "{diagnostic}");
    assert!(!diagnostic.contains("unreachable"), "{diagnostic}");
}

#[test]
fn invalid_control_placement_prevents_all_cli_execution() {
    assert_control_placement_failure("invalid-control.botwork", 2, 13, "Return");
}

#[test]
fn invalid_control_in_unused_definitions_is_rejected_before_execution() {
    assert_control_placement_failure("invalid-control-unused-break.botwork", 3, 5, "Break");
    assert_control_placement_failure("invalid-control-unused-continue.botwork", 3, 5, "Continue");
}

#[test]
fn nested_definitions_cannot_inherit_their_enclosing_loops_for_control_placement() {
    assert_control_placement_failure("invalid-control-definition-in-for.botwork", 4, 9, "Break");
    assert_control_placement_failure(
        "invalid-control-definition-in-while.botwork",
        4,
        9,
        "Continue",
    );
}

#[test]
fn invalid_control_in_unselected_branches_and_nested_catches_is_rejected() {
    assert_control_placement_failure("invalid-control-false-branch.botwork", 3, 5, "Break");
    assert_control_placement_failure("invalid-control-nested-catch.botwork", 5, 9, "Return");
}

#[test]
fn invalid_control_diagnostics_preserve_unicode_crlf_and_tab_locations() {
    assert_control_placement_failure("invalid-control-unicode.botwork", 2, 20, "Return");
}

#[test]
fn signed_integer_overflow_fails_after_printing_the_valid_minimum() {
    let path = fixture("signed-integer-overflow.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"-2147483648\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("signed-integer-overflow.botwork"));
    assert!(diagnostic.contains("Arithmetic error:"));
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn invalid_unicode_identifiers_are_rejected_before_output() {
    let path = fixture("invalid-unicode-identifier.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("invalid-unicode-identifier.botwork"));
    assert!(diagnostic.contains("Parsing error:"));
    assert!(diagnostic.contains("🙂"));
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn duplicate_statements_report_both_locations_and_stop_after_prior_output() {
    let path = fixture("duplicate-statement.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("Duplicate statement `choice`"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("duplicate-statement.botwork:1:1"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("duplicate-statement.botwork:3:1"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn duplicate_parameters_in_skipped_definitions_prevent_all_output_and_tracing() {
    let path = fixture("duplicate-parameter.botwork");
    for debug in [false, true] {
        let mut arguments = vec!["--file", path.to_str().unwrap()];
        if debug {
            arguments.push("--debug");
        }
        let output = run(&arguments);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(
            diagnostic.contains("Duplicate parameter `x`"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("duplicate-parameter.botwork:3:11"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("duplicate-parameter.botwork:3:20"),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("debug:"));
        assert!(!diagnostic.contains("panicked"));
    }
}

#[test]
fn unclosed_block_comments_and_continuations_prevent_all_output() {
    for name in [
        "unclosed-block-comment.botwork",
        "invalid-continuation.botwork",
    ] {
        let path = fixture(name);
        for debug in [false, true] {
            let mut arguments = vec!["--file", path.to_str().unwrap()];
            if debug {
                arguments.push("--debug");
            }
            let output = run(&arguments);
            assert_eq!(output.status.code(), Some(1), "{name}");
            assert!(output.stdout.is_empty(), "{name}");
            let diagnostic = String::from_utf8(output.stderr).unwrap();
            assert!(diagnostic.contains(name));
            assert!(diagnostic.contains("Parsing error:"));
            assert!(!diagnostic.contains("panicked"));
            assert!(!diagnostic.contains("debug:"));
        }
    }
}

#[test]
fn collection_ordering_fails_after_valid_equality_output() {
    let path = fixture("invalid-collection-ordering.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"true\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("invalid-collection-ordering.botwork"));
    assert!(diagnostic.contains("Operation performed on incompatible types:"));
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn computed_access_failures_preserve_prior_output_and_stop_execution() {
    let path = fixture("computed-access-out-of-bounds.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("computed-access-out-of-bounds.botwork"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("Collection access failed: data[\"items\"][index] at `[index]`: array index is out of bounds for length 1"), "{diagnostic}");
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn malformed_skipped_computed_access_prevents_all_cli_output() {
    let path = fixture("invalid-computed-access.botwork");
    for debug in [false, true] {
        let mut arguments = vec!["--file", path.to_str().unwrap()];
        if debug {
            arguments.push("--debug");
        }
        let output = run(&arguments);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("invalid-computed-access.botwork"));
        assert!(!diagnostic.contains("panicked"));
        assert!(!diagnostic.contains("debug:"));
    }
}

#[test]
fn help_describes_file_argument_on_stdout() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--file"));
    assert!(output.stderr.is_empty());
}

#[test]
fn version_matches_package_version() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("botwork {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_file_argument_fails_on_stderr() {
    let output = run(&[]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--file"));
}

#[test]
fn nonexistent_file_fails_on_stderr() {
    let path = fixture("does-not-exist.botwork");
    assert!(!path.exists());
    let output = run(&["--file", path.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn assignment_program_succeeds_without_output() {
    let path = fixture("assignment.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn caught_evaluation_error_succeeds_without_diagnostics() {
    let path = fixture("caught-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_catch_is_a_syntax_error_before_any_execution() {
    let path = fixture("missing-catch.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "a syntax error must prevent prior Log output"
    );
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("missing-catch.botwork"));
    assert!(diagnostic.contains("expected stmt_catch"));
}

#[test]
fn catch_runs_once_and_execution_continues_after_a_handled_failure() {
    let path = fixture("catch-output.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"before\ntrying\ncaught\nafter\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn handler_failure_stops_execution_and_fails_the_cli() {
    let path = fixture("catch-failure.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"handler\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("catch-failure.botwork"));
    assert!(diagnostic.contains("Variable not defined: handler_missing"));
}

#[test]
fn out_of_bounds_access_fails_without_panicking_or_executing_later_statements() {
    let path = fixture("access-out-of-bounds.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("access-out-of-bounds.botwork"));
    assert!(diagnostic.contains(
        "Collection access failed: m.items.1 at `1`: array index is out of bounds for length 1"
    ));
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn caught_access_failure_resumes_the_script_successfully() {
    let path = fixture("caught-access.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"caught\nafter\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn uncaught_arithmetic_errors_fail_without_panicking() {
    let path = fixture("arithmetic-failure.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("arithmetic-failure.botwork"));
    assert!(diagnostic.contains("Arithmetic error: divide by zero"));
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn unknown_keyword_prefix_is_reported_as_a_complete_statement_name() {
    let path = fixture("keyword-prefix-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("keyword-prefix-error.botwork"));
    assert!(diagnostic.contains("Statement not defined: Return-value"));
    assert!(!diagnostic.contains("panicked"));
}

#[test]
fn an_invalid_left_boolean_type_fails_before_the_right_operand() {
    let path = fixture("boolean-left-type-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"before\n");
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("boolean-left-type-error.botwork"));
    assert!(diagnostic.contains("Operation performed on incompatible types"));
    assert!(diagnostic.contains("left operand of `and` must be a boolean"));
    assert!(!diagnostic.contains("Variable not defined"));
}

#[test]
fn skipped_boolean_operands_still_require_valid_syntax_before_execution() {
    let path = fixture("boolean-skipped-syntax-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("boolean-skipped-syntax-error.botwork"));
    assert!(diagnostic.contains("expected"));
}

#[test]
fn runtime_error_reports_file_and_variable_and_stops_execution() {
    let path = fixture("runtime-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("runtime-error.botwork"));
    assert!(diagnostic.contains("Variable not defined: missing"));
    assert!(!diagnostic.contains("must not execute"));
}

#[test]
fn syntax_error_reports_source_location_on_stderr() {
    let path = fixture("syntax-error.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("syntax-error.botwork"));
    // Newlines are valid in expressions; the missing closing pipe fails at EOF.
    assert!(diagnostic.contains("2:1"));
    assert!(diagnostic.contains("expected"));
}

#[test]
fn runtime_failure_preserves_prior_output_but_skips_later_statements() {
    let path = fixture("runtime-after-output.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let messages = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(messages.contains("before failure"));
    assert!(messages.contains("Variable not defined: missing"));
    assert!(!messages.contains("after failure"));
}

#[test]
fn syntax_failure_prevents_execution_of_the_whole_program() {
    let path = fixture("syntax-after-output.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("syntax-after-output.botwork"));
    assert!(!diagnostic.contains("must not execute"));
}

#[test]
fn log_writes_readable_values_to_stdout() {
    let path = fixture("log-values.botwork");
    let output = run(&["--file", path.to_str().unwrap()]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "hello\n7\n1.5\ntrue\n[1, \"x\", false]\n{\"a\": 1, \"z\": 2}\nfirst\nsecond\n"
    );
    assert!(output.stderr.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn log_failure_on_a_full_output_device_reports_its_call_site_and_nonzero_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--file")
        .arg(fixture("log-values.botwork"))
        .stdout(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("[BW4001]"), "{diagnostic}");
    assert!(
        diagnostic.contains("log-values.botwork:1:1"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("bytes already written"), "{diagnostic}");
    assert!(!diagnostic.contains("panicked"), "{diagnostic}");
}

#[test]
fn debug_traces_locations_without_changing_stdout() {
    let path = fixture("log-values.botwork");
    let normal = run(&["--file", path.to_str().unwrap()]);
    let debug = run(&["--debug", "--file", path.to_str().unwrap()]);
    assert!(debug.status.success());
    assert_eq!(debug.stdout, normal.stdout);
    let trace = String::from_utf8(debug.stderr).unwrap();
    let lines: Vec<_> = trace.lines().collect();
    assert_eq!(lines.len(), 7);
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(
            *line,
            format!("debug: {}:{}:1: call", path.display(), index + 1)
        );
    }
    assert!(
        !trace.contains("hello"),
        "trace should not copy literal values"
    );
}
