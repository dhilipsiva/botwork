//! Eventually and Retry: bounded attempts under scoped stops, retaining the last
//! failure and a fixed-size attempt history.
use super::*;
use crate::core::{
    ast::PollMode,
    diagnostic::{DiagnosticCode as Code, FormattedDetail},
};
use futures_util::FutureExt;
use std::{collections::VecDeque, fmt, panic::AssertUnwindSafe, time::Duration};
use tokio::time::Instant;

#[cfg(test)]
mod tests;

const MAX_MS: i32 = 86_400_000;
const MAX_ATTEMPTS: i32 = 1_000_000;
const MAX_BACKOFF: f64 = 10.0;
const DEFAULT_INTERVAL_MS: u64 = 100;
const DEFAULT_MAX_INTERVAL_MS: u64 = 60_000;
/// Only the most recent records are kept; `attempts` still counts every attempt.
pub(super) const HISTORY_RECORDS: usize = 16;
const OPTIONS: &str =
    "Use only timeout_ms, attempts, interval_ms, backoff, max_interval_ms, and retry_on";

#[derive(Debug, PartialEq)]
struct Policy {
    mode: PollMode,
    timeout_ms: Option<u64>,
    attempts: Option<u32>,
    interval_ms: u64,
    backoff: f64,
    max_interval_ms: u64,
    retry_on: Option<Vec<Code>>,
}

impl Policy {
    fn parse(mode: PollMode, value: &Literal) -> Result<Self, &'static str> {
        let Literal::Map(options) = value else {
            return Err("Eventually and Retry options must be a Map");
        };
        const KEYS: [&str; 6] = [
            "timeout_ms",
            "attempts",
            "interval_ms",
            "backoff",
            "max_interval_ms",
            "retry_on",
        ];
        if !options.keys().all(|key| KEYS.contains(&key.as_str())) {
            return Err(OPTIONS);
        }
        let integer =
            |key: &str, minimum: i32, maximum: i32, message: &'static str| match options.get(key) {
                None => Ok(None),
                Some(Literal::Int(value)) if (minimum..=maximum).contains(value) => {
                    Ok(Some(*value as u32))
                }
                Some(_) => Err(message),
            };
        let timeout_ms = integer(
            "timeout_ms",
            1,
            MAX_MS,
            "timeout_ms must be an Int from 1 to 86400000",
        )?
        .map(u64::from);
        let attempts = integer(
            "attempts",
            1,
            MAX_ATTEMPTS,
            "attempts must be an Int from 1 to 1000000",
        )?;
        let interval_ms = integer(
            "interval_ms",
            0,
            MAX_MS,
            "interval_ms must be an Int from 0 to 86400000",
        )?
        .map_or(DEFAULT_INTERVAL_MS, u64::from);
        let max_interval_ms = integer(
            "max_interval_ms",
            0,
            MAX_MS,
            "max_interval_ms must be an Int from 0 to 86400000",
        )?
        .map(u64::from);
        if max_interval_ms.is_some_and(|maximum| maximum < interval_ms) {
            return Err("max_interval_ms must not be less than interval_ms");
        }
        let backoff = match options.get("backoff") {
            None => 1.0,
            Some(Literal::Int(value)) => f64::from(*value),
            Some(Literal::Float(value)) => f64::from(*value),
            Some(_) => return Err("backoff must be an Int or Float from 1 to 10"),
        };
        if !(1.0..=MAX_BACKOFF).contains(&backoff) {
            return Err("backoff must be an Int or Float from 1 to 10");
        }
        let retry_on = match options.get("retry_on") {
            None => None,
            Some(Literal::Array(items)) if !items.is_empty() => {
                let mut codes = Vec::with_capacity(items.len().min(Code::ALL.len()));
                for item in items {
                    let Literal::String(text) = item else {
                        return Err("retry_on must contain BWnnnn diagnostic code Strings");
                    };
                    let code = Code::parse(text)
                        .ok_or("retry_on must contain known BWnnnn diagnostic codes")?;
                    if matches!(code, Code::Cancelled | Code::ResourceLimit) {
                        return Err("retry_on cannot include BW5001 or BW8001; run cancellation and resource limits always stop");
                    }
                    if codes.contains(&code) {
                        return Err("retry_on must not repeat a diagnostic code");
                    }
                    codes.push(code);
                }
                Some(codes)
            }
            Some(_) => return Err("retry_on must be a nonempty Array of diagnostic code Strings"),
        };
        match mode {
            PollMode::Eventually if timeout_ms.is_none() => {
                return Err("Eventually requires timeout_ms");
            }
            PollMode::Retry if attempts.is_none() => return Err("Retry requires attempts"),
            _ => (),
        }
        Ok(Self {
            mode,
            timeout_ms,
            attempts,
            interval_ms,
            backoff,
            max_interval_ms: max_interval_ms
                .unwrap_or_else(|| DEFAULT_MAX_INTERVAL_MS.max(interval_ms)),
            retry_on,
        })
    }

    /// Observations retry every attempt failure. An action's timeout leaves its
    /// effects ambiguous, so Retry repeats it only when retry_on names BW5002.
    fn retries(&self, code: Code) -> bool {
        match &self.retry_on {
            Some(codes) => codes.contains(&code),
            None => self.mode == PollMode::Eventually || code != Code::Timeout,
        }
    }

    /// The wait after `failures` failed attempts, rounded down to milliseconds.
    fn delay_ms(&self, failures: u32) -> u64 {
        if self.interval_ms == 0 {
            return 0;
        }
        let exponent = failures.saturating_sub(1).min(1024) as i32;
        let scaled = self.interval_ms as f64 * self.backoff.powi(exponent);
        scaled.min(self.max_interval_ms as f64) as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Record {
    attempt: u32,
    started_ms: u64,
    duration_ms: u64,
    outcome: Code,
}

#[derive(Default)]
struct History {
    attempts: u32,
    recent: VecDeque<Record>,
}

impl History {
    fn push(&mut self, record: Record) {
        if self.recent.len() == HISTORY_RECORDS {
            self.recent.pop_front();
        }
        self.recent.push_back(record);
    }
}

/// A JSON array written directly into the admitted diagnostic string.
struct Json<'a>(&'a VecDeque<Record>);
impl fmt::Display for Json<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for (index, record) in self.0.iter().enumerate() {
            if index != 0 {
                f.write_str(",")?;
            }
            write!(
                f,
                r#"{{"attempt":{},"started_ms":{},"duration_ms":{},"outcome":"{}"}}"#,
                record.attempt,
                record.started_ms,
                record.duration_ms,
                record.outcome.as_str()
            )?;
        }
        f.write_str("]")
    }
}

#[derive(Clone, Copy)]
enum Exhausted {
    /// The polling deadline stopped the attempt in progress.
    Interrupted,
    /// The next attempt would not have started before the deadline.
    Deadline,
    /// The configured attempt count was reached.
    Limit,
}

struct Reason<'a> {
    policy: &'a Policy,
    why: Exhausted,
    attempts: u32,
    last: Code,
}
impl fmt::Display for Reason<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let attempts = self.attempts;
        let plural = if attempts == 1 { "" } else { "s" };
        match self.policy.mode {
            PollMode::Eventually => f.write_str("no attempt succeeded")?,
            PollMode::Retry => f.write_str("the action failed")?,
        }
        match (self.why, self.policy.timeout_ms) {
            (Exhausted::Limit, _) | (_, None) => {
                write!(f, " in {attempts} attempt{plural} (the attempt limit)")?
            }
            (Exhausted::Interrupted, Some(timeout)) => write!(
                f,
                " within {timeout} ms ({attempts} attempt{plural}; the deadline interrupted the last)"
            )?,
            (Exhausted::Deadline, Some(timeout)) => write!(
                f,
                " within {timeout} ms ({attempts} attempt{plural}; no further attempt could start before the deadline)"
            )?,
        }
        write!(
            f,
            "; the last attempt failed with {}, retained as the first cause",
            self.last.as_str()
        )
    }
}

enum Outcome {
    Completed(Completion),
    /// A run stop or resource limit, already latched by the owning budget.
    Stopped(RuntimeDiagnostic),
    /// A catchable failure, or an operation timeout latched only by the attempt.
    Failed(RuntimeDiagnostic, Option<BWErr>),
    Interrupted(RuntimeDiagnostic),
}

/// Swap in an attempt budget/control, as cleanup does, and restore on drop.
struct AttemptScope<'a> {
    context: &'a mut Context,
    budget: Option<RunBudget>,
    environment: Option<Arc<RunEnvironment>>,
}

impl<'a> AttemptScope<'a> {
    fn enter(context: &'a mut Context, deadline: Option<Instant>) -> Self {
        let budget = context
            .budget
            .as_ref()
            .map(|budget| budget.for_attempt(deadline));
        let environment = context.environment.as_ref().map(|environment| {
            Arc::new(
                environment.with_control(
                    budget
                        .as_ref()
                        .map(|budget| budget.control().clone())
                        .unwrap_or_else(|| environment.control().child(deadline)),
                ),
            )
        });
        let budget = std::mem::replace(&mut context.budget, budget);
        let environment = std::mem::replace(&mut context.environment, environment);
        Self {
            context,
            budget,
            environment,
        }
    }
}

impl Drop for AttemptScope<'_> {
    fn drop(&mut self) {
        self.context.budget = self.budget.take();
        self.context.environment = self.environment.take();
    }
}

async fn attempt(body: &Block, deadline: Option<Instant>, context: &mut Context) -> Outcome {
    let scope = AttemptScope::enter(context, deadline);
    let result = execution::evaluate_block(body, scope.context).await;
    let local = scope.context.budget.as_ref().and_then(RunBudget::latched);
    drop(scope);
    let error = match result {
        Ok(completion) => return Outcome::Completed(completion),
        Err(error) => error,
    };
    // Run cancellation, run deadlines and earlier latches belong to the owner.
    if context.checkpoint().is_err() {
        return Outcome::Stopped(context.after_evaluation::<()>(Err(error)).unwrap_err());
    }
    match local {
        None => Outcome::Failed(error, None),
        Some(BWErr::Timeout(_)) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
            Outcome::Interrupted(error)
        }
        Some(stop @ BWErr::Timeout(_)) => Outcome::Failed(error, Some(stop)),
        // Resource limits consume shared quotas; other stops also end the run.
        Some(stop) => {
            if let Some(budget) = &context.budget {
                budget.stop(stop);
            }
            Outcome::Stopped(error)
        }
    }
}

async fn wait_until(next: Instant, context: &mut Context) -> EvaluationResult<()> {
    let control = context
        .environment
        .as_ref()
        .map(|environment| environment.control().clone())
        .or_else(|| {
            context
                .budget
                .as_ref()
                .map(|budget| budget.control().clone())
        })
        .unwrap_or_default();
    let wait = async {
        tokio::select! {
            biased;
            stop = control.stopped() => Err(stop),
            () = tokio::time::sleep_until(next) => Ok(()),
        }
    };
    match AssertUnwindSafe(wait).catch_unwind().await {
        Ok(Ok(())) => Ok(context.checkpoint()?),
        Ok(Err(stop)) => {
            if let Some(budget) = &context.budget {
                budget.stop((*stop.error).clone());
            }
            Err(stop.into())
        }
        Err(_) => Err(context.detail_error(
            BWErr::AsyncRuntime,
            "Eventually and Retry require a Tokio time driver",
            None,
            false,
        )),
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Boxed, and passed only the statement, to keep the recursive evaluator's frame small.
pub(super) fn evaluate<'a>(
    statement: &'a Statement,
    context: &'a mut Context,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = CompletionResult> + Send + 'a>> {
    Box::pin(async move {
        let StatementKind::Poll {
            mode,
            options,
            body,
            header,
        } = &statement.kind
        else {
            unreachable!("only polling statements are dispatched here")
        };
        let mode = *mode;
        // Waiting needs a live timer; reject before option or body effects.
        if !context.asynchronous {
            return Err(context.detail_error(
                BWErr::AsyncRuntime,
                "Use asynchronous execution for Eventually and Retry",
                Some(header),
                false,
            ));
        }
        let value = execution::evaluate_expression(options, context).await?;
        let policy = Policy::parse(mode, &value).map_err(|message| {
            context.detail_error(
                BWErr::OperationIncompatibleError,
                message,
                Some(&options.span),
                true,
            )
        })?;
        drop(value);
        let start = Instant::now();
        let deadline = policy
            .timeout_ms
            .map(|timeout| start.checked_add(Duration::from_millis(timeout)))
            .map(|deadline| {
                deadline.ok_or_else(|| {
                    context.detail_error(
                        BWErr::OperationIncompatibleError,
                        "timeout_ms exceeds the monotonic clock range",
                        Some(&options.span),
                        true,
                    )
                })
            })
            .transpose()?;
        let mut history = History::default();
        loop {
            execution::tick(context).await?;
            let began = Instant::now();
            let outcome = attempt(body, deadline, context).await;
            history.attempts += 1;
            let (error, why) = match outcome {
                Outcome::Completed(completion) => return Ok(completion),
                Outcome::Stopped(error) => return Err(error),
                Outcome::Interrupted(error) => (error, Some(Exhausted::Interrupted)),
                Outcome::Failed(error, local) => {
                    if !policy.retries(error.code()) {
                        // Unretried attempt-local timeouts keep their run-stop meaning.
                        if let (Some(stop), Some(budget)) = (local, &context.budget) {
                            budget.stop(stop);
                        }
                        return Err(error);
                    }
                    (error, None)
                }
            };
            let ended = Instant::now();
            history.push(Record {
                attempt: history.attempts,
                started_ms: millis(began - start),
                duration_ms: millis(ended - began),
                outcome: error.code(),
            });
            let next = ended
                .checked_add(Duration::from_millis(policy.delay_ms(history.attempts)))
                .unwrap_or(ended);
            let why = why.or_else(|| {
                if policy
                    .attempts
                    .is_some_and(|limit| history.attempts >= limit)
                {
                    Some(Exhausted::Limit)
                } else if deadline.is_some_and(|deadline| next >= deadline) {
                    Some(Exhausted::Deadline)
                } else {
                    None
                }
            });
            if let Some(why) = why {
                return Err(exhausted(&policy, why, &history, error, header, context));
            }
            if next > ended {
                wait_until(next, context)
                    .await
                    .map_err(|stop| stop.while_handling(error, context.budget.as_ref()))?;
            } else {
                tokio::task::yield_now().await;
                context.checkpoint()?;
            }
        }
    })
}

fn exhausted(
    policy: &Policy,
    why: Exhausted,
    history: &History,
    last: RuntimeDiagnostic,
    header: &Span,
    context: &Context,
) -> RuntimeDiagnostic {
    let reason = Reason {
        policy,
        why,
        attempts: history.attempts,
        last: last.code(),
    };
    let category: fn([String; 3]) -> BWErr = match policy.mode {
        PollMode::Eventually => |[reason, attempts, history]| BWErr::ConditionNotMet {
            reason,
            attempts,
            history,
        },
        PollMode::Retry => |[reason, attempts, history]| BWErr::RetriesExhausted {
            reason,
            attempts,
            history,
        },
    };
    let json = Json(&history.recent);
    let error = context.constructed_fields(
        category,
        [
            FormattedDetail::exact(format_args!("{reason}")),
            FormattedDetail::exact(format_args!("{}", history.attempts)),
            FormattedDetail::exact(format_args!("{json}")),
        ],
        Some((header, false)),
        None,
    );
    error.while_handling(last, context.budget.as_ref())
}
