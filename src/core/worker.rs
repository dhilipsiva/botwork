//! Supervised external workers. Linux provides process-group termination;
//! other platforms reject entry explicitly. See docs/isolated-workers.md.
#![doc = include_str!("../../docs/isolated-workers.md")]

use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    num::NonZeroUsize,
    path::PathBuf,
    process::ExitStatus,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

use super::{
    diagnostic::{Diagnostic, DiagnosticResult},
    grammar::BWErr,
    operation::OperationControl,
};

#[cfg(any(target_os = "linux", windows))]
pub mod journal;
pub mod protocol;
#[cfg(any(unix, windows))]
mod supervisor;
#[cfg(all(test, unix))]
mod tests;
mod waiting;

/// Trusted host configuration; the supervisor never constructs a shell command.
/// Environment inheritance is disabled. Executable and cwd must be absolute.
#[derive(Clone, Debug)]
pub struct WorkerCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub directory: PathBuf,
    pub environment: BTreeMap<OsString, OsString>,
}

#[derive(Clone, Debug)]
pub struct WorkerLimits {
    pub max_in_flight: NonZeroUsize,
    pub request_bytes: usize,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub timeout: Duration,
    pub cleanup_timeout: Duration,
    /// Metadata only. Completed stdout/stderr belong to the receiving host.
    pub history_records: usize,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            max_in_flight: NonZeroUsize::new(4).unwrap(),
            request_bytes: 1024 * 1024,
            stdout_bytes: 1024 * 1024,
            stderr_bytes: 1024 * 1024,
            timeout: Duration::from_secs(30),
            cleanup_timeout: Duration::from_secs(1),
            history_records: 128,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerOutcome {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerCleanup {
    NotStarted,
    /// The direct child was reaped. Inherited-group termination was attempted.
    Reaped,
    /// Every descendant of the worker ended with it: on Linux the guardian
    /// reaped them and exited; on Windows the worker's Job Object emptied.
    TreeReaped,
    /// The kernel reaped a namespace init and all its processes. Worker status
    /// was not verified; this cleanup evidence can never establish success.
    NamespaceReaped,
    /// Cleanup exceeded its allowance; the supervisor still owns capacity.
    Pending,
    /// Child ownership was lost. Its slot is quarantined, never silently reused.
    Unverified,
}

#[derive(Debug)]
pub struct WorkerReport {
    pub id: u64,
    pub outcome: WorkerOutcome,
    pub cleanup: WorkerCleanup,
    pub exit_status: Option<ExitStatus>,
    pub stdin_written: usize,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Both output streams reached EOF and every request byte was written.
    pub io_complete: bool,
    /// False for early/uncertain reports: an in-flight OS call may still make progress.
    pub progress_complete: bool,
    pub diagnostic: Option<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct WorkerRecord {
    pub id: u64,
    pub outcome: WorkerOutcome,
    pub cleanup: WorkerCleanup,
    pub exit_status: Option<ExitStatus>,
}

#[derive(Clone, Debug)]
pub struct ActiveWorker {
    pub id: u64,
    /// None until process creation returns; this does not prove that no process exists.
    pub pid: Option<u32>,
    pub stopping: bool,
    /// Pending or unverified cleanup survives completed-history eviction.
    pub cleanup: Option<WorkerCleanup>,
}

#[derive(Clone, Debug)]
pub struct WorkerSnapshot {
    pub closed: bool,
    pub active: Vec<ActiveWorker>,
    pub completed: Vec<WorkerRecord>,
    pub omitted_records: u64,
}

struct Request {
    control: OperationControl,
    abandoned: AtomicBool,
    // Read by the Linux supervisor; other platforms refuse entry before it.
    #[cfg_attr(not(any(unix, windows)), allow(dead_code))]
    limits: WorkerLimits,
    #[cfg(any(target_os = "linux", windows))]
    journal: Option<Arc<journal::Ticket>>,
}

struct Active {
    _retention: Option<Arc<dyn Send + Sync>>,
    request: Arc<Request>,
    pid: Option<u32>,
    stopping: bool,
    cleanup: Option<WorkerCleanup>,
}

struct State {
    closed: bool,
    next_id: u64,
    active: BTreeMap<u64, Active>,
    completed: VecDeque<WorkerRecord>,
    omitted_records: u64,
}

struct Shared {
    changed: Condvar,
    #[cfg(all(test, any(unix, windows)))]
    launcher: Mutex<Option<supervisor::LaunchHook>>,
    #[cfg(all(test, any(unix, windows)))]
    io_hooks: Mutex<VecDeque<supervisor::IoHook>>,
    limits: WorkerLimits,
    #[cfg(target_os = "linux")]
    guardian: Option<PathBuf>,
    /// Whether the pool owns each worker's whole process tree.
    #[cfg(windows)]
    tree: bool,
    #[cfg(any(target_os = "linux", windows))]
    journal: Option<journal::WorkerJournal>,
    #[cfg(target_os = "linux")]
    namespaced: bool,
    state: Mutex<State>,
}

struct Owner(Arc<Shared>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Clones share capacity, shutdown, and bounded reconciliation records.
#[derive(Clone)]
pub struct WorkerPool(Arc<Owner>);

impl WorkerPool {
    pub fn new(limits: WorkerLimits) -> DiagnosticResult<Self> {
        Self::configured(limits, None)
    }

    /// Supervise detached descendants as well as the worker's own group. On
    /// Linux a dedicated Botwork guardian executable adopts them; the absolute
    /// path must name a matching Botwork CLI build. On Windows the worker's Job
    /// Object already holds every descendant, so the path is checked but unused.
    /// macOS cannot follow a process that leaves the group, so it refuses this
    /// mode rather than offer a weaker one (decision D12).
    pub fn with_process_tree(limits: WorkerLimits, guardian: PathBuf) -> DiagnosticResult<Self> {
        if cfg!(target_os = "macos") {
            return Err(configuration(
                "Process-tree workers are unavailable on macOS, which cannot follow a process that leaves the worker's group",
            ));
        }
        if !cfg!(any(target_os = "linux", windows)) {
            return Err(configuration(
                "Process-tree workers are unavailable on this platform",
            ));
        }
        if !guardian.is_absolute() {
            return Err(configuration(
                "Worker guardians require an absolute executable path",
            ));
        }
        Self::configured(limits, Some(guardian))
    }

    /// Add durable invocation metadata to a process-tree pool. Journal IDs are
    /// available on admitted handles. Await a report then flush the journal to
    /// acknowledge persistence; ordinary report delivery never waits for disk.
    /// On Linux the guardian writes each tree receipt, even after the host is
    /// lost. On Windows the pool writes it once the worker's Job Object is
    /// empty; losing the host closes the job, which ends the tree, so no
    /// receipt follows then.
    #[cfg(any(target_os = "linux", windows))]
    pub fn with_recovery(
        limits: WorkerLimits,
        guardian: PathBuf,
        journal: journal::WorkerJournal,
    ) -> DiagnosticResult<Self> {
        let mut pool = Self::with_process_tree(limits, guardian)?;
        let owner = Arc::get_mut(&mut pool.0).expect("new pool is unique");
        Arc::get_mut(&mut owner.0)
            .expect("new state is unique")
            .journal = Some(journal);
        Ok(pool)
    }

    /// Contain guardian failure with a fresh Linux user/PID/mount namespace.
    /// Requires clone3, unprivileged namespaces, and a private proc mount.
    /// There is no fallback. The optional journal persists transport evidence.
    #[cfg(target_os = "linux")]
    pub fn with_pid_namespace(
        limits: WorkerLimits,
        guardian: PathBuf,
        journal: Option<journal::WorkerJournal>,
    ) -> DiagnosticResult<Self> {
        let mut pool = Self::with_process_tree(limits, guardian)?;
        let owner = Arc::get_mut(&mut pool.0).expect("new pool is unique");
        let shared = Arc::get_mut(&mut owner.0).expect("new state is unique");
        shared.journal = journal;
        shared.namespaced = true;
        Ok(pool)
    }

    fn configured(limits: WorkerLimits, _guardian: Option<PathBuf>) -> DiagnosticResult<Self> {
        let now = Instant::now();
        if now
            .checked_add(limits.timeout)
            .and_then(|end| end.checked_add(limits.cleanup_timeout))
            .is_none()
        {
            return Err(configuration(
                "Worker timeouts exceed the monotonic clock range",
            ));
        }
        Ok(Self(Arc::new(Owner(Arc::new(Shared {
            changed: Condvar::new(),
            #[cfg(all(test, any(unix, windows)))]
            launcher: Mutex::new(None),
            #[cfg(all(test, any(unix, windows)))]
            io_hooks: Mutex::new(VecDeque::new()),
            limits,
            #[cfg(windows)]
            tree: _guardian.is_some(),
            #[cfg(target_os = "linux")]
            guardian: _guardian,
            #[cfg(any(target_os = "linux", windows))]
            journal: None,
            #[cfg(target_os = "linux")]
            namespaced: false,
            state: Mutex::new(State {
                closed: false,
                next_id: 1,
                active: BTreeMap::new(),
                completed: VecDeque::new(),
                omitted_records: 0,
            }),
        })))))
    }

    pub fn limits(&self) -> &WorkerLimits {
        &self.0 .0.limits
    }

    /// Admit immediately or fail; no unbounded queue of pending requests exists.
    /// Starts supervision even if the returned handle is never awaited.
    pub fn start(
        &self,
        command: WorkerCommand,
        input: Vec<u8>,
        control: OperationControl,
    ) -> DiagnosticResult<WorkerHandle> {
        self.start_retained(command, input, control, None)
    }

    pub(crate) fn start_retained(
        &self,
        command: WorkerCommand,
        input: Vec<u8>,
        control: OperationControl,
        retention: Option<Arc<dyn Send + Sync>>,
    ) -> DiagnosticResult<WorkerHandle> {
        self.start_configured_retained(
            command,
            input,
            control,
            retention,
            self.limits().clone(),
            false,
        )
    }

    // DSL processes share pool capacity/history, while pipe and time limits are
    // local to each invocation. Public worker entry retains absolute-path rules.
    pub(crate) fn start_configured_retained(
        &self,
        command: WorkerCommand,
        input: Vec<u8>,
        control: OperationControl,
        retention: Option<Arc<dyn Send + Sync>>,
        limits: WorkerLimits,
        search_path: bool,
    ) -> DiagnosticResult<WorkerHandle> {
        let input = RetainedInput {
            bytes: input,
            _retention: retention.clone(),
        };
        control.checkpoint()?;
        if !cfg!(any(unix, windows)) {
            return Err(configuration(
                "Isolated workers are unavailable on this platform",
            ));
        }
        let bare_name = command.executable.components().count() == 1
            && matches!(
                command.executable.components().next(),
                Some(std::path::Component::Normal(_))
            );
        if (!command.executable.is_absolute() && !(search_path && bare_name))
            || !command.directory.is_absolute()
        {
            return Err(configuration(
                "Worker executable and directory must be absolute",
            ));
        }
        let shared = &self.0 .0;
        if input.len() > limits.request_bytes {
            return Err(limit("worker request bytes", limits.request_bytes));
        }
        let deadline = Instant::now()
            .checked_add(limits.timeout)
            .ok_or_else(|| configuration("Worker timeout exceeds the monotonic clock range"))?;
        deadline
            .checked_add(limits.cleanup_timeout)
            .ok_or_else(|| {
                configuration("Worker cleanup timeout exceeds the monotonic clock range")
            })?;
        let (id, request) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(configuration("Worker pool is shut down"));
            }
            if state.active.len() >= shared.limits.max_in_flight.get() {
                return Err(limit("isolated workers", shared.limits.max_in_flight.get()));
            }
            let id = state.next_id;
            state.next_id = id
                .checked_add(1)
                .ok_or_else(|| configuration("Worker identifier space exhausted"))?;
            let request = Arc::new(Request {
                control: control.child(None),
                abandoned: AtomicBool::new(false),
                limits,
                #[cfg(any(target_os = "linux", windows))]
                journal: shared
                    .journal
                    .as_ref()
                    .map(journal::WorkerJournal::reserve)
                    .transpose()?,
            });
            state.active.insert(
                id,
                Active {
                    _retention: retention.clone(),
                    request: request.clone(),
                    pid: None,
                    stopping: false,
                    cleanup: None,
                },
            );
            (id, request)
        };
        let (send, receive) = oneshot::channel();
        let send = WorkerDelivery {
            send: Some(send),
            retention,
        };
        let thread_shared = Arc::clone(shared);
        let thread_request = Arc::clone(&request);
        let spawn = std::thread::Builder::new()
            .name(format!("botwork-worker-{id}"))
            .spawn(move || {
                #[cfg(any(unix, windows))]
                supervisor::supervise(
                    id,
                    command,
                    input,
                    thread_request,
                    deadline,
                    thread_shared,
                    send,
                );
                #[cfg(not(any(unix, windows)))]
                let _ = (
                    id,
                    command,
                    input,
                    thread_request,
                    deadline,
                    thread_shared,
                    send,
                );
            });
        if let Err(error) = spawn {
            #[cfg(any(target_os = "linux", windows))]
            if let Some(ticket) = &request.journal {
                let metadata = journal::JournalMetadata {
                    outcome: WorkerOutcome::Failed,
                    cleanup: WorkerCleanup::NotStarted,
                    exit_status: None,
                    io_complete: false,
                    progress_complete: true,
                };
                ticket.submit(Some(metadata), Some(metadata));
            }
            shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .active
                .remove(&id);
            shared.changed.notify_all();
            return Err(runtime(format_args!(
                "Starting worker supervisor failed: {error}"
            )));
        }
        Ok(WorkerHandle {
            id,
            request,
            receive: Some(receive),
        })
    }

    /// Stop accepting work and request cancellation of every owned child.
    /// The returned snapshot exposes pending cleanup; call snapshot() to reconcile.
    pub fn shutdown(&self) -> WorkerSnapshot {
        self.0 .0.close();
        self.snapshot()
    }

    /// Close admission and block for at most the given monotonic allowance.
    /// A nonempty active list explicitly means cleanup is still unresolved.
    /// This observes supervisors without consuming their per-worker reports.
    pub fn shutdown_wait(&self, timeout: Duration) -> DiagnosticResult<WorkerSnapshot> {
        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            configuration("Worker shutdown timeout exceeds the monotonic clock range")
        })?;
        let shared = &self.0 .0;
        shared.close();
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while !state.active.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let waited = shared
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner());
            state = waited.0;
        }
        Ok(state.snapshot())
    }

    pub fn snapshot(&self) -> WorkerSnapshot {
        self.0
             .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot()
    }
}

impl State {
    fn snapshot(&self) -> WorkerSnapshot {
        WorkerSnapshot {
            closed: self.closed,
            active: self
                .active
                .iter()
                .map(|(&id, active)| ActiveWorker {
                    id,
                    pid: active.pid,
                    stopping: active.stopping,
                    cleanup: active.cleanup,
                })
                .collect(),
            completed: self.completed.iter().cloned().collect(),
            omitted_records: self.omitted_records,
        }
    }
}

// Payload fields drop before reservations. Supervisors and quarantined active
// slots hold independent clones until process ownership is settled.
pub(crate) struct RetainedReport {
    pub report: WorkerReport,
    pub _retention: Option<Arc<dyn Send + Sync>>,
}
struct RetainedInput {
    bytes: Vec<u8>,
    _retention: Option<Arc<dyn Send + Sync>>,
}
impl std::ops::Deref for RetainedInput {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
#[cfg_attr(not(any(unix, windows)), allow(dead_code))]
struct WorkerDelivery {
    send: Option<oneshot::Sender<RetainedReport>>,
    retention: Option<Arc<dyn Send + Sync>>,
}

pub struct WorkerHandle {
    id: u64,
    request: Arc<Request>,
    receive: Option<oneshot::Receiver<RetainedReport>>,
}

impl WorkerHandle {
    pub fn id(&self) -> u64 {
        self.id
    }
    #[cfg(any(target_os = "linux", windows))]
    pub fn journal_id(&self) -> Option<journal::JournalId> {
        self.request.journal.as_ref().map(|ticket| ticket.id)
    }
    pub fn cancel(&self) {
        self.request.control.cancel();
    }

    /// Await exactly one terminal report. Dropping this future interrupts the worker.
    /// No Tokio runtime is needed by the supervisor or the receiver itself.
    pub async fn wait(self) -> WorkerReport {
        self.wait_retained().await.report
    }
    pub(crate) async fn wait_retained(mut self) -> RetainedReport {
        let result = self.receive.take().expect("one receiver").await;
        match result {
            Ok(report) => report,
            Err(_) => RetainedReport {
                report: WorkerReport::failure(
                    self.id,
                    WorkerOutcome::Interrupted,
                    WorkerCleanup::Unverified,
                    runtime(format_args!(
                        "Worker supervisor ended without a terminal report"
                    )),
                ),
                _retention: None,
            },
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.request.abandoned.store(true, Ordering::Release);
        self.request.control.cancel();
    }
}

impl Shared {
    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        for active in state.active.values_mut() {
            active.stopping = true;
            active.request.control.cancel();
        }
        self.changed.notify_all();
    }
    #[cfg(any(unix, windows))]
    fn update(&self, id: u64, pid: Option<u32>, stopping: bool, cleanup: Option<WorkerCleanup>) {
        if let Some(active) = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .get_mut(&id)
        {
            active.pid = pid;
            active.stopping = stopping;
            active.cleanup = cleanup;
        }
    }
    #[cfg(any(unix, windows))]
    fn finish(&self, report: &WorkerReport) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if report.cleanup != WorkerCleanup::Unverified {
            state.active.remove(&report.id);
        } else if let Some(active) = state.active.get_mut(&report.id) {
            active.stopping = true;
            active.cleanup = Some(WorkerCleanup::Unverified);
        }
        self.changed.notify_all();
        if self.limits.history_records == 0 {
            state.omitted_records = state.omitted_records.saturating_add(1);
            return;
        }
        if state.completed.len() == self.limits.history_records {
            state.completed.pop_front();
            state.omitted_records = state.omitted_records.saturating_add(1);
        }
        state.completed.push_back(WorkerRecord {
            id: report.id,
            outcome: report.outcome,
            cleanup: report.cleanup,
            exit_status: report.exit_status,
        });
    }
}

impl WorkerReport {
    fn failure(
        id: u64,
        outcome: WorkerOutcome,
        cleanup: WorkerCleanup,
        diagnostic: Diagnostic,
    ) -> Self {
        Self {
            id,
            outcome,
            cleanup,
            exit_status: None,
            stdin_written: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
            io_complete: false,
            progress_complete: cleanup == WorkerCleanup::NotStarted,
            diagnostic: Some(diagnostic),
        }
    }
}

fn configuration(message: &str) -> Diagnostic {
    Diagnostic::formatted(BWErr::RunConfiguration, format_args!("{message}"))
}
fn runtime(message: std::fmt::Arguments<'_>) -> Diagnostic {
    Diagnostic::formatted(BWErr::AsyncRuntime, message)
}
fn limit(resource: &'static str, limit: usize) -> Diagnostic {
    Diagnostic::new(BWErr::ResourceLimit {
        resource,
        limit: limit as u64,
    })
}

/// Internal CLI dispatch for the process-tree guardian protocol.
#[doc(hidden)]
pub fn guardian_main() -> Option<u8> {
    #[cfg(target_os = "linux")]
    {
        supervisor::unix::guardian::entry()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
