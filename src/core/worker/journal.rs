//! Optional Linux worker metadata journal. No payloads, commands, or PIDs persist.
use super::{WorkerCleanup, WorkerOutcome, WorkerReport};
use std::{
    fmt, io,
    num::NonZeroUsize,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

mod format;
mod storage;
use format::{Frame, Role};
use storage::Directory;

/// Identity is unique across pools sharing a journal and across reopened sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct JournalId {
    pub session: [u8; 16],
    pub sequence: u64,
}
impl fmt::Display for JournalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.session {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "-{:016x}", self.sequence)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalMetadata {
    pub outcome: WorkerOutcome,
    pub cleanup: WorkerCleanup,
    pub exit_status: Option<i32>,
    pub io_complete: bool,
    pub progress_complete: bool,
}
impl From<&WorkerReport> for JournalMetadata {
    fn from(report: &WorkerReport) -> Self {
        use std::os::unix::process::ExitStatusExt;
        Self {
            outcome: report.outcome,
            cleanup: report.cleanup,
            exit_status: report.exit_status.map(|status| status.into_raw()),
            io_complete: report.io_complete,
            progress_complete: report.progress_complete,
        }
    }
}

/// A guardian receipt does not establish guardian exit or operation success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardianReceipt {
    NotStarted { errno: i32 },
    TreeSettled { exit_status: i32, errno: i32 },
}

#[derive(Clone, Debug)]
pub struct RecoveredWorker {
    pub id: JournalId,
    pub published: Option<JournalMetadata>,
    pub reconciled: Option<JournalMetadata>,
    pub guardian: Option<GuardianReceipt>,
    /// Invalid, truncated, conflicting, or unrecognized bytes were encountered.
    pub damaged: bool,
    /// None means this session has no durable publication yet. Past unfinished
    /// sessions are Interrupted. This is transport evidence, not typed/run success.
    pub outcome: Option<WorkerOutcome>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalFlush {
    /// Accepted metadata writes that have not finished syncing.
    pub pending: usize,
    /// Failed writes since opening this handle; zero pending alone is not success.
    pub failed: usize,
}

struct Store {
    directory: Arc<Directory>,
    session: [u8; 16],
    admission: Mutex<(u64, usize)>,
    maximum: usize,
    sender: mpsc::SyncSender<Job>,
}

/// One exclusive writer per private directory. Opening/reading may block on disk;
/// worker observation and flush_wait's deadline never perform filesystem calls.
#[derive(Clone)]
pub struct WorkerJournal(Arc<Store>);

impl WorkerJournal {
    pub fn open(path: &Path, maximum: NonZeroUsize) -> io::Result<Self> {
        let queue = maximum
            .get()
            .checked_mul(2)
            .ok_or_else(|| invalid("Journal limit is too large"))?;
        let directory = Arc::new(Directory::open(path)?);
        let ids = directory.ids(maximum.get())?;
        let mut session = [0; 16];
        let mut unique = false;
        for _ in 0..16 {
            storage::random(&mut session)?;
            if !ids.iter().any(|id| id.session == session) {
                unique = true;
                break;
            }
        }
        if !unique {
            return Err(invalid("Journal session collision"));
        }
        let (sender, receiver) = mpsc::sync_channel::<Job>(queue);
        let owned_directory = directory.clone();
        std::thread::Builder::new()
            .name("botwork-journal".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    #[cfg(test)]
                    Directory::hook(&owned_directory.write_hook);
                    let result = job.ticket.file().and_then(|file| {
                        if let Some(metadata) = job.published {
                            format::write(
                                &file,
                                Frame::host(job.ticket.id, Role::Published, metadata),
                            )?;
                        }
                        if let Some(metadata) = job.reconciled {
                            format::write(
                                &file,
                                Frame::host(job.ticket.id, Role::Reconciled, metadata),
                            )?;
                        }
                        file.sync_data()
                    });
                    if job.reconciled.is_some() {
                        job.ticket.close_file();
                    }
                    owned_directory.completed(result.is_err());
                }
            })?;
        Ok(Self(Arc::new(Store {
            directory,
            session,
            admission: Mutex::new((1, ids.len())),
            maximum: maximum.get(),
            sender,
        })))
    }

    /// Reads at most the configured number of fixed-size records. A concurrent
    /// writer can yield a conservative damaged snapshot; rescan after flush/cleanup.
    pub fn records(&self) -> io::Result<Vec<RecoveredWorker>> {
        self.0
            .directory
            .ids(self.0.maximum)?
            .into_iter()
            .map(|id| self.0.directory.read(id, self.0.session))
            .collect()
    }

    /// Waits only for already accepted host metadata writes, not worker completion.
    /// Call after awaiting reports; a successful flush acknowledges their fsyncs.
    pub fn flush_wait(&self, timeout: Duration) -> io::Result<JournalFlush> {
        let end = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| invalid("Journal timeout exceeds clock range"))?;
        let directory = &self.0.directory;
        let mut state = directory.progress.lock().unwrap_or_else(|e| e.into_inner());
        while state.pending != 0 {
            let remaining = end.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            state = directory
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        Ok(*state)
    }

    pub(super) fn reserve(&self) -> super::DiagnosticResult<Arc<Ticket>> {
        let mut admission = self.0.admission.lock().unwrap_or_else(|e| e.into_inner());
        if admission.1 == self.0.maximum {
            return Err(super::limit("worker journal records", self.0.maximum));
        }
        let sequence = admission.0;
        admission.0 = sequence
            .checked_add(1)
            .ok_or_else(|| super::configuration("Journal identifiers exhausted"))?;
        admission.1 += 1;
        Ok(Arc::new(Ticket {
            store: self.0.clone(),
            id: JournalId {
                session: self.0.session,
                sequence,
            },
            file: OnceLock::new(),
            published: AtomicBool::new(false),
            reconciled: AtomicBool::new(false),
        }))
    }
}

type RecordFile = Mutex<Option<Arc<std::fs::File>>>;
pub(super) struct Ticket {
    store: Arc<Store>,
    pub id: JournalId,
    file: OnceLock<Result<RecordFile, String>>,
    published: AtomicBool,
    reconciled: AtomicBool,
}
impl Ticket {
    /// Only OS owner/writer threads call this. Intent and its directory entry are
    /// durable before this returns; the observer does not acquire the OnceLock.
    pub fn file(&self) -> io::Result<Arc<std::fs::File>> {
        self.file
            .get_or_init(|| {
                self.store
                    .directory
                    .create(self.id)
                    .map(|file| Mutex::new(Some(Arc::new(file))))
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|error| io::Error::other(error.clone()))?
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| invalid("Journal record is already reconciled"))
    }
    fn close_file(&self) {
        if let Some(Ok(file)) = self.file.get() {
            let owned = file.lock().unwrap_or_else(|e| e.into_inner()).take();
            // Final metadata is queued after OS ownership ends. Close on the
            // journal writer, outside locks, before acknowledging its flush.
            drop(owned);
        }
    }
    pub fn submit(
        self: &Arc<Self>,
        published: Option<JournalMetadata>,
        reconciled: Option<JournalMetadata>,
    ) {
        let published = published.filter(|_| !self.published.swap(true, Ordering::AcqRel));
        let reconciled = reconciled.filter(|_| !self.reconciled.swap(true, Ordering::AcqRel));
        if published.is_none() && reconciled.is_none() {
            return;
        }
        self.store
            .directory
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending += 1;
        // At most two jobs per reserved record, and the queue has twice the record
        // capacity. Never block the observer, including if the writer has failed.
        if self
            .store
            .sender
            .try_send(Job {
                ticket: self.clone(),
                published,
                reconciled,
            })
            .is_err()
        {
            self.store.directory.completed(true);
        }
    }
}
struct Job {
    ticket: Arc<Ticket>,
    published: Option<JournalMetadata>,
    reconciled: Option<JournalMetadata>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Guardian owns only this invocation's descriptor, never the journal lock.
pub(super) fn guardian_file(file: &std::fs::File) -> io::Result<JournalId> {
    storage::validate(file, Some(format::FILE_BYTES as u64))?;
    let intent =
        format::read_frame(file, Role::Intent)?.ok_or_else(|| invalid("Missing journal intent"))?;
    Ok(intent.id)
}
pub(super) fn receipt(
    file: &std::fs::File,
    id: JournalId,
    receipt: GuardianReceipt,
) -> io::Result<()> {
    format::write(
        file,
        Frame {
            id,
            role: Role::Guardian,
            host: None,
            guardian: Some(receipt),
        },
    )?;
    file.sync_data()
}

#[cfg(test)]
mod tests;
