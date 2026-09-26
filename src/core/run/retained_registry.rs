use super::*;
use crate::core::ast::{Definition, SourceFile, Span};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
mod tests;

/// Live statement/namespace records, their string payload, and source owners.
#[derive(Clone, Debug)]
pub struct RetainedRegistryLimits {
    pub entries: usize,
    pub nodes: usize,
    pub name_bytes: usize,
    pub text_bytes: usize,
    pub source_bytes: usize,
}

impl Default for RetainedRegistryLimits {
    fn default() -> Self {
        Self {
            entries: 65_536,
            nodes: 262_144,
            name_bytes: 65_536,
            text_bytes: 8 * 1024 * 1024,
            source_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Default)]
struct Usage {
    entries: usize,
    nodes: usize,
    text_bytes: usize,
    source_bytes: usize,
    sources: HashMap<usize, usize>,
}

pub(super) struct RetainedRegistry {
    limits: RetainedRegistryLimits,
    used: Mutex<Usage>,
}

/// String/source measurements precede owned signature and qualified-key copies.
pub(crate) struct RegistryPlan<'a> {
    prefix: Option<&'a str>,
    name: &'a str,
    existing_key: Option<Arc<str>>,
    nodes: Option<usize>,
    text_bytes: Option<usize>,
    sources: Vec<Arc<SourceFile>>,
    seen: HashSet<usize>,
}

impl<'a> RegistryPlan<'a> {
    fn source(&mut self, span: &Span) {
        if self.seen.insert(source_id(span.source())) {
            self.sources.push(Arc::clone(span.source()));
        }
    }

    pub(crate) fn definition(definition: &'a Definition) -> Self {
        let mut plan = Self {
            prefix: None,
            name: &definition.signature,
            existing_key: None,
            nodes: definition.parameters.len().checked_add(1),
            text_bytes: definition.signature.len().checked_mul(2),
            sources: vec![],
            seen: HashSet::new(),
        };
        plan.source(&definition.header);
        for parameter in &definition.parameters {
            plan.text_bytes = plan
                .text_bytes
                .and_then(|bytes| bytes.checked_add(parameter.text.len()));
            plan.source(&parameter.span);
        }
        plan
    }

    pub(crate) fn signature(signature: &'a StatementSignature) -> Self {
        let mut plan = Self {
            prefix: None,
            name: signature.normalized(),
            existing_key: None,
            nodes: signature
                .parameters()
                .len()
                .checked_add(signature.documented_errors().len())
                .and_then(|nodes| nodes.checked_add(1)),
            text_bytes: signature.retained_bytes(),
            sources: vec![],
            seen: HashSet::new(),
        };
        plan.source(signature.header());
        for parameter in signature.parameters() {
            plan.source(&parameter.span);
        }
        plan
    }

    pub(crate) fn qualified(
        signature: &'a StatementSignature,
        namespace: &str,
        normalized: &'a str,
        import_site: &Span,
    ) -> Self {
        let mut plan = Self::signature(signature);
        plan.prefix = Some(normalized);
        // The exported lookup key is shared with the retained module frame.
        plan.text_bytes = signature
            .qualified_bytes(namespace, normalized)
            .and_then(|bytes| bytes.checked_sub(signature.normalized().len()));
        plan.source(import_site);
        plan
    }

    pub(crate) fn namespace(name: &'a str, span: &Span) -> Self {
        let mut plan = Self {
            prefix: None,
            name,
            existing_key: None,
            nodes: Some(1),
            text_bytes: Some(name.len()),
            sources: vec![],
            seen: HashSet::new(),
        };
        plan.source(span);
        plan
    }

    pub(crate) fn with_key(mut self, key: Arc<str>) -> Self {
        self.existing_key = Some(key);
        self
    }

    fn name_bytes(&self) -> Option<usize> {
        match self.prefix {
            Some(prefix) => prefix.len().checked_add(2)?.checked_add(self.name.len()),
            None => Some(self.name.len()),
        }
    }

    pub(crate) fn key(&self) -> Arc<str> {
        if let Some(key) = &self.existing_key {
            return Arc::clone(key);
        }
        match self.prefix {
            Some(prefix) => format!("{prefix}::{}", self.name).into(),
            None => self.name.into(),
        }
    }
}

fn exceeded(resource: &'static str, limit: usize) -> BWErr {
    BWErr::ResourceLimit {
        resource,
        limit: limit as u64,
    }
}
fn source_id(source: &Arc<SourceFile>) -> usize {
    Arc::as_ptr(source) as usize
}

impl RetainedRegistry {
    pub(super) fn new(limits: RetainedRegistryLimits) -> Self {
        Self {
            limits,
            used: Mutex::new(Usage::default()),
        }
    }

    fn reserve(
        self: &Arc<Self>,
        plan: RegistryPlan<'_>,
    ) -> Result<Arc<RegistryReservation>, BWErr> {
        if plan
            .name_bytes()
            .is_none_or(|bytes| bytes > self.limits.name_bytes)
        {
            return Err(exceeded("registry name bytes", self.limits.name_bytes));
        }
        let nodes = plan
            .nodes
            .ok_or_else(|| exceeded("registry nodes", self.limits.nodes))?;
        let text = plan
            .text_bytes
            .ok_or_else(|| exceeded("registry text bytes", self.limits.text_bytes))?;
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let entries = used
            .entries
            .checked_add(1)
            .filter(|value| *value <= self.limits.entries)
            .ok_or_else(|| exceeded("registry entries", self.limits.entries))?;
        let total_nodes = used
            .nodes
            .checked_add(nodes)
            .filter(|value| *value <= self.limits.nodes)
            .ok_or_else(|| exceeded("registry nodes", self.limits.nodes))?;
        let text_bytes = used
            .text_bytes
            .checked_add(text)
            .filter(|value| *value <= self.limits.text_bytes)
            .ok_or_else(|| exceeded("registry text bytes", self.limits.text_bytes))?;
        let mut source_bytes = used.source_bytes;
        for source in &plan.sources {
            if let Some(references) = used.sources.get(&source_id(source)) {
                references
                    .checked_add(1)
                    .ok_or_else(|| exceeded("registry entries", self.limits.entries))?;
            } else {
                source_bytes = source_bytes
                    .checked_add(source.text().len())
                    .and_then(|value| value.checked_add(source.name().len()))
                    .filter(|value| *value <= self.limits.source_bytes)
                    .ok_or_else(|| exceeded("registry source bytes", self.limits.source_bytes))?;
            }
        }
        let reservation = Arc::new(RegistryReservation {
            key: plan.key(),
            nodes,
            text_bytes: text,
            sources: plan.sources,
            owner: Arc::clone(self),
        });
        for source in &reservation.sources {
            *used.sources.entry(source_id(source)).or_insert(0) += 1;
        }
        used.entries = entries;
        used.nodes = total_nodes;
        used.text_bytes = text_bytes;
        used.source_bytes = source_bytes;
        Ok(reservation)
    }
}

pub(crate) struct RegistryReservation {
    pub(crate) key: Arc<str>,
    nodes: usize,
    text_bytes: usize,
    sources: Vec<Arc<SourceFile>>,
    owner: Arc<RetainedRegistry>,
}

impl Drop for RegistryReservation {
    fn drop(&mut self) {
        let mut used = self.owner.used.lock().unwrap_or_else(|e| e.into_inner());
        used.entries -= 1;
        used.nodes -= self.nodes;
        used.text_bytes -= self.text_bytes;
        for source in &self.sources {
            let id = source_id(source);
            let references = used.sources.get_mut(&id).expect("reserved registry source");
            *references -= 1;
            if *references == 0 {
                used.sources.remove(&id);
                used.source_bytes -= source.text().len() + source.name().len();
            }
        }
    }
}

impl RunBudget {
    pub(crate) fn reserve_registry(
        &self,
        plan: RegistryPlan<'_>,
    ) -> DiagnosticResult<Arc<RegistryReservation>> {
        self.checkpoint()?;
        self.0
            .retained_registry
            .reserve(plan)
            .map_err(|error| self.stop(error))
    }
}
