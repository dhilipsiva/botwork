//! Bounded rerun selection, with an incomplete marker before case effects.
use super::*;
use crate::atomic_file::AtomicFile;
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashSet, fs::OpenOptions, io::Read, path::Path};

const MAX_HISTORY_BYTES: usize = 2 * 1024 * 1024;
const FORMAT: &str = "botwork-failed-cases";
/// The version `--failures` writes. Version 1 is still read, and deprecated.
const VERSION: u8 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    format: String,
    version: u8,
    complete: bool,
    #[serde(deserialize_with = "failures")]
    failed: Vec<String>,
}

fn failures<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded list of distinct suite/case IDs or suite/case/row IDs")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut failed = Vec::new();
            let mut seen = HashSet::new();
            while let Some(id) = sequence.next_element::<String>()? {
                if failed.len() == suite::MAX_SELECTED_CASES
                    || !suite::valid_run_id(&id)
                    || !seen.insert(id.clone())
                {
                    return Err(serde::de::Error::custom(
                        "invalid, duplicate, or excessive failed case IDs",
                    ));
                }
                failed.push(id);
            }
            Ok(failed)
        }
    }
    deserializer.deserialize_seq(Visitor)
}

fn read(path: &Path) -> Result<Record, CliError> {
    let mut bytes = Vec::new();
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options
        .open(path)
        .and_then(|file| {
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "failed-case input must be an ordinary file",
                ));
            }
            file.take(MAX_HISTORY_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|source| CliError::Read {
            file: path.to_owned(),
            source,
        })?;
    if bytes.len() > MAX_HISTORY_BYTES {
        return Err(suite::resource("failed-case record bytes", MAX_HISTORY_BYTES).into());
    }
    let invalid = |error: serde_json::Error| {
        suite::configuration(format!(
            "Invalid failed-case record {}: {error}",
            path.display()
        ))
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(invalid)?;
    // The version comes first, so a newer record's new fields are reported as
    // its version rather than as unknown fields.
    if value.get("format").and_then(serde_json::Value::as_str) == Some(FORMAT) {
        if let Some(version) = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .filter(|version| *version > u64::from(VERSION))
        {
            return Err(suite::configuration(format!(
                "Failed-case record {} is version {version}, from a newer Botwork; this one reads versions 1 and {VERSION}. Upgrade Botwork to use it",
                path.display()
            ))
            .into());
        }
    }
    let record: Record = serde_json::from_value(value).map_err(invalid)?;
    if record.format != FORMAT
        || !matches!(record.version, 1 | VERSION)
        || (record.version == 1 && record.failed.iter().any(|id| !suite::valid_case_id(id)))
        || (!record.complete && !record.failed.is_empty())
    {
        return Err(suite::configuration("Unsupported or inconsistent failed-case record").into());
    }
    Ok(record)
}

/// The IDs a completed record selects, and a deprecation notice when it is
/// version 1.
pub(super) fn load(path: &Path) -> Result<(Vec<String>, Option<String>), CliError> {
    let record = read(path)?;
    if !record.complete {
        return Err(suite::configuration(
            "Failed-case record is incomplete; rerun selection is unavailable",
        )
        .into());
    }
    let deprecated = (record.version < VERSION).then(|| {
        let path = path.display();
        format!(
            "failed-case record {path} is version {}, which a later Botwork will stop reading; rewrite it as version {VERSION} by also passing --failures {path}",
            record.version
        )
    });
    Ok((record.failed, deprecated))
}

pub(super) struct History {
    file: AtomicFile,
}

impl History {
    pub(super) fn begin(path: PathBuf) -> Result<Self, CliError> {
        if path.file_name().is_none() {
            return Err(suite::configuration("Failed-case output needs a filename").into());
        }
        // Do not overwrite unrelated existing files.
        let file = AtomicFile::begin(path, "Failed-case record", |path| read(path).map(|_| ()))?;
        let history = Self { file };
        history.write(false, vec![])?;
        Ok(history)
    }

    pub(super) fn finish(&self, failed: Vec<String>) -> Result<(), CliError> {
        self.write(true, failed)
    }

    fn write(&self, complete: bool, failed: Vec<String>) -> Result<(), CliError> {
        let mut seen = HashSet::new();
        if failed.len() > suite::MAX_SELECTED_CASES
            || failed
                .iter()
                .any(|id| !suite::valid_run_id(id) || !seen.insert(id))
        {
            return Err(suite::configuration("Invalid failed-case result IDs").into());
        }
        self.file.write(&Record {
            format: FORMAT.into(),
            version: VERSION,
            complete,
            failed,
        })
    }
}

#[cfg(test)]
mod tests;
