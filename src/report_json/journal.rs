//! The report journal: an append-only record of runs as they are selected,
//! start, and finish, written beside the report. A forced termination leaves it
//! behind with the incomplete marker; `--reconcile-report` folds it into a final
//! report in which started runs without an outcome are interrupted.
use super::{Fixture, Summary};
use crate::{BWErr, CliError, Diagnostic};
use botwork::core::report::{RunIdentity, RunRecord};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

pub(super) const FORMAT: &str = "botwork-report-journal";
pub(super) const VERSION: u32 = 1;
/// Journals hold bounded records, so a larger file is not one of ours.
const MAX_JOURNAL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Serialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub(super) enum Line<'a> {
    Header {
        format: &'static str,
        version: u32,
        mode: &'a str,
        started_at: &'a str,
        json: Option<&'a Path>,
        html: Option<&'a Path>,
    },
    Selected {
        count: usize,
    },
    Started {
        number: usize,
        identity: &'a RunIdentity,
        started_at: &'a str,
    },
    Run {
        number: usize,
        record: &'a RunRecord,
    },
    Summary {
        #[serde(flatten)]
        summary: &'a Summary,
    },
    Fixture {
        #[serde(flatten)]
        fixture: &'a Fixture,
    },
}

#[derive(Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub(super) enum Stored {
    Header {
        format: String,
        version: u32,
        mode: String,
        started_at: String,
        json: Option<PathBuf>,
        html: Option<PathBuf>,
    },
    Selected {
        count: usize,
    },
    Started {
        number: usize,
        identity: RunIdentity,
        started_at: String,
    },
    Run {
        number: usize,
        record: Box<RunRecord>,
    },
    Summary {
        #[serde(flatten)]
        summary: Summary,
    },
    Fixture {
        #[serde(flatten)]
        fixture: Fixture,
    },
}

/// The journal beside a report output.
pub(super) fn path_for(output: &Path) -> PathBuf {
    let mut path = output.as_os_str().to_owned();
    path.push(".journal");
    PathBuf::from(path)
}

fn failure(path: &Path, reason: impl std::fmt::Display) -> CliError {
    Diagnostic::new(BWErr::OutputError(format!(
        "Report journal {}: {reason}",
        path.display()
    )))
    .into()
}

struct State {
    file: Option<File>,
    error: Option<String>,
    /// A run started, so the journal is evidence until the report is final.
    started: bool,
    finished: bool,
}

pub(super) struct Journal {
    path: PathBuf,
    state: Mutex<State>,
}

impl Journal {
    /// Create the journal with its header. An existing journal belongs to an
    /// invocation that never finished its report, so it is never replaced.
    pub(super) fn create(path: PathBuf, header: &Line<'_>) -> Result<Self, CliError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                failure(
                    &path,
                    "an earlier invocation did not finish its report; run \
                     `botwork --reconcile-report` on that report first",
                )
            } else {
                failure(&path, error)
            }
        })?;
        let journal = Self {
            path,
            state: Mutex::new(State {
                file: Some(file),
                error: None,
                started: false,
                finished: false,
            }),
        };
        journal.append(header);
        journal.sync_created();
        if let Some(error) = journal.failure() {
            return Err(failure(&journal.path, error));
        }
        Ok(journal)
    }

    /// Make the new journal survive an operating-system crash: its header and,
    /// on Unix, its name in its directory.
    fn sync_created(&self) {
        let mut state = self.state();
        let Some(file) = state.file.as_ref() else {
            return;
        };
        let synced = sync(file, File::sync_all).and_then(|()| sync_directory(&self.path));
        if let Err(error) = synced {
            state.error = Some(error.to_string());
            state.file = None;
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Append one line with a single write. The first failure stops journaling.
    pub(super) fn append(&self, line: &Line<'_>) {
        let mut bytes = match serde_json::to_vec(line) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.state().error.get_or_insert(error.to_string());
                return;
            }
        };
        bytes.push(b'\n');
        let mut state = self.state();
        if matches!(line, Line::Started { .. }) {
            state.started = true;
        }
        let Some(file) = state.file.as_mut() else {
            return;
        };
        // A started line is synced before its run goes on, so after an
        // operating-system crash every run that began is still known to have.
        let written = file.write_all(&bytes).and_then(|()| {
            if matches!(line, Line::Started { .. }) {
                sync(file, File::sync_data)
            } else {
                Ok(())
            }
        });
        if let Err(error) = written {
            state.error = Some(error.to_string());
            state.file = None;
        }
    }

    pub(super) fn failure(&self) -> Option<String> {
        self.state().error.clone()
    }

    /// The report is final: remove the journal.
    pub(super) fn finish(&self) -> Result<(), CliError> {
        let mut state = self.state();
        state.finished = true;
        state.file = None;
        fs::remove_file(&self.path).map_err(|error| failure(&self.path, error))
    }
}

/// Sync the directory holding `path`, so its entry for the file survives an
/// operating-system crash. Windows has no such call; NTFS journals the entry.
fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(directory) = path.parent() {
        let directory = if directory.as_os_str().is_empty() {
            Path::new(".")
        } else {
            directory
        };
        // Some filesystems cannot sync a directory and say so with EINVAL;
        // the journal's own data is synced either way.
        return File::open(directory)
            .and_then(|directory| directory.sync_all())
            .or_else(|error| match error.raw_os_error() {
                Some(libc::EINVAL) => Ok(()),
                _ => Err(error),
            });
    }
    let _ = path;
    Ok(())
}

/// Sync `file` with `how`; tests can make it fail.
fn sync(file: &File, how: fn(&File) -> io::Result<()>) -> io::Result<()> {
    #[cfg(test)]
    if tests::FAIL_SYNC.with(std::cell::Cell::get) {
        return Err(io::Error::other("injected sync failure"));
    }
    how(file)
}

impl Drop for Journal {
    fn drop(&mut self) {
        // With no started run there is nothing to reconcile; the marker stays.
        let state = self.state();
        if !state.finished && !state.started {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// A journal's entries, and the byte count of a torn final line that a
/// termination cut short.
pub(super) struct Contents {
    pub(super) entries: Vec<Stored>,
    pub(super) torn: usize,
}

pub(super) fn read(path: &Path) -> Result<Contents, CliError> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the journal must be an ordinary file",
                ));
            }
            file.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes)
        })
        .map_err(|error| failure(path, error))?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(failure(path, "the journal exceeds 256 MiB"));
    }
    // Every complete line ends with a newline; a termination can cut the last.
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    let torn = bytes.len() - complete;
    let mut entries = Vec::new();
    for (number, line) in bytes[..complete]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        let parse =
            |error: serde_json::Error| failure(path, format_args!("line {}: {error}", number + 1));
        let entry: Stored = if number == 0 {
            // The header's version comes first, so a newer journal's reshaped
            // header is reported as its version.
            let value: serde_json::Value = serde_json::from_slice(line).map_err(parse)?;
            if value.get("format").and_then(serde_json::Value::as_str) == Some(FORMAT) {
                if let Some(version) = value
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|version| *version > u64::from(VERSION))
                {
                    return Err(failure(
                        path,
                        format_args!(
                            "the journal is version {version}, from a newer Botwork; this one reads version {VERSION}. Reconcile it with the Botwork that wrote it"
                        ),
                    ));
                }
            }
            serde_json::from_value(value).map_err(parse)?
        } else {
            serde_json::from_slice(line).map_err(parse)?
        };
        let header = matches!(entry, Stored::Header { .. });
        if header != (number == 0) {
            return Err(failure(
                path,
                "the journal must start with exactly one header",
            ));
        }
        if let Stored::Header {
            format, version, ..
        } = &entry
        {
            if format != FORMAT || *version != VERSION {
                return Err(failure(
                    path,
                    format_args!("unsupported journal {format} version {version}"),
                ));
            }
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(failure(path, "the journal has no header"));
    }
    Ok(Contents { entries, torn })
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        pub(super) static FAIL_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    #[test]
    fn a_started_line_that_cannot_be_synced_stops_journaling() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json.journal");
        let journal = Journal::create(path.clone(), &Line::Selected { count: 1 }).unwrap();
        let identity = RunIdentity::new("one", "one");
        let started = Line::Started {
            number: 1,
            identity: &identity,
            started_at: "2026-10-02T00:00:00Z",
        };
        // Other lines are not synced, so they go on while syncing fails.
        FAIL_SYNC.with(|fail| fail.set(true));
        journal.append(&Line::Selected { count: 2 });
        assert_eq!(journal.failure(), None);
        journal.append(&started);
        FAIL_SYNC.with(|fail| fail.set(false));
        assert_eq!(journal.failure().as_deref(), Some("injected sync failure"));
        assert!(journal.state().file.is_none(), "journaling stops");
        // A journal whose creation cannot be made durable is refused.
        FAIL_SYNC.with(|fail| fail.set(true));
        let refused = Journal::create(
            directory.path().join("other.json.journal"),
            &Line::Selected { count: 1 },
        );
        FAIL_SYNC.with(|fail| fail.set(false));
        assert!(refused.is_err());
    }

    #[test]
    fn write_failures_are_kept_and_stop_journaling() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json.journal");
        let journal = Journal::create(path.clone(), &Line::Selected { count: 1 }).unwrap();
        assert_eq!(journal.failure(), None);
        // A read-only handle makes the next append fail.
        journal.state().file = Some(File::open(&path).unwrap());
        journal.append(&Line::Selected { count: 2 });
        assert!(journal.failure().is_some());
        assert!(journal.state().file.is_none(), "journaling stops");
        journal.append(&Line::Selected { count: 3 });
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{\"entry\":\"selected\",\"count\":1}\n"
        );
    }
}
