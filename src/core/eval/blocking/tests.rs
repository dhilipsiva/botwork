use super::*;
use crate::core::diagnostic::DiagnosticCode;

#[tokio::test]
async fn filesystem_workers_copy_only_admitted_low_level_directory_snapshots() {
    use crate::core::run::SnapshotLimits;
    for (allowance, allowed) in [(2, false), (3, true)] {
        let mut context = Context::with_limits(RunLimits {
            snapshots: SnapshotLimits {
                path_bytes: allowance,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        context.working_directory = Ok(PathBuf::from("abc"));
        let result = context
            .blocking_with_directory(true, |worker| {
                assert_eq!(
                    worker.working_directory.as_ref().unwrap(),
                    &PathBuf::from("abc")
                );
                Ok(())
            })
            .await;
        assert_eq!(result.is_ok(), allowed);
        if !allowed {
            assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit);
        }
    }
}

#[tokio::test]
async fn stop_during_error_handoff_preserves_the_completed_error() {
    for latched_limit in [false, true] {
        let control = OperationControl::default();
        let mut context = Context::with_control(RunLimits::default(), control.clone()).unwrap();
        let error = context
            .blocking(move |worker| {
                let original =
                    worker.detail_error(BWErr::NativeError, "completed failure", None, false);
                if latched_limit {
                    worker.budget.as_ref().unwrap().limit("source bytes", 0);
                }
                // Arrange the race after the callback constructs its error but
                // before the worker hands it back to the run.
                control.cancel();
                Err::<(), _>(original)
            })
            .await
            .unwrap_err();
        let cancelled = if latched_limit {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes.len(), 1);
            &error.causes[0]
        } else {
            &*error
        };
        assert_eq!(cancelled.code(), DiagnosticCode::Cancelled);
        assert_eq!(cancelled.causes.len(), 1);
        assert_eq!(cancelled.causes[0].code(), DiagnosticCode::Native);
        assert!(cancelled.causes[0]
            .to_string()
            .contains("completed failure"));
    }
}
