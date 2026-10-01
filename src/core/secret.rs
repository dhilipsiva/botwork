//! Secret-marked values and exact-text masking. A [`Secrets`] registry holds the
//! texts of secret values; every output that leaves a run passes through it, so
//! a secret's exact text never reaches stdout, diagnostics, records, reports, or
//! listeners. Derived values, such as a substring or a re-encoding, are not
//! secrets unless they are marked too.
use super::grammar::{Literal, QuotedString};
use regex::Regex;
use std::{
    borrow::Cow,
    collections::BTreeSet,
    fmt,
    io::{self, Write},
    sync::{Arc, RwLock},
};

/// What replaces every occurrence of a secret's text.
pub(crate) const MASK: &str = "***";

#[derive(Default)]
struct Masker {
    texts: BTreeSet<String>,
    pattern: Option<Regex>,
}

/// A shared, append-only registry of secret texts. Clones share the registry,
/// so a value marked after an output was set up is still masked there.
#[derive(Clone, Default)]
pub struct Secrets(Arc<RwLock<Arc<Masker>>>);

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never render the texts themselves.
        write!(f, "Secrets({} texts)", self.masker().texts.len())
    }
}

/// Every spelling of a string that an output can contain: the text itself, its
/// escaped forms inside JSON strings and quoted collection values, and its URL
/// form encoding, as in a query string.
fn spellings(text: &str, texts: &mut BTreeSet<String>) {
    if text.is_empty() {
        return;
    }
    texts.insert(text.to_owned());
    let json = serde_json::to_string(text).expect("strings serialize");
    texts.insert(json[1..json.len() - 1].to_owned());
    let quoted = QuotedString(text).to_string();
    texts.insert(quoted[1..quoted.len() - 1].to_owned());
    texts.insert(url::form_urlencoded::byte_serialize(text.as_bytes()).collect());
}

/// The string and number leaves of a value, however deeply nested. Booleans and
/// None carry too little to mask without masking ordinary output.
fn leaves(value: &Literal, texts: &mut BTreeSet<String>) {
    match value {
        Literal::String(text) => spellings(text, texts),
        Literal::Int(number) => spellings(&number.to_string(), texts),
        Literal::Float(number) => spellings(&number.to_string(), texts),
        Literal::Array(items) => items.iter().for_each(|item| leaves(item, texts)),
        Literal::Map(entries) => entries.values().for_each(|item| leaves(item, texts)),
        Literal::Bool(_) | Literal::None => {}
    }
}

impl Secrets {
    fn masker(&self) -> Arc<Masker> {
        Arc::clone(&self.0.read().unwrap_or_else(|error| error.into_inner()))
    }

    /// Mark a value secret: its string and number leaves are masked from now on.
    pub fn add(&self, value: &Literal) {
        let mut texts = BTreeSet::new();
        leaves(value, &mut texts);
        self.add_texts(texts);
    }

    /// Mark exact texts secret.
    pub fn add_texts(&self, texts: impl IntoIterator<Item = String>) {
        let mut registry = self.0.write().unwrap_or_else(|error| error.into_inner());
        let mut all = registry.texts.clone();
        let before = all.len();
        all.extend(texts.into_iter().filter(|text| !text.is_empty()));
        if all.len() == before {
            return;
        }
        // Longer texts first, so a secret containing another is masked whole.
        let mut ordered: Vec<&String> = all.iter().collect();
        ordered.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        let alternation = ordered
            .iter()
            .map(|text| regex::escape(text))
            .collect::<Vec<_>>()
            .join("|");
        let pattern = regex::RegexBuilder::new(&alternation)
            .size_limit(64 * 1024 * 1024)
            .build()
            .expect("escaped literals form a valid pattern");
        *registry = Arc::new(Masker {
            texts: all,
            pattern: Some(pattern),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.masker().pattern.is_none()
    }

    /// `text` with every secret occurrence replaced by `***`.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        match &self.masker().pattern {
            Some(pattern) => match pattern.replace_all(text, MASK) {
                Cow::Borrowed(_) => Cow::Borrowed(text),
                Cow::Owned(masked) => Cow::Owned(masked),
            },
            None => Cow::Borrowed(text),
        }
    }

    /// A copy of `value` whose strings and keys have every secret masked and
    /// whose secret number leaves become the mask, or None when nothing is
    /// secret. Messages built from the copy cannot reveal a secret through a
    /// cut preview or a character-level difference.
    pub fn mask_value(&self, value: &Literal) -> Option<Literal> {
        if self.is_empty() {
            return None;
        }
        self.masked(value)
    }

    fn masked(&self, value: &Literal) -> Option<Literal> {
        let text = |text: &str| match self.redact(text) {
            Cow::Owned(masked) => Some(masked),
            Cow::Borrowed(_) => None,
        };
        match value {
            Literal::String(value) => text(value).map(Literal::String),
            Literal::Int(number) => text(&number.to_string()).map(|_| Literal::String(MASK.into())),
            Literal::Float(number) => {
                text(&number.to_string()).map(|_| Literal::String(MASK.into()))
            }
            Literal::Array(items) => {
                let masked: Vec<_> = items.iter().map(|item| self.masked(item)).collect();
                masked.iter().any(Option::is_some).then(|| {
                    Literal::Array(
                        masked
                            .into_iter()
                            .zip(items)
                            .map(|(masked, item)| masked.unwrap_or_else(|| item.clone()))
                            .collect(),
                    )
                })
            }
            Literal::Map(entries) => {
                let masked: Vec<_> = entries
                    .iter()
                    .map(|(key, item)| (text(key), self.masked(item)))
                    .collect();
                masked
                    .iter()
                    .any(|(key, item)| key.is_some() || item.is_some())
                    .then(|| {
                        Literal::Map(
                            masked
                                .into_iter()
                                .zip(entries)
                                .map(|((key, masked), (original, item))| {
                                    (
                                        key.unwrap_or_else(|| original.clone()),
                                        masked.unwrap_or_else(|| item.clone()),
                                    )
                                })
                                .collect(),
                        )
                    })
            }
            Literal::Bool(_) | Literal::None => None,
        }
    }

    /// A writer that masks each flushed record before it reaches `inner`.
    pub fn writer<W: Write>(&self, inner: W) -> Redacting<W> {
        Redacting {
            secrets: self.clone(),
            inner,
            buffer: Vec::new(),
        }
    }
}

/// Buffers writes until `flush`, then masks the whole record, so a secret that
/// spans formatting chunks is still found. Without secrets it passes through.
pub struct Redacting<W: Write> {
    secrets: Secrets,
    inner: W,
    buffer: Vec<u8>,
}

impl<W: Write> Write for Redacting<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.buffer.is_empty() && self.secrets.is_empty() {
            return self.inner.write(bytes);
        }
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            let text = String::from_utf8_lossy(&self.buffer);
            let masked = self.secrets.redact(&text);
            self.inner.write_all(masked.as_bytes())?;
            self.buffer.clear();
        }
        self.inner.flush()
    }
}

impl<W: Write> Drop for Redacting<W> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests;
