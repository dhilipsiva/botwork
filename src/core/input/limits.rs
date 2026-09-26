use super::*;
use crate::core::{ast::Span, value_limits::ValueLimits};

#[cfg(test)]
mod tests;

/// Limits for one parse call or a complete ordered file/flag load.
#[derive(Clone, Debug)]
pub struct InputLimits {
    pub source_bytes: usize,
    pub total_bytes: usize,
    pub sources: usize,
    pub raw_nodes: usize,
    pub variables: usize,
    pub values: ValueLimits,
}

impl Default for InputLimits {
    fn default() -> Self {
        Self {
            source_bytes: 8 * 1024 * 1024,
            total_bytes: 32 * 1024 * 1024,
            sources: 128,
            raw_nodes: 262_144,
            variables: 16_384,
            values: ValueLimits::default(),
        }
    }
}

pub(super) fn resource(origin: &str, error: BWErr) -> Diagnostic {
    Diagnostic::new(error).at(&Span::input_origin(origin))
}
fn exceeded(origin: &str, name: &'static str, maximum: usize) -> Diagnostic {
    resource(
        origin,
        BWErr::ResourceLimit {
            resource: name,
            limit: maximum as u64,
        },
    )
}

pub(super) struct Budget<'a> {
    pub limits: &'a InputLimits,
    sources: usize,
    bytes: usize,
    nodes: usize,
}

impl<'a> Budget<'a> {
    pub fn new(limits: &'a InputLimits) -> DiagnosticResult<Self> {
        limits.values.validate()?;
        Ok(Self {
            limits,
            sources: 0,
            bytes: 0,
            nodes: 0,
        })
    }
    pub fn source(&mut self, origin: &str) -> DiagnosticResult<()> {
        if self.sources >= self.limits.sources {
            return Err(exceeded(origin, "input sources", self.limits.sources));
        }
        self.sources += 1;
        Ok(())
    }
    pub fn remaining(&self) -> usize {
        self.limits
            .total_bytes
            .saturating_sub(self.bytes)
            .min(self.limits.source_bytes)
    }
    pub fn bytes(&mut self, origin: &str, count: usize) -> DiagnosticResult<()> {
        if count > self.limits.source_bytes {
            return Err(exceeded(
                origin,
                "input source bytes",
                self.limits.source_bytes,
            ));
        }
        self.bytes = self
            .bytes
            .checked_add(count)
            .filter(|n| *n <= self.limits.total_bytes)
            .ok_or_else(|| exceeded(origin, "total input bytes", self.limits.total_bytes))?;
        Ok(())
    }
    fn node(&mut self, origin: &str) -> DiagnosticResult<()> {
        if self.nodes >= self.limits.raw_nodes {
            return Err(exceeded(origin, "input raw nodes", self.limits.raw_nodes));
        }
        self.nodes += 1;
        Ok(())
    }
    pub fn variable_count(&self, origin: &str, count: usize) -> DiagnosticResult<()> {
        if count > self.limits.variables {
            return Err(exceeded(origin, "input variables", self.limits.variables));
        }
        Ok(())
    }

    pub fn preflight(&mut self, origin: &str, text: &str) -> DiagnosticResult<()> {
        let bytes = text.as_bytes();
        let mut frames: Vec<(u8, usize)> = vec![];
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' | b',' | b':' => {
                    index += 1;
                    continue;
                }
                b']' | b'}' => {
                    frames.pop();
                    index += 1;
                    continue;
                }
                _ => (),
            }
            self.node(origin)?;
            if let Some((b'[', entries)) = frames.last_mut() {
                *entries += 1;
                self.limits
                    .values
                    .container_header(*entries)
                    .map_err(|error| resource(origin, error))?;
            }
            match byte {
                b'[' | b'{' => {
                    if frames.len() >= MAX_JSON_DEPTH {
                        return Err(invalid(origin, "$", "JSON exceeds 128 nested containers"));
                    }
                    frames.push((byte, 0));
                    index += 1;
                }
                b'"' => {
                    let (end, size) = string_extent(bytes, index);
                    let mut after = end;
                    while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                        after += 1;
                    }
                    if bytes.get(after) == Some(&b':') {
                        self.limits
                            .values
                            .key_size(size)
                            .map_err(|error| resource(origin, error))?;
                    } else {
                        self.limits
                            .values
                            .string_size(size)
                            .map_err(|error| resource(origin, error))?;
                    }
                    index = end;
                }
                _ => {
                    index += 1;
                    while index < bytes.len()
                        && !bytes[index].is_ascii_whitespace()
                        && !b",:{}[]\"".contains(&bytes[index])
                    {
                        index += 1;
                    }
                }
            }
        }
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    bytes.iter().try_fold(0u16, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        Some(value * 16 + u16::from(digit))
    })
}

// Count decoded UTF-8 before serde allocates escape buffers or owned strings.
// Serde remains authoritative for malformed escapes and JSON structure.
pub(super) fn string_extent(bytes: &[u8], start: usize) -> (usize, usize) {
    let mut index = start + 1;
    let mut size = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return (index + 1, size),
            b'\\' if bytes.get(index + 1) == Some(&b'u') => {
                if let Some(code) = bytes.get(index + 2..index + 6).and_then(hex) {
                    if (0xd800..=0xdbff).contains(&code)
                        && bytes.get(index + 6..index + 8) == Some(b"\\u")
                        && bytes
                            .get(index + 8..index + 12)
                            .and_then(hex)
                            .is_some_and(|low| (0xdc00..=0xdfff).contains(&low))
                    {
                        size += 4;
                        index += 12;
                        continue;
                    }
                    size += char::from_u32(u32::from(code)).map_or(3, char::len_utf8);
                    index += 6;
                    continue;
                }
                size += 1;
                index += 2;
            }
            b'\\' => {
                size += 1;
                index = (index + 2).min(bytes.len());
            }
            _ => {
                size += 1;
                index += 1;
            }
        }
    }
    (index, size)
}
