use super::*;
use crate::core::run::SnapshotSize;

#[cfg(test)]
mod tests;

impl Frame {
    pub(super) fn snapshot_size(&self, size: &mut SnapshotSize) {
        size.entries(self.variables.len());
        size.entries(self.statements.len());
        size.entries(self.namespaces.len());
    }
}

// Rebuild from live entries: HashMap::clone can copy unused historical capacity.
impl Clone for Frame {
    fn clone(&self) -> Self {
        Self {
            variables: self
                .variables
                .iter()
                .map(|(key, value)| (key.clone(), Arc::clone(value)))
                .collect(),
            statements: self
                .statements
                .iter()
                .map(|(key, value)| (Arc::clone(key), value.clone()))
                .collect(),
            namespaces: self
                .namespaces
                .iter()
                .map(|(key, value)| (Arc::clone(key), value.clone()))
                .collect(),
            dependency_depth: self.dependency_depth,
            parent: self.parent,
        }
    }
}

impl Context {
    pub(super) fn charge_snapshot(&self, size: SnapshotSize) -> DiagnosticResult<()> {
        self.budget
            .as_ref()
            .map_or(Ok(()), |budget| budget.charge_snapshot(size))
    }

    pub(super) fn isolated_snapshot_size(&self) -> SnapshotSize {
        let mut size = SnapshotSize::default();
        self.modules.snapshot_size(&mut size);
        for path in &self.loading {
            size.path(path);
        }
        match &self.working_directory {
            Ok(path) => size.path(path),
            Err(message) => size.path_bytes(message.len()),
        }
        size
    }

    /// Admit table/path copying against this Context's cumulative snapshot budget.
    /// Successful copies inherit the updated work counters; a failed admission
    /// latches this Context. Immutable stored payloads and live trackers are shared.
    /// Calls/handlers and owned result/diagnostic payloads have separate contracts.
    pub fn try_clone(&self) -> DiagnosticResult<Self> {
        let mut size = self.isolated_snapshot_size();
        for frame in &self.frames {
            frame.snapshot_size(&mut size);
        }
        self.charge_snapshot(size)?;
        Ok(self.clone())
    }

    /// Engine templates contain only a root native registry. Admit before copying.
    pub(crate) fn copy_native_template(&mut self, template: &Context) -> DiagnosticResult<()> {
        let mut size = SnapshotSize::default();
        template.frames[0].snapshot_size(&mut size);
        self.charge_snapshot(size)?;
        self.frames[0] = template.frames[0].clone();
        self.admit_native_registry()
    }
}
