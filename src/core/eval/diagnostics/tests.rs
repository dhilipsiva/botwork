use super::*;
use crate::core::diagnostic::DiagnosticLimits;
use crate::core::run::{RetainedDiagnosticLimits, SnapshotLimits};

#[test]
fn collision_construction_preserves_native_names_and_marks_only_replaced_location_fields() {
    let program = Program::parse("é.botwork", "Read {}").unwrap();
    let span = &program.statements[0].span;
    for native in [false, true] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        let mut sibling = context.clone();
        let error = context.duplicate_error(
            |[signature, original, duplicate]| BWErr::DuplicateStatement {
                signature,
                original,
                duplicate,
            },
            "read",
            (span, native),
            span,
            "first definition",
        );
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(
            error.causes[0].omissions.as_ref().unwrap().detail_fields,
            if native { 1 } else { 2 }
        );
        let BWErr::DuplicateStatement {
            original,
            duplicate,
            ..
        } = error.causes[0].error.as_ref()
        else {
            panic!("duplicate")
        };
        if native {
            assert_eq!(original, "é.botwork");
        } else {
            assert!(original.ends_with("coordinates omitted]"));
        }
        assert!(duplicate.ends_with("coordinates omitted]"));
        assert!(context.checkpoint().is_err());
        assert!(sibling.checkpoint().is_ok());
        let next = Program::parse("next", "|x| = |1|").unwrap();
        assert!(evaluate_program_detailed(&next, &mut context).is_err());
        evaluate_program_detailed(&next, &mut sibling).unwrap();
    }
}

#[test]
fn snapshots_share_call_and_handler_records_and_charge_their_copied_handles() {
    let program = Program::parse("snapshot", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let mut context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            records: 2,
            ..RetainedDiagnosticLimits::default()
        },
        snapshots: SnapshotLimits {
            entries: 2,
            ..SnapshotLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .calls
        .push(context.retain_call("read", span, None).unwrap());
    context.handlers.push(HandledError {
        invocation: 0,
        diagnostic: context
            .retain_handler(Diagnostic::new(BWErr::NativeError("reason".into())).at(span))
            .unwrap(),
    });
    let held = context.try_clone().unwrap();
    assert!(Arc::ptr_eq(&context.calls[0], &held.calls[0]));
    assert!(Arc::ptr_eq(
        &context.handlers[0].diagnostic,
        &held.handlers[0].diagnostic
    ));
    assert!(context.try_clone().is_err());
    held.checkpoint().unwrap();
    let mut probe = held.clone();
    probe.calls.clear();
    probe.handlers.clear();
    context.calls.clear();
    context.handlers.clear();
    let failed = probe.clone();
    assert!(failed
        .retain_handler(BWErr::NativeError("new".into()).into())
        .is_err());
    assert!(failed.checkpoint().is_err());
    probe.checkpoint().unwrap();
    drop(held);
    drop(
        probe
            .retain_handler(BWErr::NativeError("new".into()).into())
            .unwrap(),
    );
    probe.checkpoint().unwrap();
}

#[test]
fn stopped_calls_release_their_record_and_cancelled_handler_admission_keeps_evidence() {
    let program = Program::parse("cancel", "|x| = |1|").unwrap();
    let control = OperationControl::default();
    let mut context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
    let mut weak = std::sync::Weak::new();
    let result = context.with_call("stop", &program.statements[0].span, None, |context| {
        weak = Arc::downgrade(context.calls.last().unwrap());
        control.cancel();
        context.temporary(Literal::None)
    });
    assert_eq!(result.unwrap_err().code(), DiagnosticCode::Cancelled);
    assert!(weak.upgrade().is_none());
    assert!(context.calls.is_empty());
    let error = context
        .retain_handler(BWErr::NativeError("original".into()).into())
        .err()
        .unwrap();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    assert!(error.is_emergency());
}
