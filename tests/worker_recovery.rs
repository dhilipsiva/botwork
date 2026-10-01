//! The worker journal across a lost host: on Linux the guardian outlives the
//! host and writes its receipt late; on Windows the host's Job Object ends the
//! tree with the host, and recovery reports the worker interrupted.
#![cfg(any(target_os = "linux", windows))]
use botwork::core::{
    operation::OperationControl,
    worker::{
        journal::{GuardianReceipt, JournalFlush, WorkerJournal},
        WorkerCleanup, WorkerCommand, WorkerHandle, WorkerLimits, WorkerOutcome, WorkerPool,
        WorkerReport,
    },
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "botwork-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn journal(&self, maximum: usize) -> WorkerJournal {
        WorkerJournal::open(&self.0.join("journal"), NonZeroUsize::new(maximum).unwrap()).unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[track_caller]
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "recovery handshake timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn pool(journal: &WorkerJournal) -> WorkerPool {
    WorkerPool::with_recovery(
        WorkerLimits {
            timeout: Duration::from_secs(20),
            cleanup_timeout: Duration::from_millis(40),
            ..Default::default()
        },
        env!("CARGO_BIN_EXE_botwork").into(),
        journal.clone(),
    )
    .unwrap()
}
fn command(executable: &str, directory: &Path) -> WorkerCommand {
    WorkerCommand {
        executable: executable.into(),
        arguments: vec![],
        directory: directory.into(),
        environment: Default::default(),
    }
}
fn python() -> PathBuf {
    let names: &[&str] = if cfg!(windows) {
        &["python.exe", "python3.exe"]
    } else {
        &["python3"]
    };
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_absolute() && path.is_file())
        .expect("Python, which the repository's checks use")
}
/// tests/support/worker_tool.py in `mode`, which behaves the same everywhere.
fn tool(mode: &str, argument: Option<&Path>, directory: &Path) -> WorkerCommand {
    let mut environment = BTreeMap::new();
    // Python on Windows needs SystemRoot to start.
    if let Some(root) = std::env::var_os("SystemRoot") {
        environment.insert(OsString::from("SystemRoot"), root);
    }
    WorkerCommand {
        executable: python(),
        arguments: [
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/support/worker_tool.py")
                .into_os_string(),
            mode.into(),
        ]
        .into_iter()
        .chain(argument.map(|path| path.as_os_str().to_owned()))
        .collect(),
        directory: directory.into(),
        environment,
    }
}
fn missing_worker() -> &'static str {
    if cfg!(windows) {
        r"C:\missing-worker.exe"
    } else {
        "/missing-worker"
    }
}
fn wait(handle: WorkerHandle) -> WorkerReport {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(8), handle.wait())
            .await
            .unwrap()
    })
}
fn flush(journal: &WorkerJournal) {
    assert_eq!(
        journal.flush_wait(Duration::from_secs(5)).unwrap(),
        JournalFlush::default()
    );
}
fn reopen(workspace: &Workspace, maximum: usize) -> WorkerJournal {
    let mut journal = None;
    until(|| {
        journal = WorkerJournal::open(
            &workspace.0.join("journal"),
            NonZeroUsize::new(maximum).unwrap(),
        )
        .ok();
        journal.is_some()
    });
    journal.unwrap()
}

#[test]
fn final_transport_records_survive_reopen_without_payloads_and_quota_is_shared() {
    let workspace = Workspace::new();
    let journal = workspace.journal(3);
    let first = pool(&journal);
    let second = pool(&journal);
    let secret = b"payload-and-environment-must-not-persist";
    let mut cat = tool("cat", None, &workspace.0);
    cat.environment.insert(
        "SECRET".into(),
        String::from_utf8(secret.to_vec()).unwrap().into(),
    );
    let handle = first
        .start(cat, secret.to_vec(), OperationControl::default())
        .unwrap();
    let first_id = handle.journal_id().unwrap();
    let local = handle.id();
    let report = wait(handle);
    assert_eq!(report.outcome, WorkerOutcome::Succeeded);
    assert_eq!(report.stdout, secret);
    let handle = second
        .start(
            tool("fail", None, &workspace.0),
            vec![],
            OperationControl::default(),
        )
        .unwrap();
    assert_eq!(handle.id(), local);
    assert_ne!(handle.journal_id().unwrap(), first_id);
    assert_eq!(wait(handle).outcome, WorkerOutcome::Failed);
    let handle = first
        .start(
            command(missing_worker(), &workspace.0),
            vec![],
            OperationControl::default(),
        )
        .unwrap();
    assert_eq!(wait(handle).cleanup, WorkerCleanup::NotStarted);
    assert!(second
        .start(
            tool("cat", None, &workspace.0),
            vec![],
            OperationControl::default()
        )
        .is_err());
    flush(&journal);
    drop(first);
    drop(second);
    drop(journal);
    let journal = reopen(&workspace, 3);
    let records = journal.records().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].id, first_id);
    assert_eq!(records[0].outcome, Some(WorkerOutcome::Succeeded));
    assert_eq!(records[1].outcome, Some(WorkerOutcome::Failed));
    assert_eq!(records[2].outcome, Some(WorkerOutcome::Failed));
    assert!(matches!(
        records[2].guardian,
        Some(GuardianReceipt::NotStarted { .. })
    ));
    assert!(records.iter().all(|record| !record.damaged));
    for entry in fs::read_dir(workspace.0.join("journal")).unwrap() {
        let entry = entry.unwrap();
        // An empty file, such as the lock that Windows bars others from
        // reading, holds nothing.
        if entry.metadata().unwrap().len() == 0 {
            continue;
        }
        let bytes = fs::read(entry.path()).unwrap();
        assert_eq!(bytes.len(), 256);
        assert!(!bytes.windows(secret.len()).any(|window| window == secret));
    }
}

struct Host(Child);
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[cfg(target_os = "linux")]
struct Resume(i32);
#[cfg(target_os = "linux")]
impl Drop for Resume {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                libc::kill(self.0, libc::SIGCONT);
            }
        }
    }
}

#[cfg(target_os = "linux")]
struct Adoption(i32);
#[cfg(target_os = "linux")]
impl Adoption {
    fn new() -> Self {
        let mut previous = 0;
        assert_eq!(
            unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut previous, 0, 0, 0) },
            0
        );
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
            0
        );
        Self(previous)
    }
}
#[cfg(target_os = "linux")]
impl Drop for Adoption {
    fn drop(&mut self) {
        unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, self.0, 0, 0, 0);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn host_death_recovers_interruption_or_frozen_stop_and_accepts_late_guardian_receipt() {
    // Adopt the deliberately paused guardian so its process group does not
    // become orphaned and receive SIGHUP. Reap only that exact owned child.
    let _adoption = Adoption::new();
    for published in [false, true] {
        let workspace = Workspace::new();
        let mut host = Host(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "subprocess_journal_host", "--nocapture"])
                .env("BOTWORK_JOURNAL_HOST", &workspace.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        until(|| workspace.0.join("host-ready").exists());
        let pid: i32 = fs::read_to_string(workspace.0.join("guardian"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
        let mut resume = Resume(pid);
        until(|| {
            fs::read_to_string(format!("/proc/{pid}/status"))
                .unwrap()
                .lines()
                .any(|line| line.starts_with("State:") && line.contains('T'))
        });
        if published {
            fs::write(workspace.0.join("cancel"), []).unwrap();
            until(|| workspace.0.join("published").exists());
        }
        host.0.kill().unwrap();
        host.0.wait().unwrap();
        let journal = reopen(&workspace, 8);
        let expected = if published {
            WorkerOutcome::Cancelled
        } else {
            WorkerOutcome::Interrupted
        };
        let record = journal.records().unwrap().remove(0);
        assert_eq!(record.outcome, Some(expected));
        assert!(!record.damaged);
        assert!(record.guardian.is_none());
        assert!(record.reconciled.is_none());
        if published {
            assert_eq!(record.published.unwrap().cleanup, WorkerCleanup::Pending);
        } else {
            assert!(record.published.is_none());
        }
        assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
        resume.0 = 0;
        // New journal owns the directory lock while the old guardian uses only
        // its invocation FD. No PID-based action is taken by recovery itself.
        until(|| {
            if journal.records().unwrap()[0].guardian.is_some() {
                return true;
            }
            let mut status = 0;
            let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            assert_eq!(
                reaped, 0,
                "guardian exited without receipt: status={status}"
            );
            false
        });
        let record = journal.records().unwrap().remove(0);
        assert_eq!(record.outcome, Some(expected));
        assert!(!record.damaged);
        assert!(matches!(
            record.guardian,
            Some(GuardianReceipt::TreeSettled { .. })
        ));
        assert!(record.reconciled.is_none());
        for name in ["root", "middle", "leaf"] {
            let child = fs::read_to_string(workspace.0.join(name)).unwrap();
            assert!(
                !Path::new(&format!("/proc/{child}")).exists(),
                "{name} remains unreaped"
            );
        }
        until(|| {
            let mut status = 0;
            let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            assert!(reaped >= 0);
            if reaped == pid {
                assert_eq!(status, 0);
                true
            } else {
                false
            }
        });
    }
}

#[cfg(target_os = "linux")]
#[test]
fn subprocess_journal_host() {
    let Some(directory) = std::env::var_os("BOTWORK_JOURNAL_HOST") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let journal =
        WorkerJournal::open(&directory.join("journal"), NonZeroUsize::new(8).unwrap()).unwrap();
    let pool = pool(&journal);
    let python = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("python3"))
        .find(|path| path.is_absolute() && path.is_file())
        .unwrap();
    let mut command = command(python.to_str().unwrap(), &directory);
    command.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/support/tree_worker.py")
            .into_os_string(),
        "wait".into(),
        directory.clone().into_os_string(),
    ];
    let control = OperationControl::default();
    let handle = pool.start(command, vec![], control.clone()).unwrap();
    until(|| directory.join("ready").exists());
    fs::write(
        directory.join("guardian"),
        pool.snapshot().active[0].pid.unwrap().to_string(),
    )
    .unwrap();
    fs::write(directory.join("host-ready"), []).unwrap();
    until(|| directory.join("cancel").exists());
    control.cancel();
    let report = wait(handle);
    assert_eq!(report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(report.cleanup, WorkerCleanup::Pending);
    flush(&journal);
    fs::write(directory.join("published"), []).unwrap();
    std::thread::sleep(Duration::from_secs(10));
}

/// Whether the heartbeat file has stopped growing, so the descendant that
/// writes it every 20 ms has ended.
#[cfg(windows)]
fn stopped(heartbeat: &Path) -> bool {
    let size = || fs::metadata(heartbeat).map_or(0, |metadata| metadata.len());
    let before = size();
    std::thread::sleep(Duration::from_millis(300));
    size() == before
}

#[cfg(windows)]
#[test]
fn a_lost_windows_host_ends_the_tree_and_recovers_interruption_or_the_reaped_stop() {
    for published in [false, true] {
        let workspace = Workspace::new();
        let mut host = Host(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "windows_subprocess_journal_host", "--nocapture"])
                .env("BOTWORK_JOURNAL_HOST", &workspace.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        until(|| workspace.0.join("host-ready").exists());
        if published {
            fs::write(workspace.0.join("cancel"), []).unwrap();
            until(|| workspace.0.join("published").exists());
        }
        host.0.kill().unwrap();
        host.0.wait().unwrap();
        // Closing the lost host's job ended the worker and its descendant.
        let heartbeat = workspace.0.join("heartbeat");
        let begun = Instant::now();
        while !stopped(&heartbeat) {
            assert!(
                begun.elapsed() < Duration::from_secs(10),
                "the tree outlived its host"
            );
        }
        let journal = reopen(&workspace, 8);
        let record = journal.records().unwrap().remove(0);
        assert!(!record.damaged);
        if published {
            // The host saw the cancelled tree reaped, and its receipt and
            // reconciliation persisted, before it was lost.
            assert_eq!(record.outcome, Some(WorkerOutcome::Cancelled));
            assert_eq!(
                record.reconciled.unwrap().cleanup,
                WorkerCleanup::TreeReaped
            );
            assert!(matches!(
                record.guardian,
                Some(GuardianReceipt::TreeSettled { errno: 0, .. })
            ));
        } else {
            // No receipt follows a lost host on Windows: its job ended the
            // tree, and the record says the worker was interrupted.
            assert_eq!(record.outcome, Some(WorkerOutcome::Interrupted));
            assert!(record.published.is_none());
            assert!(record.reconciled.is_none());
            assert!(record.guardian.is_none());
        }
    }
}

#[cfg(windows)]
#[test]
fn windows_subprocess_journal_host() {
    let Some(directory) = std::env::var_os("BOTWORK_JOURNAL_HOST") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let journal =
        WorkerJournal::open(&directory.join("journal"), NonZeroUsize::new(8).unwrap()).unwrap();
    let pool = pool(&journal);
    let heartbeat = directory.join("heartbeat");
    let control = OperationControl::default();
    let handle = pool
        .start(
            tool("tree", Some(&heartbeat), &directory),
            vec![],
            control.clone(),
        )
        .unwrap();
    until(|| fs::metadata(&heartbeat).is_ok_and(|metadata| metadata.len() > 0));
    fs::write(directory.join("host-ready"), []).unwrap();
    // The test kills this host, here or after publishing.
    while !directory.join("cancel").exists() {
        std::thread::sleep(Duration::from_millis(5));
    }
    control.cancel();
    let report = wait(handle);
    assert_eq!(report.outcome, WorkerOutcome::Cancelled);
    // Once the tree is reaped, its receipt and reconciliation are queued.
    until(|| pool.snapshot().active.is_empty());
    flush(&journal);
    fs::write(directory.join("published"), []).unwrap();
    std::thread::sleep(Duration::from_secs(60));
}
