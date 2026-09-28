//! Bounded, per-call HTTP/1.1 transport with explicit redirect and retry policies.
use super::*;
use crate::core::{
    operation::OperationControl, run::TemporaryReservation, value_limits::ValueSize,
};
use futures_util::FutureExt;
use std::{
    panic::AssertUnwindSafe,
    sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    },
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

mod config;
mod output;
#[cfg(test)]
mod tests;
mod transport;

const MAX_URL: usize = 8192;
const MAX_BODY: usize = 1024 * 1024;
const WORKSPACE: usize = 4 * 1024 * 1024;
const GLOBAL_BYTES: usize = 64 * 1024 * 1024;
static BYTES: AtomicUsize = AtomicUsize::new(0);
static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Clone, Copy)]
pub(in crate::core::eval) enum HttpOp {
    Text,
    TextOptions,
    Binary,
    BinaryOptions,
}
impl HttpOp {
    fn binary(self) -> bool {
        matches!(self, Self::Binary | Self::BinaryOptions)
    }
    fn options(self) -> bool {
        matches!(self, Self::TextOptions | Self::BinaryOptions)
    }
    pub(super) fn signature(self) -> StatementSignature {
        let header = match self {
            Self::Text => "HTTP Request |method| To |url|",
            Self::TextOptions => "HTTP Request |method| To |url| Options |options|",
            Self::Binary => "HTTP Binary Request |method| To |url|",
            Self::BinaryOptions => "HTTP Binary Request |method| To |url| Options |options|",
        };
        let mut signature = StatementSignature::native_at("<http>", header).expect("fixed HTTP signature")
            .parameter("method", Kind::String).expect("fixed parameter")
            .parameter("url", Kind::String).expect("fixed parameter")
            .returns(Kind::Map)
            .description("Asynchronously request HTTP(S). Return status, headers, body, url, success, redirects and attempts. Text is strict UTF-8; binary bodies and header values are byte Arrays. Redirects and retries default off; TLS is verified.");
        if self.options() {
            signature = signature
                .parameter("options", Kind::Map)
                .expect("fixed parameter");
        }
        for (code, reason) in [
            (
                Code::IncompatibleType,
                "Invalid method, URL, option, header, byte or UTF-8 response.",
            ),
            (
                Code::Native,
                "DNS, connection, TLS, HTTP framing or redirect policy failed.",
            ),
            (
                Code::ResourceLimit,
                "Request, response, value, temporary or shared admission exceeded its limit.",
            ),
            (
                Code::AsyncRuntime,
                "Requires asynchronous execution and Tokio I/O and time drivers.",
            ),
            (Code::Cancelled, "The inherited operation was cancelled."),
            (
                Code::Timeout,
                "The end-to-end HTTP or inherited deadline expired.",
            ),
        ] {
            signature = signature
                .documents_error(code, reason)
                .expect("fixed error");
        }
        signature
    }
    // Keep transport state out of every recursive evaluator frame, including
    // programs that never invoke HTTP.
    pub(in crate::core::eval) fn invoke<'a>(
        self,
        arguments: &'a [TemporaryValue],
        context: &'a mut Context,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TemporaryResult> + Send + 'a>> {
        Box::pin(self.execute(arguments, context))
    }

    async fn execute(self, arguments: &[TemporaryValue], context: &mut Context) -> TemporaryResult {
        context.checkpoint()?;
        let slot = SLOTS
            .get_or_init(|| Arc::new(Semaphore::new(32)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| limit(context, "HTTP in-flight requests", 32))?;
        let workspace_global = Global::new(context, WORKSPACE)?;
        let workspace = context.temporary_reservation(ValueSize {
            nodes: 1,
            depth: 1,
            payload_bytes: WORKSPACE,
        })?;
        let plan = config::Plan::new(
            context,
            self.binary(),
            &arguments[0],
            &arguments[1],
            self.options().then(|| &*arguments[2]),
        )?;
        let size = output::planned(context, &plan)?;
        let output_global = Global::new(context, size.payload_bytes)?;
        let output = context.temporary_reservation(size)?;
        let retention = Arc::new(Retention {
            _slot: slot,
            _workspace: workspace,
            _workspace_global: workspace_global,
            _output_global: output_global,
        });
        let parent = context
            .environment
            .as_ref()
            .map(|env| env.control().clone())
            .or_else(|| {
                context
                    .budget
                    .as_ref()
                    .map(|budget| budget.control().clone())
            })
            .unwrap_or_default();
        let control = parent.child(Some(
            tokio::time::Instant::now() + Duration::from_millis(plan.timeout_ms),
        ));
        let work = async {
            tokio::select! {
                biased;
                error = control.stopped() => Err(stopped(context, error)),
                result = transport::request(context, plan, retention.clone(), &control) => result,
            }
        };
        let response = AssertUnwindSafe(work).catch_unwind().await.map_err(|_| {
            context.detail_error(
                BWErr::AsyncRuntime,
                "HTTP transport requires Tokio I/O and time drivers",
                None,
                false,
            )
        })??;
        context.checkpoint()?;
        let result = output::build(context, self.binary(), response, size, output)?;
        control
            .checkpoint()
            .map_err(|error| stopped(context, error))?;
        Ok(result)
    }
}
struct Retention {
    _slot: OwnedSemaphorePermit,
    _workspace: Option<TemporaryReservation>,
    _workspace_global: Global,
    _output_global: Global,
}
struct Global(usize);
impl Global {
    fn new(context: &Context, bytes: usize) -> EvaluationResult<Self> {
        BYTES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= GLOBAL_BYTES)
            })
            .map_err(|_| limit(context, "HTTP in-flight bytes", GLOBAL_BYTES))?;
        Ok(Self(bytes))
    }
}
impl Drop for Global {
    fn drop(&mut self) {
        BYTES.fetch_sub(self.0, Ordering::AcqRel);
    }
}
fn invalid(context: &Context, message: &str) -> RuntimeDiagnostic {
    context.detail_error(BWErr::OperationIncompatibleError, message, None, false)
}
fn failed(context: &Context, message: &str) -> RuntimeDiagnostic {
    context.detail_error(BWErr::NativeError, message, None, false)
}
fn limit(context: &Context, resource: &'static str, maximum: usize) -> RuntimeDiagnostic {
    context
        .retain_limit(Diagnostic::new(BWErr::ResourceLimit {
            resource,
            limit: maximum as u64,
        }))
        .into()
}
fn admitted<T>(context: &Context, result: Result<T, BWErr>) -> EvaluationResult<T> {
    result.map_err(|error| context.retain_limit(Diagnostic::new(error)).into())
}
fn stopped(context: &Context, error: Diagnostic) -> RuntimeDiagnostic {
    if matches!(error.code(), Code::Timeout | Code::Cancelled) {
        if let Some(budget) = &context.budget {
            budget.stop((*error.error).clone());
        }
    }
    error.into()
}
lazy_static::lazy_static! {
    pub(super) static ref FIXED: Vec<(Builtin, Arc<StatementSignature>)> = [HttpOp::Text, HttpOp::TextOptions, HttpOp::Binary, HttpOp::BinaryOptions]
        .into_iter().map(|kind| (Builtin::Http(kind), Arc::new(kind.signature()))).collect();
}
