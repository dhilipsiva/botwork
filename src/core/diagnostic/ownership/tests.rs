use super::*;
use crate::core::{
    ast::Program,
    diagnostic::{CallFrame, RelatedLocation},
};

fn tree() -> Diagnostic {
    let program = Program::parse("é", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let mut root = Diagnostic::new(BWErr::NativeError("é".into())).at(span);
    root.call_stack.push(CallFrame {
        signature: "read".into(),
        statement: None,
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    root.related.push(RelatedLocation {
        message: "first".into(),
        span: span.clone(),
    });
    root.causes
        .push(Diagnostic::new(BWErr::Cancelled("stop".into())).at(span));
    root
}

#[test]
fn exact_counts_include_label_utf8_context_and_unique_source_owners() {
    let root = tree();
    let limits = DiagnosticLimits::default();
    let size = limits.check(&root).unwrap();
    assert_eq!(
        size,
        DiagnosticSize {
            diagnostics: 2,
            depth: 2,
            call_frames: 1,
            related_locations: 1,
            text_bytes: 2 * "source".len()
                + "é".len()
                + "read".len()
                + "first".len()
                + "stop".len(),
            source_bytes: "é".len() + "|x| = |1|".len(),
        }
    );
    for field in 0..6 {
        for accepted in [true, false] {
            let mut limits = limits.clone();
            let (target, exact) = match field {
                0 => (&mut limits.diagnostics, size.diagnostics),
                1 => (&mut limits.depth, size.depth),
                2 => (&mut limits.call_frames, size.call_frames),
                3 => (&mut limits.related_locations, size.related_locations),
                4 => (&mut limits.text_bytes, size.text_bytes),
                _ => (&mut limits.source_bytes, size.source_bytes),
            };
            *target = exact - usize::from(!accepted);
            assert_eq!(limits.check(&root).is_ok(), accepted);
            assert_eq!(root.try_clone_with_limits(&limits).is_ok(), accepted);
        }
    }
}

#[test]
fn source_identity_distinguishes_equal_allocations_and_checks_names_too() {
    let mut root = tree();
    let first = DiagnosticLimits::default().check(&root).unwrap();
    let program = Program::parse("é", "|x| = |1|").unwrap();
    root.causes[0].span = Some(program.statements[0].span.clone());
    let second = DiagnosticLimits::default().check(&root).unwrap();
    assert_eq!(second.source_bytes, 2 * first.source_bytes);
    let limits = DiagnosticLimits {
        source_bytes: 1,
        ..DiagnosticLimits::default()
    };
    assert!(limits.check(&root).is_err());
}

#[test]
fn checked_arithmetic_and_unsupported_depth_fail_without_mutation() {
    let mut value = usize::MAX;
    assert!(add(&mut value, 1, usize::MAX, "diagnostic nodes").is_err());
    assert_eq!(value, usize::MAX);
    assert!(matches!(
        DiagnosticLimits {
            depth: 65,
            ..DiagnosticLimits::default()
        }
        .check(&tree()),
        Err(BWErr::RunConfiguration(_))
    ));
}

#[test]
fn clones_share_immutable_identity_and_sources_but_copy_mutable_vectors() {
    let original = tree();
    let mut cloned = original
        .try_clone_with_limits(&DiagnosticLimits::default())
        .unwrap();
    assert!(Arc::ptr_eq(&original.error, &cloned.error));
    assert!(Arc::ptr_eq(
        original.span.as_ref().unwrap().source(),
        cloned.span.as_ref().unwrap().source()
    ));
    assert!(Arc::ptr_eq(
        &original.causes[0].error,
        &cloned.causes[0].error
    ));
    cloned.call_stack[0].signature.push_str(" changed");
    cloned.related[0].message.clear();
    cloned.causes[0].label = "changed";
    assert_eq!(original.call_stack[0].signature, "read");
    assert_eq!(original.related[0].message, "first");
    assert_eq!(original.causes[0].label, "source");
}

fn deep(depth: usize) -> (Diagnostic, std::sync::Weak<BWErr>) {
    let mut diagnostic = Diagnostic::new(BWErr::NativeError("leaf".into()));
    let leaf = Arc::downgrade(&diagnostic.error);
    for _ in 1..depth {
        let mut root = Diagnostic::new(BWErr::NativeError("parent".into()));
        root.causes.push(diagnostic);
        diagnostic = root;
    }
    (diagnostic, leaf)
}

#[test]
fn deep_rejection_clone_explicit_discard_and_owned_guard_are_iterative() {
    let (original, leaf) = deep(100_000);
    assert!(matches!(
        DiagnosticLimits::default().check(&original),
        Err(BWErr::ResourceLimit {
            resource: "diagnostic depth",
            ..
        })
    ));
    let cloned = original.clone();
    original.discard();
    assert!(leaf.upgrade().is_some());
    drop(OwnedDiagnostic::new(cloned));
    assert!(leaf.upgrade().is_none());
}

#[test]
fn compatibility_category_conversion_and_guard_transfer_destroy_deep_causes_safely() {
    let (original, leaf) = deep(100_000);
    let root = OwnedDiagnostic::new(original).into_inner().into_error();
    assert!(leaf.upgrade().is_none());
    assert!(matches!(root, BWErr::NativeError(text) if text == "parent"));
    let shared = tree();
    assert_eq!(shared.clone().into_error().code(), shared.code());
}

#[test]
fn source_free_empty_and_wide_trees_have_explicit_boundaries() {
    let mut root = Diagnostic::new(BWErr::NativeError("".into()));
    let limits = DiagnosticLimits {
        source_bytes: 0,
        call_frames: 0,
        related_locations: 0,
        text_bytes: "source".len(),
        ..DiagnosticLimits::default()
    };
    assert!(limits.check(&root).is_ok());
    root.causes = (0..1024)
        .map(|_| Diagnostic::new(BWErr::NativeError("".into())))
        .collect();
    assert!(matches!(
        limits.check(&root),
        Err(BWErr::ResourceLimit {
            resource: "diagnostic nodes",
            ..
        })
    ));
    let raised = DiagnosticLimits {
        diagnostics: 1025,
        text_bytes: 1025 * "source".len(),
        ..limits
    };
    assert_eq!(raised.check(&root).unwrap().diagnostics, 1025);
    root.discard();
}

#[test]
fn prospective_stack_matches_owned_metrics_and_preserves_an_existing_snapshot() {
    let original = tree();
    let frames = original.call_stack.clone();
    let mut empty = original.clone();
    empty.call_stack.clear();
    let limits = DiagnosticLimits::default();
    assert_eq!(
        limits.check_with_stack(&empty, frames.iter()).unwrap(),
        limits.check(&original).unwrap()
    );
    let mut incoming = frames.clone();
    incoming.extend(frames.clone());
    let limits = DiagnosticLimits {
        call_frames: 1,
        ..limits
    };
    let rejection = limits.admit_with_stack(empty, incoming.iter()).unwrap_err();
    assert_eq!(
        rejection.causes[0].omissions.as_ref().unwrap().call_frames,
        2
    );
    let accepted = limits.admit_with_stack(original, incoming.iter()).unwrap();
    assert_eq!(accepted.call_stack.len(), 1);
    assert_eq!(accepted.call_stack[0].signature, "read");
}

#[test]
fn prospective_cause_measurement_matches_attached_tree_and_checks_shifted_depth() {
    let primary = tree();
    let cause = tree(); // Equal source text, distinct allocation.
    let limits = DiagnosticLimits::default();
    let (size, sources) = limits
        .retained_mutation_size(&primary, None, Some(&cause))
        .unwrap();
    let attached = primary.clone().while_handling(cause.clone());
    assert_eq!(size, limits.check(&attached).unwrap());
    assert_eq!(size.depth, 3);
    assert_eq!(sources.len(), 2);
    let mut cause = cause;
    cause.span = primary.span.clone();
    cause.call_stack = primary.call_stack.clone();
    cause.related = primary.related.clone();
    cause.causes[0].span = primary.span.clone();
    let (shared, sources) = limits
        .retained_mutation_size(&primary, None, Some(&cause))
        .unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(shared.source_bytes * 2, size.source_bytes);
    assert!(matches!(
        DiagnosticLimits { depth: 2, ..limits }.retained_mutation_size(
            &primary,
            None,
            Some(&cause)
        ),
        Err(BWErr::ResourceLimit {
            resource: "diagnostic depth",
            limit: 2
        })
    ));
}

#[test]
fn prospective_runtime_context_matches_owned_location_label_and_stack_without_replacement() {
    let first = tree();
    let program = Program::parse("new location", "Missing").unwrap();
    let context = Some((&program.statements[0].span, true));
    for existing in [false, true] {
        let mut error = first.clone();
        if !existing {
            error.span = None;
            error.call_stack.clear();
            error.label = "custom label that will be replaced";
        }
        let limits = DiagnosticLimits::default();
        let (prospective, sources) = limits
            .retained_runtime_size(&error, first.call_stack.iter(), context, None, None)
            .unwrap();
        let expected = error
            .clone()
            .capture_context(context, first.call_stack.iter());
        assert_eq!(prospective, limits.check(&expected).unwrap());
        assert_eq!(sources.len(), if existing { 1 } else { 2 });
        assert_eq!(
            expected.label,
            if existing { "source" } else { "expression" }
        );
        let exact = DiagnosticLimits {
            text_bytes: prospective.text_bytes,
            ..limits
        };
        assert!(exact
            .retained_runtime_size(&error, first.call_stack.iter(), context, None, None)
            .is_ok());
        // Attaching a statement span preserves the caller's custom label; expressions replace it.
        if !existing {
            assert!(exact
                .retained_runtime_size(
                    &error,
                    first.call_stack.iter(),
                    Some((&program.statements[0].span, false)),
                    None,
                    None
                )
                .is_err());
        }
    }
}
