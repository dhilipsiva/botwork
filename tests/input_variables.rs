#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode,
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    input::{load_variables, parse_variable, parse_variables, MAX_JSON_DEPTH},
};
use cli_harness::Harness;
use std::{collections::BTreeMap, fs, process::Command, time::Duration};

fn evaluate(context: &mut Context, source: &str) -> Literal {
    evaluate_program_detailed(&Program::parse("inputs.botwork", source).unwrap(), context).unwrap()
}

#[test]
fn json_values_preserve_all_kinds_and_arbitrary_nested_keys() {
    let values = parse_variables(
        "data.json",
        r#"{
        "nothing": null, "yes": true, "no": false, "text": "é\n\t\uD83D\uDE42",
        "min": -2147483648, "max": 2147483647, "float": 1.5,
        "array": [null, 1, true, "hello", []],
        "object": {"": 1, "true": 2, "a/b~c": 3, "தமிழ்": 4,
                   "$serde_json::private::Number": "ordinary text"}
    }"#,
    )
    .unwrap();
    assert!(matches!(values["nothing"], Literal::None));
    assert!(matches!(values["yes"], Literal::Bool(true)));
    assert!(matches!(values["no"], Literal::Bool(false)));
    assert_eq!(values["text"].to_string(), "é\n\t🙂");
    assert!(matches!(values["min"], Literal::Int(i32::MIN)));
    assert!(matches!(values["max"], Literal::Int(i32::MAX)));
    assert!(matches!(values["float"], Literal::Float(1.5)));
    assert_eq!(
        values["array"].to_string(),
        "[none, 1, true, \"hello\", []]"
    );
    let Literal::Map(object) = &values["object"] else {
        panic!("map")
    };
    assert_eq!(object.len(), 5);
    assert_eq!(
        object["$serde_json::private::Number"].to_string(),
        "ordinary text"
    );
}

#[test]
fn numbers_use_token_kind_and_direct_f32_rounding() {
    for token in ["0", "-0", "2147483647", "-2147483648"] {
        assert!(matches!(
            parse_variable("flag", &format!("x={token}")).unwrap().1,
            Literal::Int(_)
        ));
    }
    for token in [
        "1.0",
        "1e0",
        "1E+0",
        "2147483648.0",
        "3.4028235e38",
        "1e-9999",
        "-0.0",
        "-1e-9999",
        "1.000000059604644775390625000000000001",
    ] {
        let Literal::Float(value) = parse_variable("flag", &format!("x={token}")).unwrap().1 else {
            panic!("{token} must be a float")
        };
        assert_eq!(value.to_bits(), token.parse::<f32>().unwrap().to_bits());
    }
}

#[test]
fn out_of_range_numbers_are_rejected_at_their_collection_path() {
    for token in [
        "2147483648",
        "-2147483649",
        "999999999999999999999999999999999",
        "3.4028236e38",
        "-3.4028236e38",
        "1e9999",
    ] {
        let error =
            parse_variables("config.json", &format!(r#"{{"x":[{{"key":{token}}}]}}"#)).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Input);
        assert!(
            error
                .to_string()
                .contains("config.json: $[\"x\"][0][\"key\"]"),
            "{error}"
        );
        assert!(
            !error.to_string().contains(&format!(": {token}")),
            "do not dump rejected input: {error}"
        );
    }
}

#[test]
fn only_exact_case_sensitive_dsl_identifiers_are_accepted() {
    for name in [
        "x",
        "X",
        "_",
        "_item2",
        "True",
        "If",
        "orphan",
        "é",
        "தமிழ்",
        "e\u{301}",
    ] {
        assert_eq!(
            parse_variable("flag", &format!("{name}=null")).unwrap().0,
            name
        );
    }
    for name in [
        "",
        " x",
        "x ",
        "x\n",
        "x#comment",
        "x.y",
        "x[0]",
        "true",
        "false",
        "and",
        "or",
        "3x",
        "🙂",
        "\u{301}x",
        "x| = |7",
        "x=1",
    ] {
        let text = serde_json::to_string(&BTreeMap::from([(name, 1)])).unwrap();
        let error = parse_variables("names.json", &text).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Input, "{name:?}");
    }
    let mut context = Context::default();
    context
        .set_input_variables(parse_variables("data", r#"{"x":1,"X":2}"#).unwrap())
        .unwrap();
    assert_eq!(
        evaluate(&mut context, "|r| = |[x, X]|").to_string(),
        "[1, 2]"
    );
}

#[test]
fn settings_split_at_first_equals_and_strings_are_never_executed() {
    let (_, value) = parse_variable("flag", r#"text="a=b; @{ Log |123| }""#).unwrap();
    assert_eq!(value.to_string(), "a=b; @{ Log |123| }");
    for setting in [
        "x",
        "x=",
        "x=unquoted",
        "x=@{ Log |123| }",
        "x=true false",
        "x=NaN",
        "x=Infinity",
        "x=01",
        "x=+1",
        "x=1.",
    ] {
        assert_eq!(
            parse_variable("flag", setting).unwrap_err().code(),
            DiagnosticCode::Input,
            "{setting}"
        );
    }
}

#[test]
fn invalid_json_reports_origin_and_position_without_publishing_values() {
    for text in [
        "",
        "null",
        "[]",
        "1",
        r#""string""#,
        "{\n\"x\": }",
        r#"{"x":1,}"#,
        r#"{"x":1} garbage"#,
        r#"{"x":"\q"}"#,
        r#"{"x":"\uD800"}"#,
        r#"{"x":{"\uD800":1}}"#,
        r#"{"x":/*comment*/1}"#,
    ] {
        let error = parse_variables("broken.json", text).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Input);
        assert!(error.to_string().contains("broken.json:"));
    }
    let error = parse_variables("broken.json", "{\n\"x\": }").unwrap_err();
    assert!(error.to_string().contains("line 2 column"), "{error}");
}

#[test]
fn duplicate_json_keys_use_the_last_value_before_conversion() {
    let values = parse_variables("data", r#"{"x":2147483648,"x":2,"m":{"a":1,"a":3}}"#).unwrap();
    assert_eq!(values["x"].to_string(), "2");
    assert_eq!(values["m"].to_string(), "{\"a\": 3}");
    assert_eq!(parse_variable("flag", "x= {}").unwrap().1.to_string(), "{}");
}

#[test]
fn nesting_limit_counts_containers_and_ignores_escaped_string_delimiters() {
    let nested = |depth| format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
    parse_variable("flag", &format!("x={}", nested(MAX_JSON_DEPTH))).unwrap();
    assert!(parse_variable("flag", &format!("x={}", nested(MAX_JSON_DEPTH + 1))).is_err());
    parse_variables(
        "data",
        &format!(r#"{{"x":{}}}"#, nested(MAX_JSON_DEPTH - 1)),
    )
    .unwrap();
    assert!(parse_variables(
        "data",
        &format!(r#"{{"x":{},"x":0}}"#, nested(MAX_JSON_DEPTH))
    )
    .is_err());
    let text = serde_json::to_string(&"[\"\\{}]".repeat(1000)).unwrap();
    parse_variable("flag", &format!("x={text}")).unwrap();
    assert!(parse_variable("flag", &format!("x={}", nested(10000))).is_err());
}

#[test]
fn host_installation_is_atomic_and_rejects_nested_non_finite_values() {
    let mut context = Context::default();
    context
        .set_input_variables(BTreeMap::from([("a".into(), Literal::Int(1))]))
        .unwrap();
    for invalid in [
        Literal::Float(f32::NAN),
        Literal::Array(vec![Literal::Float(f32::INFINITY)]),
        Literal::Map([("v".into(), Literal::Float(f32::NEG_INFINITY))].into()),
    ] {
        let error = context
            .set_input_variables(BTreeMap::from([
                ("a".into(), Literal::Int(2)),
                ("z".into(), invalid),
            ]))
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Input);
        assert_eq!(evaluate(&mut context, "|r| = |a|").to_string(), "1");
        assert!(evaluate_program_detailed(
            &Program::parse("check", "|r| = |z|").unwrap(),
            &mut context
        )
        .is_err());
    }
    assert!(context
        .set_input_variables(BTreeMap::from([
            ("a".into(), Literal::Int(2)),
            ("z invalid".into(), Literal::None),
        ]))
        .is_err());
    assert_eq!(evaluate(&mut context, "|r| = |a|").to_string(), "1");
}

#[test]
fn root_inputs_obey_lexical_shadowing_restoration_and_context_isolation() {
    let mut context = Context::default();
    context
        .set_input_variables(parse_variables("data", r#"{"x":7,"item":null}"#).unwrap())
        .unwrap();
    let mut cloned = context.clone();
    let result = evaluate(&mut context, "Read { Return |x| }\nLocal |x| {\n|x| = |x + 1|\nReturn |[x, @{ Read }]|\n}\nFor |item| In |[1]| { |x| = |8| }\n|r| = |[@{ Local |2| }, x, item]|");
    assert_eq!(result.to_string(), "[[3, 8], 8, none]");
    assert_eq!(evaluate(&mut cloned, "|r| = |x|").to_string(), "7");
    assert!(evaluate_program_detailed(
        &Program::parse("fresh", "|r| = |x|").unwrap(),
        &mut Context::default()
    )
    .is_err());
}

#[test]
fn loading_files_then_flags_has_explicit_replacement_precedence() {
    let harness = Harness::new();
    let first = harness.workspace.join("first.json");
    let second = harness.workspace.join("second.json");
    fs::write(&first, r#"{"x":1,"m":{"old":1,"keep":2},"fileOnly":true}"#).unwrap();
    fs::write(&second, r#"{"x":2,"m":{"new":3}}"#).unwrap();
    let files = vec![first.clone(), second.clone()];
    let values = load_variables(&files, &["x=3".into(), "x=4".into()]).unwrap();
    assert_eq!(values["x"].to_string(), "4");
    assert_eq!(values["m"].to_string(), "{\"new\": 3}");
    assert_eq!(values["fileOnly"].to_string(), "true");
    fs::write(&first, r#"{"x":2147483648}"#).unwrap();
    assert!(load_variables(&files, &["x=4".into()]).is_err());
    assert!(load_variables(&[], &["x=2147483648".into(), "x=4".into()]).is_err());
    assert!(load_variables(&[], &[]).unwrap().is_empty());
}

#[test]
fn input_file_failures_include_source_and_stable_code() {
    let harness = Harness::new();
    let bad = harness.workspace.join("bad.json");
    fs::write(&bad, [0xff, 0xfe]).unwrap();
    for path in [
        bad,
        harness.workspace.join("missing.json"),
        harness.workspace.clone(),
    ] {
        let error = load_variables(std::slice::from_ref(&path), &[]).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Input);
        assert!(error.to_string().contains(&path.display().to_string()));
    }
}

#[test]
fn cli_combines_files_flags_and_script_assignments_from_another_directory() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("one.json"),
        r#"{"x":1,"text":"file","items":[1,null]}"#,
    )
    .unwrap();
    fs::write(harness.workspace.join("two.json"), r#"{"x":2}"#).unwrap();
    let output = harness
        .run_with_args(
            "inputs",
            "Log |[x, text, items]|\n|x| = |9|\nLog |x|",
            &[
                "--var",
                "x=3",
                "--vars-file",
                "one.json",
                "--vars-file",
                "two.json",
                "--var",
                "x=4",
                "--var",
                r#"text="hello=🙂""#,
            ],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        "[4, \"hello=🙂\", [1, none]]\n9\n".as_bytes()
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn cli_rejects_bad_inputs_before_any_script_output_or_debug_trace() {
    let harness = Harness::new();
    for arguments in [
        vec!["--var", "x=2147483648"],
        vec!["--var", "x=secret-bare-text"],
        vec!["--vars-file", "missing.json"],
        vec!["--var", "x =1"],
    ] {
        let mut arguments = arguments;
        arguments.push("--debug");
        let output = harness
            .run_with_args(
                "bad-input",
                "Log |\"unreachable\"|",
                &arguments,
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("[BW7001]"), "{stderr}");
        assert!(
            !stderr.contains("debug:") && !stderr.contains("secret-bare-text"),
            "{stderr}"
        );
    }
}

#[test]
fn cli_inputs_are_not_inherited_by_imported_modules() {
    let harness = Harness::new();
    let absent = harness
        .run("absent-input", "Log |input|", Duration::from_secs(5))
        .unwrap();
    assert_eq!(absent.status.code(), Some(1));
    fs::write(
        harness.workspace.join("module.botwork"),
        "Read { Return |input| }\nEcho |value| { Return |value| }",
    )
    .unwrap();
    let output = harness.run_with_args("isolated", "Import |\"module.botwork\"| As |module|\nTry { module::Read } Catch |error| { Log |error.code| }\nLog |@{ module::Echo |input| }|", &["--var", "input=7"], Duration::from_secs(5)).unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"BW2001\n7\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn cli_help_describes_inputs_and_rejects_execution_help_combinations() {
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--var <NAME=JSON>") && help.contains("--vars-file <PATH>"));
    for args in [
        vec!["--list-statements", "--var", "x=1"],
        vec!["--statement-help", "Log |value|", "--vars-file", "x.json"],
        vec!["--var", "x=1"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn documented_input_example_matches_exact_cli_output() {
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("defaults.json"),
        include_str!("../examples/inputs/defaults.json"),
    )
    .unwrap();
    let output = harness
        .run_with_args(
            "example-inputs",
            include_str!("../examples/20-input-variables.botwork"),
            &[
                "--vars-file",
                "defaults.json",
                "--var",
                r#"name="Ada""#,
                "--var",
                "attempts=3",
            ],
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        "[\"Ada\", 3, true, [\"local\", \"தமிழ்\"]]\nnone\n".as_bytes()
    );
}
