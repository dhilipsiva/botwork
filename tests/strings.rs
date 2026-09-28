use botwork::core::{
    diagnostic::DiagnosticCode as Code,
    grammar::Literal,
    run::{Engine, RunLimits, RunOptions, RunOutcome, SnapshotLimits, TemporaryLimits},
    value_limits::ValueLimits,
};

fn run(source: &str) -> botwork::core::run::RunResult {
    Engine::default().run_source("strings.botwork", source, RunOptions::default())
}

const SUCCESS: &str = r#"
|original| = |"a🙂éz"|
Assert |@{ String Length |original| }| Equals |5|
Assert |@{ Slice String |original| From |1| To |4| }| Equals |"🙂é"|
Assert |@{ Slice String |original| From |3| To |4| }| Equals |"́"|
Assert |@{ Slice String |original| From |5| To |5| }| Equals |""|
Assert |@{ Slice String |""| From |0| To |0| }| Equals |""|
Assert |original| Equals |"a🙂éz"|
Assert |@{ Format String |"{1}: {{{0}}} {0}"| With |[42, "value"]| }| Equals |"value: {42} 42"|
Assert |@{ Format String |"{தமிழ்}/{}/{a.b}"| With |{"தமிழ்": "வணக்கம்", "": true, "a.b": [1, 2]}| }| Equals |"வணக்கம்/true/[1, 2]"|
Assert |@{ Format String |"{0}"| With |[@{ No Operation }]| }| Equals |"none"|
Assert |@{ Format String |"{Name}:{name}:{ key }"| With |{"Name": "A", "name": "b", " key ": "c"}| }| Equals |"A:b:c"|
Assert |@{ Format String |"{0}"| With |["{1}"]| }| Equals |"{1}"|
Assert |@{ Format String |"{{}}"| With |[]| }| Equals |"{}"|
Assert |@{ Format String |"{00}"| With |[2]| }| Equals |"2"|
Assert |@{ Format String |"{0}"| With |[{z: 1, a: ["é"]}]| }| Equals |"{\"a\": [\"é\"], \"z\": 1}"|
Assert |@{ Join Strings |["a", "", "🙂"]| With |"::"| }| Equals |"a::::🙂"|
Assert |@{ Join Strings |[]| With |"unused"| }| Equals |""|
Assert |@{ Join Strings |["a", "b"]| With |""| }| Equals |"ab"|
Assert |@{ Split String |"::a::::b::"| On |"::"| }| Equals |["", "a", "", "b", ""]|
Assert |@{ Split String |""| On |","| }| Equals |[""]|
Assert |@{ Split String |"abc"| On |"z"| }| Equals |["abc"]|
Assert |@{ Split String |"a🙂b🙂"| On |"🙂"| }| Equals |["a", "b", ""]|
Assert |@{ Split Lines |"a\nb\n"| }| Equals |["a", "b"]|
Assert |@{ Split Lines |""| }| Equals |[]|
Assert |@{ Split Lines |"\n\n"| }| Equals |["", ""]|
Assert |@{ Replace String |"aaaaa"| Find |"aa"| With |"$1"| }| Equals |"$1$1a"|
Assert |@{ Replace String |"ababa"| Find |"aba"| With |"x"| }| Equals |"xba"|
Assert |@{ Replace String |"🙂a🙂"| Find |"🙂"| With |""| }| Equals |"a"|
Assert |@{ Replace String |""| Find |"x"| With |"y"| }| Equals |""|
Assert |@{ String Contains |"hello"| Text |"ell"| }|
Assert |@{ String Contains |""| Text |""| }|
Assert |@{ String Contains |"é"| Text |"é"| }| Equals |false|
Assert |@{ String Starts With |"Hello"| Prefix |"He"| }|
Assert |@{ String Starts With |"Hello"| Prefix |"he"| }| Equals |false|
Assert |@{ String Ends With |"🙂z"| Suffix |"z"| }|
Assert |@{ String Ends With |"z"| Suffix |""| }|
Assert |@{ Trim String |" hello　\n"| }| Equals |"hello"|
Assert |@{ Trim String |" x y "| }| Equals |"x y"|
Assert |@{ Uppercase String |"Straße ﬃ தமிழ்"| }| Equals |"STRASSE FFI தமிழ்"|
Assert |@{ Lowercase String |"ΟΣ Σ ΟΣΑ İ"| }| Equals |"ος σ οσα i̇"|
Assert |@{ String Matches |"abc123"| Regex |"[0-9]+"| }|
Assert |@{ String Matches |"abc123"| Regex |"\\A[0-9]+\\z"| }| Equals |false|
Assert |@{ String Matches |"CAFÉ"| Regex |"(?i)café"| }|
Assert |@{ Find Matches In |"a12b345"| Regex |"[0-9]+"| }| Equals |["12", "345"]|
Assert |@{ Find Matches In |"é🙂"| Regex |""| }| Equals |["", "", ""]|
Assert |@{ Find Matches In |"abc"| Regex |".*"| }| Equals |["abc"]|
Assert |@{ Capture From |"ab"| Regex |"(a)(x)?(b)"| }| Equals |["ab", "a", @{ No Operation }, "b"]|
Assert |@{ Capture From |"ab"| Regex |"z"| }| Equals |[]|
Assert |@{ Capture From |""| Regex |"()"| }| Equals |["", ""]|
"#;

#[test]
fn string_catalogue_formats_transforms_and_matches_without_mutating_inputs() {
    let result = run(SUCCESS);
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[tokio::test]
async fn string_catalogue_preserves_results_across_inline_and_worker_execution() {
    let result = Engine::default()
        .run_source_async("strings-async", SUCCESS, RunOptions::default())
        .await;
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn host_strings_preserve_crlf_bare_cr_tabs_nul_and_exact_scalar_units() {
    use std::collections::BTreeMap;
    let options = RunOptions {
        variables: BTreeMap::from([
            ("lines".into(), Literal::String("a\r\nb\nc\rd\n".into())),
            (
                "expected".into(),
                Literal::Array(vec![
                    Literal::String("a".into()),
                    Literal::String("b".into()),
                    Literal::String("c\rd".into()),
                ]),
            ),
            (
                "whitespace".into(),
                Literal::String("\t \u{a0}hello\r\n".into()),
            ),
            ("nul".into(), Literal::String("a\0🙂".into())),
        ]),
        ..Default::default()
    };
    let result = Engine::default().run_source(
        "controls",
        r#"
Assert |@{ Split Lines |lines| }| Equals |expected|
Assert |@{ Trim String |whitespace| }| Equals |"hello"|
Assert |@{ String Length |nul| }| Equals |3|
Assert |@{ Slice String |nul| From |2| To |3| }| Equals |"🙂"|
"#,
        options,
    );
    assert!(result.result.is_ok(), "{:?}", result.result);
}

#[test]
fn malformed_templates_and_missing_fields_report_bounded_typed_errors() {
    for (template, values) in [
        ("{", "[]"),
        ("}", "[]"),
        ("{0", "[1]"),
        ("{x{y}", "{}"),
        ("{}", "[]"),
        ("{1}", "[1]"),
        ("{-1}", "[1]"),
        ("{0:x}", "[1]"),
        ("{ 0}", "[1]"),
        ("{१}", "[1]"),
        ("{999999999999999999999999999999}", "[1]"),
        ("{missing}", "{}"),
    ] {
        let source = format!(
            "Format String |{}| With |{values}|",
            serde_json::to_string(template).unwrap()
        );
        let error = run(&source).result.unwrap_err();
        assert_eq!(error.code(), Code::IncompatibleType, "{source}: {error}");
        assert_eq!(error.call_stack.len(), 1);
        assert_eq!(
            error.span.as_ref().unwrap().source().name(),
            "strings.botwork"
        );
    }
}

#[test]
fn parameters_separators_ranges_and_regex_syntax_fail_without_coercion() {
    for source in [
        "Format String |1| With |[]|",
        "Format String |\"x\"| With |1|",
        "Join Strings |{}| With |\",\"|",
        "Join Strings |[\"a\", 1]| With |\",\"|",
        "Join Strings |[]| With |1|",
        "Split String |1| On |\",\"|",
        "Split String |\"x\"| On |\"\"|",
        "Split Lines |[]|",
        "Replace String |\"x\"| Find |\"\"| With |\"y\"|",
        "Replace String |\"x\"| Find |1| With |\"y\"|",
        "Replace String |\"x\"| Find |\"x\"| With |1|",
        "String Contains |1| Text |\"a\"|",
        "String Starts With |\"a\"| Prefix |1|",
        "String Ends With |1| Suffix |\"a\"|",
        "String Length |[]|",
        "Slice String |\"é\"| From |0.0| To |1|",
        "Slice String |\"é\"| From |0| To |2|",
        "Slice String |\"a\"| From |-1| To |1|",
        "Slice String |\"a\"| From |1| To |0|",
        "Trim String |1|",
        "Uppercase String |false|",
        "Lowercase String |[]|",
        "String Matches |1| Regex |\"x\"|",
        "String Matches |\"x\"| Regex |1|",
        "Find Matches In |\"x\"| Regex |\"[\"|",
        "Capture From |\"x\"| Regex |\"(?=x)\"|",
        "String Matches |\"aa\"| Regex |\"(a)\\\\1\"|",
    ] {
        let error = run(source).result.unwrap_err();
        assert_eq!(error.code(), Code::IncompatibleType, "{source}: {error}");
    }
}

#[test]
fn failures_preserve_assignments_and_support_catch_and_finally() {
    let result = run(r#"
|destination| = |"original"|
Try { |destination| = Format String |"{absent}"| With |{}| }
Catch |error| { Assert |error.code| Equals |"BW3003"| }
Finally { |cleaned| = |true| }
Assert |destination| Equals |"original"|
Assert |cleaned|
"#);
    assert!(result.result.is_ok(), "{:?}", result.result);
    for source in [
        "Join Strings |[@{ Fail |\"first\"| }]| With |\",\"|",
        "Replace String |\"a\"| Find |\"\"| With |@{ Fail |\"first\"| }|",
    ] {
        assert_eq!(
            run(source).result.unwrap_err().code(),
            Code::ExplicitFailure
        );
    }
}

#[test]
fn output_limits_admit_only_complete_string_and_array_results() {
    for (source, values) in [
        (
            "Format String |\"{0}{0}\"| With |[\"abcd\"]|",
            ValueLimits {
                string_bytes: 7,
                ..Default::default()
            },
        ),
        (
            "Join Strings |[\"abcd\", \"abcd\"]| With |\",\"|",
            ValueLimits {
                string_bytes: 8,
                ..Default::default()
            },
        ),
        (
            "Replace String |\"aaa\"| Find |\"a\"| With |\"123\"|",
            ValueLimits {
                string_bytes: 8,
                ..Default::default()
            },
        ),
        (
            "Uppercase String |\"ΐ\"|",
            ValueLimits {
                string_bytes: 5,
                ..Default::default()
            },
        ),
        (
            "Lowercase String |\"İ\"|",
            ValueLimits {
                string_bytes: 2,
                ..Default::default()
            },
        ),
        (
            "Split String |\"a,b,c\"| On |\",\"|",
            ValueLimits {
                entries: 2,
                ..Default::default()
            },
        ),
        (
            "Split Lines |\"a\nb\nc\"|",
            ValueLimits {
                nodes: 3,
                ..Default::default()
            },
        ),
        (
            "Find Matches In |\"abc\"| Regex |\".\"|",
            ValueLimits {
                entries: 2,
                ..Default::default()
            },
        ),
        (
            "Capture From |\"abc\"| Regex |\"(a)(b)(c)\"|",
            ValueLimits {
                entries: 3,
                ..Default::default()
            },
        ),
    ] {
        let result = Engine::default().run_source(
            "quota",
            source,
            RunOptions {
                limits: RunLimits {
                    values,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(
            result.outcome(),
            RunOutcome::LimitExceeded,
            "{source}: {:?}",
            result.result
        );
    }
}

#[test]
fn temporary_limits_include_inputs_and_admitted_output_at_the_same_time() {
    for (source, payload) in [
        ("Join Strings |[\"ab\", \"c\"]| With |\",\"|", 8),
        ("Replace String |\"aaa\"| Find |\"a\"| With |\"xy\"|", 12),
        ("Format String |\"{0}\"| With |[\"abcd\"]|", 11),
        ("Split String |\"a,b\"| On |\",\"|", 6),
        ("Lowercase String |\"İ\"|", 5),
        ("Find Matches In |\"a1b2\"| Regex |\"[0-9]\"|", 11),
        ("Capture From |\"ab\"| Regex |\"(a)(b)\"|", 12),
    ] {
        for allowed in [false, true] {
            let result = Engine::default().run_source(
                "temporary",
                source,
                RunOptions {
                    limits: RunLimits {
                        temporaries: TemporaryLimits {
                            payload_bytes: payload - usize::from(!allowed),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            assert_eq!(
                result.result.is_ok(),
                allowed,
                "{source}: {:?}",
                result.result
            );
            if !allowed {
                assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
            }
        }
    }
}

#[tokio::test]
async fn async_regex_uses_worker_snapshot_admission_while_literal_operations_stay_inline() {
    for (source, entries, expected) in [
        ("String Contains |\"abc\"| Text |\"b\"|", 93, None),
        (
            "String Matches |\"abc\"| Regex |\"b\"|",
            93,
            Some(Code::ResourceLimit),
        ),
        ("String Matches |\"abc\"| Regex |\"b\"|", 94, None),
        (
            "Find Matches In |\"a\"| Regex |\"[\"|",
            93,
            Some(Code::ResourceLimit),
        ),
        (
            "Find Matches In |\"a\"| Regex |\"[\"|",
            94,
            Some(Code::IncompatibleType),
        ),
        (
            "Capture From |\"a\"| Regex |\"a\"|",
            93,
            Some(Code::ResourceLimit),
        ),
    ] {
        let result = Engine::default()
            .run_source_async(
                "worker",
                source,
                RunOptions {
                    limits: RunLimits {
                        snapshots: SnapshotLimits {
                            entries,
                            path_bytes: usize::MAX,
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(
            result.result.as_ref().err().map(|e| e.code()),
            expected,
            "{source}: {:?}",
            result.result
        );
    }
}

#[test]
fn regex_pattern_compilation_and_repeated_search_budgets_are_enforced() {
    for (text, pattern, code, resource) in [
        (
            "a".into(),
            "a".repeat(16385),
            Code::ResourceLimit,
            "regex pattern bytes",
        ),
        (
            "a".into(),
            "a{1000000}".into(),
            Code::ResourceLimit,
            "regex compiled bytes",
        ),
        (
            "a".repeat(6000),
            "a".into(),
            Code::ResourceLimit,
            "regex search bytes",
        ),
        (
            "a".into(),
            format!("{}a{}", "(".repeat(70), ")".repeat(70)),
            Code::IncompatibleType,
            "Invalid regex",
        ),
    ] {
        let source = format!(
            "Find Matches In |{}| Regex |{}|",
            serde_json::to_string(&text).unwrap(),
            serde_json::to_string(&pattern).unwrap()
        );
        let error = run(&source).result.unwrap_err();
        assert_eq!(error.code(), code, "{error}");
        assert!(error.to_string().contains(resource), "{error}");
    }
}

#[test]
fn string_metadata_registration_is_idempotent_and_preserves_overrides() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let mut context = Context::default();
    context
        .register_native("Trim String |text|", |_| Ok(Literal::Int(42)))
        .unwrap();
    context.init_statements();
    context.init_statements();
    assert_eq!(context.statement_signatures().len(), 93);
    let result = evaluate_program_detailed(
        &Program::parse("override", "Trim String |\"x\"|").unwrap(),
        &mut context,
    )
    .unwrap();
    assert!(matches!(result, Literal::Int(42)));
    for (header, fragments) in [
        (
            "Format String |t| With |v|",
            vec![
                "template: String",
                "values: Array | Map",
                "returns: String",
                "BW3003",
            ],
        ),
        (
            "String Length |t|",
            vec!["text: String", "returns: Int", "BW3002"],
        ),
        (
            "Capture From |t| Regex |r|",
            vec!["pattern: String", "returns: Array", "BW8001"],
        ),
    ] {
        let help = context.statement_signature(header).unwrap().unwrap().help();
        for fragment in fragments {
            assert!(help.contains(fragment), "{help}");
        }
    }
}
