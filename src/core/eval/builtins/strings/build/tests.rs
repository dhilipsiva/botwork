use super::*;

#[test]
fn string_size_accumulation_rejects_overflow_without_advancing_the_counter() {
    use std::fmt::Write;
    let context = Context::default();
    let mut counter = Counter {
        context: &context,
        limits: context.limits().values,
        bytes: usize::MAX,
        failure: None,
    };
    assert!(counter.write_str("x").is_err());
    assert_eq!(counter.bytes, usize::MAX);
    assert_eq!(
        counter.failure.unwrap().into_diagnostic().code(),
        Code::ResourceLimit
    );
}

#[test]
fn case_plans_match_full_unicode_rendering_including_contextual_sigma() {
    let context = Context::default();
    for value in [
        "",
        "ABC",
        "ΟΣ Σ ΟΣΑ",
        "İ Straße ﬃ ΐ",
        "a🙂é",
        "தமிழ்",
        "Σ\u{301} ΟΣ\u{301}",
    ] {
        for upper in [false, true] {
            let result = if upper {
                value.to_uppercase()
            } else {
                value.to_lowercase()
            };
            assert_eq!(
                case_size(&context, value, upper).unwrap(),
                context
                    .limits()
                    .values
                    .check(&Literal::String(result))
                    .unwrap()
            );
        }
    }
}

#[test]
fn rendering_rejects_changed_lengths_without_extending_the_reserved_result() {
    for second in ["", "longer"] {
        let context = Context::default();
        let error = render(&context, |output, sorted| {
            output.write_str(if sorted { second } else { "x" })?;
            Ok(())
        })
        .unwrap_err()
        .into_diagnostic();
        assert_eq!(error.code(), Code::IncompatibleType);
        assert!(error.to_string().contains("String rendering"));
    }
}

#[test]
fn rendering_observes_cancellation_before_copying_the_next_output_chunk() {
    let control = crate::core::operation::OperationControl::default();
    let context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    let error = render(&context, |output, sorted| {
        output.write_str("first")?;
        if sorted {
            control.cancel();
        }
        output.write_str("second")?;
        Ok(())
    })
    .unwrap_err()
    .into_diagnostic();
    assert_eq!(error.code(), Code::Cancelled);
}

#[test]
fn empty_outputs_still_require_one_admitted_value_node() {
    let context = Context::with_limits(RunLimits {
        values: crate::core::value_limits::ValueLimits {
            nodes: 0,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        render(&context, |_, _| Ok(()))
            .unwrap_err()
            .into_diagnostic()
            .code(),
        Code::ResourceLimit
    );
}
