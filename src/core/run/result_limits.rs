use super::*;
use crate::core::value_limits::{ValueSize, MAX_VALUE_DEPTH};

#[cfg(test)]
mod tests;

/// Logical owned return/root-export budgets, independent of live runtime storage.
#[derive(Clone, Debug)]
pub struct ResultLimits {
    pub values: usize,
    pub nodes: usize,
    pub name_bytes: usize,
    pub payload_bytes: usize,
}

impl Default for ResultLimits {
    fn default() -> Self {
        Self {
            values: 65_537,
            nodes: 327_680,
            name_bytes: 4 * 1024 * 1024,
            payload_bytes: 40 * 1024 * 1024,
        }
    }
}

impl ResultLimits {
    fn exceeded(&self, resource: &'static str, maximum: usize) -> Diagnostic {
        BWErr::ResourceLimit {
            resource,
            limit: maximum as u64,
        }
        .into()
    }

    pub(crate) fn count(&self, bindings: usize, has_result: bool) -> DiagnosticResult<()> {
        if bindings
            .checked_add(usize::from(has_result))
            .is_none_or(|values| values > self.values)
        {
            return Err(self.exceeded("result values", self.values));
        }
        Ok(())
    }

    pub(crate) fn names(&self, names: impl Iterator<Item = usize>) -> DiagnosticResult<()> {
        let mut bytes = 0usize;
        for name in names {
            bytes = bytes
                .checked_add(name)
                .filter(|bytes| *bytes <= self.name_bytes)
                .ok_or_else(|| self.exceeded("result name bytes", self.name_bytes))?;
        }
        Ok(())
    }

    pub(crate) fn value(&self, value: &Literal, used: &mut ValueSize) -> DiagnosticResult<()> {
        let limits = ValueLimits {
            nodes: self.nodes.saturating_sub(used.nodes),
            depth: MAX_VALUE_DEPTH,
            payload_bytes: self.payload_bytes.saturating_sub(used.payload_bytes),
            string_bytes: usize::MAX,
            key_bytes: usize::MAX,
            entries: usize::MAX,
        };
        let measured = limits.check(value).map_err(|error| match error {
            BWErr::ResourceLimit {
                resource: "value nodes",
                ..
            } => self.exceeded("result nodes", self.nodes),
            BWErr::ResourceLimit {
                resource: "value payload bytes",
                ..
            } => self.exceeded("result payload bytes", self.payload_bytes),
            error => Diagnostic::new(error),
        })?;
        let nodes = used
            .nodes
            .checked_add(measured.nodes)
            .filter(|nodes| *nodes <= self.nodes)
            .ok_or_else(|| self.exceeded("result nodes", self.nodes))?;
        let payload_bytes = used
            .payload_bytes
            .checked_add(measured.payload_bytes)
            .filter(|bytes| *bytes <= self.payload_bytes)
            .ok_or_else(|| self.exceeded("result payload bytes", self.payload_bytes))?;
        used.nodes = nodes;
        used.payload_bytes = payload_bytes;
        Ok(())
    }
}
