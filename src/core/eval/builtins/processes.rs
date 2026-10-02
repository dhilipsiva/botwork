//! Literal process arguments, bounded capture, and supervisor-owned cleanup.
use super::*;
use crate::core::{
    run::TemporaryReservation,
    value_limits::ValueSize,
    worker::{WorkerCleanup, WorkerLimits, WorkerOutcome, WorkerPool, WorkerReport},
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex, OnceLock,
};

mod config;
mod output;
#[cfg(all(test, target_os = "linux"))]
mod tests;

const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_COMMAND_ENTRIES: usize = 16_384;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_IN_FLIGHT_BYTES: usize = 64 * 1024 * 1024;
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static POOL: OnceLock<WorkerPool> = OnceLock::new();

#[derive(Clone, Copy)]
pub(in crate::core::eval) enum ProcessOp {
    Text,
    TextOptions,
    Binary,
    BinaryOptions,
}

impl ProcessOp {
    fn binary(self) -> bool {
        matches!(self, Self::Binary | Self::BinaryOptions)
    }
    fn options(self) -> bool {
        matches!(self, Self::TextOptions | Self::BinaryOptions)
    }
    pub(super) fn signature(self) -> StatementSignature {
        let header = match self {
            Self::Text => "Run Process |executable| With Arguments |arguments|",
            Self::TextOptions => {
                "Run Process |executable| With Arguments |arguments| Options |options|"
            }
            Self::Binary => "Run Binary Process |executable| With Arguments |arguments|",
            Self::BinaryOptions => {
                "Run Binary Process |executable| With Arguments |arguments| Options |options|"
            }
        };
        let mut signature = StatementSignature::native_at("<processes>", header)
            .expect("fixed process signature")
            .parameter("executable", Kind::String).expect("fixed parameter")
            .parameter("arguments", Kind::Array).expect("fixed parameter")
            .returns(Kind::Map)
            .description(if self.binary() {
                "Run a Linux process with literal String arguments. Return stdout/stderr byte Arrays, exit_code, signal, and success; nonzero exit is a result. Options configure input, environment, directory, capture limits and deadlines."
            } else {
                "Run a Linux process with literal String arguments. Return strict UTF-8 stdout/stderr, exit_code, signal, and success; nonzero exit is a result. Options configure input, environment, directory, capture limits and deadlines."
            });
        if self.options() {
            signature = signature
                .parameter("options", Kind::Map)
                .expect("fixed parameter");
        }
        for (code, reason) in [
            (
                Code::IncompatibleType,
                "Invalid argument, option, input byte, or UTF-8 output.",
            ),
            (
                Code::RunConfiguration,
                "Requires a configured run environment on a supported platform.",
            ),
            (
                Code::AsyncRuntime,
                "Process launch, I/O or cleanup could not be verified.",
            ),
            (
                Code::ResourceLimit,
                "Command, input, capture or in-flight admission exceeded its limit.",
            ),
            (
                Code::Cancelled,
                "Cancellation stops the process group and observes cleanup.",
            ),
            (
                Code::Timeout,
                "The process or inherited run deadline expired; cleanup is observed.",
            ),
        ] {
            signature = signature
                .documents_error(code, reason)
                .expect("fixed error");
        }
        signature
    }

    pub(super) fn invoke(self, arguments: &[TemporaryValue], context: &Context) -> TemporaryResult {
        context.checkpoint()?;
        if !cfg!(any(unix, windows)) {
            return Err(context.detail_error(
                BWErr::RunConfiguration,
                "Process statements are unavailable on this platform",
                None,
                false,
            ));
        }
        let environment = context.environment.as_ref().ok_or_else(|| {
            context.detail_error(
                BWErr::RunConfiguration,
                "Process statements require a configured run environment",
                None,
                false,
            )
        })?;
        let options = self.options().then(|| &*arguments[2]);
        let plan = config::Plan::new(
            context,
            environment,
            self.binary(),
            &arguments[0],
            &arguments[1],
            options,
        )?;
        let size = output::size(
            context,
            self.binary(),
            plan.limits.stdout_bytes,
            plan.limits.stderr_bytes,
            false,
            false,
        )?;
        let workspace = plan.workspace_bytes(context)?;
        let total = workspace
            .checked_add(size.payload_bytes)
            .ok_or_else(|| limit(context, "process in-flight bytes", MAX_IN_FLIGHT_BYTES))?;
        let global = GlobalReservation::new(context, total)?;
        let output = context.temporary_reservation(size)?;
        let workspace = context.temporary_reservation(ValueSize {
            nodes: 1,
            depth: 1,
            payload_bytes: workspace,
        })?;
        let retention = Arc::new(Retention {
            output: Mutex::new(output),
            _workspace: workspace,
            _global: global,
        });
        context.checkpoint()?;
        let command = plan.command(context)?;
        let input = plan.input(context)?;
        let pool = POOL.get_or_init(|| {
            WorkerPool::new(WorkerLimits {
                max_in_flight: std::num::NonZeroUsize::new(32).unwrap(),
                history_records: 0,
                ..WorkerLimits::default()
            })
            .expect("fixed process pool limits")
        });
        let worker = pool
            .start_configured_retained(
                command,
                input,
                environment.control().clone(),
                Some(retention.clone()),
                plan.limits,
                true,
            )
            .map_err(|error| failure(context, error))?;
        // The run waits as it ends for a worker whose cleanup this call did
        // not see finish, including one it abandoned at a stop.
        let id = worker.id();
        environment.workers.record(pool, id);
        let mut completed = worker.wait_blocking_retained();
        if matches!(
            completed.report.cleanup,
            WorkerCleanup::Reaped | WorkerCleanup::TreeReaped | WorkerCleanup::NotStarted
        ) {
            environment.workers.forget(pool, id);
        }
        let report = &mut completed.report;
        if !complete(report) {
            let error = report.diagnostic.take().unwrap_or_else(|| {
                Diagnostic::formatted(
                    BWErr::AsyncRuntime,
                    format_args!("Process I/O or cleanup is incomplete"),
                )
            });
            return Err(failure(context, error));
        }
        context.checkpoint()?;
        output::build(context, self.binary(), report, size, &retention)
    }
}

fn complete(report: &WorkerReport) -> bool {
    report.cleanup == WorkerCleanup::Reaped
        && report.io_complete
        && report.progress_complete
        && report.exit_status.is_some_and(|status| {
            if status.success() {
                report.outcome == WorkerOutcome::Succeeded && report.diagnostic.is_none()
            } else {
                report.outcome == WorkerOutcome::Failed
                    && report.diagnostic.as_ref().is_some_and(|error| {
                        error.code() == Code::Native
                            && error.causes.is_empty()
                            && error.omissions.is_none()
                    })
            }
        })
}

fn invalid(context: &Context, reason: &str) -> RuntimeDiagnostic {
    context.detail_error(BWErr::OperationIncompatibleError, reason, None, false)
}
fn limit(context: &Context, resource: &'static str, maximum: usize) -> RuntimeDiagnostic {
    context
        .retain_limit(Diagnostic::new(BWErr::ResourceLimit {
            resource,
            limit: maximum as u64,
        }))
        .into()
}
fn admitted<T>(context: &Context, value: Result<T, BWErr>) -> EvaluationResult<T> {
    value.map_err(|error| context.retain_limit(Diagnostic::new(error)).into())
}
fn failure(context: &Context, error: Diagnostic) -> RuntimeDiagnostic {
    // A process-local deadline must stop the containing run too; otherwise Catch
    // could swallow the timeout because the inherited control is still live.
    if matches!(error.code(), Code::Cancelled | Code::Timeout) {
        if let Some(budget) = &context.budget {
            budget.stop((*error.error).clone());
        }
        return error.into();
    }
    fn resource(error: &Diagnostic) -> Option<(&'static str, u64)> {
        if let BWErr::ResourceLimit { resource, limit } = &*error.error {
            Some((*resource, *limit))
        } else {
            error.causes.iter().find_map(resource)
        }
    }
    if let Some((resource, limit)) = resource(&error) {
        let primary =
            context.retain_limit(Diagnostic::new(BWErr::ResourceLimit { resource, limit }));
        return RuntimeDiagnostic::from(primary)
            .while_handling(error.into(), context.budget.as_ref());
    }
    if error.code() == Code::Native {
        // An ordinary nonzero status already passed through complete(). Here it
        // is only the leading evidence for incomplete transport or cleanup.
        return RuntimeDiagnostic::from(Diagnostic::formatted(
            BWErr::AsyncRuntime,
            format_args!("Process I/O or cleanup is incomplete"),
        ))
        .while_handling(error.into(), context.budget.as_ref());
    }
    error.into()
}

struct Retention {
    output: Mutex<Option<TemporaryReservation>>,
    _workspace: Option<TemporaryReservation>,
    _global: GlobalReservation,
}
struct GlobalReservation(usize);
impl GlobalReservation {
    fn new(context: &Context, bytes: usize) -> EvaluationResult<Self> {
        IN_FLIGHT
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= MAX_IN_FLIGHT_BYTES)
            })
            .map_err(|_| limit(context, "process in-flight bytes", MAX_IN_FLIGHT_BYTES))?;
        Ok(Self(bytes))
    }
}
impl Drop for GlobalReservation {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(self.0, Ordering::AcqRel);
    }
}

lazy_static::lazy_static! {
    pub(super) static ref FIXED: Vec<(Builtin, Arc<StatementSignature>)> = [
        ProcessOp::Text, ProcessOp::TextOptions, ProcessOp::Binary, ProcessOp::BinaryOptions,
    ].into_iter().map(|kind| (Builtin::Process(kind), Arc::new(kind.signature()))).collect();
}
