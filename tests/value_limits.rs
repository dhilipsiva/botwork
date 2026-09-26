use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode,
    eval::{evaluate_program_detailed, Context},
    grammar::{Literal, Operate, Rule},
    operation::{NativeOperation, OperationControl},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
    signature::StatementSignature,
    value_limits::{discard, ValueLimits, ValueSize, MAX_VALUE_DEPTH},
};
use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

fn deep(depth: usize) -> Literal {
    (0..depth).fold(Literal::None, |value, index| {
        if index % 2 == 0 {
            Literal::Array(vec![value])
        } else {
            Literal::Map([("key".into(), value)].into_iter().collect())
        }
    })
}

#[test]
fn counts_are_exact_for_nodes_depth_unicode_and_scalar_payload() {
    let value = Literal::Map(
        [(
            "é".into(),
            Literal::Array(vec![
                Literal::String("🙂".into()),
                Literal::Int(7),
                Literal::None,
            ]),
        )]
        .into_iter()
        .collect(),
    );
    let exact = ValueLimits {
        nodes: 5,
        depth: 3,
        string_bytes: 4,
        key_bytes: 2,
        entries: 3,
        payload_bytes: 10,
    };
    assert_eq!(
        exact.check(&value).unwrap(),
        ValueSize {
            nodes: 5,
            depth: 3,
            payload_bytes: 10
        }
    );
    for (limits, resource) in [
        (
            ValueLimits {
                nodes: 4,
                ..exact.clone()
            },
            "value nodes",
        ),
        (
            ValueLimits {
                depth: 2,
                ..exact.clone()
            },
            "value depth",
        ),
        (
            ValueLimits {
                string_bytes: 3,
                ..exact.clone()
            },
            "value string bytes",
        ),
        (
            ValueLimits {
                key_bytes: 1,
                ..exact.clone()
            },
            "value key bytes",
        ),
        (
            ValueLimits {
                entries: 2,
                ..exact.clone()
            },
            "value container entries",
        ),
        (
            ValueLimits {
                payload_bytes: 9,
                ..exact.clone()
            },
            "value payload bytes",
        ),
    ] {
        assert!(limits
            .check(&value)
            .unwrap_err()
            .to_string()
            .contains(resource));
    }
}

#[test]
fn borrowed_validation_and_public_discard_handle_extreme_host_depth() {
    let value = deep(100_000);
    assert!(ValueLimits::default()
        .check(&value)
        .unwrap_err()
        .to_string()
        .contains("value depth"));
    discard(value);
}

#[test]
fn rejected_owned_input_and_early_run_failures_destroy_deep_values_safely() {
    for entry in 0..5 {
        let variables = BTreeMap::from([("x".into(), deep(20_000))]);
        let error = match entry {
            0 => Context::default()
                .set_input_variables(variables)
                .unwrap_err(),
            _ => {
                let mut options = RunOptions {
                    variables,
                    ..RunOptions::default()
                };
                match entry {
                    1 => options.control.cancel(),
                    2 => options.limits.values.depth = MAX_VALUE_DEPTH + 1,
                    3 => {
                        options.working_directory =
                            Some(std::path::PathBuf::from("/nonexistent-botwork-value-test"))
                    }
                    _ => (),
                }
                Engine::default()
                    .run_source("input", "", options)
                    .result
                    .unwrap_err()
            }
        };
        assert_eq!(
            error.code(),
            match entry {
                1 => DiagnosticCode::Cancelled,
                2 | 3 => DiagnosticCode::RunConfiguration,
                _ => DiagnosticCode::ResourceLimit,
            }
        );
    }
}

#[test]
fn all_inputs_are_admitted_before_any_binding_or_script_effect() {
    let mut engine = Engine::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    engine
        .register_native("Mark", move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let result = engine.run_source(
        "input",
        "Mark",
        RunOptions {
            variables: BTreeMap::from([
                ("a".into(), Literal::Int(1)),
                ("z".into(), Literal::String("too large".into())),
            ]),
            limits: RunLimits {
                values: ValueLimits {
                    string_bytes: 3,
                    ..ValueLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert!(result.variables.is_empty());
    assert_eq!(result.steps, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn native_results_are_admitted_before_publication_and_limits_bypass_handlers() {
    let mut engine = Engine::default();
    engine
        .register_native("Host", |_, _| Ok(deep(10_000)))
        .unwrap();
    let result = engine.run_source(
        "native",
        "|kept| = |7|\nTry { |value| = Host } Catch { |handled| = |true| }",
        RunOptions::default(),
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(result.variables["kept"].to_string(), "7");
    assert!(!result.variables.contains_key("value"));
    assert!(!result.variables.contains_key("handled"));
    assert_eq!(result.result.unwrap_err().call_stack[0].signature, "host");
}

#[test]
fn cancelled_native_results_keep_stop_priority_and_safe_destruction() {
    let mut engine = Engine::default();
    engine
        .register_native("Host", |_, environment| {
            environment.control().cancel();
            Ok(deep(10_000))
        })
        .unwrap();
    let result = engine.run_source("native", "|value| = Host", RunOptions::default());
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.variables.is_empty());
}

#[test]
fn public_operators_admit_both_inputs_and_results_with_safe_early_cleanup() {
    for deep_on_left in [true, false] {
        let (left, right) = if deep_on_left {
            (deep(20_000), Literal::None)
        } else {
            (Literal::None, deep(20_000))
        };
        assert_eq!(
            Rule::equal.operate_binary(left, right).unwrap_err().code(),
            DiagnosticCode::ResourceLimit
        );
    }
    assert_eq!(
        Rule::logical_not
            .operate_unary(deep(20_000))
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    let limits = ValueLimits {
        string_bytes: 3,
        ..ValueLimits::default()
    };
    assert!(Rule::plus
        .operate_binary_bounded(
            Literal::String("ab".into()),
            Literal::String("cd".into()),
            &limits
        )
        .unwrap_err()
        .to_string()
        .contains("value string bytes"));
}

#[test]
fn expression_results_are_checked_and_context_stops_remain_latched() {
    let mut context = Context::with_limits(RunLimits {
        values: ValueLimits {
            entries: 2,
            ..ValueLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let program = Program::parse("array", "|value| = |[1,2,3]|").unwrap();
    assert_eq!(
        evaluate_program_detailed(&program, &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    assert_eq!(
        context.checkpoint().unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn zero_payload_accepts_empty_values_and_depth_configuration_is_checked() {
    let limits = ValueLimits {
        payload_bytes: 0,
        string_bytes: 0,
        key_bytes: 0,
        entries: 0,
        ..ValueLimits::default()
    };
    for value in [
        Literal::None,
        Literal::String(String::new()),
        Literal::Array(vec![]),
        Literal::Map(Default::default()),
    ] {
        limits.check(&value).unwrap();
    }
    assert!(limits.check(&Literal::Bool(false)).is_err());
    assert!(ValueLimits {
        nodes: 0,
        ..limits.clone()
    }
    .check(&Literal::None)
    .is_err());
    assert!(ValueLimits {
        depth: 0,
        ..limits.clone()
    }
    .check(&Literal::None)
    .is_err());
    assert_eq!(
        Context::with_limits(RunLimits {
            values: ValueLimits {
                depth: MAX_VALUE_DEPTH + 1,
                ..limits
            },
            ..RunLimits::default()
        })
        .err()
        .unwrap()
        .code(),
        DiagnosticCode::RunConfiguration
    );
}

#[test]
fn dropping_an_unpolled_operation_releases_owned_input_iteratively() {
    let operation = NativeOperation::asynchronous(
        StatementSignature::native("Read |value|").unwrap(),
        |values, _| async move { Ok(values.into_iter().next().unwrap()) },
    )
    .unwrap();
    let future = operation.invoke(vec![deep(20_000)], OperationControl::default());
    drop(future);
}

#[tokio::test]
async fn async_input_failure_and_cancelled_entry_do_not_construct_callback() {
    let operation = NativeOperation::asynchronous(
        StatementSignature::native("Read |value|").unwrap(),
        |_, _| {
            panic!("callback must not start");
            #[allow(unreachable_code)]
            async {
                Ok(Literal::None)
            }
        },
    )
    .unwrap();
    for cancelled in [false, true] {
        let control = OperationControl::default();
        if cancelled {
            control.cancel();
        }
        let error = operation
            .invoke(vec![deep(20_000)], control)
            .await
            .unwrap_err();
        assert_eq!(
            error.code(),
            if cancelled {
                DiagnosticCode::Cancelled
            } else {
                DiagnosticCode::ResourceLimit
            }
        );
    }
}

#[tokio::test]
async fn async_and_blocking_results_are_checked_and_discarded_after_stop() {
    for blocking in [false, true] {
        for cancel in [false, true] {
            let signature = StatementSignature::native("Host").unwrap();
            let operation = if blocking {
                NativeOperation::blocking(
                    signature,
                    NonZeroUsize::new(1).unwrap(),
                    move |_, control| {
                        if cancel {
                            control.cancel();
                        }
                        Ok(deep(20_000))
                    },
                )
                .unwrap()
            } else {
                NativeOperation::asynchronous(signature, move |_, control| async move {
                    if cancel {
                        control.cancel();
                    }
                    Ok(deep(20_000))
                })
                .unwrap()
            };
            let error = operation
                .invoke(vec![], OperationControl::default())
                .await
                .unwrap_err();
            assert_eq!(
                error.code(),
                if cancel {
                    DiagnosticCode::Cancelled
                } else {
                    DiagnosticCode::ResourceLimit
                }
            );
        }
    }
}

#[tokio::test]
async fn operations_accept_local_value_limits_without_changing_siblings() {
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Host").unwrap(), |_, _| async {
            Ok(Literal::String("four".into()))
        })
        .unwrap();
    let limited = operation
        .clone()
        .with_value_limits(ValueLimits {
            string_bytes: 3,
            ..ValueLimits::default()
        })
        .unwrap();
    assert_eq!(
        limited
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap_err()
            .code(),
        DiagnosticCode::ResourceLimit
    );
    assert_eq!(
        operation
            .invoke(vec![], OperationControl::default())
            .await
            .unwrap()
            .to_string(),
        "four"
    );
}

#[test]
fn value_limit_unwinding_restores_iterator_and_call_frames() {
    let mut engine = Engine::default();
    engine
        .register_native("Host", |_, _| Ok(Literal::String("long".into())))
        .unwrap();
    let result=engine.run_source("cleanup","|item| = |9|\nRead { Return |@{Host}| }\nTry { For |item| In |[1]| { Read } } Catch { |handled| = |true| }",RunOptions {
        limits:RunLimits {values:ValueLimits {string_bytes:3,..ValueLimits::default()},..RunLimits::default()},..RunOptions::default()
    });
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(result.variables["item"].to_string(), "9");
    assert!(!result.variables.contains_key("handled"));
    assert_eq!(result.result.unwrap_err().call_stack.len(), 2);
}

#[test]
fn accepted_deep_values_survive_recursive_calls_and_result_snapshots() {
    let result=Engine::default().run_source("combined","Read |n| { If |n > 0| { Return |@{Read |n-1|}| } Else { Return |value| } }\n|result| = Read |20|",RunOptions {
        variables:BTreeMap::from([("value".into(),deep(MAX_VALUE_DEPTH-1))]),..RunOptions::default()
    });
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(
        ValueLimits::default()
            .check(&result.variables["result"])
            .unwrap()
            .depth,
        MAX_VALUE_DEPTH
    );
}

#[test]
fn invalid_names_and_signature_arity_still_release_rejected_values() {
    let error = Context::default()
        .set_input_variables(BTreeMap::from([("bad name".into(), deep(20_000))]))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Input);
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Host").unwrap(), |_, _| async {
            Ok(Literal::None)
        })
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let error = runtime
        .block_on(operation.invoke(vec![deep(20_000)], OperationControl::default()))
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ParameterCount);
}

#[test]
fn oversized_catch_binding_preserves_original_failure_and_previous_binding() {
    let result = Engine::default().run_source(
        "catch",
        "|error| = |7|\nTry { |x| = |missing| } Catch |error| { |handled| = |true| }",
        RunOptions {
            limits: RunLimits {
                values: ValueLimits {
                    entries: 1,
                    ..ValueLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(result.variables["error"].to_string(), "7");
    assert!(!result.variables.contains_key("handled"));
    let error = result.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
}
