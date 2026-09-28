use super::*;

#[test]
fn output_planners_match_independently_measured_owned_values() {
    let limits = ValueLimits::default();
    let children = vec![
        Literal::Int(1),
        Literal::Array(vec![Literal::String("é".into()), Literal::Bool(true)]),
    ];
    assert_eq!(
        array_size(&limits, 2, children.iter().map(|v| limits.check(v))).unwrap(),
        limits.check(&Literal::Array(children.clone())).unwrap()
    );
    let map: HashMap<String, Literal> = HashMap::from([
        ("🙂".into(), children[0].clone()),
        ("".into(), children[1].clone()),
    ]);
    assert_eq!(
        map_size(&limits, 2, map.iter().map(|(k, v)| (k.as_str(), v))).unwrap(),
        limits.check(&Literal::Map(map)).unwrap()
    );
    for count in [0, 1, 2, 7] {
        for value in &children {
            assert_eq!(
                build::repeat_size(&limits, value, count).unwrap(),
                limits
                    .check(&Literal::Array(vec![value.clone(); count]))
                    .unwrap()
            );
        }
    }
}

#[test]
fn repetition_rejects_checked_count_multiplication_overflow_without_allocating() {
    let limits = ValueLimits {
        entries: usize::MAX,
        nodes: usize::MAX,
        payload_bytes: usize::MAX,
        ..ValueLimits::default()
    };
    for (value, count, resource) in [
        (Literal::None, usize::MAX, "value nodes"),
        (
            Literal::Array(vec![Literal::None]),
            usize::MAX / 2 + 1,
            "value nodes",
        ),
        (
            Literal::String("1234".into()),
            usize::MAX / 3,
            "value payload bytes",
        ),
    ] {
        assert!(
            matches!(build::repeat_size(&limits, &value, count), Err(BWErr::ResourceLimit { resource: actual, .. }) if actual == resource)
        );
    }
}

#[test]
fn length_and_enumeration_index_conversion_are_checked_at_int_boundary() {
    let context = Context::default();
    assert_eq!(integer(&context, i32::MAX as usize).unwrap(), i32::MAX);
    for value in [i32::MAX as usize + 1, usize::MAX] {
        assert_eq!(
            integer(&context, value)
                .unwrap_err()
                .into_diagnostic()
                .code(),
            Code::Arithmetic
        );
    }
}

#[test]
fn zero_count_is_an_empty_container_and_exact_quotas_are_accepted() {
    let limits = ValueLimits {
        entries: 0,
        nodes: 1,
        depth: 1,
        payload_bytes: 0,
        ..ValueLimits::default()
    };
    assert_eq!(
        build::repeat_size(&limits, &Literal::String("unused".into()), 0).unwrap(),
        ValueSize {
            nodes: 1,
            depth: 1,
            payload_bytes: 0
        }
    );
    let limits = ValueLimits {
        entries: 2,
        nodes: 5,
        depth: 3,
        payload_bytes: 4,
        ..ValueLimits::default()
    };
    assert_eq!(
        build::repeat_size(
            &limits,
            &Literal::Array(vec![Literal::String("é".into())]),
            2
        )
        .unwrap(),
        ValueSize {
            nodes: 5,
            depth: 3,
            payload_bytes: 4
        }
    );
}
