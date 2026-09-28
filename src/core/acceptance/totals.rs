use super::*;

/// Bounded-memory counts of terminal observations; expected failures stay visible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CaseTotals {
    total: usize,
    succeeded: usize,
    expected_failure: usize,
    failed: usize,
    unexpected_pass: usize,
    skipped: usize,
    cancelled: usize,
    timed_out: usize,
    limit_exceeded: usize,
    interrupted: usize,
}

impl CaseTotals {
    pub fn total(&self) -> usize {
        self.total
    }

    pub fn count(&self, status: CaseStatus) -> usize {
        match status {
            CaseStatus::Succeeded => self.succeeded,
            CaseStatus::ExpectedFailure => self.expected_failure,
            CaseStatus::Failed => self.failed,
            CaseStatus::UnexpectedPass => self.unexpected_pass,
            CaseStatus::Skipped => self.skipped,
            CaseStatus::Cancelled => self.cancelled,
            CaseStatus::TimedOut => self.timed_out,
            CaseStatus::LimitExceeded => self.limit_exceeded,
            CaseStatus::Interrupted => self.interrupted,
        }
    }

    pub fn record(&mut self, status: CaseStatus) -> DiagnosticResult<()> {
        self.record_many(status, 1)
    }

    /// Admit both counters before publishing either, including overflow checks.
    pub fn record_many(&mut self, status: CaseStatus, count: usize) -> DiagnosticResult<()> {
        let total = self
            .total
            .checked_add(count)
            .ok_or_else(|| configuration("Acceptance result count overflow"))?;
        let slot = match status {
            CaseStatus::Succeeded => &mut self.succeeded,
            CaseStatus::ExpectedFailure => &mut self.expected_failure,
            CaseStatus::Failed => &mut self.failed,
            CaseStatus::UnexpectedPass => &mut self.unexpected_pass,
            CaseStatus::Skipped => &mut self.skipped,
            CaseStatus::Cancelled => &mut self.cancelled,
            CaseStatus::TimedOut => &mut self.timed_out,
            CaseStatus::LimitExceeded => &mut self.limit_exceeded,
            CaseStatus::Interrupted => &mut self.interrupted,
        };
        let next = slot
            .checked_add(count)
            .ok_or_else(|| configuration("Acceptance result count overflow"))?;
        *slot = next;
        self.total = total;
        Ok(())
    }

    pub fn finish(self, fixture_failures: usize, delivery: Delivery) -> RunVerdict {
        let status = if self.interrupted != 0 || delivery == Delivery::Interrupted {
            CaseStatus::Interrupted
        } else if delivery != Delivery::Complete
            || fixture_failures != 0
            || self.failed != 0
            || self.unexpected_pass != 0
            || self.skipped != 0
            || self.cancelled != 0
            || self.timed_out != 0
            || self.limit_exceeded != 0
        {
            CaseStatus::Failed
        } else {
            CaseStatus::Succeeded
        };
        RunVerdict {
            status,
            complete: delivery == Delivery::Complete && self.interrupted == 0,
            cases: self,
            fixture_failures,
            delivery,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Complete,
    Failed,
    Interrupted,
}

/// Final verdict requires successful outcome/report/history delivery. A failed
/// writer or interrupted owner cannot publish a complete successful execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RunVerdict {
    status: CaseStatus,
    complete: bool,
    cases: CaseTotals,
    fixture_failures: usize,
    delivery: Delivery,
}

impl RunVerdict {
    pub fn status(&self) -> CaseStatus {
        self.status
    }
    pub fn complete(&self) -> bool {
        self.complete
    }
    pub fn cases(&self) -> &CaseTotals {
        &self.cases
    }
    pub fn fixture_failures(&self) -> usize {
        self.fixture_failures
    }
    pub fn exit_code(&self) -> u8 {
        self.status.exit_code()
    }
    pub fn failed(&self) -> bool {
        self.status.failed()
    }
}
