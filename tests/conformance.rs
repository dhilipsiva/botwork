#[path = "support/cli_harness.rs"]
mod cli_harness;
#[path = "conformance/cases.rs"]
mod corpus;

use botwork::core::grammar::{BWErr, Literal, Operate, Rule};
use cli_harness::Harness;
use corpus::{cases, Case, Input};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    time::Duration,
};

const SPECIFICATION: &str = include_str!("../docs/language-specification.md");

fn rule_ids(specification: &str) -> Result<BTreeSet<&str>, String> {
    let mut rules = BTreeSet::new();
    for line in specification.lines() {
        if let Some((id, _)) = line
            .strip_prefix("**")
            .and_then(|line| line.split_once(" — "))
        {
            if !rules.insert(id) {
                return Err(format!("duplicate specification rule: {id}"));
            }
        }
    }
    if rules.is_empty() {
        return Err("specification has no rule IDs".into());
    }
    Ok(rules)
}

fn inventory(cases: &[Case], specification: &str) -> Result<(), String> {
    let rules = rule_ids(specification)?;
    let mut coverage: BTreeMap<_, [bool; 3]> = rules.iter().map(|id| (*id, [false; 3])).collect();
    let mut names = BTreeSet::new();
    for case in cases {
        if !names.insert(case.id) {
            return Err(format!("duplicate case: {}", case.id));
        }
        if case.error.is_some() != !case.invalid.is_empty() {
            return Err(format!("{}: invalid cases must expect an error", case.id));
        }
        if case.error.is_some() != case.code.is_some() {
            return Err(format!("{}: error expectations require a code", case.id));
        }
        let mut count = 0;
        for (category, ids) in [case.positive, case.invalid, case.boundary]
            .iter()
            .enumerate()
        {
            let mut seen = BTreeSet::new();
            for id in *ids {
                if !seen.insert(id) {
                    return Err(format!("{}: duplicate rule {id}", case.id));
                }
                let present = coverage
                    .get_mut(id)
                    .ok_or_else(|| format!("{}: unknown rule {id}", case.id))?;
                present[category] = true;
                count += 1;
            }
        }
        if count == 0 {
            return Err(format!("{}: case covers no rules", case.id));
        }
    }
    for (id, present) in coverage {
        for (index, category) in ["positive", "invalid", "boundary"].iter().enumerate() {
            if !present[index] {
                return Err(format!("{id}: missing {category} evidence"));
            }
        }
    }
    Ok(())
}

#[test]
fn corpus_covers_each_specified_rule_and_registers_every_fixture() {
    let cases = cases();
    inventory(&cases, SPECIFICATION).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "botwork")
        {
            let name = path.file_stem().unwrap().to_str().unwrap();
            let case = cases
                .iter()
                .find(|case| case.id == name)
                .unwrap_or_else(|| panic!("unregistered corpus fixture: {name}"));
            let Input::Script(source) = case.input else {
                panic!("script fixture must run through CLI")
            };
            assert_eq!(source, fs::read_to_string(path).unwrap());
        }
    }
}

#[test]
fn inventory_rejects_new_rules_gaps_unknown_tags_duplicates_and_wrong_expectations() {
    assert!(inventory(
        &cases(),
        &format!("{SPECIFICATION}\n**X1 — New rule.** Text.")
    )
    .unwrap_err()
    .contains("X1"));
    let mut missing = cases();
    missing.retain(|case| case.id != "structural-host-invalid");
    assert_eq!(
        inventory(&missing, SPECIFICATION).unwrap_err(),
        "V6: missing invalid evidence"
    );
    let mut unknown = cases();
    unknown[0].positive = &["X1"];
    assert!(inventory(&unknown, SPECIFICATION)
        .unwrap_err()
        .contains("unknown rule X1"));
    let mut duplicate = cases();
    duplicate.push(duplicate[0].clone());
    assert!(inventory(&duplicate, SPECIFICATION)
        .unwrap_err()
        .contains("duplicate case"));
    let mut repeated_rule = cases();
    repeated_rule[0].positive = &["E2", "E2"];
    assert!(inventory(&repeated_rule, SPECIFICATION)
        .unwrap_err()
        .contains("duplicate rule"));
    let mut wrong_status = cases();
    wrong_status[0].error = Some("unexpected");
    assert!(inventory(&wrong_status, SPECIFICATION).is_err());
    let mut missing_code = cases();
    missing_code.last_mut().unwrap().code = None;
    assert!(inventory(&missing_code, SPECIFICATION)
        .unwrap_err()
        .contains("require a code"));
    assert!(rule_ids("no rules").is_err());
    assert!(rule_ids("**E1 — First.**\n**E1 — Duplicate.**").is_err());
}

fn check_host_case(case: &Case) {
    // Source literals cannot produce non-finite values. Exercise the public Rust
    // operator API on both sides, including nested values and mismatched shapes.
    for number in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for invalid in [
            Literal::Float(number),
            Literal::Array(vec![Literal::Int(1), Literal::Float(number)]),
            Literal::Map(
                [(
                    "nested".into(),
                    Literal::Array(vec![Literal::Float(number)]),
                )]
                .into_iter()
                .collect(),
            ),
        ] {
            for operator in [Rule::equal, Rule::not_equal] {
                for (left, right) in [
                    (invalid.clone(), Literal::None),
                    (Literal::None, invalid.clone()),
                ] {
                    let error = operator.operate_binary(left, right).unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(
                        matches!(error, BWErr::ArithmeticError(_)),
                        "{}: {error}",
                        case.id
                    );
                    assert!(error.to_string().contains(case.error.unwrap()));
                }
            }
        }
    }
}

fn check_native_case(case: &Case) {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let mut context = Context::default();
    let value = match case.input {
        Input::NativeReturn => Literal::Array(vec![Literal::None, Literal::Int(i32::MIN)]),
        Input::NativeInvalidReturn => Literal::Array(vec![Literal::Float(f32::INFINITY)]),
        _ => unreachable!(),
    };
    context
        .register_native("Host", move |arguments| {
            assert!(arguments.is_empty());
            Ok(value.clone())
        })
        .unwrap();
    let result = evaluate_program_detailed(
        &Program::parse("native-corpus.botwork", "|result| = Host").unwrap(),
        &mut context,
    );
    match case.error {
        Some(expected) => {
            let error = result.unwrap_err();
            assert_eq!(error.code().as_str(), case.code.unwrap());
            assert!(error.to_string().contains(expected));
            assert_eq!(error.call_stack[0].signature, "host");
        }
        None => assert_eq!(result.unwrap().to_string(), "[none, -2147483648]"),
    }
}

fn check_signature_case(case: &Case) {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        signature::{StatementSignature, ValueKind, ValueKinds},
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let count = Arc::new(AtomicUsize::new(0));
    let invoked = Arc::clone(&count);
    let mut context = Context::default();
    let signature = StatementSignature::native("Host |value|")
        .unwrap()
        .parameter(
            "value",
            ValueKinds::one(ValueKind::Int).union(ValueKind::Float.into()),
        )
        .unwrap()
        .returns(ValueKind::Int);
    let expected_help = signature.help();
    context
        .register_native_with_signature(signature, move |_| {
            invoked.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(7))
        })
        .unwrap();
    assert_eq!(
        context
            .statement_signature("h o s t |other|")
            .unwrap()
            .unwrap()
            .help(),
        expected_help
    );
    let source = if case.error.is_some() {
        "Host |true|"
    } else {
        "Host |1.0|"
    };
    let result = evaluate_program_detailed(
        &Program::parse("signature-corpus.botwork", source).unwrap(),
        &mut context,
    );
    match case.error {
        Some(expected) => {
            let error = result.unwrap_err();
            assert_eq!(error.code().as_str(), case.code.unwrap());
            assert!(error.to_string().contains(expected));
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(error.call_stack.is_empty());
        }
        None => {
            assert!(matches!(result, Ok(Literal::Int(7))));
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
    }
}

fn check_async_case(case: &Case) {
    use botwork::core::{
        operation::{NativeOperation, OperationControl},
        signature::StatementSignature,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&count);
        let operation = NativeOperation::asynchronous(
            StatementSignature::native("Host").unwrap(),
            move |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(Literal::Int(7)) }
            },
        )
        .unwrap();
        let mut control = OperationControl::default();
        if case.error.is_some() {
            control = control.child(Some(tokio::time::Instant::now()));
        }
        let result = operation.invoke(vec![], control).await;
        match case.error {
            Some(expected) => {
                let error = result.unwrap_err();
                assert_eq!(error.code().as_str(), case.code.unwrap());
                assert!(error.to_string().contains(expected));
                assert_eq!(count.load(Ordering::SeqCst), 0);
            }
            None => {
                assert!(matches!(result, Ok(Literal::Int(7))));
                assert_eq!(count.load(Ordering::SeqCst), 1);
            }
        }
    });
}

#[test]
fn conformance_inputs_match_status_stdout_and_error_contracts() {
    let cases = cases();
    inventory(&cases, SPECIFICATION).unwrap();
    let harness = Harness::new();
    for case in cases {
        let mut arguments = vec![];
        let source = match case.input {
            Input::Script(source) => source,
            Input::NonFiniteHost => {
                check_host_case(&case);
                continue;
            }
            Input::NativeReturn | Input::NativeInvalidReturn => {
                check_native_case(&case);
                continue;
            }
            Input::SignatureValid | Input::SignatureInvalid => {
                check_signature_case(&case);
                continue;
            }
            Input::AsyncSuccess | Input::AsyncExpired => {
                check_async_case(&case);
                continue;
            }
            Input::EmbeddedSuccess | Input::EmbeddedLimit => {
                check_embedded_case(&case);
                continue;
            }
            Input::RuntimeBoundary => {
                use botwork::core::{
                    ast::Program,
                    diagnostic::DiagnosticCode,
                    eval::{evaluate_program_detailed, Context},
                    run::RunLimits,
                };
                let mut context = Context::with_limits(RunLimits {
                    steps: 4,
                    evaluation_depth: 3,
                    ..RunLimits::default()
                })
                .unwrap();
                let program = Program::parse("runtime-boundary", "|x| = |1+2|").unwrap();
                assert_eq!(
                    evaluate_program_detailed(&program, &mut context)
                        .unwrap()
                        .to_string(),
                    "3"
                );
                assert_eq!(
                    evaluate_program_detailed(&program, &mut context)
                        .unwrap_err()
                        .code(),
                    DiagnosticCode::ResourceLimit
                );
                continue;
            }
            Input::AstBoundary | Input::AstLimit => {
                use botwork::core::{
                    ast::Program,
                    ast_limits::AstLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let mut program = Program::parse("ast-corpus", "|x| = |1|").unwrap();
                program.statements.push(program.statements[0].clone());
                let run = Engine::default().run_program(
                    &program,
                    RunOptions {
                        limits: RunLimits {
                            ast: AstLimits {
                                nodes: if case.error.is_some() { 5 } else { 6 },
                                depth: 2,
                                source_bytes: program.source.text().len(),
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                if let Some(expected) = case.error {
                    let error = run.result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert!(run.variables.is_empty());
                    assert_eq!(run.steps, 0);
                } else {
                    assert_eq!(run.result.unwrap().to_string(), "1");
                    assert_eq!(run.steps, 4);
                }
                continue;
            }
            Input::ImportSuccess => {
                fs::create_dir_all(harness.workspace.join("modules")).unwrap();
                fs::write(
                    harness.workspace.join("modules/arithmetic.botwork"),
                    include_str!("../examples/modules/arithmetic.botwork"),
                )
                .unwrap();
                include_str!("../examples/19-local-imports.botwork")
            }
            Input::ValueBoundary | Input::ValueLimit => {
                use botwork::core::{
                    run::{Engine, RunLimits, RunOptions},
                    value_limits::ValueLimits,
                };
                let run = Engine::default().run_source(
                    "value-corpus",
                    "|out| = |x|",
                    RunOptions {
                        variables: BTreeMap::from([(
                            "x".into(),
                            Literal::Array(vec![Literal::Int(1), Literal::Int(2)]),
                        )]),
                        limits: RunLimits {
                            values: ValueLimits {
                                nodes: 3,
                                depth: 2,
                                payload_bytes: 8,
                                entries: if case.error.is_some() { 1 } else { 2 },
                                ..ValueLimits::default()
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                if let Some(expected) = case.error {
                    let error = run.result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert!(run.variables.is_empty());
                    assert_eq!(run.steps, 0);
                } else {
                    assert_eq!(run.result.unwrap().to_string(), "[1, 2]");
                    assert_eq!(run.steps, 2);
                }
                continue;
            }
            Input::ImportBudgetBoundary | Input::ImportBudgetLimit => {
                use botwork::core::run::{Engine, ImportLimits, RunLimits, RunOptions};
                let module = "Read { Return |7| }";
                fs::write(harness.workspace.join("bounded.botwork"), module).unwrap();
                let run = Engine::default().run_source("root", "Import |\"bounded.botwork\"| As |a|\nImport |\"bounded.botwork\"| As |b|\n|value| = b::Read", RunOptions {
                    working_directory: Some(harness.workspace.clone()),
                    limits: RunLimits { imports: ImportLimits {
                        loads:1, source_bytes:module.len(), paths:1,
                        bindings: if case.error.is_some() {3} else {4}, ..ImportLimits::default()
                    }, ..RunLimits::default() }, ..RunOptions::default()
                });
                if let Some(expected) = case.error {
                    let error = run.result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert!(!run.variables.contains_key("value"));
                } else {
                    assert_eq!(run.result.unwrap().to_string(), "7");
                    assert_eq!(run.variables["value"].to_string(), "7");
                }
                continue;
            }
            Input::ImportCycle => {
                fs::write(
                    harness.workspace.join("cycle.botwork"),
                    "Import |\"cycle.botwork\"| As |again|",
                )
                .unwrap();
                "Import |\"cycle.botwork\"| As |cycle|"
            }
            Input::VariablesSuccess => {
                fs::write(
                    harness.workspace.join("variables.json"),
                    r#"{"x":1,"min":-2147483648,"max":2147483647,"nothing":null,"yes":true}"#,
                )
                .unwrap();
                arguments = vec!["--var", "x=2", "--vars-file", "variables.json"];
                "Log |[x, min, max, nothing, yes]|"
            }
            Input::VariablesInvalid => {
                fs::write(
                    harness.workspace.join("variables-invalid.json"),
                    r#"{"x":2147483648}"#,
                )
                .unwrap();
                arguments = vec!["--vars-file", "variables-invalid.json", "--debug"];
                "Log |\"unreachable\"|"
            }
        };
        let output = if arguments.is_empty() {
            harness.run(case.id, source, Duration::from_secs(5))
        } else {
            harness.run_with_args(case.id, source, &arguments, Duration::from_secs(5))
        }
        .unwrap_or_else(|error| panic!("{error}"));
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        let label = format!(
            "{}; {}; timeout=5s\n{source}\n{diagnostic}",
            case.id, harness.environment
        );
        assert_eq!(output.stdout, case.stdout.as_bytes(), "{label}");
        match case.error {
            Some(expected) => {
                assert!(
                    diagnostic.contains(&format!("[{}]", case.code.unwrap())),
                    "{label}"
                );
                assert_eq!(output.status.code(), Some(1), "{label}");
                assert!(
                    diagnostic.contains(expected),
                    "expected {expected}: {label}"
                );
                let extension = if matches!(case.input, Input::VariablesInvalid) {
                    "json"
                } else {
                    "botwork"
                };
                assert!(
                    diagnostic.contains(&format!("{}.{extension}", case.id)),
                    "{label}"
                );
                assert!(!diagnostic.contains("panicked"), "{label}");
            }
            None => {
                assert_eq!(output.status.code(), Some(0), "{label}");
                assert!(diagnostic.is_empty(), "{label}");
            }
        }
    }
}

fn check_embedded_case(case: &Case) {
    use botwork::core::run::{Engine, RunLimits, RunOptions, RunOutcome};
    let engine = Engine::default();
    let run = engine.run_source(
        "embedded.botwork",
        "|result| = |input|",
        RunOptions {
            variables: BTreeMap::from([("input".into(), Literal::Int(7))]),
            limits: RunLimits {
                steps: if case.error.is_some() { 1 } else { 2 },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    match case.error {
        Some(expected) => {
            assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
            assert!(!run.variables.contains_key("result"));
            let error = run.result.unwrap_err();
            assert_eq!(error.code().as_str(), case.code.unwrap());
            assert!(error.to_string().contains(expected));
        }
        None => {
            assert_eq!(run.outcome(), RunOutcome::Succeeded);
            assert_eq!(run.result.unwrap().to_string(), "7");
            assert_eq!(run.variables["result"].to_string(), "7");
            assert_eq!(run.steps, 2);
            assert!(engine
                .run_source("fresh", "", RunOptions::default())
                .variables
                .is_empty());
        }
    }
}
