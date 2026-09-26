use super::*;

#[cfg(test)]
mod tests;

pub const MAX_MODULE_CHAIN_DEPTH: usize = 32;

/// Cumulative import work and retained namespace/cache admission budgets.
#[derive(Clone, Debug)]
pub struct ImportLimits {
    pub loads: usize,
    pub source_bytes: usize,
    pub paths: usize,
    pub bindings: usize,
    pub metadata_bytes: usize,
    pub dependency_depth: usize,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            loads: 128,
            source_bytes: 8 * 1024 * 1024,
            paths: 512,
            bindings: 16_384,
            metadata_bytes: 8 * 1024 * 1024,
            dependency_depth: MAX_MODULE_CHAIN_DEPTH,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ImportResource {
    Loads,
    SourceBytes,
    Paths,
    Bindings,
    MetadataBytes,
}

impl ImportResource {
    fn description(self) -> &'static str {
        match self {
            Self::Loads => "module load attempts",
            Self::SourceBytes => "module source bytes",
            Self::Paths => "module path cache entries",
            Self::Bindings => "imported bindings",
            Self::MetadataBytes => "import metadata bytes",
        }
    }
    fn maximum(self, limits: &ImportLimits) -> usize {
        match self {
            Self::Loads => limits.loads,
            Self::SourceBytes => limits.source_bytes,
            Self::Paths => limits.paths,
            Self::Bindings => limits.bindings,
            Self::MetadataBytes => limits.metadata_bytes,
        }
    }
}

impl RunBudget {
    pub(crate) fn import_limit(&self, resource: ImportResource) -> Diagnostic {
        self.limit(
            resource.description(),
            resource.maximum(&self.0.limits.imports) as u64,
        )
    }

    pub(crate) fn import_remaining(&self, resource: ImportResource) -> usize {
        let usage = self.0.imports.lock().unwrap_or_else(|e| e.into_inner());
        resource
            .maximum(&self.0.limits.imports)
            .saturating_sub(usage[resource as usize])
    }

    pub(crate) fn charge_imports(
        &self,
        charges: &[(ImportResource, usize)],
    ) -> DiagnosticResult<()> {
        self.checkpoint()?;
        let mut usage = self.0.imports.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = *usage;
        for &(resource, amount) in charges {
            let Some(value) = next[resource as usize]
                .checked_add(amount)
                .filter(|value| *value <= resource.maximum(&self.0.limits.imports))
            else {
                drop(usage);
                return Err(self.import_limit(resource));
            };
            next[resource as usize] = value;
        }
        *usage = next;
        Ok(())
    }
}
