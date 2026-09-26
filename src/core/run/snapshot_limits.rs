use super::*;

#[cfg(test)]
mod tests;

/// Cumulative work admitted before copying frame and module-cache tables.
#[derive(Clone, Debug)]
pub struct SnapshotLimits {
    pub entries: usize,
    pub path_bytes: usize,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            entries: 1_048_576,
            path_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SnapshotSize {
    entries: Option<usize>,
    path_bytes: Option<usize>,
}

impl Default for SnapshotSize {
    fn default() -> Self {
        Self {
            entries: Some(0),
            path_bytes: Some(0),
        }
    }
}

impl SnapshotSize {
    pub(crate) fn entries(&mut self, count: usize) {
        self.entries = self.entries.and_then(|used| used.checked_add(count));
    }

    pub(crate) fn path(&mut self, path: &Path) {
        self.path_bytes(path.as_os_str().len());
    }

    pub(crate) fn path_bytes(&mut self, bytes: usize) {
        self.path_bytes = self.path_bytes.and_then(|used| used.checked_add(bytes));
    }
}

impl RunBudget {
    pub(crate) fn charge_snapshot(&self, size: SnapshotSize) -> DiagnosticResult<()> {
        self.checkpoint()?;
        let limits = &self.0.limits.snapshots;
        let mut usage = self
            .0
            .snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let next = [
            size.entries.and_then(|amount| usage[0].checked_add(amount)),
            size.path_bytes
                .and_then(|amount| usage[1].checked_add(amount)),
        ];
        for (value, limit, resource) in [
            (next[0], limits.entries, "snapshot table entries"),
            (next[1], limits.path_bytes, "snapshot path bytes"),
        ] {
            if value.is_none_or(|value| value > limit) {
                drop(usage);
                return Err(self.limit(resource, limit as u64));
            }
        }
        *usage = [next[0].unwrap(), next[1].unwrap()];
        Ok(())
    }
}
