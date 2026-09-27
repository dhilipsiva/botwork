//! Typed worker handoff: reserve before copying frames or decoded payloads.
use super::*;
use crate::core::worker::{
    protocol::{ResponsePlan, WorkerProtocol},
    WorkerCleanup, WorkerCommand, WorkerOutcome, WorkerPool,
};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct Isolated {
    pool: WorkerPool,
    command: WorkerCommand,
    protocol: WorkerProtocol,
    wire: Arc<WireBudget>,
}
struct WireBudget {
    limit: usize,
    used: AtomicUsize,
}
struct WireReservation {
    budget: Arc<WireBudget>,
    bytes: usize,
}
impl Drop for WireReservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Retention {
    _scope: Arc<Reservation>,
    _wire: WireReservation,
}

impl WireBudget {
    fn reserve(self: &Arc<Self>, bytes: Option<usize>) -> DiagnosticResult<WireReservation> {
        let rejected = || {
            Diagnostic::new(BWErr::ResourceLimit {
                resource: "worker protocol in-flight bytes",
                limit: self.limit as u64,
            })
        };
        let bytes = bytes.ok_or_else(rejected)?;
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= self.limit)
            })
            .map_err(|_| rejected())?;
        Ok(WireReservation {
            budget: self.clone(),
            bytes,
        })
    }
}

impl NativeOperation {
    /// One typed request per process. Linux supervision owns cancellation and cleanup.
    /// Clones share worker capacity and the protocol's encoded-byte budget.
    pub fn isolated(
        signature: StatementSignature,
        pool: WorkerPool,
        command: WorkerCommand,
        protocol: WorkerProtocol,
    ) -> DiagnosticResult<Self> {
        protocol.validate()?;
        let wire = Arc::new(WireBudget {
            limit: protocol.limits.in_flight_bytes,
            used: AtomicUsize::new(0),
        });
        Self::new(
            signature,
            Implementation::Isolated(Arc::new(Isolated {
                pool,
                command,
                protocol,
                wire,
            })),
        )
    }

    /// Encoded request plus reserved stdout/stderr capacity, including cleanup.
    pub fn isolated_in_flight_bytes(&self) -> Option<usize> {
        match &self.implementation {
            Implementation::Isolated(worker) => Some(worker.wire.used.load(Ordering::Acquire)),
            _ => None,
        }
    }
}

impl Isolated {
    pub(super) async fn invoke(
        &self,
        operation: &NativeOperation,
        admission: Admission,
        control: OperationControl,
        cleanup_stop: &mut Option<DiagnosticCode>,
    ) -> OperationResult {
        let scope = Some(admission.scope.clone());
        let fail = |error: Diagnostic| operation.track_error(error, false, true, &scope);
        let plan = self
            .protocol
            .request_plan(&admission.values, &control)
            .map_err(&fail)?;
        if plan.bytes() > self.pool.limits().request_bytes {
            return Err(fail(Diagnostic::new(BWErr::ResourceLimit {
                resource: "worker request bytes",
                limit: self.pool.limits().request_bytes as u64,
            })));
        }
        let wire = self
            .wire
            .reserve(
                plan.bytes()
                    .checked_add(self.pool.limits().stdout_bytes)
                    .and_then(|bytes| bytes.checked_add(self.pool.limits().stderr_bytes)),
            )
            .map_err(&fail)?;
        let input = plan.encode().map_err(&fail)?;
        let retention: Arc<dyn Send + Sync> = Arc::new(Retention {
            _scope: admission.scope.clone(),
            _wire: wire,
        });
        let worker = self
            .pool
            .start_retained(
                self.command.clone(),
                input,
                control.clone(),
                Some(retention),
            )
            .map_err(&fail)?;
        // The worker owns an encoded copy; scope remains charged through cleanup.
        drop(admission);
        let mut retained = worker.wait_retained().await;
        let report = &mut retained.report;
        let stop = match report.outcome {
            WorkerOutcome::Cancelled | WorkerOutcome::Interrupted => {
                Some(DiagnosticCode::Cancelled)
            }
            WorkerOutcome::TimedOut => Some(DiagnosticCode::Timeout),
            _ => None,
        };
        *cleanup_stop = stop;
        let preserve = stop.is_some();
        let successful = report.outcome == WorkerOutcome::Succeeded
            && matches!(
                report.cleanup,
                WorkerCleanup::Reaped | WorkerCleanup::TreeReaped
            )
            && report.io_complete
            && report.progress_complete
            && report.exit_status.is_some_and(|status| status.success())
            && report.diagnostic.is_none();
        // A terminal stop still permits bounded recovery of a complete foreign
        // diagnostic. It never permits publication of a successful value.
        let scan_control = if successful {
            control.clone()
        } else {
            OperationControl::default()
        };
        let mut protocol = self.protocol.clone();
        let local = &operation.value_limits;
        let values = &mut protocol.limits.values;
        values.nodes = values.nodes.min(local.nodes);
        values.depth = values.depth.min(local.depth);
        values.string_bytes = values.string_bytes.min(local.string_bytes);
        values.key_bytes = values.key_bytes.min(local.key_bytes);
        values.entries = values.entries.min(local.entries);
        values.payload_bytes = values.payload_bytes.min(local.payload_bytes);
        let response = protocol.response_plan(&report.stdout, &scan_control);
        let mut transport = (!successful).then(|| {
            operation.track_error(
                report.diagnostic.take().unwrap_or_else(|| {
                    Diagnostic::formatted(
                        BWErr::AsyncRuntime,
                        format_args!("Worker transport did not complete successfully"),
                    )
                }),
                preserve,
                false,
                &scope,
            )
        });
        let result = match response {
            Ok(ResponsePlan::Error(plan)) => {
                let reservation = plan
                    .size_with_header(operation.signature.header(), &operation.diagnostic_limits)
                    .and_then(|size| {
                        operation
                            .ownership
                            .diagnostic(size, None)
                            .map_err(|rejected| Diagnostic::new(rejected.0))
                    });
                let error = match reservation {
                    Ok(reservation) => operation.track_error(
                        PendingDiagnostic {
                            value: OwnedDiagnostic::new(
                                plan.build_with_header(Some(operation.signature.header())),
                            ),
                            reservation: Some(reservation),
                        },
                        preserve,
                        false,
                        &scope,
                    ),
                    Err(violation) => operation.track_error(
                        plan.rejected(violation, Some(operation.signature.header())),
                        preserve,
                        true,
                        &scope,
                    ),
                };
                Err(match transport.take() {
                    Some(primary) => operation.combine_errors(primary, error, preserve, &scope),
                    None => error,
                })
            }
            Ok(ResponsePlan::Value { bytes, size, kind }) if successful => {
                operation
                    .signature
                    .validate_return_kind(kind, |message| operation.signature_error(message))
                    .map_err(|error| operation.track_error(error, false, true, &scope))?;
                let reservation = operation
                    .ownership
                    .value(size)
                    .map_err(|error| fail(error.into()))?;
                let value = Owned::new(protocol.build_value(bytes));
                let value = Tracked {
                    value,
                    reservation: Some(reservation),
                    _scope: scope.clone(),
                };
                control.checkpoint().map_err(&fail)?;
                Ok(value)
            }
            Err(error) if successful => Err(fail(error)),
            _ => Err(transport.take().expect("failed transport")),
        };
        // retained disposes encoded payloads before refunding their reservations.
        result
    }
}
