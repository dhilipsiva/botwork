//! Exclusive, atomically replaced report outputs: a persistent lock sidecar, an
//! incomplete marker written before any effects, and fsync'd rename publication.
use super::{BWErr, CliError, Diagnostic};
use serde::Serialize;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) static TEMPORARY: AtomicU64 = AtomicU64::new(0);

pub(super) struct AtomicFile {
    path: PathBuf,
    /// Names the output in errors, such as `Failed-case record`.
    label: &'static str,
    // The persistent sidecar inode is never replaced or removed by a writer.
    // Its advisory lock is released by the kernel when this handle closes.
    _lock: File,
}

impl AtomicFile {
    /// Lock `path` and accept an existing file only when `existing` recognizes it,
    /// so unrelated files are never overwritten.
    pub(super) fn begin(
        path: PathBuf,
        label: &'static str,
        existing: impl FnOnce(&Path) -> Result<(), CliError>,
    ) -> Result<Self, CliError> {
        let error = |path: &Path, error: &dyn std::fmt::Display| output_error(label, path, error);
        let filename = path.file_name().ok_or_else(|| {
            Diagnostic::new(BWErr::RunConfiguration(format!(
                "{label} output needs a filename"
            )))
        })?;
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent =
            botwork::core::paths::canonicalize(parent).map_err(|cause| error(&path, &cause))?;
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
            .map_err(|cause| error(&path, &cause))?;
        if !lock
            .metadata()
            .map_err(|cause| error(&path, &cause))?
            .is_file()
        {
            return Err(error(&path, &"lock must be an ordinary file"));
        }
        lock.try_lock().map_err(|cause| error(&path, &cause))?;
        remove_stale(&parent, filename).map_err(|cause| error(&path, &cause))?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(error(&path, &"output must be an ordinary file"))
            }
            Ok(_) => existing(&path)?,
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => {}
            Err(cause) => return Err(error(&path, &cause)),
        }
        Ok(Self {
            path,
            label,
            _lock: lock,
        })
    }

    /// The output's path in its canonical directory.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// The output's canonical directory.
    pub(super) fn directory(&self) -> &Path {
        self.path.parent().expect("canonical parent")
    }

    pub(super) fn error(&self, error: impl std::fmt::Display) -> CliError {
        output_error(self.label, &self.path, &error)
    }

    /// Publish `value` as one JSON line by fsync'd temporary file and rename.
    pub(super) fn write(&self, value: &impl Serialize) -> Result<(), CliError> {
        self.publish(|writer| {
            serde_json::to_writer(&mut *writer, value)?;
            writer.write_all(b"\n")
        })
    }

    /// Publish `bytes` by fsync'd temporary file and rename.
    pub(super) fn write_bytes(&self, bytes: &[u8]) -> Result<(), CliError> {
        self.publish(|writer| writer.write_all(bytes))
    }

    fn publish(
        &self,
        fill: impl FnOnce(&mut io::BufWriter<&mut File>) -> io::Result<()>,
    ) -> Result<(), CliError> {
        let parent = self.path.parent().expect("canonical parent");
        for _ in 0..64 {
            let temporary = parent.join(temporary_name(
                self.path.file_name().expect("output filename"),
                std::process::id(),
                TEMPORARY.fetch_add(1, Ordering::Relaxed),
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
                Err(error) => return Err(self.error(error)),
            };
            let mut fill = Some(fill);
            let result = (|| -> Result<(), CliError> {
                let mut writer = io::BufWriter::new(&mut file);
                (fill.take().expect("one publication"))(&mut writer)
                    .and_then(|_| writer.flush())
                    .map_err(|error| self.error(error))?;
                drop(writer);
                file.sync_all().map_err(|error| self.error(error))?;
                drop(file);
                fs::rename(&temporary, &self.path).map_err(|error| self.error(error))?;
                #[cfg(unix)]
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| self.error(error))?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            return result;
        }
        Err(self.error("temporary file collisions exhausted"))
    }
}

/// `.{output}.{pid}.{n}.tmp`: a temporary file that belongs to one output.
fn temporary_name(output: &OsStr, process: u32, sequence: u64) -> OsString {
    let mut name = OsString::from(".");
    name.push(output);
    name.push(format!(".{process}.{sequence}.tmp"));
    name
}

/// Remove this output's temporary files left by an interrupted publication.
/// Only the lock holder writes them, so any found under the lock are stale;
/// other outputs' files, symbolic links, and non-matching names are kept.
fn remove_stale(parent: &Path, output: &OsStr) -> io::Result<usize> {
    let mut prefix = OsString::from(".");
    prefix.push(output);
    prefix.push(".");
    let prefix = prefix.to_string_lossy().into_owned();
    let mut removed = 0;
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let stale = name
            .strip_prefix(prefix.as_str())
            .and_then(|rest| rest.strip_suffix(".tmp"))
            .and_then(|rest| rest.split_once('.'))
            .is_some_and(|(process, sequence)| {
                [process, sequence]
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            });
        if stale && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn output_error(label: &str, path: &Path, error: &dyn std::fmt::Display) -> CliError {
    Diagnostic::new(BWErr::OutputError(format!(
        "{label} {}: {error}",
        path.display()
    )))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_temporaries_of_this_output_are_removed_under_the_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let stale = root.join(temporary_name(OsStr::new("report.json"), 4321, 7));
        assert_eq!(stale.file_name().unwrap(), ".report.json.4321.7.tmp");
        fs::write(&stale, "interrupted").unwrap();
        let kept = [
            ".other.json.1.2.tmp",
            ".report.json.x.tmp",
            ".report.json.1.2.3.tmp",
            ".report.json.1..tmp",
            "report.json.1.2.tmp",
            ".12.34.tmp",
        ];
        for name in kept {
            fs::write(root.join(name), "keep").unwrap();
        }
        fs::create_dir(root.join(".report.json.8.9.tmp")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("report.json"), root.join(".report.json.5.6.tmp"))
            .unwrap();
        let file = AtomicFile::begin(root.join("report.json"), "Test output", |_| Ok(())).unwrap();
        assert!(!stale.exists(), "the interrupted temporary is removed");
        for name in kept {
            assert_eq!(
                fs::read_to_string(root.join(name)).unwrap(),
                "keep",
                "{name}"
            );
        }
        assert!(root.join(".report.json.8.9.tmp").is_dir());
        #[cfg(unix)]
        assert!(fs::symlink_metadata(root.join(".report.json.5.6.tmp")).is_ok());
        file.write_bytes(b"done").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("report.json")).unwrap(),
            "done"
        );
    }
}
