use super::super::{OperationControl, WorkerCommand, WorkerLimits, WorkerPool};
use super::*;
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::{fs, path::PathBuf, sync::atomic::AtomicU64};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "botwork-journal-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("journal")
    }
    fn journal(&self, maximum: usize) -> WorkerJournal {
        WorkerJournal::open(&self.path(), NonZeroUsize::new(maximum).unwrap()).unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn settled(journal: &WorkerJournal) {
    assert_eq!(
        journal.flush_wait(Duration::from_secs(5)).unwrap(),
        JournalFlush::default()
    );
}
fn stopped() -> JournalMetadata {
    JournalMetadata {
        outcome: WorkerOutcome::Cancelled,
        cleanup: WorkerCleanup::Pending,
        exit_status: None,
        io_complete: false,
        progress_complete: false,
    }
}
/// An absolute guardian path, which these tests never run.
fn guardian() -> PathBuf {
    if cfg!(windows) {
        r"C:\missing-guardian".into()
    } else {
        "/missing-guardian".into()
    }
}
/// A worker that cannot start: on Linux its guardian is missing, and on
/// Windows, where the pool runs no guardian, the executable is.
fn unstartable() -> PathBuf {
    if cfg!(windows) {
        r"C:\missing-worker.exe".into()
    } else {
        "/bin/true".into()
    }
}
/// Receipt that the worker's tree settled with status 0, written directly as
/// Linux's guardian writes it.
fn tree_settled(file: &std::fs::File, id: JournalId) {
    format::write(
        file,
        Frame {
            id,
            role: Role::Guardian,
            host: None,
            guardian: Some(GuardianReceipt::TreeSettled {
                exit_status: 0,
                errno: 0,
            }),
        },
    )
    .unwrap();
    file.sync_data().unwrap();
}
fn success() -> JournalMetadata {
    JournalMetadata {
        outcome: WorkerOutcome::Succeeded,
        cleanup: WorkerCleanup::TreeReaped,
        exit_status: Some(0),
        io_complete: true,
        progress_complete: true,
    }
}

#[test]
fn exclusive_lock_quota_and_ids_survive_reopening() {
    let workspace = Workspace::new();
    let journal = workspace.journal(2);
    assert!(WorkerJournal::open(&workspace.path(), NonZeroUsize::new(2).unwrap()).is_err());
    let one = journal.reserve().unwrap();
    let two = journal.clone().reserve().unwrap();
    assert_ne!(one.id, two.id);
    assert!(journal.reserve().is_err());
    one.file().unwrap();
    two.file().unwrap();
    assert_eq!(journal.records().unwrap().len(), 2);
    assert!(journal
        .records()
        .unwrap()
        .iter()
        .all(|record| record.outcome.is_none()));
    let session = one.id.session;
    drop(one);
    drop(two);
    drop(journal);
    let mut reopened = None;
    until(|| {
        reopened = WorkerJournal::open(&workspace.path(), NonZeroUsize::new(3).unwrap()).ok();
        reopened.is_some()
    });
    let journal = reopened.unwrap();
    assert!(journal
        .records()
        .unwrap()
        .iter()
        .all(|record| record.outcome == Some(WorkerOutcome::Interrupted)));
    assert_ne!(journal.reserve().unwrap().id.session, session);
    assert!(journal.reserve().is_err());
}

#[test]
fn publication_and_cleanup_remain_distinct_under_damage_and_late_reconciliation() {
    let workspace = Workspace::new();
    let journal = workspace.journal(8);
    let ticket = journal.reserve().unwrap();
    let file = ticket.file().unwrap();
    tree_settled(&file, ticket.id);
    assert_eq!(journal.records().unwrap()[0].outcome, None);
    ticket.submit(Some(stopped()), None);
    settled(&journal);
    let mut late = stopped();
    late.cleanup = WorkerCleanup::TreeReaped;
    late.progress_complete = true;
    late.exit_status = Some(0);
    ticket.submit(None, Some(late));
    settled(&journal);
    let record = journal.records().unwrap().remove(0);
    assert!(!record.damaged);
    assert_eq!(record.outcome, Some(WorkerOutcome::Cancelled));
    assert_eq!(record.published.unwrap().cleanup, WorkerCleanup::Pending);
    assert_eq!(
        record.reconciled.unwrap().cleanup,
        WorkerCleanup::TreeReaped
    );

    let passed = journal.reserve().unwrap();
    let file = passed.file().unwrap();
    passed.submit(Some(success()), Some(success()));
    settled(&journal);
    assert_eq!(
        journal.records().unwrap()[1].outcome,
        Some(WorkerOutcome::Interrupted)
    );
    tree_settled(&file, passed.id);
    assert_eq!(
        journal.records().unwrap()[1].outcome,
        Some(WorkerOutcome::Succeeded)
    );
    storage::write_all_at(&file, &[42], 200).unwrap();
    file.sync_data().unwrap();
    let record = journal.records().unwrap().remove(1);
    assert!(record.damaged);
    assert_eq!(record.outcome, Some(WorkerOutcome::Interrupted));

    let conflicting = journal.reserve().unwrap();
    let file = conflicting.file().unwrap();
    let mut changed = stopped();
    changed.outcome = WorkerOutcome::TimedOut;
    conflicting.submit(Some(stopped()), Some(changed));
    settled(&journal);
    let record = journal.records().unwrap().remove(2);
    assert!(record.damaged);
    assert_eq!(record.outcome, Some(WorkerOutcome::Cancelled));
    file.set_len(70).unwrap();
    assert_eq!(
        journal.records().unwrap()[2].outcome,
        Some(WorkerOutcome::Interrupted)
    );
}

#[cfg(unix)]
#[test]
fn private_regular_files_and_canonical_names_are_required() {
    let workspace = Workspace::new();
    fs::create_dir(workspace.path()).unwrap();
    fs::set_permissions(workspace.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(WorkerJournal::open(&workspace.path(), NonZeroUsize::new(1).unwrap()).is_err());
    fs::set_permissions(workspace.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let target = workspace.0.join("target");
    fs::write(&target, b"unchanged").unwrap();
    symlink(&target, workspace.path().join(".lock")).unwrap();
    assert!(WorkerJournal::open(&workspace.path(), NonZeroUsize::new(1).unwrap()).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"unchanged");
    fs::remove_file(workspace.path().join(".lock")).unwrap();
    let journal = workspace.journal(1);
    let ticket = journal.reserve().unwrap();
    symlink(&target, workspace.path().join(format!("{}.bwk", ticket.id))).unwrap();
    assert!(ticket.file().is_err());
    assert!(journal.records().is_err());
    ticket.submit(Some(stopped()), None);
    let state = journal.flush_wait(Duration::from_secs(5)).unwrap();
    assert_eq!(
        state,
        JournalFlush {
            pending: 0,
            failed: 1
        }
    );
    assert_eq!(fs::read(&target).unwrap(), b"unchanged");
}

struct Gate {
    entered: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
}
impl Gate {
    fn new(hook: &Mutex<Option<Box<dyn FnOnce() + Send>>>) -> Self {
        let (ready, entered) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        *hook.lock().unwrap() = Some(Box::new(move || {
            ready.send(()).unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(5));
        }));
        Self {
            entered,
            release: Some(release),
        }
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        self.release();
    }
}

#[test]
fn blocked_intent_never_enters_worker_and_observer_preserves_capacity() {
    let workspace = Workspace::new();
    let journal = workspace.journal(2);
    let mut gate = Gate::new(&journal.0.directory.intent_hook);
    let pool = WorkerPool::with_recovery(
        WorkerLimits {
            max_in_flight: NonZeroUsize::new(1).unwrap(),
            cleanup_timeout: Duration::from_millis(20),
            ..Default::default()
        },
        guardian(),
        journal.clone(),
    )
    .unwrap();
    let command = WorkerCommand {
        executable: unstartable(),
        arguments: vec![],
        directory: workspace.0.clone(),
        environment: Default::default(),
    };
    let control = OperationControl::default();
    let marker = Arc::new(());
    let weak = Arc::downgrade(&marker);
    let handle = pool
        .start_retained(command.clone(), vec![], control.clone(), Some(marker))
        .unwrap();
    gate.entered.recv_timeout(Duration::from_secs(2)).unwrap();
    control.cancel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let report = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), handle.wait_retained())
            .await
            .unwrap()
    });
    assert_eq!(report.report.cleanup, WorkerCleanup::Pending);
    assert_eq!(report.report.outcome, WorkerOutcome::Cancelled);
    drop(report);
    assert!(weak.upgrade().is_some());
    assert!(pool
        .start(command, vec![], OperationControl::default())
        .is_err());
    assert_eq!(
        journal
            .flush_wait(Duration::from_millis(10))
            .unwrap()
            .pending,
        1
    );
    assert!(pool.shutdown_wait(Duration::ZERO).unwrap().active[0]
        .pid
        .is_none());
    gate.release();
    until(|| pool.snapshot().active.is_empty() && weak.upgrade().is_none());
    settled(&journal);
    let record = journal.records().unwrap().remove(0);
    assert_eq!(record.outcome, Some(WorkerOutcome::Cancelled));
    assert_eq!(
        record.reconciled.unwrap().cleanup,
        WorkerCleanup::NotStarted
    );
    assert!(record.guardian.is_none());
}

#[test]
fn stalled_writer_keeps_exclusive_lock_after_handles_drop_and_flush_is_bounded() {
    let workspace = Workspace::new();
    let journal = workspace.journal(1);
    let mut gate = Gate::new(&journal.0.directory.write_hook);
    let ticket = journal.reserve().unwrap();
    ticket.file().unwrap();
    ticket.submit(Some(stopped()), None);
    gate.entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let started = Instant::now();
    assert_eq!(
        journal
            .flush_wait(Duration::from_millis(20))
            .unwrap()
            .pending,
        1
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(journal.flush_wait(Duration::MAX).is_err());
    drop(ticket);
    drop(journal);
    assert!(WorkerJournal::open(&workspace.path(), NonZeroUsize::new(1).unwrap()).is_err());
    gate.release();
    let mut reopened = None;
    until(|| {
        reopened = WorkerJournal::open(&workspace.path(), NonZeroUsize::new(1).unwrap()).ok();
        reopened.is_some()
    });
    assert_eq!(
        reopened.unwrap().records().unwrap()[0].outcome,
        Some(WorkerOutcome::Cancelled)
    );
}

#[test]
fn stalled_journal_writer_does_not_retain_typed_payload_ownership() {
    let workspace = Workspace::new();
    let journal = workspace.journal(1);
    let mut gate = Gate::new(&journal.0.directory.write_hook);
    let pool =
        WorkerPool::with_recovery(WorkerLimits::default(), guardian(), journal.clone()).unwrap();
    let marker = Arc::new(());
    let weak = Arc::downgrade(&marker);
    let command = WorkerCommand {
        executable: unstartable(),
        arguments: vec![],
        directory: workspace.0.clone(),
        environment: Default::default(),
    };
    let handle = pool
        .start_retained(
            command,
            vec![1; 1024],
            OperationControl::default(),
            Some(marker),
        )
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let report = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), handle.wait_retained())
            .await
            .unwrap()
    });
    assert_eq!(report.report.cleanup, WorkerCleanup::NotStarted);
    assert_eq!(report.report.outcome, WorkerOutcome::Failed);
    gate.entered.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(report);
    assert!(pool
        .shutdown_wait(Duration::ZERO)
        .unwrap()
        .active
        .is_empty());
    drop(pool);
    until(|| weak.upgrade().is_none());
    // On Windows the pool also queued the receipt that the worker never ran,
    // which the stalled writer holds.
    assert_eq!(
        journal.flush_wait(Duration::ZERO).unwrap().pending,
        if cfg!(windows) { 2 } else { 1 }
    );
    gate.release();
    settled(&journal);
    assert_eq!(
        journal.records().unwrap()[0].outcome,
        Some(WorkerOutcome::Failed)
    );
}

#[test]
fn intent_failure_prevents_process_entry_and_exposes_persistence_failure() {
    let workspace = Workspace::new();
    let journal = workspace.journal(1);
    let id = JournalId {
        session: journal.0.session,
        sequence: 1,
    };
    journal.0.directory.plant(id).unwrap();
    let pool =
        WorkerPool::with_recovery(WorkerLimits::default(), guardian(), journal.clone()).unwrap();
    let entered = Arc::new(AtomicBool::new(false));
    let marker = entered.clone();
    *pool.0 .0.launcher.lock().unwrap() = Some(Box::new(move |_| {
        marker.store(true, Ordering::Release);
        Err(io::Error::other("must never enter"))
    }));
    let command = WorkerCommand {
        executable: unstartable(),
        arguments: vec![],
        directory: workspace.0.clone(),
        environment: Default::default(),
    };
    let handle = pool
        .start(command, vec![], OperationControl::default())
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let report = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), handle.wait())
            .await
            .unwrap()
    });
    assert_eq!(report.cleanup, WorkerCleanup::NotStarted);
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert!(report
        .diagnostic
        .unwrap()
        .to_string()
        .contains("Persisting worker intent failed"));
    assert!(!entered.load(Ordering::Acquire));
    assert_eq!(
        journal.flush_wait(Duration::from_secs(5)).unwrap(),
        JournalFlush {
            pending: 0,
            failed: 1
        }
    );
    assert!(journal.records().unwrap()[0].damaged);
}

#[test]
fn records_with_a_second_link_are_refused() {
    let workspace = Workspace::new();
    let journal = workspace.journal(1);
    let ticket = journal.reserve().unwrap();
    ticket.file().unwrap();
    assert_eq!(journal.records().unwrap().len(), 1);
    fs::hard_link(
        workspace.path().join(format!("{}.bwk", ticket.id)),
        workspace.0.join("alias"),
    )
    .unwrap();
    assert!(journal.records().is_err());
}

/// The pool queues a tree's receipt ahead of the publications that follow it,
/// and only the first.
#[cfg(windows)]
#[test]
fn receipts_queue_ahead_of_publications_and_only_once() {
    let workspace = Workspace::new();
    let journal = workspace.journal(1);
    let ticket = journal.reserve().unwrap();
    ticket.file().unwrap();
    let settled_tree = GuardianReceipt::TreeSettled {
        exit_status: 0,
        errno: 0,
    };
    ticket.receipt(settled_tree);
    ticket.receipt(GuardianReceipt::NotStarted { errno: 2 });
    ticket.submit(Some(success()), Some(success()));
    settled(&journal);
    let record = journal.records().unwrap().remove(0);
    assert!(!record.damaged);
    assert_eq!(record.guardian, Some(settled_tree));
    assert_eq!(record.outcome, Some(WorkerOutcome::Succeeded));
}

#[cfg(windows)]
#[test]
fn windows_journals_are_private_and_refuse_shared_or_linked_directories() {
    use std::process::Command;
    let workspace = Workspace::new();
    // A directory made as usual inherits the temporary directory's access
    // list, which other accounts share.
    fs::create_dir(workspace.path()).unwrap();
    assert!(WorkerJournal::open(&workspace.path(), NonZeroUsize::new(1).unwrap()).is_err());
    fs::remove_dir(workspace.path()).unwrap();
    // A junction in its place is refused rather than followed.
    let target = workspace.0.join("target");
    fs::create_dir(&target).unwrap();
    let linked = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(workspace.path())
        .arg(&target)
        .output()
        .unwrap();
    assert!(linked.status.success(), "{linked:?}");
    assert!(WorkerJournal::open(&workspace.path(), NonZeroUsize::new(1).unwrap()).is_err());
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
    fs::remove_dir(workspace.path()).unwrap();
    // The journal makes its directory private: one entry, granting this user
    // everything, inherited by what it holds.
    let journal = workspace.journal(1);
    let listing = Command::new("icacls")
        .arg(workspace.path())
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&listing.stdout);
    let entries: Vec<&str> = listing
        .lines()
        .take_while(|line| !line.starts_with("Successfully"))
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(entries.len(), 1, "{listing}");
    let user = std::env::var("USERNAME").unwrap();
    assert!(
        entries[0].ends_with(":(OI)(CI)(F)") && entries[0].contains(&user),
        "{listing}"
    );
    drop(journal);
}

#[test]
fn dropping_the_last_handle_releases_the_directory_at_once() {
    let workspace = Workspace::new();
    for _ in 0..20 {
        let journal = workspace.journal(4);
        let directory = Arc::downgrade(&journal.0.directory);
        drop(journal);
        // The writer released its share before the drop returned, so the
        // directory can be removed at once, as Windows requires.
        assert!(directory.upgrade().is_none());
        fs::remove_dir_all(workspace.path()).unwrap();
    }
}
