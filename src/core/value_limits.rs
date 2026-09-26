//! Admission and nonrecursive cleanup for owned host values.

use super::grammar::{BWErr, Literal};
use std::{
    collections::{hash_map, BTreeMap},
    ops::{Deref, DerefMut},
};

pub const MAX_VALUE_DEPTH: usize = 64;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct ValueLimits {
    pub nodes: usize,
    pub depth: usize,
    pub string_bytes: usize,
    pub key_bytes: usize,
    pub entries: usize,
    pub payload_bytes: usize,
}

impl Default for ValueLimits {
    fn default() -> Self {
        Self {
            nodes: 65_536,
            depth: MAX_VALUE_DEPTH,
            string_bytes: 1024 * 1024,
            key_bytes: 65_536,
            entries: 16_384,
            payload_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValueSize {
    pub nodes: usize,
    pub depth: usize,
    pub payload_bytes: usize,
}

fn exceeded(resource: &'static str, limit: usize) -> BWErr {
    BWErr::ResourceLimit {
        resource,
        limit: limit as u64,
    }
}

enum Cursor<'a> {
    Value(&'a Literal, usize),
    Array(std::slice::Iter<'a, Literal>, usize),
    Map(hash_map::Iter<'a, String, Literal>, usize),
}

impl ValueLimits {
    pub(crate) fn validate(&self) -> Result<(), BWErr> {
        if self.depth > MAX_VALUE_DEPTH {
            return Err(BWErr::RunConfiguration(format!(
                "Value depth cannot exceed {MAX_VALUE_DEPTH}"
            )));
        }
        Ok(())
    }

    /// Inspect borrowed data without recursive calls or a queue proportional to width.
    /// Numeric finiteness remains the caller's existing semantic validation step.
    pub fn check(&self, value: &Literal) -> Result<ValueSize, BWErr> {
        self.validate()?;
        let mut size = ValueSize::default();
        let mut pending = vec![Cursor::Value(value, 1)];
        while let Some(cursor) = pending.pop() {
            match cursor {
                Cursor::Array(mut values, depth) => {
                    if let Some(value) = values.next() {
                        pending.push(Cursor::Array(values, depth));
                        pending.push(Cursor::Value(value, depth));
                    }
                }
                Cursor::Map(mut values, depth) => {
                    if let Some((key, value)) = values.next() {
                        if key.len() > self.key_bytes {
                            return Err(exceeded("value key bytes", self.key_bytes));
                        }
                        self.add_bytes(&mut size, key.len())?;
                        pending.push(Cursor::Map(values, depth));
                        pending.push(Cursor::Value(value, depth));
                    }
                }
                Cursor::Value(value, depth) => {
                    if size.nodes >= self.nodes {
                        return Err(exceeded("value nodes", self.nodes));
                    }
                    if depth > self.depth {
                        return Err(exceeded("value depth", self.depth));
                    }
                    size.nodes += 1;
                    size.depth = size.depth.max(depth);
                    match value {
                        Literal::None => (),
                        Literal::Int(_) | Literal::Float(_) => self.add_bytes(&mut size, 4)?,
                        Literal::Bool(_) => self.add_bytes(&mut size, 1)?,
                        Literal::String(value) => {
                            if value.len() > self.string_bytes {
                                return Err(exceeded("value string bytes", self.string_bytes));
                            }
                            self.add_bytes(&mut size, value.len())?;
                        }
                        Literal::Array(values) => {
                            if values.len() > self.entries {
                                return Err(exceeded("value container entries", self.entries));
                            }
                            pending.push(Cursor::Array(values.iter(), depth + 1));
                        }
                        Literal::Map(values) => {
                            if values.len() > self.entries {
                                return Err(exceeded("value container entries", self.entries));
                            }
                            pending.push(Cursor::Map(values.iter(), depth + 1));
                        }
                    }
                }
            }
        }
        Ok(size)
    }

    fn add_bytes(&self, size: &mut ValueSize, bytes: usize) -> Result<(), BWErr> {
        size.payload_bytes = size
            .payload_bytes
            .checked_add(bytes)
            .filter(|total| *total <= self.payload_bytes)
            .ok_or_else(|| exceeded("value payload bytes", self.payload_bytes))?;
        Ok(())
    }
}

/// Destroy arbitrarily nested owned data without recursive Literal destruction.
/// Work is proportional to supplied data; this cannot undo a host's prior allocations.
pub fn discard(value: Literal) {
    enum Children {
        Array(std::vec::IntoIter<Literal>),
        Map(hash_map::IntoValues<String, Literal>),
    }
    let mut parents = vec![];
    let mut next = Some(value);
    loop {
        if let Some(value) = next.take() {
            match value {
                Literal::Array(values) => parents.push(Children::Array(values.into_iter())),
                Literal::Map(values) => parents.push(Children::Map(values.into_values())),
                _ => (),
            }
        }
        let Some(parent) = parents.last_mut() else {
            break;
        };
        next = match parent {
            Children::Array(values) => values.next(),
            Children::Map(values) => values.next(),
        };
        if next.is_none() {
            parents.pop();
        }
    }
}

pub(crate) trait ClearValues: Default {
    fn clear_values(&mut self);
}
impl ClearValues for Literal {
    fn clear_values(&mut self) {
        discard(std::mem::take(self));
    }
}
impl ClearValues for Vec<Literal> {
    fn clear_values(&mut self) {
        for value in self.drain(..) {
            discard(value);
        }
    }
}
impl ClearValues for BTreeMap<String, Literal> {
    fn clear_values(&mut self) {
        while let Some((_, value)) = self.pop_first() {
            discard(value);
        }
    }
}

pub(crate) struct Owned<T: ClearValues>(T);
impl<T: ClearValues> Owned<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(value)
    }
    pub(crate) fn into_inner(mut self) -> T {
        std::mem::take(&mut self.0)
    }
}
impl<T: ClearValues> Deref for Owned<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: ClearValues> DerefMut for Owned<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
impl<T: ClearValues> Drop for Owned<T> {
    fn drop(&mut self) {
        self.0.clear_values();
    }
}
