use super::execution_contract::{context, events, visits};
use super::*;

fn parsed_call(text: &str) -> Call {
    let program = Program::parse("native-contract.botwork", text).unwrap();
    let StatementKind::Invoke(call) = program.statements[0].kind() else {
        panic!("call")
    };
    call.clone()
}

#[test]
fn native_calls_share_argument_visitation_order_and_skip_the_callback_on_failure() {
    for source in ["Record |[1, 2, 3]|", "Record |[1, missing, 3]|"] {
        let mut context = context();
        let result = invoke(&parsed_call(source), &mut context);
        if source.contains("missing") {
            assert!(result.is_err());
            assert_eq!(visits(&context), ["[1, missing, 3]", "1", "missing"]);
            assert!(events(&context).is_empty());
        } else {
            result.unwrap();
            assert_eq!(visits(&context), ["[1, 2, 3]", "1", "2", "3"]);
            assert_eq!(events(&context), ["[1, 2, 3]"]);
        }
        assert!(context.calls.is_empty());
        assert_eq!(context.frames.len(), 1);
    }
}

#[test]
fn malformed_resolved_calls_reject_arity_before_visiting_any_argument() {
    let mut context = context();
    for extra in [false, true] {
        let mut call = parsed_call("Record |missing|");
        if extra {
            call.arguments.push(call.arguments[0].clone());
        } else {
            call.arguments.clear();
        }
        let error = invoke(&call, &mut context).unwrap_err();
        assert!(matches!(*error.error, BWErr::ParameterMissingError(_)));
        assert!(visits(&context).is_empty());
        assert!(events(&context).is_empty());
        assert!(error.call_stack.is_empty());
    }
}

#[test]
fn invalid_nested_host_arguments_never_reach_native_or_custom_bodies() {
    let mut context = context();
    context
        .set_variable(
            "invalid",
            Literal::Map([("x".into(), Literal::Array(vec![Literal::Float(f32::NAN)]))].into()),
        )
        .unwrap();
    evaluate_program(
        &Program::parse(
            "native-contract.botwork",
            "Custom |value| { Record |value| }",
        )
        .unwrap(),
        &mut context,
    )
    .unwrap();
    for source in ["Record |invalid|", "Custom |invalid|"] {
        let error = invoke(&parsed_call(source), &mut context).unwrap_err();
        assert!(matches!(*error.error, BWErr::ArithmeticError(_)));
        assert_eq!(error.span.unwrap().text(), "invalid");
        assert!(error.call_stack.is_empty());
        assert!(events(&context).is_empty());
    }
}

#[test]
fn signature_queries_observe_lexical_shadowing_and_restore_parent_visibility() {
    let mut context = Context::default();
    context
        .register_native("Value |value|", |values| Ok(values[0].clone()))
        .unwrap();
    context
        .register_native("Parent", |_| Ok(Literal::None))
        .unwrap();
    let parent = context
        .statement_signature("Value |x|")
        .unwrap()
        .unwrap()
        .help();
    context
        .with_invocation(
            Frame {
                parent: Some(0),
                ..Frame::default()
            },
            |context| {
                let program = Program::parse(
                    "local.botwork",
                    "Value |local| { Return |local| }\nChild {}",
                )
                .unwrap();
                evaluate_program_detailed(&program, context)?;
                let signatures = context.statement_signatures();
                assert_eq!(
                    signatures
                        .iter()
                        .map(|signature| signature.normalized())
                        .collect::<Vec<_>>(),
                    ["child", "parent", "value|param|"]
                );
                let signature = context.statement_signature("Value |x|")?.unwrap();
                assert_eq!(signature.origin(), StatementOrigin::Dsl);
                assert_eq!(signature.parameters()[0].name, "local");
                assert_eq!(context.complete_statements("Value").len(), 1);
                let call = parsed_call("Value |1|");
                assert_eq!(
                    context.signature_for_call(&call).unwrap().help(),
                    signature.help()
                );
                Ok(Completion::Normal(context.temporary(Literal::None)?))
            },
        )
        .unwrap();
    assert_eq!(
        context
            .statement_signature("Value |x|")
            .unwrap()
            .unwrap()
            .help(),
        parent
    );
    assert!(context.statement_signature("Child").unwrap().is_none());
}

#[test]
fn collection_access_retains_the_original_base_across_effectful_index_calls() {
    let mut context = Context::default();
    context
        .set_variable("data", Literal::Array(vec![Literal::Int(1)]))
        .unwrap();
    context
        .register_callback(
            "<test Swap>",
            "Swap",
            Arc::new(|_, context| {
                // One root binding and one access snapshot; the value tree was not copied.
                assert_eq!(
                    Arc::strong_count(context.get_variable_binding("data", None).unwrap()),
                    2
                );
                context
                    .set_variable("data", Literal::Array(vec![Literal::Int(2)]))
                    .unwrap();
                Ok(Literal::Int(0))
            }),
        )
        .unwrap();
    let program = Program::parse("snapshot.botwork", "|result| = |data[@{Swap}]|").unwrap();
    assert!(matches!(
        evaluate_program(&program, &mut context),
        Ok(Literal::Int(1))
    ));
    assert_eq!(context.get_variable("data").unwrap().to_string(), "[2]");
    assert_eq!(
        Arc::strong_count(context.get_variable_binding("data", None).unwrap()),
        1
    );
}

#[test]
fn context_clones_share_immutable_value_storage_and_isolate_replacement_bindings() {
    let mut original = Context::default();
    original
        .set_variable("data", Literal::Array(vec![Literal::Int(1)]))
        .unwrap();
    let mut cloned = original.clone();
    assert!(Arc::ptr_eq(
        original.get_variable_binding("data", None).unwrap(),
        cloned.get_variable_binding("data", None).unwrap()
    ));
    cloned
        .set_variable("data", Literal::Array(vec![Literal::Int(2)]))
        .unwrap();
    assert_eq!(original.get_variable("data").unwrap().to_string(), "[1]");
    assert_eq!(cloned.get_variable("data").unwrap().to_string(), "[2]");
}

#[test]
fn effectful_access_keeps_the_old_value_reservation_until_lookup_finishes() {
    use crate::core::run::RetainedValueLimits;
    let mut context = Context::with_limits(RunLimits {
        retained_values: RetainedValueLimits {
            values: 2,
            ..RetainedValueLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .set_variable("data", Literal::Array(vec![Literal::Int(1)]))
        .unwrap();
    context
        .register_callback(
            "<test Swap>",
            "Swap",
            Arc::new(|_, context| {
                context
                    .set_variable("data", Literal::Array(vec![Literal::Int(2)]))
                    .unwrap();
                // The original access snapshot still occupies the other stored allocation.
                context
                    .set_variable("extra", Literal::None)
                    .map_err(Diagnostic::into_error)?;
                Ok(Literal::Int(0))
            }),
        )
        .unwrap();
    let error = evaluate_program_detailed(
        &Program::parse("snapshot", "|result| = |data[@{Swap}]|").unwrap(),
        &mut context,
    )
    .unwrap_err();
    assert!(error.to_string().contains("retained values"));
    assert_eq!(context.frames[0].variables["data"].value.to_string(), "[2]");
    assert!(!context.frames[0].variables.contains_key("result"));
    assert!(!context.frames[0].variables.contains_key("extra"));
}
