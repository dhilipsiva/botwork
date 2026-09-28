#![cfg(target_os = "linux")]
use botwork::core::{
    operation::OperationControl,
    worker::{
        journal::{JournalFlush, WorkerJournal},
        WorkerCleanup, WorkerCommand, WorkerHandle, WorkerLimits, WorkerOutcome, WorkerPool,
        WorkerReport,
    },
};
use std::{
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
            "botwork-ns-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn journal(&self) -> WorkerJournal {
        WorkerJournal::open(&self.0.join("journal"), NonZeroUsize::new(8).unwrap()).unwrap()
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
        assert!(Instant::now() < deadline, "namespace handshake timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn pool(journal: Option<WorkerJournal>) -> WorkerPool {
    WorkerPool::with_pid_namespace(
        WorkerLimits {
            timeout: Duration::from_secs(10),
            cleanup_timeout: Duration::from_millis(200),
            max_in_flight: NonZeroUsize::new(1).unwrap(),
            ..Default::default()
        },
        env!("CARGO_BIN_EXE_botwork").into(),
        journal,
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
fn tree(directory: &Path) -> WorkerCommand {
    let python = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("python3"))
        .find(|path| path.is_absolute() && path.is_file())
        .unwrap();
    let mut command = command(python.to_str().unwrap(), directory);
    command.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/support/tree_worker.py")
            .into_os_string(),
        "wait".into(),
        directory.into(),
    ];
    command
}
fn host_pids(guardian: u32, directory: &Path) -> Vec<u32> {
    let namespace = fs::read_link(format!("/proc/{guardian}/ns/pid")).unwrap();
    let local: Vec<u32> = ["root", "leaf"]
        .into_iter()
        .map(|name| {
            fs::read_to_string(directory.join(name))
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    let mut matched = Vec::new();
    for entry in fs::read_dir("/proc").unwrap() {
        let entry = entry.unwrap();
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if fs::read_link(entry.path().join("ns/pid")).ok().as_ref() != Some(&namespace) {
            continue;
        }
        let status = fs::read_to_string(entry.path().join("status")).unwrap_or_default();
        if status
            .lines()
            .find_map(|line| line.strip_prefix("NSpid:"))
            .and_then(|ids| ids.split_whitespace().last())
            .and_then(|id| id.parse::<u32>().ok())
            .is_some_and(|id| local.contains(&id))
        {
            matched.push(pid);
        }
    }
    assert_eq!(
        matched.len(),
        2,
        "root and detached leaf are live in their namespace"
    );
    matched
}
fn gone(pids: &[u32]) {
    for pid in pids {
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "namespace member {pid} remains unreaped"
        );
    }
}

#[test]
fn namespace_transports_bytes_and_exposes_consistent_private_proc_and_identity() {
    let workspace = Workspace::new();
    let pool = pool(None);
    let mut specification = command("/bin/sh", &workspace.0);
    specification.arguments = vec!["-c".into(), "test $$ -eq 2 && test -d /proc/1 && test -d /proc/2 && test ! -e /proc/999999 && cat; printf '%s' \"$MARK\" >&2".into()];
    specification.environment.insert("MARK".into(), "é".into());
    let input = vec![b'x'; 128 * 1024];
    let report = wait(
        pool.start(specification, input.clone(), OperationControl::default())
            .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Succeeded, "{report:?}");
    assert_eq!(report.cleanup, WorkerCleanup::TreeReaped);
    assert_eq!(report.stdout, input);
    assert_eq!(report.stderr, "é".as_bytes());
    assert!(report.io_complete && report.progress_complete);
    assert!(pool.snapshot().active.is_empty());
    let mut identity = command("/bin/sh", &workspace.0);
    identity.arguments = vec![
        "-c".into(),
        "id -u; id -g; readlink /proc/self; cat /proc/self/status".into(),
    ];
    let report = wait(
        pool.start(identity, vec![], OperationControl::default())
            .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Succeeded);
    let output = String::from_utf8(report.stdout).unwrap();
    let mut lines = output.lines();
    assert_eq!(
        lines.next().unwrap(),
        unsafe { libc::geteuid() }.to_string()
    );
    assert_eq!(
        lines.next().unwrap(),
        unsafe { libc::getegid() }.to_string()
    );
    assert!(output.lines().any(|line| line == "NoNewPrivs:\t1"));
}

#[test]
fn normal_worker_failure_and_signal_preserve_status_with_verified_tree_cleanup() {
    use std::os::unix::process::ExitStatusExt;
    let workspace = Workspace::new();
    let pool = pool(None);
    for script in ["exit 7", "kill -USR1 $$"] {
        let mut specification = command("/bin/sh", &workspace.0);
        specification.arguments = vec!["-c".into(), script.into()];
        let report = wait(
            pool.start(specification, vec![], OperationControl::default())
                .unwrap(),
        );
        assert_eq!(report.outcome, WorkerOutcome::Failed);
        assert_eq!(report.cleanup, WorkerCleanup::TreeReaped);
        let status = report.exit_status.unwrap();
        if script == "exit 7" {
            assert_eq!(status.code(), Some(7));
        } else {
            assert_eq!(status.signal(), Some(libc::SIGUSR1));
        }
    }
}

#[test]
fn killed_guardian_leaves_no_detached_members_and_releases_capacity_without_success() {
    let workspace = Workspace::new();
    let journal = workspace.journal();
    let pool = pool(Some(journal.clone()));
    let handle = pool
        .start(tree(&workspace.0), vec![], OperationControl::default())
        .unwrap();
    until(|| workspace.0.join("ready").exists());
    let guardian = pool.snapshot().active[0].pid.unwrap();
    let pids = host_pids(guardian, &workspace.0);
    assert_eq!(unsafe { libc::kill(guardian as i32, libc::SIGKILL) }, 0);
    let report = wait(handle);
    assert_eq!(report.cleanup, WorkerCleanup::NamespaceReaped, "{report:?}");
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert!(report.exit_status.is_none());
    assert!(report.progress_complete);
    gone(&pids);
    assert!(!Path::new(&format!("/proc/{guardian}")).exists());
    assert!(pool.snapshot().active.is_empty());
    assert_eq!(
        journal.flush_wait(Duration::from_secs(5)).unwrap(),
        JournalFlush::default()
    );
    let record = journal.records().unwrap().remove(0);
    assert_eq!(record.outcome, Some(WorkerOutcome::Failed));
    assert_eq!(
        record.reconciled.unwrap().cleanup,
        WorkerCleanup::NamespaceReaped
    );
    assert!(!record.damaged);
    assert_eq!(
        wait(
            pool.start(
                command("/bin/true", &workspace.0),
                vec![],
                OperationControl::default()
            )
            .unwrap()
        )
        .outcome,
        WorkerOutcome::Succeeded
    );
}

#[test]
fn cancellation_terminates_a_stopped_namespace_init() {
    let workspace = Workspace::new();
    let pool = pool(None);
    let control = OperationControl::default();
    let handle = pool
        .start(tree(&workspace.0), vec![], control.clone())
        .unwrap();
    until(|| workspace.0.join("ready").exists());
    let guardian = pool.snapshot().active[0].pid.unwrap();
    let pids = host_pids(guardian, &workspace.0);
    assert_eq!(unsafe { libc::kill(guardian as i32, libc::SIGSTOP) }, 0);
    control.cancel();
    let report = wait(handle);
    assert_eq!(report.outcome, WorkerOutcome::Cancelled);
    assert_eq!(report.cleanup, WorkerCleanup::NamespaceReaped, "{report:?}");
    gone(&pids);
    assert!(pool.snapshot().active.is_empty());
}

#[test]
fn mismatched_helper_never_enters_worker_and_kernel_cleanup_is_reusable() {
    let workspace = Workspace::new();
    let pool =
        WorkerPool::with_pid_namespace(WorkerLimits::default(), "/bin/true".into(), None).unwrap();
    let mut specification = command("/bin/sh", &workspace.0);
    specification.arguments = vec!["-c".into(), "touch must-not-exist".into()];
    for _ in 0..2 {
        let report = wait(
            pool.start(specification.clone(), vec![], OperationControl::default())
                .unwrap(),
        );
        assert_eq!(report.cleanup, WorkerCleanup::NamespaceReaped);
        assert_eq!(report.outcome, WorkerOutcome::Failed);
        assert!(pool.snapshot().active.is_empty());
        assert!(!workspace.0.join("must-not-exist").exists());
    }
}

#[test]
fn typed_response_cannot_pass_after_guardian_failure_and_reservations_are_released() {
    use botwork::core::{
        grammar::Literal,
        operation::{NativeOperation, OperationUsage},
        signature::StatementSignature,
        worker::protocol::WorkerProtocol,
    };
    use std::{
        future::Future,
        task::{Context, Waker},
    };
    let workspace = Workspace::new();
    let pool = pool(None);
    let protocol = WorkerProtocol::default();
    let response = protocol.encode_response(Ok(&Literal::Int(17))).unwrap();
    let mut specification = tree(&workspace.0);
    specification.arguments[1] = "typed-wait".into();
    specification.arguments.push(
        response
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .into(),
    );
    let operation = NativeOperation::isolated(
        StatementSignature::native("Echo |x|").unwrap(),
        pool.clone(),
        specification,
        protocol,
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let mut invocation = Box::pin(operation.invoke(
        vec![Literal::String("retained".into())],
        OperationControl::default(),
    ));
    runtime.block_on(async {
        assert!(invocation
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
    });
    until(|| workspace.0.join("ready").exists());
    let guardian = pool.snapshot().active[0].pid.unwrap();
    let pids = host_pids(guardian, &workspace.0);
    assert!(operation.isolated_in_flight_bytes().unwrap() > 0);
    assert_eq!(unsafe { libc::kill(guardian as i32, libc::SIGKILL) }, 0);
    let result = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), invocation)
            .await
            .unwrap()
    });
    assert!(result.is_err());
    until(|| {
        operation.ownership_budget().usage() == OperationUsage::default()
            && operation.isolated_in_flight_bytes() == Some(0)
    });
    gone(&pids);
    assert!(pool.snapshot().active.is_empty());
}

#[test]
fn denied_namespace_facilities_reject_before_worker_effects_without_fallback() {
    for facility in ["clone3", "mount"] {
        let workspace = Workspace::new();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "subprocess_namespace_facility_failure",
                "--nocapture",
            ])
            .env("BOTWORK_NS_DENY", facility)
            .env("BOTWORK_NS_DIRECTORY", &workspace.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!workspace.0.join("must-not-exist").exists());
    }
}

#[test]
fn subprocess_namespace_facility_failure() {
    let Ok(facility) = std::env::var("BOTWORK_NS_DENY") else {
        return;
    };
    let syscall = if facility == "clone3" {
        libc::SYS_clone3
    } else {
        libc::SYS_mount
    };
    let filter = [
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: syscall as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr().cast_mut(),
    };
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
        0
    );
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) },
        0
    );
    let directory = PathBuf::from(std::env::var_os("BOTWORK_NS_DIRECTORY").unwrap());
    let pool = pool(None);
    let mut specification = command("/bin/sh", &directory);
    specification.arguments = vec!["-c".into(), "touch must-not-exist".into()];
    let report = wait(
        pool.start(specification, vec![], OperationControl::default())
            .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Failed);
    assert_eq!(report.cleanup, WorkerCleanup::NotStarted);
    assert!(report
        .diagnostic
        .unwrap()
        .to_string()
        .contains("Function not implemented"));
    assert!(!directory.join("must-not-exist").exists());
    assert!(pool.snapshot().active.is_empty());
}

struct Host(Child);
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn host_death_kills_even_a_stopped_guardian_and_recovery_preserves_interruption() {
    for closed_stdio in [false, true] {
        let workspace = Workspace::new();
        let mut host = Host(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "subprocess_namespace_host", "--nocapture"])
                .env("BOTWORK_NS_HOST", &workspace.0)
                .env(
                    "BOTWORK_NS_CLOSE_STDIO",
                    if closed_stdio { "1" } else { "0" },
                )
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        until(|| workspace.0.join("host-ready").exists());
        let guardian: u32 = fs::read_to_string(workspace.0.join("guardian"))
            .unwrap()
            .parse()
            .unwrap();
        let pids = host_pids(guardian, &workspace.0);
        assert_eq!(unsafe { libc::kill(guardian as i32, libc::SIGSTOP) }, 0);
        host.0.kill().unwrap();
        host.0.wait().unwrap();
        until(|| {
            pids.iter()
                .all(|pid| !Path::new(&format!("/proc/{pid}")).exists())
        });
        gone(&pids);
        let journal = workspace.journal();
        let record = journal.records().unwrap().remove(0);
        assert_eq!(record.outcome, Some(WorkerOutcome::Interrupted));
        assert!(record.published.is_none());
        assert!(!record.damaged);
        until(|| {
            fs::read_to_string(format!("/proc/{guardian}/status")).map_or(true, |status| {
                status
                    .lines()
                    .any(|line| line.starts_with("State:") && line.contains('Z'))
            })
        });
    }
}

#[test]
fn subprocess_namespace_host() {
    let Some(directory) = std::env::var_os("BOTWORK_NS_HOST") else {
        return;
    };
    let directory = PathBuf::from(directory);
    if std::env::var("BOTWORK_NS_CLOSE_STDIO").unwrap() == "1" {
        unsafe {
            libc::close(0);
            libc::close(1);
            libc::close(2);
        }
    }
    let journal =
        WorkerJournal::open(&directory.join("journal"), NonZeroUsize::new(8).unwrap()).unwrap();
    let pool = pool(Some(journal));
    let _handle = pool
        .start(tree(&directory), vec![], OperationControl::default())
        .unwrap();
    until(|| directory.join("ready").exists());
    fs::write(
        directory.join("guardian"),
        pool.snapshot().active[0].pid.unwrap().to_string(),
    )
    .unwrap();
    fs::write(directory.join("host-ready"), []).unwrap();
    std::thread::sleep(Duration::from_secs(15));
}
