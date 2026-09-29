//! Parsed modules shared between runs. A syntax tree is immutable, so runs that
//! import the same module text under the same parse limits share one tree.
//! Module state (globals, definitions, and initialization) is still built fresh
//! in every run; only parsing is shared.
use super::Program;
use crate::core::{ast_limits::AstLimits, syntax_limits::SyntaxLimits};
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

/// Source text a cache keeps before evicting its least recently used modules.
pub const DEFAULT_COMPILED_SOURCE_BYTES: usize = 64 * 1024 * 1024;

/// Counts for one cache; hits and misses accumulate over its lifetime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompiledStatistics {
    /// Distinct parses kept: one per path, text, and limit combination.
    pub modules: usize,
    /// Source text held by those parses, in UTF-8 bytes.
    pub source_bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

/// Everything a parse depends on besides the text: identical inputs always
/// produce an identical tree, so a hit is exactly what a fresh parse returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParseLimits {
    pub(crate) source_bytes: usize,
    pub(crate) syntax: SyntaxLimits,
    pub(crate) ast: AstLimits,
}

struct Entry {
    source: String,
    limits: ParseLimits,
    program: Arc<Program>,
    used: u64,
}

#[derive(Default)]
struct Store {
    capacity: usize,
    by_name: HashMap<String, Vec<Entry>>,
    clock: u64,
    statistics: CompiledStatistics,
}

impl Store {
    fn find(&mut self, name: &str, source: &str, limits: &ParseLimits) -> Option<Arc<Program>> {
        self.clock += 1;
        let clock = self.clock;
        let entry = self
            .by_name
            .get_mut(name)?
            .iter_mut()
            .find(|entry| entry.limits == *limits && entry.source == source)?;
        entry.used = clock;
        Some(Arc::clone(&entry.program))
    }

    /// Evict the least recently used parses until the text fits the capacity.
    fn evict(&mut self) {
        while self.statistics.source_bytes > self.capacity {
            let Some((name, index)) = self
                .by_name
                .iter()
                .flat_map(|(name, entries)| {
                    entries
                        .iter()
                        .enumerate()
                        .map(move |(index, entry)| (entry.used, name, index))
                })
                .min()
                .map(|(_, name, index)| (name.clone(), index))
            else {
                return;
            };
            let entries = self.by_name.get_mut(&name).expect("present");
            let entry = entries.swap_remove(index);
            if entries.is_empty() {
                self.by_name.remove(&name);
            }
            self.statistics.modules -= 1;
            self.statistics.source_bytes -= entry.source.len();
            self.statistics.evictions += 1;
        }
    }
}

/// A shared cache of parsed modules. Clones share one cache; give runs the same
/// cache to parse each unchanged module once.
#[derive(Clone)]
pub struct CompiledModules(Arc<Mutex<Store>>);

impl Default for CompiledModules {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_COMPILED_SOURCE_BYTES)
    }
}

impl fmt::Debug for CompiledModules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CompiledModules({:?})", self.statistics())
    }
}

impl CompiledModules {
    /// A cache that keeps at most `source_bytes` of module text; a module larger
    /// than that is parsed but never kept.
    pub fn with_capacity(source_bytes: usize) -> Self {
        Self(Arc::new(Mutex::new(Store {
            capacity: source_bytes,
            ..Store::default()
        })))
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn statistics(&self) -> CompiledStatistics {
        self.store().statistics
    }

    /// Drop every kept parse; counters are kept.
    pub fn clear(&self) {
        let mut store = self.store();
        store.by_name.clear();
        store.statistics.modules = 0;
        store.statistics.source_bytes = 0;
    }

    /// The kept tree for this exact text and these limits, or `parse`'s result,
    /// which is kept when it succeeds. Parsing happens outside the lock.
    pub(crate) fn get_or_parse<E>(
        &self,
        name: &str,
        source: &str,
        limits: ParseLimits,
        parse: impl FnOnce() -> Result<Program, E>,
    ) -> Result<Arc<Program>, E> {
        {
            let mut store = self.store();
            if let Some(program) = store.find(name, source, &limits) {
                store.statistics.hits += 1;
                return Ok(program);
            }
            store.statistics.misses += 1;
        }
        let program = Arc::new(parse()?);
        let mut store = self.store();
        if source.len() > store.capacity {
            return Ok(program);
        }
        // A concurrent run may have parsed the same text meanwhile; keep one.
        if let Some(existing) = store.find(name, source, &limits) {
            return Ok(existing);
        }
        let used = store.clock;
        store
            .by_name
            .entry(name.to_owned())
            .or_default()
            .push(Entry {
                source: source.to_owned(),
                limits,
                program: Arc::clone(&program),
                used,
            });
        store.statistics.modules += 1;
        store.statistics.source_bytes += source.len();
        store.evict();
        Ok(program)
    }
}

#[cfg(test)]
mod tests;
