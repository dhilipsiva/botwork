use super::*;

#[test]
fn scalar_and_empty_container_metrics_have_stable_payload_sizes() {
    for (value, bytes) in [
        (Literal::None, 0),
        (Literal::Bool(true), 1),
        (Literal::Int(7), 4),
        (Literal::Float(1.5), 4),
        (Literal::String("é".into()), 2),
        (Literal::Array(vec![]), 0),
        (Literal::Map(Default::default()), 0),
    ] {
        assert_eq!(
            ValueLimits::default().check(&value).unwrap(),
            ValueSize {
                nodes: 1,
                depth: 1,
                payload_bytes: bytes
            }
        );
    }
}

#[test]
fn payload_accounting_uses_checked_arithmetic() {
    let limits = ValueLimits {
        payload_bytes: usize::MAX,
        ..ValueLimits::default()
    };
    let mut size = ValueSize {
        payload_bytes: usize::MAX,
        ..ValueSize::default()
    };
    assert!(limits.add_bytes(&mut size, 1).is_err());
    assert_eq!(size.payload_bytes, usize::MAX);
}

#[test]
fn depth_boundary_is_accepted_and_owned_release_preserves_values() {
    let value = (1..MAX_VALUE_DEPTH).fold(Literal::None, |value, _| Literal::Array(vec![value]));
    assert_eq!(
        ValueLimits::default().check(&value).unwrap().depth,
        MAX_VALUE_DEPTH
    );
    let value = Owned::new(value).into_inner();
    let over = Owned::new(Literal::Array(vec![value]));
    assert!(ValueLimits::default().check(&over).is_err());
}

#[test]
fn clearing_root_collections_empties_them_without_changing_live_ownership() {
    let mut values = Owned::new(vec![Literal::Array(vec![Literal::Int(7)])]);
    assert_eq!(values.pop().unwrap().to_string(), "[7]");
    assert!(values.into_inner().is_empty());
    let mut variables = Owned::new(BTreeMap::from([("x".into(), Literal::Bool(true))]));
    assert_eq!(variables.remove("x").unwrap().to_string(), "true");
    assert!(variables.into_inner().is_empty());
}

#[test]
fn construction_accounting_is_atomic_on_each_overflow() {
    let limits = ValueLimits {
        nodes: usize::MAX,
        payload_bytes: usize::MAX,
        entries: usize::MAX,
        ..ValueLimits::default()
    };
    for (mut parent, child) in [
        (
            ValueSize {
                nodes: usize::MAX,
                depth: 1,
                payload_bytes: 0,
            },
            ValueSize {
                nodes: 1,
                depth: 1,
                payload_bytes: 0,
            },
        ),
        (
            ValueSize {
                nodes: 1,
                depth: 1,
                payload_bytes: 0,
            },
            ValueSize {
                nodes: 1,
                depth: usize::MAX,
                payload_bytes: 0,
            },
        ),
        (
            ValueSize {
                nodes: 1,
                depth: 1,
                payload_bytes: usize::MAX,
            },
            ValueSize {
                nodes: 1,
                depth: 1,
                payload_bytes: 1,
            },
        ),
    ] {
        let original = parent;
        assert!(limits.add_child(&mut parent, child).is_err());
        assert_eq!(parent, original);
    }
    assert!(limits.container_header(usize::MAX).is_err());
}
