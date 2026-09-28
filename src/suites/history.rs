//! Bounded rerun selection, with an incomplete marker before case effects.
use super::*;
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_HISTORY_BYTES: usize = 2 * 1024 * 1024;
const FORMAT: &str = "botwork-failed-cases";
static TEMPORARY: AtomicU64 = AtomicU64::new(0);

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
    let record: Record = serde_json::from_slice(&bytes).map_err(|error| {
        suite::configuration(format!(
            "Invalid failed-case record {}: {error}",
            path.display()
        ))
    })?;
    if record.format != FORMAT
        || !matches!(record.version, 1 | 2)
        || (record.version == 1 && record.failed.iter().any(|id| !suite::valid_case_id(id)))
        || (!record.complete && !record.failed.is_empty())
    {
        return Err(suite::configuration("Unsupported or inconsistent failed-case record").into());
    }
    Ok(record)
}

pub(super) fn load(path: &Path) -> Result<Vec<String>, CliError> {
    let record = read(path)?;
    if !record.complete {
        return Err(suite::configuration(
            "Failed-case record is incomplete; rerun selection is unavailable",
        )
        .into());
    }
    Ok(record.failed)
}

fn output_error(path: &Path, error: impl std::fmt::Display) -> CliError {
    Diagnostic::new(BWErr::OutputError(format!(
        "Failed-case record {}: {error}",
        path.display()
    )))
    .into()
}

pub(super) struct History {
    path: PathBuf,
    // The persistent sidecar inode is never replaced or removed by a writer.
    // Its advisory lock is released by the kernel when this handle closes.
    _lock: File,
}

impl History {
    pub(super) fn begin(path: PathBuf) -> Result<Self, CliError> {
        let filename = path
            .file_name()
            .ok_or_else(|| suite::configuration("Failed-case output needs a filename"))?;
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent).map_err(|error| output_error(&path, error))?;
        let path = parent.join(filename);
        let mut lock_name = filename.to_os_string();
        lock_name.push(".lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let lock = options
            .open(parent.join(lock_name))
            .map_err(|error| output_error(&path, error))?;
        if !lock
            .metadata()
            .map_err(|error| output_error(&path, error))?
            .is_file()
        {
            return Err(output_error(&path, "lock must be an ordinary file"));
        }
        lock.try_lock()
            .map_err(|error| output_error(&path, error))?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(output_error(&path, "output must be an ordinary file"))
            }
            Ok(_) => {
                read(&path)?;
            } // Do not overwrite unrelated existing files.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(output_error(&path, error)),
        }
        let history = Self { path, _lock: lock };
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
        let record = Record {
            format: FORMAT.into(),
            version: 2,
            complete,
            failed,
        };
        let parent = self.path.parent().expect("canonical parent");
        for _ in 0..64 {
            let temporary = parent.join(format!(
                ".botwork-failures-{}-{}.tmp",
                std::process::id(),
                TEMPORARY.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = match options.open(&temporary) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(output_error(&self.path, error)),
            };
            let result = (|| -> Result<(), CliError> {
                serde_json::to_writer(&mut file, &record)
                    .map_err(|error| output_error(&self.path, error))?;
                file.write_all(b"\n")
                    .and_then(|_| file.sync_all())
                    .map_err(|error| output_error(&self.path, error))?;
                drop(file);
                fs::rename(&temporary, &self.path)
                    .map_err(|error| output_error(&self.path, error))?;
                #[cfg(unix)]
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| output_error(&self.path, error))?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            return result;
        }
        Err(output_error(
            &self.path,
            "temporary file collisions exhausted",
        ))
    }
}

#[cfg(test)]
mod tests;
