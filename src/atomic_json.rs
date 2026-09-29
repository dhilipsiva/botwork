//! Exclusive, atomically replaced JSON outputs: a persistent lock sidecar, an
//! incomplete marker written before any effects, and fsync'd rename publication.
use super::{BWErr, CliError, Diagnostic};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) static TEMPORARY: AtomicU64 = AtomicU64::new(0);

pub(super) struct AtomicJson {
    path: PathBuf,
    /// Names the output in errors, such as `Failed-case record`.
    label: &'static str,
    /// Temporary files are `.{prefix}-{pid}-{n}.tmp` beside the output.
    prefix: &'static str,
    // The persistent sidecar inode is never replaced or removed by a writer.
    // Its advisory lock is released by the kernel when this handle closes.
    _lock: File,
}

impl AtomicJson {
    /// Lock `path` and accept an existing file only when `existing` recognizes it,
    /// so unrelated files are never overwritten.
    pub(super) fn begin(
        path: PathBuf,
        label: &'static str,
        prefix: &'static str,
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
        let parent = fs::canonicalize(parent).map_err(|cause| error(&path, &cause))?;
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
            prefix,
            _lock: lock,
        })
    }

    pub(super) fn error(&self, error: impl std::fmt::Display) -> CliError {
        output_error(self.label, &self.path, &error)
    }

    /// Publish `value` as one JSON line by fsync'd temporary file and rename.
    pub(super) fn write(&self, value: &impl Serialize) -> Result<(), CliError> {
        let parent = self.path.parent().expect("canonical parent");
        for _ in 0..64 {
            let temporary = parent.join(format!(
                ".{}-{}-{}.tmp",
                self.prefix,
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
                Err(error) => return Err(self.error(error)),
            };
            let result = (|| -> Result<(), CliError> {
                let mut writer = io::BufWriter::new(&mut file);
                serde_json::to_writer(&mut writer, value).map_err(|error| self.error(error))?;
                writer
                    .write_all(b"\n")
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

fn output_error(label: &str, path: &Path, error: &dyn std::fmt::Display) -> CliError {
    Diagnostic::new(BWErr::OutputError(format!(
        "{label} {}: {error}",
        path.display()
    )))
    .into()
}
