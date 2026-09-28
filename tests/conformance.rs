#[path = "support/cli_harness.rs"]
mod cli_harness;
#[path = "conformance/cases.rs"]
mod corpus;
#[path = "support/worker_panic.rs"]
mod worker_panic;

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

fn check_async_program_case(case: &Case) {
    use botwork::core::{
        operation::NativeOperation,
        run::{Engine, RunOptions, RunOutcome},
        signature::StatementSignature,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::clone(&calls);
    let mut engine = Engine::default();
    engine
        .register_operation(
            NativeOperation::asynchronous(
                StatementSignature::native("Later |value|").unwrap(),
                move |mut values, _| {
                    entered.fetch_add(1, Ordering::SeqCst);
                    async move {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                        Ok(values.remove(0))
                    }
                },
            )
            .unwrap(),
        )
        .unwrap();
    let source = "Double |n| { Return |@{ Later |n| } * 2| }\n|answer| = Double |21|";
    if let Some(expected) = case.error {
        let result = engine.run_source("sync", source, RunOptions::default());
        let error = result.result.unwrap_err();
        assert_eq!(error.code().as_str(), case.code.unwrap());
        assert!(error.to_string().contains(expected));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(result.variables.is_empty());
    } else {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let result =
            runtime.block_on(engine.run_source_async("async", source, RunOptions::default()));
        assert_eq!(result.outcome(), RunOutcome::Succeeded);
        assert!(matches!(result.variables["answer"], Literal::Int(42)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
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
            Input::NamespaceBoundary | Input::NamespaceHelperFailure => {
                check_namespace_case(&case);
                continue;
            }
            Input::JournalBoundary | Input::JournalLimit => {
                check_journal_case(&case);
                continue;
            }
            Input::TreeBoundary | Input::TreeUnverified => {
                check_tree_case(&case);
                continue;
            }
            Input::ProgressBoundary | Input::ProgressIncomplete => {
                check_progress_case(&case);
                continue;
            }
            Input::ShutdownBoundary | Input::ShutdownInvalid => {
                check_shutdown_case(&case);
                continue;
            }
            Input::ProtocolBoundary | Input::ProtocolLimit => {
                check_protocol_case(&case);
                continue;
            }
            Input::WorkerBoundary | Input::WorkerLimit => {
                check_worker_case(&case);
                continue;
            }
            Input::OutputBoundary | Input::OutputLimit => {
                arguments.extend([
                    "--max-output-bytes",
                    if case.error.is_some() { "2" } else { "3" },
                ]);
                "Log |\"é\"|"
            }
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
            Input::AsyncProgram | Input::AsyncProgramSyncRejected => {
                check_async_program_case(&case);
                continue;
            }
            Input::DiagnosticRenderingBoundary | Input::DiagnosticRenderingLimit => {
                use botwork::core::diagnostic::{
                    Diagnostic, DiagnosticCode, DiagnosticRenderLimits, RENDER_SUMMARY_BYTES,
                };
                let error = Diagnostic::new(BWErr::NativeError("é".into()));
                let complete = error.to_string();
                let rendered = error.render_with_limits(&DiagnosticRenderLimits {
                    output_bytes: complete.len() - usize::from(case.error.is_some()),
                    ..Default::default()
                });
                if let Some(expected) = case.error {
                    assert!(rendered.truncation.is_some());
                    assert!(rendered.text.contains(expected));
                    assert!(rendered
                        .text
                        .starts_with(&format!("[{}]", case.code.unwrap())));
                    assert!(rendered.text.len() <= RENDER_SUMMARY_BYTES);
                } else {
                    assert!(rendered.truncation.is_none());
                    assert_eq!(rendered.text, complete);
                }
                assert_eq!(error.code(), DiagnosticCode::Native);
                continue;
            }
            Input::OperationOwnershipBoundary | Input::OperationOwnershipLimit => {
                use botwork::core::{
                    operation::{
                        NativeOperation, OperationBudget, OperationControl,
                        OperationOwnershipLimits, OperationUsage,
                    },
                    signature::StatementSignature,
                };
                let budget = OperationBudget::new(OperationOwnershipLimits {
                    invocations: 1,
                    values: 2,
                    nodes: 2,
                    payload_bytes: 8 - usize::from(case.error.is_some()),
                    ..OperationOwnershipLimits::default()
                });
                let operation = NativeOperation::asynchronous(
                    StatementSignature::native("Echo |value|").unwrap(),
                    |mut values, _| async move { Ok(values.pop().unwrap()) },
                )
                .unwrap()
                .with_ownership_budget(budget.clone());
                let invocation =
                    operation.invoke(vec![Literal::Int(7)], OperationControl::default());
                assert_eq!(budget.usage().invocations, 1);
                assert_eq!(budget.usage().payload_bytes, 4);
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .unwrap();
                let result = runtime.block_on(invocation);
                if let Some(expected) = case.error {
                    let error = result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                } else {
                    assert_eq!(result.unwrap().to_string(), "7");
                }
                assert_eq!(budget.usage(), OperationUsage::default());
                continue;
            }
            Input::OperationDiagnosticBoundary
            | Input::OperationDiagnosticLimit
            | Input::OperationPanicBoundary
            | Input::OperationPanicLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    operation::{NativeOperation, OperationControl},
                    signature::StatementSignature,
                };
                let panics = matches!(
                    case.input,
                    Input::OperationPanicBoundary | Input::OperationPanicLimit
                );
                let expected_code = if panics { "BW4003" } else { "BW4002" };
                let bytes = if panics { 10 } else { 12 };
                let operation = NativeOperation::asynchronous(
                    StatementSignature::native("Fail").unwrap(),
                    move |_, _| async move {
                        assert!(!panics, "corpus panic");
                        Err(BWErr::NativeError("reason".into()).into())
                    },
                )
                .unwrap()
                .with_diagnostic_limits(DiagnosticLimits {
                    text_bytes: bytes - usize::from(case.error.is_some()),
                    ..DiagnosticLimits::default()
                })
                .unwrap();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .unwrap();
                let error = runtime
                    .block_on(operation.invoke(vec![], OperationControl::default()))
                    .unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), expected_code);
                    assert!(error.causes[0].omissions.is_some());
                } else {
                    assert_eq!(error.code().as_str(), expected_code);
                    assert!(error.omissions.is_none());
                    assert_eq!(error.span.as_ref().unwrap().source().name(), "<native>");
                }
                continue;
            }
            Input::NumericDiagnosticBoundary | Input::NumericDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::{DiagnosticCode, DiagnosticLimits},
                    run::{Engine, RunLimits, RunOptions},
                };
                let error = Engine::default()
                    .run_source(
                        "numeric",
                        "Compute { Return |1 / 0| }\nCompute",
                        RunOptions {
                            limits: RunLimits {
                                diagnostics: DiagnosticLimits {
                                    // expression label + divide detail + compute frame
                                    text_bytes: 31 - usize::from(case.error.is_some()),
                                    ..DiagnosticLimits::default()
                                },
                                ..RunLimits::default()
                            },
                            ..RunOptions::default()
                        },
                    )
                    .result
                    .unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
                    let omitted = error.causes[0].omissions.as_ref().unwrap();
                    assert_eq!(omitted.call_frames, 1);
                    assert_eq!(omitted.detail_fields, 0);
                    assert!(!omitted.prior_summary);
                    assert!(error.causes[0].span.is_none());
                } else {
                    assert_eq!(error.code(), DiagnosticCode::Arithmetic);
                    assert!(
                        matches!(error.error.as_ref(), BWErr::ArithmeticError(reason) if reason == "divide by zero")
                    );
                    assert_eq!(error.span.as_ref().unwrap().text(), "1 / 0");
                    assert_eq!(error.call_stack[0].signature, "compute");
                    assert!(error.omissions.is_none());
                }
                continue;
            }
            Input::WorkerJoinBoundary | Input::WorkerJoinLimit => {
                use botwork::core::{
                    diagnostic::{DiagnosticCode, DiagnosticLimits},
                    operation::{NativeOperation, OperationControl},
                    signature::StatementSignature,
                };
                let operation = NativeOperation::blocking(
                    StatementSignature::native("Read").unwrap(),
                    std::num::NonZeroUsize::new(1).unwrap(),
                    |_, _| {
                        std::panic::panic_any(worker_panic::EscapingPanic(Some("join é\n".into())))
                    },
                )
                .unwrap()
                .with_diagnostic_limits(DiagnosticLimits {
                    source_bytes: 12 - usize::from(case.error.is_some()),
                    ..DiagnosticLimits::default()
                })
                .unwrap();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .unwrap();
                let error = runtime
                    .block_on(operation.invoke(vec![], OperationControl::default()))
                    .unwrap_err();
                let detail = if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code(), DiagnosticCode::AsyncRuntime);
                    assert!(error.causes[0].span.is_none());
                    let omitted = error.causes[0].omissions.as_ref().unwrap();
                    assert_eq!(omitted.detail_fields, 0);
                    assert!(!omitted.prior_summary);
                    error.causes[0].error.as_ref()
                } else {
                    assert_eq!(error.code(), DiagnosticCode::AsyncRuntime);
                    assert_eq!(error.span.as_ref().unwrap().text(), "Read");
                    assert!(error.omissions.is_none());
                    error.error.as_ref()
                };
                let BWErr::AsyncRuntime(detail) = detail else {
                    panic!("join category")
                };
                assert!(detail.starts_with("Blocking worker ended unexpectedly: task "));
                assert!(detail.contains("join é\\n"));
                continue;
            }
            Input::BorrowedDiagnosticBoundary | Input::BorrowedDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let error = Engine::default()
                    .run_source(
                        case.id,
                        "Missing",
                        RunOptions {
                            limits: RunLimits {
                                diagnostics: DiagnosticLimits {
                                    text_bytes: if case.error.is_some() { 12 } else { 13 },
                                    ..DiagnosticLimits::default()
                                },
                                ..RunLimits::default()
                            },
                            ..RunOptions::default()
                        },
                    )
                    .result
                    .unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW2002");
                    assert!(error.causes[0].omissions.is_some());
                } else {
                    assert_eq!(error.code().as_str(), "BW2002");
                    assert!(error.omissions.is_none());
                    assert_eq!(error.span.as_ref().unwrap().text(), "Missing");
                }
                continue;
            }
            Input::SignatureDiagnosticBoundary | Input::SignatureDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    operation::{NativeOperation, OperationControl},
                    signature::{StatementSignature, ValueKind},
                };
                let message =
                    "Parameter `value` (argument 1) of `read|param|` requires Int; got Bool";
                let operation = NativeOperation::asynchronous(
                    StatementSignature::native("Read |value|")
                        .unwrap()
                        .parameter("value", ValueKind::Int)
                        .unwrap(),
                    |_, _| async { panic!("rejected callback entered") },
                )
                .unwrap()
                .with_diagnostic_limits(DiagnosticLimits {
                    text_bytes: "source".len() + message.len() - usize::from(case.error.is_some()),
                    ..DiagnosticLimits::default()
                })
                .unwrap();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .unwrap();
                let error = runtime
                    .block_on(
                        operation.invoke(vec![Literal::Bool(true)], OperationControl::default()),
                    )
                    .unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW3003");
                    assert!(error.causes[0].omissions.is_some());
                } else {
                    assert_eq!(error.code().as_str(), "BW3003");
                    let BWErr::OperationIncompatibleError(detail) = error.error.as_ref() else {
                        panic!("category")
                    };
                    assert_eq!(detail, message);
                    assert!(error.omissions.is_none());
                }
                continue;
            }
            Input::AccessDiagnosticBoundary | Input::AccessDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let bytes = "expression".len()
                    + "data.missing".len()
                    + "missing".len()
                    + "map key does not exist".len();
                let error = Engine::default()
                    .run_source(
                        case.id,
                        "|data| = |{}|\n|out| = |data.missing|",
                        RunOptions {
                            limits: RunLimits {
                                diagnostics: DiagnosticLimits {
                                    text_bytes: bytes - usize::from(case.error.is_some()),
                                    ..DiagnosticLimits::default()
                                },
                                ..RunLimits::default()
                            },
                            ..RunOptions::default()
                        },
                    )
                    .result
                    .unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW3004");
                    assert!(error.causes[0].omissions.is_some());
                } else {
                    assert_eq!(error.code().as_str(), "BW3004");
                    let BWErr::CollectionAccessError {
                        path,
                        segment,
                        reason,
                    } = error.error.as_ref()
                    else {
                        panic!("category")
                    };
                    assert_eq!(
                        (path.as_str(), segment.as_str(), reason.as_str()),
                        ("data.missing", "missing", "map key does not exist")
                    );
                    assert!(error.omissions.is_none());
                }
                continue;
            }
            Input::OperatorDiagnosticBoundary | Input::OperatorDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let message = "Bool(true) plus Int(1)";
                let error = Engine::default()
                    .run_source(
                        case.id,
                        "|out| = |true + 1|",
                        RunOptions {
                            limits: RunLimits {
                                diagnostics: DiagnosticLimits {
                                    text_bytes: "expression".len() + message.len()
                                        - usize::from(case.error.is_some()),
                                    ..DiagnosticLimits::default()
                                },
                                ..RunLimits::default()
                            },
                            ..RunOptions::default()
                        },
                    )
                    .result
                    .unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW3003");
                    assert!(error.causes[0].omissions.is_some());
                } else {
                    assert_eq!(error.code().as_str(), "BW3003");
                    let BWErr::OperationIncompatibleError(detail) = error.error.as_ref() else {
                        panic!("category")
                    };
                    assert_eq!(detail, message);
                    assert!(error.omissions.is_none());
                }
                continue;
            }
            Input::HostInputDiagnosticBoundary | Input::HostInputDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let message = "host variables: variable \"true\": expected an exact DSL identifier";
                let run = Engine::default().run_source(
                    case.id,
                    "|effect| = |1|",
                    RunOptions {
                        variables: BTreeMap::from([
                            ("a".into(), Literal::Int(1)),
                            ("true".into(), Literal::None),
                        ]),
                        limits: RunLimits {
                            diagnostics: DiagnosticLimits {
                                text_bytes: "source".len() + message.len()
                                    - usize::from(case.error.is_some()),
                                ..DiagnosticLimits::default()
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                assert!(run.variables.is_empty());
                assert_eq!(run.steps, 0);
                let error = run.result.unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW7001");
                    assert!(error.causes[0].omissions.is_some());
                } else {
                    assert_eq!(error.code().as_str(), "BW7001");
                    let BWErr::InputError(detail) = error.error.as_ref() else {
                        panic!("category")
                    };
                    assert_eq!(detail, message);
                    assert!(error.span.is_none() && error.omissions.is_none());
                }
                continue;
            }
            Input::HostInputOriginBoundary | Input::HostInputOriginLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    input::{parse_variables_with_limits, InputLimits},
                };
                let maximum = DiagnosticLimits::default().source_bytes;
                let origin = "x".repeat(maximum + usize::from(case.error.is_some()));
                let error = parse_variables_with_limits(
                    &origin,
                    "{}",
                    &InputLimits {
                        sources: 0,
                        ..InputLimits::default()
                    },
                )
                .unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert!(matches!(
                        error.causes[0].error.as_ref(),
                        BWErr::ResourceLimit {
                            resource: "input sources",
                            limit: 0
                        }
                    ));
                    let evidence = error.causes[0]
                        .omissions
                        .as_ref()
                        .unwrap()
                        .source
                        .as_ref()
                        .unwrap();
                    assert!(evidence.file_truncated && evidence.file.len() <= 256);
                    assert!(error.causes[0].span.is_none());
                } else {
                    assert!(matches!(
                        error.error.as_ref(),
                        BWErr::ResourceLimit {
                            resource: "input sources",
                            limit: 0
                        }
                    ));
                    assert!(error.causes.is_empty() && error.omissions.is_none());
                    let span = error.span.as_ref().unwrap();
                    assert_eq!(span.source().name(), origin);
                    assert_eq!(span.source().text(), "");
                    assert_eq!(span.line_column(), (1, 1));
                }
                continue;
            }
            Input::SignatureBuilderBoundary | Input::SignatureBuilderLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    signature::{StatementSignature, ValueKind},
                };
                let maximum = DiagnosticLimits::default().text_bytes;
                let overhead = "source".len() + "Unknown parameter `` in `Read |value|`".len();
                let name = "x".repeat(maximum - overhead + usize::from(case.error.is_some()));
                let error = StatementSignature::native("Read |value|")
                    .unwrap()
                    .parameter(&name, ValueKind::Int)
                    .unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW1004");
                    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
                    assert!(error.causes[0].span.is_none());
                } else {
                    assert_eq!(error.code().as_str(), "BW1004");
                    assert_eq!(
                        DiagnosticLimits::default()
                            .check(&error)
                            .unwrap()
                            .text_bytes,
                        maximum
                    );
                    assert_eq!(error.span.as_ref().unwrap().text(), "Read |value|");
                    assert!(error.omissions.is_none());
                }
                continue;
            }
            Input::EntryFileDiagnosticBoundary | Input::EntryFileDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let path = harness
                    .workspace
                    .join(format!("missing-{}.botwork", case.id));
                let engine = Engine::default();
                let baseline = engine
                    .run_file(&path, RunOptions::default())
                    .result
                    .unwrap_err();
                assert_eq!(baseline.code().as_str(), "BW7003");
                let bytes = DiagnosticLimits::default()
                    .check(&baseline)
                    .unwrap()
                    .text_bytes;
                let run = engine.run_file(
                    &path,
                    RunOptions {
                        limits: RunLimits {
                            diagnostics: DiagnosticLimits {
                                text_bytes: bytes - usize::from(case.error.is_some()),
                                ..DiagnosticLimits::default()
                            },
                            ..RunLimits::default()
                        },
                        variables: BTreeMap::from([("seed".into(), Literal::Int(7))]),
                        ..RunOptions::default()
                    },
                );
                assert_eq!(run.steps, 0);
                assert_eq!(run.variables["seed"].to_string(), "7");
                let error = run.result.unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW7003");
                    assert!(error.causes[0].span.is_none());
                } else {
                    assert_eq!(
                        error.to_value().to_string(),
                        baseline.to_value().to_string()
                    );
                }
                continue;
            }
            Input::ImportDiagnosticBoundary | Input::ImportDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let source = "Import |\"bad.txt\"| As |lib|";
                let message = "`bad.txt` must name a local .botwork file";
                let bytes = "source".len() + message.len() + "imported here".len();
                let run = Engine::default().run_source(
                    case.id,
                    source,
                    RunOptions {
                        limits: RunLimits {
                            diagnostics: DiagnosticLimits {
                                text_bytes: bytes - usize::from(case.error.is_some()),
                                ..DiagnosticLimits::default()
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                let error = run.result.unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW6001");
                    assert_eq!(
                        error.causes[0]
                            .omissions
                            .as_ref()
                            .unwrap()
                            .related_locations,
                        1
                    );
                    assert!(error.causes[0].span.is_none());
                } else {
                    assert_eq!(error.code().as_str(), "BW6001");
                    let BWErr::ImportRead(detail) = error.error.as_ref() else {
                        panic!("category")
                    };
                    assert_eq!(detail, message);
                    assert_eq!(error.span.as_ref().unwrap().text(), "\"bad.txt\"");
                    assert_eq!(error.related.len(), 1);
                    assert_eq!(error.related[0].message, "imported here");
                    assert_eq!(error.related[0].span.text(), source);
                    assert!(error.omissions.is_none());
                }
                continue;
            }
            Input::GuardDiagnosticBoundary | Input::GuardDiagnosticLimit => {
                use botwork::core::{
                    ast::Program,
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                    syntax_limits::SyntaxLimits,
                };
                let source = "Log |[[1]]|";
                let syntax = SyntaxLimits {
                    nesting: 2,
                    operators: 64,
                };
                let baseline =
                    Program::parse_bounded(case.id, source, source.len(), &syntax).unwrap_err();
                let size = DiagnosticLimits::default().check(&baseline).unwrap();
                let run = Engine::default().run_source(
                    case.id,
                    source,
                    RunOptions {
                        limits: RunLimits {
                            syntax,
                            diagnostics: DiagnosticLimits {
                                source_bytes: size.source_bytes - usize::from(case.error.is_some()),
                                ..DiagnosticLimits::default()
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                assert_eq!(run.steps, 0);
                let error = run.result.unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(
                        error.causes[0].error.to_string(),
                        baseline.error.to_string()
                    );
                    let omitted = error.causes[0].omissions.as_ref().unwrap();
                    assert_eq!(omitted.detail_fields, 0);
                    assert_eq!(omitted.source.as_ref().unwrap().start_byte, 6);
                    assert_eq!(omitted.source.as_ref().unwrap().end_byte, 7);
                    assert!(error.causes[0].span.is_none());
                } else {
                    assert_eq!(
                        error.to_value().to_string(),
                        baseline.to_value().to_string()
                    );
                    assert_eq!(error.span.as_ref().unwrap().source().text(), "Log |[[");
                }
                continue;
            }
            Input::SyntaxDiagnosticBoundary | Input::SyntaxDiagnosticLimit => {
                use botwork::core::{
                    ast::Program,
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let source = "Log |1|\n|é| = |1 +|";
                let baseline = Program::parse_detailed(case.id, source).unwrap_err();
                let size = DiagnosticLimits::default().check(&baseline).unwrap();
                let run = Engine::default().run_source(
                    case.id,
                    source,
                    RunOptions {
                        limits: RunLimits {
                            diagnostics: DiagnosticLimits {
                                text_bytes: size.text_bytes - usize::from(case.error.is_some()),
                                ..DiagnosticLimits::default()
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                assert_eq!(run.steps, 0);
                let error = run.result.unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW1001");
                    let omitted = error.causes[0].omissions.as_ref().unwrap();
                    assert_eq!(omitted.detail_fields, 1);
                    assert_eq!(
                        omitted.source.as_ref().unwrap().start_byte,
                        baseline.span.as_ref().unwrap().start()
                    );
                    let BWErr::ParsingError(detail) = error.causes[0].error.as_ref() else {
                        panic!("syntax")
                    };
                    assert!(detail.contains("source excerpt omitted"));
                    assert!(error.causes[0].span.is_none());
                } else {
                    assert_eq!(error.code().as_str(), "BW1001");
                    assert_eq!(
                        error.to_value().to_string(),
                        baseline.to_value().to_string()
                    );
                }
                continue;
            }
            Input::ValidationDiagnosticBoundary | Input::ValidationDiagnosticLimit => {
                use botwork::core::{
                    ast::Program,
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                for source in ["If |false| { Return }", "Read |é| with |é| {}"] {
                    let baseline = Program::parse_detailed(case.id, source).unwrap_err();
                    let size = DiagnosticLimits::default().check(&baseline).unwrap();
                    let run = Engine::default().run_source(
                        case.id,
                        source,
                        RunOptions {
                            limits: RunLimits {
                                diagnostics: DiagnosticLimits {
                                    text_bytes: size.text_bytes - usize::from(case.error.is_some()),
                                    ..DiagnosticLimits::default()
                                },
                                ..RunLimits::default()
                            },
                            ..RunOptions::default()
                        },
                    );
                    assert_eq!(run.steps, 0);
                    let error = run.result.unwrap_err();
                    if let Some(expected) = case.error {
                        assert_eq!(error.code().as_str(), case.code.unwrap());
                        assert!(error.to_string().contains(expected));
                        assert_eq!(error.causes[0].code(), baseline.code());
                        let omissions = error.causes[0].omissions.as_ref().unwrap();
                        assert_eq!(omissions.detail_fields, 1 + size.related_locations);
                        assert_eq!(omissions.related_locations, size.related_locations);
                        assert_eq!(
                            omissions.source.as_ref().unwrap().start_byte,
                            baseline.span.as_ref().unwrap().start()
                        );
                        assert!(error.causes[0].span.is_none());
                    } else {
                        assert_eq!(
                            error.to_value().to_string(),
                            baseline.to_value().to_string()
                        );
                    }
                }
                continue;
            }
            Input::CollisionDiagnosticBoundary | Input::CollisionDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let source = "Read {}\nRead {}";
                let original = format!("{}:1:1", case.id);
                let duplicate = format!("{}:2:1", case.id);
                let bytes = "source".len()
                    + "first definition".len()
                    + "read".len()
                    + original.len()
                    + duplicate.len();
                let run = Engine::default().run_source(
                    case.id,
                    source,
                    RunOptions {
                        limits: RunLimits {
                            diagnostics: DiagnosticLimits {
                                text_bytes: bytes - usize::from(case.error.is_some()),
                                ..DiagnosticLimits::default()
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                let error = run.result.unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW2003");
                    let omissions = error.causes[0].omissions.as_ref().unwrap();
                    assert_eq!(omissions.detail_fields, 2);
                    assert_eq!(omissions.related_locations, 1);
                    assert!(error.causes[0].span.is_none());
                    let BWErr::DuplicateStatement {
                        original,
                        duplicate,
                        ..
                    } = error.causes[0].error.as_ref()
                    else {
                        panic!("category")
                    };
                    assert_eq!(
                        original,
                        &format!("{}:[byte 0; coordinates omitted]", case.id)
                    );
                    assert_eq!(
                        duplicate,
                        &format!("{}:[byte 8; coordinates omitted]", case.id)
                    );
                } else {
                    assert_eq!(error.code().as_str(), "BW2003");
                    let BWErr::DuplicateStatement {
                        signature,
                        original: actual,
                        duplicate: second,
                    } = error.error.as_ref()
                    else {
                        panic!("category")
                    };
                    assert_eq!(signature, "read");
                    assert_eq!(actual, &original);
                    assert_eq!(second, &duplicate);
                    assert_eq!(error.related.len(), 1);
                    assert_eq!(error.related[0].message, "first definition");
                    assert!(error.omissions.is_none());
                }
                continue;
            }
            Input::SetupDiagnosticDefaults | Input::SetupDiagnosticLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RunLimits, RunOptions},
                };
                let mut options = RunOptions {
                    inherit_environment: false,
                    variables: BTreeMap::from([("seed".into(), Literal::Int(7))]),
                    limits: RunLimits {
                        diagnostics: DiagnosticLimits {
                            text_bytes: 0,
                            ..DiagnosticLimits::default()
                        },
                        ..RunLimits::default()
                    },
                    ..RunOptions::default()
                };
                if case.error.is_some() {
                    options.working_directory =
                        Some("x".repeat(DiagnosticLimits::default().text_bytes).into());
                } else {
                    options
                        .environment
                        .insert("bad=name".into(), Some("value".into()));
                }
                let run = Engine::default().run_source(case.id, "Unknown", options);
                assert!(run.variables.is_empty());
                assert_eq!(run.steps, 0);
                let error = run.result.unwrap_err();
                if let Some(expected) = case.error {
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW7002");
                    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
                } else {
                    assert_eq!(error.code().as_str(), "BW7002");
                    let BWErr::RunConfiguration(message) = error.error.as_ref() else {
                        panic!("category")
                    };
                    assert_eq!(
                        message,
                        "Environment names must be nonempty and contain neither '=' nor NUL"
                    );
                    assert!(error.causes.is_empty() && error.omissions.is_none());
                }
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
            Input::RuntimeDiagnosticBoundary
            | Input::RuntimeDiagnosticLimit
            | Input::RetainedDiagnosticBoundary
            | Input::RetainedDiagnosticLimit
            | Input::HandlerCopyBoundary
            | Input::HandlerCopyLimit
            | Input::HandlerHandoffBoundary
            | Input::HandlerHandoffLimit => {
                use botwork::core::{
                    diagnostic::DiagnosticLimits,
                    run::{Engine, RetainedDiagnosticLimits, RunLimits, RunOptions, RunOutcome},
                };
                let mut limits = RunLimits::default();
                let capacity = usize::from(case.error.is_none());
                let copying = matches!(
                    case.input,
                    Input::HandlerCopyBoundary | Input::HandlerCopyLimit
                );
                let handoff = matches!(
                    case.input,
                    Input::HandlerHandoffBoundary | Input::HandlerHandoffLimit
                );
                if handoff {
                    limits.retained_diagnostics = RetainedDiagnosticLimits {
                        records: 2,
                        diagnostics: 1 + capacity,
                        ..RetainedDiagnosticLimits::default()
                    };
                } else if matches!(
                    case.input,
                    Input::RetainedDiagnosticBoundary | Input::RetainedDiagnosticLimit
                ) || copying
                {
                    limits.retained_diagnostics = RetainedDiagnosticLimits {
                        records: capacity + usize::from(copying),
                        ..RetainedDiagnosticLimits::default()
                    };
                } else {
                    limits.diagnostics = DiagnosticLimits {
                        diagnostics: capacity,
                        ..DiagnosticLimits::default()
                    };
                }
                let run = Engine::default().run_source(
                    case.id,
                    if handoff {
                        "Try { Try { Missing } Catch { Other } } Catch { |handled| = |true| }"
                    } else if copying {
                        "Try { Try { Missing } Catch { Rethrow } } Catch { |handled| = |true| }"
                    } else {
                        "Try { Missing } Catch { |handled| = |true| }"
                    },
                    RunOptions {
                        limits,
                        ..RunOptions::default()
                    },
                );
                if let Some(expected) = case.error {
                    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
                    let error = run.result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW2002");
                    assert!(error.causes[0].omissions.is_some());
                    assert!(!run.variables.contains_key("handled"));
                } else {
                    assert_eq!(run.outcome(), RunOutcome::Succeeded);
                    assert_eq!(run.variables["handled"].to_string(), "true");
                }
                continue;
            }
            Input::DiagnosticAdmissionBoundary | Input::DiagnosticAdmissionLimit => {
                use botwork::core::diagnostic::{Diagnostic, DiagnosticLimits};
                let original = Diagnostic::new(BWErr::NativeError("reason".into()));
                let result = DiagnosticLimits {
                    text_bytes: if case.error.is_some() { 11 } else { 12 },
                    source_bytes: 0,
                    ..DiagnosticLimits::default()
                }
                .admit(original);
                if let Some(expected) = case.error {
                    let error = result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(error.causes[0].code().as_str(), "BW4002");
                    assert!(error.causes[0].omissions.is_some());
                } else {
                    let original = result.unwrap();
                    assert_eq!(original.code().as_str(), "BW4002");
                    assert!(original.omissions.is_none());
                }
                continue;
            }
            Input::DiagnosticOwnershipBoundary | Input::DiagnosticOwnershipLimit => {
                use botwork::core::diagnostic::{Diagnostic, DiagnosticLimits};
                let original = Diagnostic::new(BWErr::NativeError("reason".into()));
                let result = original.try_clone_with_limits(&DiagnosticLimits {
                    text_bytes: if case.error.is_some() { 11 } else { 12 },
                    source_bytes: 0,
                    ..DiagnosticLimits::default()
                });
                if let Some(expected) = case.error {
                    let error = result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                } else {
                    let clone = result.unwrap();
                    assert!(std::sync::Arc::ptr_eq(&clone.error, &original.error));
                    clone.discard();
                }
                assert_eq!(original.code().as_str(), "BW4002");
                original.discard();
                continue;
            }
            Input::DiagnosticValueBoundary | Input::DiagnosticValueLimit => {
                use botwork::core::{
                    diagnostic::{Diagnostic, DiagnosticValueLimits},
                    value_limits::ValueLimits,
                };
                let original = Diagnostic::new(BWErr::NativeError("reason".into()));
                let result = original.to_value_with_limits(&DiagnosticValueLimits {
                    values: ValueLimits {
                        nodes: if case.error.is_some() { 9 } else { 10 },
                        depth: 3,
                        ..ValueLimits::default()
                    },
                    position_bytes: 0,
                });
                if let Some(expected) = case.error {
                    let error = result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                } else {
                    let Literal::Map(map) = result.unwrap() else {
                        panic!("map")
                    };
                    assert_eq!(map["code"].to_string(), "BW4002");
                    assert_eq!(map.len(), 8);
                }
                assert_eq!(original.code().as_str(), "BW4002");
                continue;
            }
            Input::TemporaryBoundary | Input::TemporaryLimit => {
                use botwork::core::run::{
                    Engine, RunLimits, RunOptions, RunOutcome, TemporaryLimits,
                };
                let run = Engine::default().run_source(
                    case.id,
                    "|x| = |\"ab\"+\"cd\"|",
                    RunOptions {
                        limits: RunLimits {
                            temporaries: TemporaryLimits {
                                values: 3,
                                nodes: 3,
                                payload_bytes: if case.error.is_some() { 7 } else { 8 },
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                assert!(run.snapshot_error.is_none());
                if let Some(expected) = case.error {
                    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
                    assert!(run.variables.is_empty());
                    let error = run.result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                } else {
                    assert_eq!(run.outcome(), RunOutcome::Succeeded);
                    assert_eq!(run.variables["x"].to_string(), "abcd");
                }
                continue;
            }
            Input::ResultBoundary | Input::ResultLimit => {
                use botwork::core::run::{Engine, ResultLimits, RunLimits, RunOptions, RunOutcome};
                let run = Engine::default().run_source(
                    case.id,
                    "|é| = |[1, {\"κ\": true}]|",
                    RunOptions {
                        limits: RunLimits {
                            results: ResultLimits {
                                values: 2,
                                nodes: 8,
                                name_bytes: 2,
                                payload_bytes: if case.error.is_some() { 13 } else { 14 },
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                if let Some(expected) = case.error {
                    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
                    assert!(run.variables.is_empty());
                    let error = run.snapshot_error.unwrap();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert!(run.result.is_err());
                } else {
                    assert_eq!(run.outcome(), RunOutcome::Succeeded);
                    assert!(run.snapshot_error.is_none());
                    assert_eq!(
                        run.result.unwrap().to_string(),
                        run.variables["é"].to_string()
                    );
                }
                continue;
            }
            Input::SnapshotBoundary | Input::SnapshotLimit => {
                use botwork::core::run::{
                    Engine, RunLimits, RunOptions, RunOutcome, SnapshotLimits,
                };
                let engine = Engine::default();
                let run = engine.run_source(
                    case.id,
                    "|x| = |7|",
                    RunOptions {
                        limits: RunLimits {
                            snapshots: SnapshotLimits {
                                entries: usize::from(case.error.is_none()),
                                path_bytes: 0,
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                if let Some(expected) = case.error {
                    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
                    assert!(run.variables.is_empty());
                    assert_eq!(run.steps, 0);
                    let error = run.result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                } else {
                    assert_eq!(run.outcome(), RunOutcome::Succeeded);
                    assert_eq!(run.variables["x"].to_string(), "7");
                }
                continue;
            }
            Input::RegistryBoundary | Input::RegistryLimit => {
                use botwork::core::{
                    ast::Program,
                    diagnostic::DiagnosticCode,
                    eval::{evaluate_program_detailed, Context},
                    run::{RetainedRegistryLimits, RunLimits},
                    signature::StatementSignature,
                };
                let mut context = Context::with_limits(RunLimits {
                    retained_registry: RetainedRegistryLimits {
                        entries: 1,
                        nodes: 3,
                        name_bytes: 11,
                        text_bytes: if case.error.is_some() { 31 } else { 32 },
                        source_bytes: 20,
                    },
                    ..RunLimits::default()
                })
                .unwrap();
                let signature = StatementSignature::native("Read |value|")
                    .unwrap()
                    .description("é")
                    .documents_error(DiagnosticCode::Native, "bad")
                    .unwrap();
                let result = context
                    .register_native_with_signature(signature, |values| Ok(values[0].clone()));
                if let Some(expected) = case.error {
                    let error = result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert!(context.statement_signatures().is_empty());
                } else {
                    result.unwrap();
                    assert_eq!(context.statement_signatures().len(), 1);
                    assert_eq!(
                        evaluate_program_detailed(
                            &Program::parse(case.id, "Read |7|").unwrap(),
                            &mut context
                        )
                        .unwrap()
                        .to_string(),
                        "7"
                    );
                }
                continue;
            }
            Input::NameBoundary | Input::NameLimit => {
                use botwork::core::run::{Engine, RetainedNameLimits, RunLimits, RunOptions};
                let source = if case.error.is_some() {
                    "|é| = |1|\n|e\u{301}| = |2|"
                } else {
                    "|é| = |1|\n|é| = |2|"
                };
                let run = Engine::default().run_source(
                    case.id,
                    source,
                    RunOptions {
                        limits: RunLimits {
                            retained_names: if case.error.is_some() {
                                RetainedNameLimits {
                                    names: 2,
                                    name_bytes: 3,
                                    total_bytes: 4,
                                }
                            } else {
                                RetainedNameLimits {
                                    names: 1,
                                    name_bytes: 2,
                                    total_bytes: 2,
                                }
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
                    assert_eq!(run.variables["é"].to_string(), "1");
                    assert_eq!(run.variables.len(), 1);
                } else {
                    assert_eq!(run.result.unwrap().to_string(), "2");
                    assert_eq!(run.variables["é"].to_string(), "2");
                }
                continue;
            }
            Input::DefinitionBoundary | Input::DefinitionLimit => {
                use botwork::core::{
                    ast::Program,
                    eval::{evaluate_program_detailed, Context},
                    run::{RetainedDefinitionLimits, RunLimits},
                };
                let source = "First {}\nSecond {}";
                let mut context = Context::with_limits(RunLimits {
                    retained_definitions: RetainedDefinitionLimits {
                        definitions: if case.error.is_some() { 1 } else { 2 },
                        nodes: 6,
                        source_bytes: source.len() + case.id.len(),
                    },
                    ..RunLimits::default()
                })
                .unwrap();
                let result = evaluate_program_detailed(
                    &Program::parse(case.id, source).unwrap(),
                    &mut context,
                );
                assert!(context.statement_signature("First").unwrap().is_some());
                if let Some(expected) = case.error {
                    let error = result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert!(context.statement_signature("Second").unwrap().is_none());
                } else {
                    result.unwrap();
                    assert!(context.statement_signature("Second").unwrap().is_some());
                }
                continue;
            }
            Input::RetentionBoundary | Input::RetentionLimit => {
                use botwork::core::run::{Engine, RetainedValueLimits, RunLimits, RunOptions};
                let run = Engine::default().run_source(
                    case.id,
                    "|x| = |1|\n|x| = |2|",
                    RunOptions {
                        limits: RunLimits {
                            retained_values: RetainedValueLimits {
                                values: if case.error.is_some() { 1 } else { 2 },
                                nodes: 2,
                                payload_bytes: 8,
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
                    assert_eq!(run.variables["x"].to_string(), "1");
                } else {
                    assert_eq!(run.result.unwrap().to_string(), "2");
                    assert_eq!(run.variables["x"].to_string(), "2");
                }
                continue;
            }
            Input::JsonBudgetBoundary | Input::JsonBudgetLimit => {
                use botwork::core::{
                    input::{parse_variables_with_limits, InputLimits},
                    value_limits::ValueLimits,
                };
                let source = r#"{"x":[1,2]}"#;
                let result = parse_variables_with_limits(
                    "input-corpus",
                    source,
                    &InputLimits {
                        source_bytes: source.len(),
                        total_bytes: source.len(),
                        sources: 1,
                        variables: 1,
                        raw_nodes: if case.error.is_some() { 4 } else { 5 },
                        values: ValueLimits {
                            nodes: 3,
                            depth: 2,
                            entries: 2,
                            payload_bytes: 8,
                            ..ValueLimits::default()
                        },
                    },
                );
                if let Some(expected) = case.error {
                    let error = result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert!(error.span.unwrap().source().text().is_empty());
                } else {
                    assert_eq!(result.unwrap()["x"].to_string(), "[1, 2]");
                }
                continue;
            }
            Input::ConstructionBoundary | Input::ConstructionLimit => {
                use botwork::core::{
                    run::{Engine, RunLimits, RunOptions},
                    value_limits::ValueLimits,
                };
                use std::sync::{
                    atomic::{AtomicUsize, Ordering},
                    Arc,
                };
                let count = Arc::new(AtomicUsize::new(0));
                let observed = count.clone();
                let mut engine = Engine::default();
                engine
                    .register_native("Mark |value|", move |values, _| {
                        observed.fetch_add(1, Ordering::SeqCst);
                        Ok(values[0].clone())
                    })
                    .unwrap();
                let (source, limits) = if case.error.is_some() {
                    (
                        "|x| = |[@{Mark |1|},@{Mark |2|},@{Mark |3|}]|",
                        ValueLimits {
                            entries: 2,
                            ..ValueLimits::default()
                        },
                    )
                } else {
                    (
                        "|x| = |{k:@{Mark |1|},k:@{Mark |2|}}|",
                        ValueLimits {
                            nodes: 2,
                            entries: 1,
                            payload_bytes: 5,
                            ..ValueLimits::default()
                        },
                    )
                };
                let run = engine.run_source(
                    "construction-corpus",
                    source,
                    RunOptions {
                        limits: RunLimits {
                            values: limits,
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                );
                if let Some(expected) = case.error {
                    let error = run.result.unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(error.to_string().contains(expected));
                    assert_eq!(count.load(Ordering::SeqCst), 0);
                } else {
                    assert_eq!(run.result.unwrap().to_string(), "{\"k\": 2}");
                    assert_eq!(count.load(Ordering::SeqCst), 2);
                }
                continue;
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

fn check_worker_case(case: &Case) {
    use botwork::core::{
        operation::OperationControl,
        worker::{WorkerCommand, WorkerLimits, WorkerPool},
    };
    let pool = WorkerPool::new(WorkerLimits {
        request_bytes: 2,
        stdout_bytes: if case.error.is_some() { 1 } else { 2 },
        stderr_bytes: 0,
        timeout: Duration::from_secs(2),
        ..Default::default()
    })
    .unwrap();
    let start = pool.start(
        WorkerCommand {
            executable: "/bin/cat".into(),
            arguments: vec![],
            directory: std::env::temp_dir(),
            environment: Default::default(),
        },
        "é".as_bytes().to_vec(),
        OperationControl::default(),
    );
    #[cfg(target_os = "linux")]
    {
        use botwork::core::worker::{WorkerCleanup, WorkerOutcome};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let report = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), start.unwrap().wait())
                .await
                .unwrap()
        });
        assert_eq!(report.cleanup, WorkerCleanup::Reaped);
        if let Some(expected) = case.error {
            assert_eq!(report.outcome, WorkerOutcome::Failed);
            let error = report.diagnostic.unwrap();
            assert_eq!(error.code().as_str(), case.code.unwrap());
            assert!(error.to_string().contains(expected));
            assert!(!report.io_complete);
        } else {
            assert_eq!(report.outcome, WorkerOutcome::Succeeded);
            assert_eq!(report.stdout, "é".as_bytes());
            assert!(report.io_complete);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let error = start
            .err()
            .expect("unsupported worker platform must reject before effects");
        assert_eq!(
            error.code(),
            botwork::core::diagnostic::DiagnosticCode::RunConfiguration
        );
        assert!(pool.snapshot().active.is_empty());
    }
}

fn check_protocol_case(case: &Case) {
    use botwork::core::{signature::StatementSignature, worker::protocol::WorkerProtocol};
    let mut protocol = WorkerProtocol::default();
    protocol.limits.frame_bytes = if case.error.is_some() { 27 } else { 28 };
    let request = protocol.encode_request(&[Literal::Float(-0.0)]);
    if let Some(expected) = case.error {
        let error = request.unwrap_err();
        assert_eq!(error.code().as_str(), case.code.unwrap());
        assert!(error.to_string().contains(expected));
    } else {
        let request = request.unwrap();
        assert_eq!(request.len(), 28);
        let mut output = Vec::new();
        protocol
            .serve_once(
                &StatementSignature::native("Echo |x|").unwrap(),
                &mut request.as_slice(),
                &mut output,
                |mut values| Ok(values.remove(0)),
            )
            .unwrap();
        let Literal::Float(value) = protocol.decode_response(&output).unwrap().unwrap() else {
            panic!()
        };
        assert_eq!(value.to_bits(), (-0.0f32).to_bits());
    }
}

fn check_shutdown_case(case: &Case) {
    use botwork::core::worker::{WorkerLimits, WorkerPool};
    let pool = WorkerPool::new(WorkerLimits::default()).unwrap();
    if let Some(expected) = case.error {
        let error = pool.shutdown_wait(Duration::MAX).unwrap_err();
        assert_eq!(error.code().as_str(), case.code.unwrap());
        assert!(error.to_string().contains(expected));
        assert!(!pool.snapshot().closed);
        return;
    }
    #[cfg(target_os = "linux")]
    let handle = {
        use botwork::core::{operation::OperationControl, worker::WorkerCommand};
        pool.start(
            WorkerCommand {
                executable: "/bin/sh".into(),
                arguments: vec!["-c".into(), "while :; do :; done".into()],
                directory: std::env::temp_dir(),
                environment: Default::default(),
            },
            vec![],
            OperationControl::default(),
        )
        .unwrap()
    };
    let snapshot = pool.shutdown_wait(Duration::from_secs(2)).unwrap();
    assert!(snapshot.closed);
    assert!(snapshot.active.is_empty());
    #[cfg(target_os = "linux")]
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert_eq!(
            runtime.block_on(handle.wait()).outcome,
            botwork::core::worker::WorkerOutcome::Cancelled
        );
    }
    assert!(pool
        .shutdown_wait(Duration::ZERO)
        .unwrap()
        .active
        .is_empty());
}

fn check_progress_case(case: &Case) {
    use botwork::core::{
        operation::OperationControl,
        worker::{WorkerCommand, WorkerLimits, WorkerPool},
    };
    let pool = WorkerPool::new(WorkerLimits::default()).unwrap();
    let start = pool.start(
        WorkerCommand {
            executable: "/bin/sh".into(),
            arguments: vec![
                "-c".into(),
                if case.error.is_some() {
                    "exec 0<&-; exit 0"
                } else {
                    "exec /bin/cat"
                }
                .into(),
            ],
            directory: std::env::temp_dir(),
            environment: Default::default(),
        },
        if case.error.is_some() {
            vec![1; 64 * 1024]
        } else {
            vec![]
        },
        OperationControl::default(),
    );
    #[cfg(target_os = "linux")]
    {
        use botwork::core::worker::{WorkerCleanup, WorkerOutcome};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let report = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), start.unwrap().wait())
                .await
                .unwrap()
        });
        assert_eq!(report.cleanup, WorkerCleanup::Reaped);
        assert!(report.progress_complete);
        if let Some(expected) = case.error {
            assert_eq!(report.outcome, WorkerOutcome::Failed);
            assert!(!report.io_complete);
            assert!(report.stdin_written < 64 * 1024);
            let error = report.diagnostic.unwrap();
            assert_eq!(error.code().as_str(), case.code.unwrap());
            assert!(error.to_string().contains(expected));
        } else {
            assert_eq!(report.outcome, WorkerOutcome::Succeeded);
            assert!(report.io_complete);
            assert_eq!(report.stdin_written, 0);
        }
    }
    #[cfg(not(target_os = "linux"))]
    assert!(start.is_err());
}

fn check_tree_case(case: &Case) {
    use botwork::core::worker::{WorkerLimits, WorkerPool};
    let pool = WorkerPool::with_process_tree(
        WorkerLimits::default(),
        if case.error.is_some() {
            "/bin/true".into()
        } else {
            env!("CARGO_BIN_EXE_botwork").into()
        },
    );
    #[cfg(target_os = "linux")]
    {
        use botwork::core::{
            operation::OperationControl,
            worker::{WorkerCleanup, WorkerCommand, WorkerOutcome},
        };
        let pool = pool.unwrap();
        let handle = pool
            .start(
                WorkerCommand {
                    executable: "/bin/true".into(),
                    arguments: vec![],
                    directory: std::env::temp_dir(),
                    environment: Default::default(),
                },
                vec![],
                OperationControl::default(),
            )
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let report = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), handle.wait())
                .await
                .unwrap()
        });
        if let Some(expected) = case.error {
            assert_eq!(report.cleanup, WorkerCleanup::Unverified);
            assert_ne!(report.outcome, WorkerOutcome::Succeeded);
            let error = report.diagnostic.unwrap();
            assert_eq!(error.code().as_str(), case.code.unwrap());
            assert!(error.to_string().contains(expected));
            assert_eq!(pool.snapshot().active.len(), 1);
        } else {
            assert_eq!(report.cleanup, WorkerCleanup::TreeReaped);
            assert_eq!(report.outcome, WorkerOutcome::Succeeded);
            assert!(report.io_complete && report.progress_complete);
            assert!(pool.snapshot().active.is_empty());
        }
    }
    #[cfg(not(target_os = "linux"))]
    assert!(pool.is_err());
}

fn check_journal_case(_case: &Case) {
    #[cfg(target_os = "linux")]
    {
        use botwork::core::{
            operation::OperationControl,
            worker::{
                journal::{JournalFlush, WorkerJournal},
                WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool,
            },
        };
        let workspace = Harness::new();
        let journal = WorkerJournal::open(
            &workspace.workspace.join("journal"),
            std::num::NonZeroUsize::new(1).unwrap(),
        )
        .unwrap();
        let pool = WorkerPool::with_recovery(
            WorkerLimits::default(),
            env!("CARGO_BIN_EXE_botwork").into(),
            journal.clone(),
        )
        .unwrap();
        let command = WorkerCommand {
            executable: "/bin/true".into(),
            arguments: vec![],
            directory: workspace.workspace.clone(),
            environment: Default::default(),
        };
        let handle = pool
            .start(command.clone(), vec![], OperationControl::default())
            .unwrap();
        let id = handle.journal_id().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        assert_eq!(
            runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(5), handle.wait())
                        .await
                        .unwrap()
                })
                .outcome,
            WorkerOutcome::Succeeded
        );
        assert_eq!(
            journal.flush_wait(Duration::from_secs(5)).unwrap(),
            JournalFlush::default()
        );
        let records = journal.records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, id);
        assert_eq!(records[0].outcome, Some(WorkerOutcome::Succeeded));
        assert!(!records[0].damaged);
        if let Some(expected) = _case.error {
            let error = pool
                .start(command, vec![], OperationControl::default())
                .err()
                .unwrap();
            assert_eq!(error.code().as_str(), _case.code.unwrap());
            assert!(error.to_string().contains(expected));
            assert!(pool.snapshot().active.is_empty());
            assert_eq!(journal.records().unwrap().len(), 1);
        }
    }
}

fn check_namespace_case(_case: &Case) {
    #[cfg(target_os = "linux")]
    {
        use botwork::core::{
            operation::OperationControl,
            worker::{WorkerCleanup, WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool},
        };
        let workspace = Harness::new();
        let helper = if _case.error.is_some() {
            "/bin/true"
        } else {
            env!("CARGO_BIN_EXE_botwork")
        };
        let pool =
            WorkerPool::with_pid_namespace(WorkerLimits::default(), helper.into(), None).unwrap();
        let handle = pool
            .start(
                WorkerCommand {
                    executable: "/bin/true".into(),
                    arguments: vec![],
                    directory: workspace.workspace.clone(),
                    environment: Default::default(),
                },
                vec![],
                OperationControl::default(),
            )
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let report = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), handle.wait())
                .await
                .unwrap()
        });
        if let Some(expected) = _case.error {
            assert_eq!(report.outcome, WorkerOutcome::Failed);
            assert_eq!(report.cleanup, WorkerCleanup::NamespaceReaped);
            assert!(report.exit_status.is_none());
            let error = report.diagnostic.unwrap();
            assert_eq!(error.code().as_str(), _case.code.unwrap());
            assert!(error.to_string().contains(expected));
        } else {
            assert_eq!(report.outcome, WorkerOutcome::Succeeded);
            assert_eq!(report.cleanup, WorkerCleanup::TreeReaped);
            assert!(report.io_complete && report.progress_complete);
        }
        assert!(pool.snapshot().active.is_empty());
    }
}
