use super::*;

#[test]
fn defaults_match_retained_root_plus_one_terminal_value() {
    let limits = ResultLimits::default();
    assert_eq!(limits.values, RetainedValueLimits::default().values + 1);
    assert_eq!(
        limits.nodes,
        RetainedValueLimits::default().nodes + ValueLimits::default().nodes
    );
    assert_eq!(
        limits.payload_bytes,
        RetainedValueLimits::default().payload_bytes + ValueLimits::default().payload_bytes
    );
    assert_eq!(limits.name_bytes, RetainedNameLimits::default().total_bytes);
}

#[test]
fn counts_and_names_reject_overflow_and_accept_exact_or_zero_limits() {
    let limits = ResultLimits {
        values: usize::MAX,
        nodes: usize::MAX,
        name_bytes: usize::MAX,
        payload_bytes: usize::MAX,
    };
    limits.count(usize::MAX, false).unwrap();
    assert!(limits.count(usize::MAX, true).is_err());
    limits.names([usize::MAX].into_iter()).unwrap();
    assert!(limits.names([usize::MAX, 1].into_iter()).is_err());
    let limits = ResultLimits {
        values: 0,
        nodes: 0,
        name_bytes: 0,
        payload_bytes: 0,
    };
    limits.count(0, false).unwrap();
    limits.names([0].into_iter()).unwrap();
    assert!(limits.count(0, true).is_err());
    assert!(limits
        .value(&Literal::None, &mut ValueSize::default())
        .is_err());
}

#[test]
fn aggregate_measurement_counts_map_keys_nodes_and_numeric_bytes() {
    let value = Literal::Array(vec![
        Literal::Int(1),
        Literal::Map(std::collections::HashMap::from([(
            "κ".into(),
            Literal::Bool(true),
        )])),
    ]);
    let limits = ResultLimits {
        values: 2,
        nodes: 8,
        name_bytes: 2,
        payload_bytes: 14,
    };
    let mut used = ValueSize::default();
    limits.value(&value, &mut used).unwrap();
    assert_eq!((used.nodes, used.payload_bytes), (4, 7));
    limits.value(&value, &mut used).unwrap();
    assert_eq!((used.nodes, used.payload_bytes), (8, 14));
    assert!(limits
        .value(&Literal::None, &mut used)
        .unwrap_err()
        .to_string()
        .contains("result nodes"));
    let limits = ResultLimits { nodes: 9, ..limits };
    assert!(limits
        .value(&Literal::Bool(true), &mut used)
        .unwrap_err()
        .to_string()
        .contains("result payload bytes"));
}
