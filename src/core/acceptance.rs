//! Shared acceptance verdict policy. Statement adapters supply phase evidence;
//! console, structured status views, and exit decisions consume the same verdict.
use super::{
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticResult},
    grammar::BWErr,
};
use serde::{Deserialize, Serialize};

mod totals;
mod view;
pub use totals::{CaseTotals, Delivery, RunVerdict};
pub use view::{HtmlStatus, StatusView};

#[cfg(test)]
mod tests;

pub const MAX_EXPECTATION_REASON_BYTES: usize = 512;

/// Assertions transfer control immediately; no implicit collection/continuation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionMode {
    #[default]
    Immediate,
}

/// A reviewed expectation applies only to a clean assertion failure in the body.
/// It never turns setup, cleanup, operational errors, or stops into success.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaseExpectation {
    reason: Option<String>,
}

impl CaseExpectation {
    pub fn failure(reason: &str) -> DiagnosticResult<Self> {
        if reason.len() > MAX_EXPECTATION_REASON_BYTES
            || reason.trim().is_empty()
            || reason.chars().any(char::is_control)
        {
            return Err(configuration(
                "Expected failure requires a nonblank reason of at most 512 UTF-8 bytes without control characters",
            ));
        }
        Ok(Self {
            reason: Some(reason.to_owned()),
        })
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// Classification of *all* unhandled evidence for one phase. Assertion means
/// one assertion failure with no secondary failure or omitted evidence. Mixed
/// errors, native errors/panics, and uncertain evidence use Other (or a stop).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Assertion,
    Other,
    Cancelled,
    TimedOut,
    LimitExceeded,
}

impl FailureKind {
    /// Classify retained evidence, not a message or category alone. Secondary
    /// failures and explicit omission summaries disqualify expected assertions.
    pub fn from_diagnostic(error: &Diagnostic) -> Self {
        if error.code() == DiagnosticCode::Assertion
            && error.causes.is_empty()
            && error.omissions.is_none()
        {
            Self::Assertion
        } else {
            Self::from_diagnostic_code(error.code())
        }
    }

    /// A category alone does not establish clean assertion evidence. Never infer
    /// an assertion from an error message, native statement name, or user tag.
    pub fn from_diagnostic_code(code: DiagnosticCode) -> Self {
        match code {
            DiagnosticCode::Cancelled => Self::Cancelled,
            DiagnosticCode::Timeout => Self::TimedOut,
            DiagnosticCode::ResourceLimit => Self::LimitExceeded,
            _ => Self::Other,
        }
    }

    fn status(self) -> CaseStatus {
        match self {
            Self::Assertion | Self::Other => CaseStatus::Failed,
            Self::Cancelled => CaseStatus::Cancelled,
            Self::TimedOut => CaseStatus::TimedOut,
            Self::LimitExceeded => CaseStatus::LimitExceeded,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseOutcome {
    Succeeded,
    Failed(FailureKind),
}

/// An owning run's stop observed after the phase results, including cleanup.
/// Phase-local cleanup stops remain secondary to earlier setup/body failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentStop {
    Cancelled,
    TimedOut,
    LimitExceeded,
}

impl ParentStop {
    fn status(self) -> CaseStatus {
        match self {
            Self::Cancelled => CaseStatus::Cancelled,
            Self::TimedOut => CaseStatus::TimedOut,
            Self::LimitExceeded => CaseStatus::LimitExceeded,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    SuiteSetupFailed,
    SuiteStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Completion {
    Skipped(SkipReason),
    Interrupted,
    Executed {
        setup: PhaseOutcome,
        body: Option<PhaseOutcome>,
        teardown: PhaseOutcome,
        parent_stop: Option<ParentStop>,
    },
}

/// Terminal observations only. Excluded cases have no observation. A missing
/// terminal observation is interrupted, never inferred to be a pass or skip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaseCompletion(Completion);

impl CaseCompletion {
    pub fn skipped(reason: SkipReason) -> Self {
        Self(Completion::Skipped(reason))
    }

    pub fn interrupted() -> Self {
        Self(Completion::Interrupted)
    }

    /// Setup includes preparation/library work. No body may follow failed setup;
    /// successful setup needs an observed body outcome. An absent/unarmed teardown
    /// counts as a successful no-op, not proof that cleanup code executed.
    pub fn executed(
        setup: PhaseOutcome,
        body: Option<PhaseOutcome>,
        teardown: PhaseOutcome,
        parent_stop: Option<ParentStop>,
    ) -> DiagnosticResult<Self> {
        if (setup == PhaseOutcome::Succeeded) != body.is_some() {
            return Err(configuration(
                "Case completion requires a body outcome exactly when setup succeeded; incomplete work is interrupted",
            ));
        }
        Ok(Self(Completion::Executed {
            setup,
            body,
            teardown,
            parent_stop,
        }))
    }

    pub fn skip_reason(self) -> Option<SkipReason> {
        match self.0 {
            Completion::Skipped(reason) => Some(reason),
            _ => None,
        }
    }

    pub fn decide(self, expectation: &CaseExpectation) -> CaseStatus {
        let (setup, body, teardown, parent_stop) = match self.0 {
            Completion::Skipped(_) => return CaseStatus::Skipped,
            Completion::Interrupted => return CaseStatus::Interrupted,
            Completion::Executed {
                setup,
                body,
                teardown,
                parent_stop,
            } => (setup, body, teardown, parent_stop),
        };
        if let Some(stop) = parent_stop {
            return stop.status();
        }
        // Keep the owner's first unhandled failure primary. Any cleanup failure
        // disqualifies an expected assertion, including a secondary cleanup stop.
        if let PhaseOutcome::Failed(failure) = setup {
            return failure.status();
        }
        match body.expect("validated completed body") {
            PhaseOutcome::Failed(FailureKind::Assertion)
                if expectation.reason.is_some() && teardown == PhaseOutcome::Succeeded =>
            {
                CaseStatus::ExpectedFailure
            }
            PhaseOutcome::Failed(failure) => failure.status(),
            PhaseOutcome::Succeeded => match teardown {
                PhaseOutcome::Failed(failure) => failure.status(),
                PhaseOutcome::Succeeded if expectation.reason.is_some() => {
                    CaseStatus::UnexpectedPass
                }
                PhaseOutcome::Succeeded => CaseStatus::Succeeded,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Succeeded,
    ExpectedFailure,
    Failed,
    UnexpectedPass,
    Skipped,
    Cancelled,
    TimedOut,
    LimitExceeded,
    Interrupted,
}

impl CaseStatus {
    pub fn from_diagnostic_code(code: DiagnosticCode) -> Self {
        FailureKind::from_diagnostic_code(code).status()
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::ExpectedFailure => "expected_failure",
            Self::Failed => "failed",
            Self::UnexpectedPass => "unexpected_pass",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::LimitExceeded => "limit_exceeded",
            Self::Interrupted => "interrupted",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ExpectedFailure => "expected failure",
            Self::UnexpectedPass => "unexpected pass",
            Self::TimedOut => "timed out",
            Self::LimitExceeded => "limit exceeded",
            _ => self.as_str(),
        }
    }

    pub const fn failed(self) -> bool {
        !matches!(self, Self::Succeeded | Self::ExpectedFailure)
    }

    pub const fn complete(self) -> bool {
        !matches!(self, Self::Interrupted)
    }

    /// Execution status only. CLI argument-usage errors retain their separate 2.
    pub const fn exit_code(self) -> u8 {
        if self.failed() {
            1
        } else {
            0
        }
    }
}

fn configuration(message: &'static str) -> Diagnostic {
    Diagnostic::new(BWErr::RunConfiguration(message.into()))
}
