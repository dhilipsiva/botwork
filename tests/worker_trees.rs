#![cfg(target_os = "linux")]
use botwork::core::{
    diagnostic::DiagnosticCode,
    grammar::Literal,
    operation::{NativeOperation, OperationControl, OperationUsage},
    signature::StatementSignature,
    worker::{
        protocol::WorkerProtocol, WorkerCleanup, WorkerCommand, WorkerLimits, WorkerOutcome,
        WorkerPool, WorkerReport,
    },
};
use std::{
    fs,
    os::unix::process::ExitStatusExt,
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
            "botwork-tree-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn ready(&self) {
        until(|| self.0.join("ready").exists());
    }
    fn reaped(&self) {
        for name in ["root", "middle", "leaf"] {
            let pid = fs::read_to_string(self.0.join(name)).unwrap();
            assert!(
                !Path::new(&format!("/proc/{pid}")).exists(),
                "{name} {pid} remains alive or unreaped"
            );
        }
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn until(mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < end,
            "worker tree handshake/cleanup timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn limits() -> WorkerLimits {
    WorkerLimits {
        timeout: Duration::from_secs(8),
        cleanup_timeout: Duration::from_secs(1),
        ..Default::default()
    }
}
fn pool(limits: WorkerLimits) -> WorkerPool {
    WorkerPool::with_process_tree(limits, PathBuf::from(env!("CARGO_BIN_EXE_botwork"))).unwrap()
}
fn command(directory: &Path, mode: &str) -> WorkerCommand {
    let executable = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("python3"))
        .find(|path| path.is_absolute() && path.is_file())
        .unwrap();
    WorkerCommand {
        executable,
        arguments: vec![
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/support/tree_worker.py")
                .into_os_string(),
            mode.into(),
            directory.into(),
        ],
        directory: directory.into(),
        environment: Default::default(),
    }
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}
fn wait(handle: botwork::core::worker::WorkerHandle) -> WorkerReport {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(10), handle.wait())
            .await
            .unwrap()
    })
}

#[test]
fn normal_and_failed_exits_reap_detached_descendants_and_preserve_worker_status() {
    for mode in ["exit", "failure", "signal"] {
        let workspace = Workspace::new();
        let pool = pool(limits());
        let report = wait(
            pool.start(
                command(&workspace.0, mode),
                vec![],
                OperationControl::default(),
            )
            .unwrap(),
        );
        assert_eq!(report.cleanup, WorkerCleanup::TreeReaped, "{report:?}");
        assert!(report.io_complete && report.progress_complete);
        let status = report.exit_status.unwrap();
        match mode {
            "exit" => {
                assert_eq!(report.outcome, WorkerOutcome::Succeeded);
                assert!(status.success());
            }
            "failure" => {
                assert_eq!(report.outcome, WorkerOutcome::Failed);
                assert_eq!(status.code(), Some(7));
            }
            _ => {
                assert_eq!(report.outcome, WorkerOutcome::Failed);
                assert_eq!(status.signal(), Some(libc::SIGUSR1));
            }
        }
        workspace.reaped();
        assert!(pool.snapshot().active.is_empty());
    }
}

#[test]
fn byte_streams_and_explicit_environment_survive_guardian_handoff() {
    let workspace = Workspace::new();
    let pool = pool(limits());
    let mut specification = command(&workspace.0, "echo");
    specification.environment.insert("MARK".into(), "é".into());
    let input = vec![b'x'; 128 * 1024];
    let report = wait(
        pool.start(specification, input.clone(), OperationControl::default())
            .unwrap(),
    );
    assert_eq!(report.outcome, WorkerOutcome::Succeeded);
    assert_eq!(report.stdout, input);
    assert_eq!(report.stderr, "é".as_bytes());
    assert_eq!(report.cleanup, WorkerCleanup::TreeReaped, "{report:?}");
    workspace.reaped();
}

#[test]
fn cancellation_deadline_and_abandonment_reap_the_complete_tree() {
    for mode in ["cancel", "deadline", "abandon", "shutdown"] {
        let workspace = Workspace::new();
        let pool = pool(limits());
        let control = OperationControl::default();
        let mut handle = Some(
            pool.start(
                command(&workspace.0, "wait"),
                vec![],
                if mode == "deadline" {
                    control.child(Some(
                        tokio::time::Instant::now() + Duration::from_millis(500),
                    ))
                } else {
                    control.clone()
                },
            )
            .unwrap(),
        );
        workspace.ready();
        let expected = match mode {
            "cancel" => {
                control.cancel();
                WorkerOutcome::Cancelled
            }
            "abandon" => {
                drop(handle.take());
                WorkerOutcome::Interrupted
            }
            "shutdown" => {
                pool.shutdown();
                WorkerOutcome::Cancelled
            }
            _ => WorkerOutcome::TimedOut,
        };
        if let Some(handle) = handle {
            let report = wait(handle);
            assert_eq!(report.outcome, expected);
            assert_eq!(report.cleanup, WorkerCleanup::TreeReaped, "{report:?}");
        }
        until(|| pool.snapshot().active.is_empty());
        workspace.reaped();
        assert_eq!(pool.snapshot().completed[0].outcome, expected);
    }
}

struct Resume(Option<u32>);
impl Resume {
    fn new(pid: u32) -> Self {
        // This is our currently owned guardian, not a PID read from external input.
        assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGSTOP) }, 0);
        Self(Some(pid))
    }
    fn resume(&mut self) {
        if let Some(pid) = self.0.take() {
            assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGCONT) }, 0);
        }
    }
}
impl Drop for Resume {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            unsafe {
                libc::kill(pid as i32, libc::SIGCONT);
            }
        }
    }
}

#[test]
fn pending_guardian_keeps_capacity_and_reconciles_the_original_outcome() {
    let workspace = Workspace::new();
    let pool = pool(WorkerLimits {
        max_in_flight: std::num::NonZeroUsize::new(1).unwrap(),
        cleanup_timeout: Duration::from_millis(20),
        ..limits()
    });
    let control = OperationControl::default();
    let handle = pool
        .start(command(&workspace.0, "wait"), vec![], control.clone())
        .unwrap();
    workspace.ready();
    let mut paused = Resume::new(pool.snapshot().active[0].pid.unwrap());
    control.cancel();
    let report = wait(handle);
    assert_eq!(report.cleanup, WorkerCleanup::Pending);
    assert_eq!(report.outcome, WorkerOutcome::Cancelled);
    assert!(!report.progress_complete);
    assert!(pool
        .start(
            command(&workspace.0, "exit"),
            vec![],
            OperationControl::default()
        )
        .is_err());
    assert_eq!(pool.shutdown_wait(Duration::ZERO).unwrap().active.len(), 1);
    paused.resume();
    until(|| pool.snapshot().active.is_empty());
    workspace.reaped();
    assert_eq!(
        pool.snapshot().completed[0].cleanup,
        WorkerCleanup::TreeReaped
    );
    assert_eq!(
        pool.snapshot().completed[0].outcome,
        WorkerOutcome::Cancelled
    );
}

#[test]
fn typed_values_require_complete_tree_cleanup_and_pending_ownership_outlives_runtime() {
    use std::{
        future::Future,
        task::{Context, Waker},
    };
    for stop in [false, true] {
        let workspace = Workspace::new();
        let pool = pool(WorkerLimits {
            cleanup_timeout: Duration::from_millis(20),
            ..limits()
        });
        let protocol = WorkerProtocol::default();
        let response = protocol.encode_response(Ok(&Literal::Int(17))).unwrap();
        let mut specification = command(&workspace.0, if stop { "typed-wait" } else { "typed" });
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
        let control = OperationControl::default();
        let runtime = runtime();
        let mut invocation =
            Box::pin(operation.invoke(vec![Literal::String("retained".into())], control.clone()));
        runtime.block_on(async {
            assert!(invocation
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending());
        });
        workspace.ready();
        let mut paused = stop.then(|| Resume::new(pool.snapshot().active[0].pid.unwrap()));
        if stop {
            control.cancel();
        }
        let result = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), invocation)
                .await
                .unwrap()
        });
        if stop {
            assert_eq!(result.unwrap_err().code(), DiagnosticCode::Cancelled);
            assert_eq!(operation.ownership_budget().usage().invocations, 1);
            assert!(operation.isolated_in_flight_bytes().unwrap() > 0);
            drop(runtime);
            paused.as_mut().unwrap().resume();
            until(|| operation.isolated_in_flight_bytes() == Some(0));
        } else {
            assert!(matches!(result.unwrap(), Literal::Int(17)));
        }
        assert_eq!(
            operation.ownership_budget().usage(),
            OperationUsage::default()
        );
        workspace.reaped();
    }
}

#[test]
fn guard_configuration_and_failed_startup_never_claim_tree_completion() {
    assert!(WorkerPool::with_process_tree(limits(), "relative".into()).is_err());
    for (helper, worker, expected) in [
        (
            "/botwork-missing-guardian",
            "/bin/true",
            WorkerCleanup::NotStarted,
        ),
        (
            env!("CARGO_BIN_EXE_botwork"),
            "/botwork-missing-worker",
            WorkerCleanup::NotStarted,
        ),
        ("/bin/true", "/bin/true", WorkerCleanup::Unverified),
    ] {
        let pool = WorkerPool::with_process_tree(limits(), helper.into()).unwrap();
        let report = wait(
            pool.start(
                WorkerCommand {
                    executable: worker.into(),
                    arguments: vec![],
                    directory: std::env::temp_dir(),
                    environment: Default::default(),
                },
                vec![],
                OperationControl::default(),
            )
            .unwrap(),
        );
        assert_eq!(
            report.cleanup, expected,
            "helper={helper}, worker={worker}: {report:?}"
        );
        if expected == WorkerCleanup::NotStarted {
            assert!(!report.io_complete);
        }
        assert_ne!(report.outcome, WorkerOutcome::Succeeded);
        assert_eq!(
            pool.snapshot().active.len(),
            usize::from(expected == WorkerCleanup::Unverified)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(["--botwork-worker-guardian-v1", "/bin/true"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn unrelated_host_children_are_untouched() {
    let mut unrelated = OwnedChild(Command::new("/bin/sleep").arg("10").spawn().unwrap());
    let workspace = Workspace::new();
    let pool = pool(limits());
    let report = wait(
        pool.start(
            command(&workspace.0, "exit"),
            vec![],
            OperationControl::default(),
        )
        .unwrap(),
    );
    assert_eq!(report.cleanup, WorkerCleanup::TreeReaped, "{report:?}");
    workspace.reaped();
    assert!(unrelated.0.try_wait().unwrap().is_none());
}

#[test]
fn subprocess_host() {
    let Some(directory) = std::env::var_os("BOTWORK_TREE_TEST_HOST") else {
        return;
    };
    let directory = PathBuf::from(directory);
    if std::env::var_os("BOTWORK_TREE_CLOSED_STDIO").is_some() {
        // This isolated test subprocess owns its inherited standard descriptors.
        unsafe {
            libc::close(0);
            libc::close(1);
            libc::close(2);
        }
    }
    let pool = pool(limits());
    let handle = pool
        .start(
            command(&directory, "wait"),
            vec![],
            OperationControl::default(),
        )
        .unwrap();
    until(|| directory.join("ready").exists());
    if std::env::var_os("BOTWORK_TREE_COPY_CHANNEL").is_some() {
        use std::os::unix::ffi::OsStrExt;
        let release =
            std::ffi::CString::new(directory.join("release-copier").as_os_str().as_bytes())
                .unwrap();
        // The fork child performs only raw syscalls/_exit; it retains inherited
        // channel copies until explicitly released, without running Rust destructors.
        let copied = unsafe { libc::fork() };
        assert!(copied >= 0);
        if copied == 0 {
            for _ in 0..2000 {
                unsafe {
                    if libc::access(release.as_ptr(), libc::F_OK) == 0 {
                        libc::_exit(0);
                    }
                    let delay = libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 5_000_000,
                    };
                    libc::syscall(
                        libc::SYS_nanosleep,
                        &delay,
                        std::ptr::null_mut::<libc::timespec>(),
                    );
                }
            }
            unsafe {
                libc::_exit(0);
            }
        }
        fs::write(directory.join("copier"), copied.to_string()).unwrap();
    }
    fs::write(
        directory.join("guardian"),
        pool.snapshot().active[0].pid.unwrap().to_string(),
    )
    .unwrap();
    let _ = wait(handle);
}

#[test]
fn host_sigterm_and_sigkill_trigger_detached_tree_cleanup() {
    for (signal, copy_channel, closed_stdio) in [
        (libc::SIGTERM, false, false),
        (libc::SIGKILL, false, true),
        (libc::SIGKILL, true, false),
    ] {
        let workspace = Workspace::new();
        let mut specification = Command::new(std::env::current_exe().unwrap());
        if copy_channel {
            specification.env("BOTWORK_TREE_COPY_CHANNEL", "1");
        }
        if closed_stdio {
            specification.env("BOTWORK_TREE_CLOSED_STDIO", "1");
        }
        let mut host = OwnedChild(
            specification
                .args(["--exact", "subprocess_host", "--nocapture"])
                .env("BOTWORK_TREE_TEST_HOST", &workspace.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        until(|| workspace.0.join("guardian").exists());
        let guardian = fs::read_to_string(workspace.0.join("guardian")).unwrap();
        assert_eq!(unsafe { libc::kill(host.0.id() as i32, signal) }, 0);
        until(|| host.0.try_wait().unwrap().is_some());
        until(
            || match fs::read_to_string(format!("/proc/{guardian}/status")) {
                Err(_) => true,
                Ok(status) => status
                    .lines()
                    .any(|line| line.starts_with("State:") && line.contains('Z')),
            },
        );
        // The host's init process owns the orphaned guardian; its workers have all been reaped.
        workspace.reaped();
        if copy_channel {
            let copier = fs::read_to_string(workspace.0.join("copier")).unwrap();
            let status = fs::read_to_string(format!("/proc/{copier}/status")).unwrap();
            assert!(!status
                .lines()
                .any(|line| line.starts_with("State:") && line.contains('Z')));
            fs::write(workspace.0.join("release-copier"), "release").unwrap();
            until(|| {
                fs::read_to_string(format!("/proc/{copier}/status")).map_or(true, |status| {
                    status
                        .lines()
                        .any(|line| line.starts_with("State:") && line.contains('Z'))
                })
            });
        }
    }
}
