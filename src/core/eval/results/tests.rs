use super::*;

#[test]
fn unique_export_moves_payload_and_name_allocations() {
    let mut context = Context::default();
    context
        .set_variable("name", Literal::String("large".repeat(1000)))
        .unwrap();
    let (name, stored) = context.frames[0].variables.get_key_value("name").unwrap();
    let name_pointer = name.as_str().as_ptr();
    let Literal::String(value) = &stored.value else {
        panic!()
    };
    let value_pointer = value.as_ptr();
    let (result, variables, error) =
        context.finish_result(Ok(Literal::None), &ResultLimits::default());
    result.unwrap();
    assert!(error.is_none());
    let (name, Literal::String(value)) = variables.first_key_value().unwrap() else {
        panic!()
    };
    assert_eq!(name.as_ptr(), name_pointer);
    assert_eq!(value.as_ptr(), value_pointer);
}

#[test]
fn shared_export_copies_after_admission_and_keeps_other_owner_unchanged() {
    let mut context = Context::default();
    context
        .set_variable("name", Literal::String("value".into()))
        .unwrap();
    let retained = context.clone();
    let (name, stored) = retained.frames[0].variables.get_key_value("name").unwrap();
    let Literal::String(value) = &stored.value else {
        panic!()
    };
    let (_, variables, error) = context.finish_result(Ok(Literal::None), &ResultLimits::default());
    assert!(error.is_none());
    let (copied_name, Literal::String(copied_value)) = variables.first_key_value().unwrap() else {
        panic!()
    };
    assert_eq!(copied_value, value);
    assert_ne!(copied_value.as_ptr(), value.as_ptr());
    assert_ne!(copied_name.as_ptr(), name.as_str().as_ptr());
    retained.checkpoint().unwrap();
}

#[test]
fn rejected_export_drops_all_roots_and_does_not_stop_shared_context() {
    let mut context = Context::default();
    context.set_variable("x", Literal::Int(7)).unwrap();
    let retained = context.clone();
    let (result, variables, error) = context.finish_result(
        Ok(Literal::Int(1)),
        &ResultLimits {
            values: 0,
            ..ResultLimits::default()
        },
    );
    assert!(result.is_err());
    assert!(variables.is_empty());
    assert!(error.is_some());
    assert_eq!(Arc::strong_count(&retained.frames[0].variables["x"]), 1);
    retained.checkpoint().unwrap();
}

#[test]
fn rejected_deep_terminal_data_is_discarded_iteratively() {
    let mut value = Literal::None;
    for _ in 0..100_000 {
        value = Literal::Array(vec![value]);
    }
    let (result, variables, error) =
        Context::default().finish_result(Ok(value), &ResultLimits::default());
    assert!(result.unwrap_err().to_string().contains("value depth"));
    assert!(variables.is_empty());
    assert!(error.is_some());
}
