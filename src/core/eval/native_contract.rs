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
    context.set_variable(
        "invalid".into(),
        Literal::Map([("x".into(), Literal::Array(vec![Literal::Float(f32::NAN)]))].into()),
    );
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
