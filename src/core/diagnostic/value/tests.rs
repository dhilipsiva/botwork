use super::*;
use crate::core::{ast::Program, value_limits::discard};
use std::fmt::Write;

fn diagnostic() -> Diagnostic {
    let program = Program::parse("é.botwork", "# தமிழ்\r\n\t|x| = |1|").unwrap();
    let span = program.statements[0].span.clone();
    let mut value = Diagnostic::new(BWErr::undefined_variable("café".into())).at(&span);
    value.call_stack.push(CallFrame {
        signature: "read ||".into(),
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    value.call_stack.push(CallFrame {
        signature: "native".into(),
        call_site: span.clone(),
        definition_site: None,
    });
    value.related.push(RelatedLocation {
        message: "first".into(),
        span,
    });
    value.causes.push(Diagnostic::new(BWErr::ResourceLimit {
        resource: "steps",
        limit: u64::MAX,
    }));
    value
}

fn resource(error: BWErr) -> &'static str {
    let BWErr::ResourceLimit { resource, .. } = error else {
        panic!("expected limit")
    };
    resource
}

#[test]
fn measured_shape_matches_owned_unicode_calls_related_causes_and_decimal_limits() {
    let diagnostic = diagnostic();
    let limits = DiagnosticValueLimits::default();
    let size = measure(&diagnostic, &limits).unwrap();
    let owned = diagnostic.to_value_with_limits(&limits).unwrap();
    assert_eq!(size, limits.values.check(&owned).unwrap());
    let Literal::Map(map) = owned else {
        panic!("map")
    };
    let Literal::Array(causes) = &map["causes"] else {
        panic!("causes")
    };
    let Literal::Map(cause) = &causes[0] else {
        panic!("cause")
    };
    let Literal::Map(details) = &cause["details"] else {
        panic!("details")
    };
    assert_eq!(details["limit"].to_string(), u64::MAX.to_string());
}

#[test]
fn every_value_quota_has_an_exact_boundary() {
    let diagnostic = Diagnostic::new(BWErr::NativeError("λ".repeat(100)));
    let default = DiagnosticValueLimits::default();
    let size = measure(&diagnostic, &default).unwrap();
    let string_bytes = diagnostic.error.to_string().len();
    for (field, exact, expected) in [
        (0, size.nodes, "value nodes"),
        (1, size.depth, "value depth"),
        (2, size.payload_bytes, "value payload bytes"),
        (3, string_bytes, "value string bytes"),
        (4, "call_stack".len(), "value key bytes"),
        (5, 8, "value container entries"),
    ] {
        for accepted in [true, false] {
            let mut limits = default.clone();
            let target = match field {
                0 => &mut limits.values.nodes,
                1 => &mut limits.values.depth,
                2 => &mut limits.values.payload_bytes,
                3 => &mut limits.values.string_bytes,
                4 => &mut limits.values.key_bytes,
                _ => &mut limits.values.entries,
            };
            *target = exact - usize::from(!accepted);
            let measured = measure(&diagnostic, &limits);
            if accepted {
                assert_eq!(measured.unwrap(), size);
            } else {
                assert_eq!(resource(measured.unwrap_err()), expected);
            }
        }
    }
}

#[test]
fn help_formatting_is_counted_before_copy_and_depth_configuration_is_validated() {
    let diagnostic = Diagnostic::new(BWErr::undefined_variable("λ".repeat(100)));
    let mut limits = DiagnosticValueLimits::default();
    limits.values.string_bytes = diagnostic.help().len();
    measure(&diagnostic, &limits).unwrap();
    limits.values.string_bytes -= 1;
    assert_eq!(
        resource(measure(&diagnostic, &limits).unwrap_err()),
        "value string bytes"
    );
    limits.values.depth = 65;
    assert!(matches!(
        measure(&diagnostic, &limits),
        Err(BWErr::RunConfiguration(_))
    ));
}

#[test]
fn position_budget_counts_each_source_occurrence_and_allows_exact_scan_work() {
    let diagnostic = diagnostic();
    let span = diagnostic.span.as_ref().unwrap();
    // Primary, two call sites, one definition site, one related site.
    let mut limits = DiagnosticValueLimits {
        position_bytes: 5 * 6 * (span.start() + span.end()),
        ..DiagnosticValueLimits::default()
    };
    diagnostic.to_value_with_limits(&limits).unwrap();
    limits.position_bytes -= 1;
    assert_eq!(
        resource(measure(&diagnostic, &limits).unwrap_err()),
        "diagnostic position bytes"
    );
    limits.position_bytes = 0;
    measure(
        &Diagnostic::new(BWErr::NativeError("reason".into())),
        &limits,
    )
    .unwrap();
    let origin =
        Diagnostic::new(BWErr::NativeError("reason".into())).at(&Span::input_origin("input"));
    measure(&origin, &limits).unwrap();
}

#[test]
fn counting_and_scan_arithmetic_reject_overflow_without_modifying_counters() {
    let mut counter = Counter {
        bytes: usize::MAX,
        limit: usize::MAX,
    };
    assert!(counter.write_str("x").is_err());
    assert_eq!(counter.bytes, usize::MAX);
    for (total, start, end) in [
        (0, usize::MAX, 1),
        (0, usize::MAX / 6 + 1, 0),
        (usize::MAX, 1, 0),
    ] {
        let mut current = total;
        assert_eq!(
            resource(charge_position(&mut current, start, end, usize::MAX).unwrap_err()),
            "diagnostic position bytes"
        );
        assert_eq!(current, total);
    }
}

#[test]
fn deep_causes_reject_iteratively_and_host_compatibility_build_is_iterative() {
    let make = || Diagnostic::new(BWErr::NativeError("reason".into()));
    let mut diagnostic = make();
    for _ in 0..100_000 {
        let mut parent = make();
        parent.causes.push(diagnostic);
        diagnostic = parent;
    }
    assert_eq!(
        resource(measure(&diagnostic, &DiagnosticValueLimits::default()).unwrap_err()),
        "value depth"
    );
    // Host owns unadmitted diagnostics and their cleanup; remove the deep tail iteratively.
    for _ in 0..99_000 {
        diagnostic = diagnostic.causes.pop().unwrap();
    }
    discard(diagnostic.to_value());
    while let Some(child) = diagnostic.causes.pop() {
        diagnostic = child;
    }
}

#[test]
fn wide_causes_fail_before_traversing_children() {
    let mut diagnostic = Diagnostic::new(BWErr::NativeError("reason".into()));
    diagnostic.causes = (0..16_385)
        .map(|_| Diagnostic::new(BWErr::NativeError("".into())))
        .collect();
    assert_eq!(
        resource(measure(&diagnostic, &DiagnosticValueLimits::default()).unwrap_err()),
        "value container entries"
    );
    diagnostic.causes.clear();
    assert!(measure(&diagnostic, &DiagnosticValueLimits::default()).is_ok());
}

#[test]
fn run_intersection_uses_the_tighter_limit_for_each_dimension() {
    let left = DiagnosticValueLimits {
        values: ValueLimits {
            nodes: 1,
            depth: 2,
            string_bytes: 3,
            key_bytes: 4,
            entries: 5,
            payload_bytes: 6,
        },
        position_bytes: 7,
    };
    let right = ValueLimits {
        nodes: 6,
        depth: 5,
        string_bytes: 4,
        key_bytes: 3,
        entries: 2,
        payload_bytes: 1,
    };
    let combined = left.intersect(&right);
    assert_eq!(
        [
            combined.values.nodes,
            combined.values.depth,
            combined.values.string_bytes,
            combined.values.key_bytes,
            combined.values.entries,
            combined.values.payload_bytes,
            combined.position_bytes
        ],
        [1, 2, 3, 3, 2, 1, 7]
    );
}
