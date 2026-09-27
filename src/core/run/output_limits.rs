use super::*;

#[cfg(test)]
mod tests;

pub const DEFAULT_OUTPUT_RECORD_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_OUTPUT_BYTES: usize = 32 * 1024 * 1024;

/// Bytes admitted for serialization and output, including separators/newlines.
/// Failed or cancelled writes retain their whole charge: output is not rolled back.
#[derive(Clone, Debug)]
pub struct OutputLimits {
    pub record_bytes: usize,
    pub total_bytes: usize,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            record_bytes: DEFAULT_OUTPUT_RECORD_BYTES,
            total_bytes: DEFAULT_OUTPUT_BYTES,
        }
    }
}

impl RunBudget {
    pub(crate) fn output_allowance(&self) -> (usize, &'static str, usize) {
        let limits = &self.0.limits.output;
        let remaining = limits
            .total_bytes
            .saturating_sub(self.0.output.load(Ordering::Relaxed));
        if limits.record_bytes <= remaining {
            (
                limits.record_bytes,
                "output record bytes",
                limits.record_bytes,
            )
        } else {
            (remaining, "output total bytes", limits.total_bytes)
        }
    }

    pub(crate) fn charge_output(&self, bytes: usize) -> DiagnosticResult<()> {
        self.checkpoint()?;
        let limit = self.0.limits.output.total_bytes;
        self.0
            .output
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|next| *next <= limit)
            })
            .map(|_| ())
            .map_err(|_| self.limit("output total bytes", limit as u64))
    }
}
