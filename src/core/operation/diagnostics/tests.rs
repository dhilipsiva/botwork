use super::*;
use crate::core::ast::Program;
use std::cell::Cell;

fn operation(budget: OperationBudget) -> NativeOperation {
    NativeOperation::asynchronous(StatementSignature::native("Read").unwrap(), |_, _| async {
        Ok(Literal::None)
    })
    .unwrap()
    .with_ownership_budget(budget)
}

#[test]
fn construction_reserves_before_counting_and_keeps_capacity_through_handoff() {
    struct Observe {
        operation: NativeOperation,
        pass: Cell<usize>,
    }
    impl std::fmt::Display for Observe {
        fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let usage = self.operation.ownership.usage();
            assert_eq!(usage.diagnostics, 1);
            assert_eq!(usage.text_bytes, if self.pass.get() == 0 { 6 } else { 12 });
            self.pass.set(self.pass.get() + 1);
            let competitor = self.operation.clone();
            std::thread::spawn(move || {
                let rejected = competitor.detail_error(BWErr::NativeError, "other");
                assert!(rejected.is_emergency());
            })
            .join()
            .unwrap();
            output.write_str("reason")
        }
    }
    let budget = OperationBudget::new(OperationOwnershipLimits {
        diagnostics: 1,
        text_bytes: 12,
        ..Default::default()
    });
    let operation = operation(budget.clone());
    let detail = Observe {
        operation: operation.clone(),
        pass: Cell::new(0),
    };
    let error = operation.constructed_error(BWErr::NativeError, format_args!("{detail}"), None);
    assert_eq!(detail.pass.get(), 2);
    assert_eq!(budget.usage().text_bytes, 12);
    let tracked = operation.track_error(error, false, true, &None);
    assert_eq!(tracked.value.as_ref().code(), DiagnosticCode::Native);
    assert_eq!(budget.usage().diagnostics, 1);
    let published = tracked.into_inner().into_inner();
    assert_eq!(budget.usage(), OperationUsage::default());
    assert_eq!(published.span.as_ref().unwrap().text(), "Read");
}

#[test]
fn cleanup_construction_credits_primary_and_releases_sources_after_rejected_extension() {
    let program = Program::parse("primary-source", "Read").unwrap();
    let primary = Diagnostic::new(BWErr::Cancelled("stop".into())).at(&program.statements[0].span);
    let size = DiagnosticLimits::default().check(&primary).unwrap();
    let budget = OperationBudget::new(OperationOwnershipLimits {
        diagnostics: 2,
        text_bytes: size.text_bytes + 6,
        ..Default::default()
    });
    let operation = operation(budget.clone());
    let source = Arc::downgrade(&program.source);
    let primary = operation.track_error(primary, true, true, &None);
    drop(program);
    let Tracked {
        value, reservation, ..
    } = *primary;
    let error = operation.constructed_error(
        BWErr::AsyncRuntime,
        format_args!("new cause"),
        Some(PendingDiagnostic { value, reservation }),
    );
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert_eq!(error.omissions.as_ref().unwrap().direct_causes, 1);
    assert!(source.upgrade().is_none());
    assert_eq!(budget.usage(), OperationUsage::default());
    drop(error);
    assert!(!operation
        .detail_error(BWErr::NativeError, "next")
        .is_emergency());
}

#[test]
fn cause_merging_deduplicates_sources_and_admits_depth_before_insertion() {
    for depth in [1, 2] {
        let budget = OperationBudget::new(OperationOwnershipLimits {
            diagnostics: 2,
            ..Default::default()
        });
        let operation = operation(budget.clone())
            .with_diagnostic_limits(DiagnosticLimits {
                depth,
                ..Default::default()
            })
            .unwrap();
        let primary = operation.track_error(
            Diagnostic::new(BWErr::Cancelled("stop".into())),
            true,
            true,
            &None,
        );
        let cause = operation.track_error(
            Diagnostic::new(BWErr::NativeError("cause".into())),
            false,
            false,
            &None,
        );
        let separate_sources = budget.usage().source_bytes;
        let error = operation.combine_errors(primary, cause, true, &None);
        assert_eq!(error.value.as_ref().code(), DiagnosticCode::Cancelled);
        if depth == 2 {
            assert_eq!(budget.usage().diagnostics, 2);
            assert_eq!(budget.usage().source_bytes * 2, separate_sources);
            assert_eq!(
                error.value.as_ref().causes[0].code(),
                DiagnosticCode::Native
            );
        } else {
            assert!(error.value.as_ref().is_emergency());
            assert_eq!(
                error
                    .value
                    .as_ref()
                    .omissions
                    .as_ref()
                    .unwrap()
                    .direct_causes,
                1
            );
            assert_eq!(budget.usage(), OperationUsage::default());
        }
        drop(error);
        assert_eq!(budget.usage(), OperationUsage::default());
    }
}
