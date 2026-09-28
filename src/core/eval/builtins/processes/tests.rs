use super::*;
use crate::core::value_limits::ValueLimits;
use std::os::unix::process::ExitStatusExt;

fn report(status: i32) -> WorkerReport {
    WorkerReport {
        id: 1,
        outcome: if status == 0 {
            WorkerOutcome::Succeeded
        } else {
            WorkerOutcome::Failed
        },
        cleanup: WorkerCleanup::Reaped,
        exit_status: Some(std::process::ExitStatus::from_raw(status)),
        stdin_written: 0,
        stdout: vec![],
        stderr: vec![],
        io_complete: true,
        progress_complete: true,
        diagnostic: (status != 0).then(|| Diagnostic::new(BWErr::NativeError("exit".into()))),
    }
}

#[test]
fn results_require_complete_io_cleanup_and_exact_exit_evidence() {
    for status in [0, 7 << 8, 15] {
        assert!(complete(&report(status)));
        for field in 0..8 {
            let mut value = report(status);
            match field {
                0 => value.io_complete = false,
                1 => value.progress_complete = false,
                2 => value.cleanup = WorkerCleanup::Pending,
                3 => value.cleanup = WorkerCleanup::Unverified,
                4 => value.exit_status = None,
                5 => value.outcome = WorkerOutcome::TimedOut,
                6 => value.diagnostic = Some(Diagnostic::new(BWErr::AsyncRuntime("pipe".into()))),
                _ => {
                    value.diagnostic = Some(
                        Diagnostic::new(BWErr::NativeError("exit".into()))
                            .while_handling(Diagnostic::new(BWErr::AsyncRuntime("pipe".into()))),
                    )
                }
            }
            assert!(!complete(&value), "status={status}, field={field}");
        }
    }
}

#[test]
fn planned_result_metrics_match_independent_value_measurement_and_release_unused_credit() {
    for binary in [false, true] {
        for status in [0, 7 << 8, 15] {
            let context = Context::with_limits(RunLimits::default()).unwrap();
            let planned = output::size(&context, binary, 5, 7, false, false).unwrap();
            let retention = Retention {
                output: Mutex::new(context.temporary_reservation(planned).unwrap()),
                _workspace: None,
                _global: GlobalReservation::new(&context, 0).unwrap(),
            };
            let mut report = report(status);
            report.stdout = b"abc".to_vec();
            report.stderr = b"de".to_vec();
            let value = output::build(&context, binary, &mut report, planned, &retention).unwrap();
            let expected =
                output::size(&context, binary, 3, 2, status == 15, status != 15).unwrap();
            assert_eq!(ValueLimits::default().check(&value).unwrap(), expected);
            assert!(report.stdout.is_empty() && report.stderr.is_empty());
            assert!(retention.output.lock().unwrap().is_none());
            let remaining = context.limits().temporaries.payload_bytes - expected.payload_bytes;
            let rest = context
                .temporary_reservation(ValueSize {
                    nodes: 1,
                    depth: 1,
                    payload_bytes: remaining,
                })
                .unwrap();
            assert!(context
                .temporary_reservation(ValueSize {
                    nodes: 1,
                    depth: 1,
                    payload_bytes: 1
                })
                .is_err());
            drop((rest, value));
        }
    }
}

#[test]
fn output_planner_checks_key_nodes_depth_entries_payload_and_integer_overflow() {
    let base = output::size(&Context::default(), true, 8, 5, false, false).unwrap();
    assert_eq!(
        base,
        ValueSize {
            nodes: 19,
            depth: 3,
            payload_bytes: 95
        }
    );
    for field in 0..5 {
        for below in [false, true] {
            let mut limits = RunLimits::default();
            let deficit = usize::from(below);
            match field {
                0 => limits.values.key_bytes = 9 - deficit,
                1 => limits.values.nodes = base.nodes - deficit,
                2 => limits.values.depth = base.depth - deficit,
                3 => limits.values.entries = 8 - deficit,
                _ => limits.values.payload_bytes = base.payload_bytes - deficit,
            }
            let context = Context::with_limits(limits).unwrap();
            assert_eq!(
                output::size(&context, true, 8, 5, false, false).is_ok(),
                !below
            );
        }
    }
    assert!(output::size(&Context::default(), true, usize::MAX, 0, false, false).is_err());
}

#[test]
fn resource_causes_are_promoted_and_local_timeout_latches_only_the_current_budget() {
    for secondary in [false, true] {
        let mut error = Diagnostic::new(BWErr::NativeError("exit 7".into()));
        if secondary {
            error = error.while_handling(Diagnostic::new(BWErr::AsyncRuntime("pipe".into())));
        }
        let error = failure(&Context::default(), error).into_diagnostic();
        assert_eq!(error.code(), Code::AsyncRuntime);
        assert_eq!(error.causes[0].code(), Code::Native);
    }
    let context = Context::with_limits(RunLimits::default()).unwrap();
    let error = Diagnostic::new(BWErr::NativeError("exit 7".into())).while_handling(
        Diagnostic::new(BWErr::ResourceLimit {
            resource: "worker stdout bytes",
            limit: 8,
        }),
    );
    assert_eq!(failure(&context, error).code(), Code::ResourceLimit);
    assert_eq!(
        context.checkpoint().unwrap_err().code(),
        Code::ResourceLimit
    );
    let context = Context::with_limits(RunLimits::default()).unwrap();
    let control = context.budget.as_ref().unwrap().control().clone();
    assert_eq!(
        failure(&context, Diagnostic::new(BWErr::Timeout("process".into()))).code(),
        Code::Timeout
    );
    assert!(control.checkpoint().is_ok());
    assert_eq!(context.checkpoint().unwrap_err().code(), Code::Timeout);
    assert!(context
        .budget
        .as_ref()
        .unwrap()
        .for_cleanup()
        .unwrap()
        .checkpoint()
        .is_ok());
}

#[test]
fn global_admission_rejects_oversized_reservations_without_counter_changes() {
    let context = Context::default();
    assert!(GlobalReservation::new(&context, MAX_IN_FLIGHT_BYTES + 1).is_err());
    assert!(GlobalReservation::new(&context, usize::MAX).is_err());
    let reservation = GlobalReservation::new(&context, 1).unwrap();
    drop(reservation);
}
