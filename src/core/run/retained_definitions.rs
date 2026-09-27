use super::*;
use crate::core::{
    ast::{AstFailure, Definition, SourceFile},
    ast_limits::{measure_definition, DefinitionSize},
};
use std::collections::HashMap;

#[cfg(test)]
mod tests;

/// Live installed DSL definitions and their retained syntax/source ownership.
#[derive(Clone, Debug)]
pub struct RetainedDefinitionLimits {
    pub definitions: usize,
    pub nodes: usize,
    /// Sum of text and name bytes across distinct retained SourceFile allocations.
    pub source_bytes: usize,
}

impl Default for RetainedDefinitionLimits {
    fn default() -> Self {
        Self {
            definitions: 16_384,
            nodes: 262_144,
            source_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Default)]
struct Usage {
    definitions: HashMap<usize, usize>,
    nodes: usize,
    source_bytes: usize,
    sources: HashMap<usize, SourceUsage>,
}

struct SourceUsage {
    references: usize,
}

pub(super) struct RetainedDefinitions {
    limits: RetainedDefinitionLimits,
    used: Mutex<Usage>,
}

fn source_id(source: &Arc<SourceFile>) -> usize {
    Arc::as_ptr(source) as usize
}
fn definition_id(definition: &Arc<Definition>) -> usize {
    Arc::as_ptr(definition) as usize
}

fn exceeded(resource: &'static str, limit: usize) -> BWErr {
    BWErr::ResourceLimit {
        resource,
        limit: limit as u64,
    }
}

impl RetainedDefinitions {
    pub(super) fn new(limits: RetainedDefinitionLimits) -> Self {
        Self {
            limits,
            used: Mutex::new(Usage::default()),
        }
    }

    fn reserve(
        self: &Arc<Self>,
        definition: &Arc<Definition>,
        size: DefinitionSize,
    ) -> Result<Arc<DefinitionReservation>, BWErr> {
        let id = definition_id(definition);
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(references) = used.definitions.get_mut(&id) {
            *references = references
                .checked_add(1)
                .ok_or_else(|| exceeded("retained definitions", self.limits.definitions))?;
            return Ok(Arc::new(DefinitionReservation {
                definition: Arc::clone(definition),
                sources: size.sources,
                nodes: size.nodes,
                owner: Arc::clone(self),
            }));
        }
        if used.definitions.len() >= self.limits.definitions {
            return Err(exceeded("retained definitions", self.limits.definitions));
        }
        let nodes = used
            .nodes
            .checked_add(size.nodes)
            .filter(|nodes| *nodes <= self.limits.nodes)
            .ok_or_else(|| exceeded("retained definition nodes", self.limits.nodes))?;
        let mut source_bytes = used.source_bytes;
        for source in &size.sources {
            if let Some(usage) = used.sources.get(&source_id(source)) {
                usage
                    .references
                    .checked_add(1)
                    .ok_or_else(|| exceeded("retained definitions", self.limits.definitions))?;
            } else {
                source_bytes = source_bytes
                    .checked_add(source.text().len())
                    .and_then(|bytes| bytes.checked_add(source.name().len()))
                    .filter(|bytes| *bytes <= self.limits.source_bytes)
                    .ok_or_else(|| {
                        exceeded("retained definition source bytes", self.limits.source_bytes)
                    })?;
            }
        }
        let reservation = Arc::new(DefinitionReservation {
            // Keep identities alive until removal from the tracker's pointer indexes.
            definition: Arc::clone(definition),
            sources: size.sources,
            nodes: size.nodes,
            owner: Arc::clone(self),
        });
        for source in &reservation.sources {
            used.sources
                .entry(source_id(source))
                .or_insert(SourceUsage { references: 0 })
                .references += 1;
        }
        used.nodes = nodes;
        used.source_bytes = source_bytes;
        used.definitions.insert(id, 1);
        Ok(reservation)
    }
}

pub(crate) struct DefinitionReservation {
    definition: Arc<Definition>,
    sources: Vec<Arc<SourceFile>>,
    nodes: usize,
    owner: Arc<RetainedDefinitions>,
}

impl Drop for DefinitionReservation {
    fn drop(&mut self) {
        let mut used = self.owner.used.lock().unwrap_or_else(|e| e.into_inner());
        let references = used
            .definitions
            .get_mut(&definition_id(&self.definition))
            .expect("reserved definition");
        *references -= 1;
        if *references > 0 {
            return;
        }
        used.definitions.remove(&definition_id(&self.definition));
        used.nodes -= self.nodes;
        for source in &self.sources {
            let id = source_id(source);
            let usage = used.sources.get_mut(&id).expect("reserved source");
            usage.references -= 1;
            if usage.references == 0 {
                used.sources.remove(&id);
                used.source_bytes -= source.text().len() + source.name().len();
            }
        }
    }
}

impl RunBudget {
    pub(crate) fn reserve_definition<E: From<Diagnostic>>(
        &self,
        definition: &Arc<Definition>,
        report: impl Fn(AstFailure<'_>) -> E,
    ) -> Result<Arc<DefinitionReservation>, E> {
        self.checkpoint()?;
        let size = measure_definition(definition, &self.0.limits.ast, self.0.limits.source_bytes)
            .map_err(report)?;
        self.0
            .retained_definitions
            .reserve(definition, size)
            .map_err(|error| E::from(self.stop(error)))
    }
}
