//! Real, inherited syscall refusals; each filter lives in a disposable process.
#![cfg(target_os = "linux")]
use botwork::core::{
    diagnostic::DiagnosticCode,
    operation::OperationControl,
    worker::{WorkerCleanup, WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool},
};
use std::{
    fs, io,
    num::NonZeroUsize,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

const MODES: [&str; 3] = ["group", "guardian", "namespace"];

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "botwork-facilities-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Fixture(Child);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn check(facility: &str, mode: &str, errno: i32) {
    let workspace = Workspace::new();
    let mut fixture = Fixture(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "subprocess_facility", "--nocapture"])
            .env("BOTWORK_FACILITY", facility)
            .env("BOTWORK_FACILITY_MODE", mode)
            .env("BOTWORK_FACILITY_ERRNO", errno.to_string())
            .env("BOTWORK_FACILITY_DIRECTORY", &workspace.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let end = Instant::now() + Duration::from_secs(12);
    let status = loop {
        if let Some(status) = fixture.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < end, "{facility}/{mode}/{errno} stalled");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(status.success(), "{facility}/{mode}/{errno}: {status}");
    // The fixture writes this only after both invocations and cleanup assertions.
    assert_eq!(fs::read(workspace.0.join("verified")).unwrap(), b"2");
}

fn matrix(facility: &str) {
    for mode in MODES {
        for errno in [libc::ENOSYS, libc::EPERM] {
            // glibc can need clone3 for threads/ordinary spawn. ENOSYS exercises
            // its supported fallback; EPERM is not an unrelated-mode promise.
            if facility == "clone3" && errno == libc::EPERM && mode != "namespace" {
                continue;
            }
            check(facility, mode, errno);
        }
    }
}

macro_rules! facility_tests {
    ($($test:ident => $facility:literal),+ $(,)?) => {$(
        #[test]
        fn $test() { matrix($facility); }
    )+};
}
facility_tests! {
    pid_descriptors => "pidfd",
    unix_socket_pairs => "socketpair",
    child_subreapers => "subreaper",
    namespace_creation => "clone3",
    private_mount_propagation => "mount-private",
    private_proc_mount => "mount-proc",
    parent_death_signalling => "pdeathsig",
    no_new_privileges => "no-new-privs",
    namespace_session => "setsid",
    executable_entry => "execve",
}

#[test]
fn enabled_facilities_run_in_every_mode() {
    for mode in MODES {
        check("none", mode, 0);
    }
}

fn required(facility: &str, mode: &str) -> bool {
    match facility {
        "none" => false,
        "execve" => true,
        "pidfd" | "socketpair" | "subreaper" => mode != "group",
        _ => mode == "namespace",
    }
}

// This is fault injection for these native executables, not a sandbox policy.
// Argument comparisons distinguish prctl options and the two mount operations.
fn deny(facility: &str, errno: i32) {
    let (syscall, argument) = match facility {
        "pidfd" => (libc::SYS_pidfd_open, None),
        "socketpair" => (libc::SYS_socketpair, None),
        "subreaper" => (
            libc::SYS_prctl,
            Some((0, libc::PR_SET_CHILD_SUBREAPER as u32)),
        ),
        "clone3" => (libc::SYS_clone3, None),
        "mount-private" => (
            libc::SYS_mount,
            Some((3, (libc::MS_REC | libc::MS_PRIVATE) as u32)),
        ),
        "mount-proc" => (
            libc::SYS_mount,
            Some((
                3,
                (libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC) as u32,
            )),
        ),
        "pdeathsig" => (libc::SYS_prctl, Some((0, libc::PR_SET_PDEATHSIG as u32))),
        "no-new-privs" => (libc::SYS_prctl, Some((0, libc::PR_SET_NO_NEW_PRIVS as u32))),
        "setsid" => (libc::SYS_setsid, None),
        "execve" => (libc::SYS_execve, None),
        _ => panic!("unknown facility {facility}"),
    };
    fn instruction(code: u32, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
        libc::sock_filter {
            code: code as u16,
            jt,
            jf,
            k,
        }
    }
    let mut filter = vec![
        instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
        instruction(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            0,
            if argument.is_some() { 3 } else { 1 },
            syscall as u32,
        ),
    ];
    if let Some((index, value)) = argument {
        let low_word = if cfg!(target_endian = "big") { 4 } else { 0 };
        filter.push(instruction(
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
            0,
            0,
            16 + index * 8 + low_word,
        ));
        filter.push(instruction(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            0,
            1,
            value,
        ));
    }
    filter.push(instruction(
        libc::BPF_RET | libc::BPF_K,
        0,
        0,
        libc::SECCOMP_RET_ERRNO | errno as u32,
    ));
    filter.push(instruction(
        libc::BPF_RET | libc::BPF_K,
        0,
        0,
        libc::SECCOMP_RET_ALLOW,
    ));
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
        0
    );
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) },
        0
    );
    // Prove the selected rule fires with exactly the requested errno. Null
    // pointers prevent this probe from entering an executable/creating a child.
    let mut args = [0 as libc::c_ulong; 6];
    if let Some((index, value)) = argument {
        args[index as usize] = value as libc::c_ulong;
    }
    assert_eq!(
        unsafe {
            libc::syscall(
                syscall, args[0], args[1], args[2], args[3], args[4], args[5],
            )
        },
        -1
    );
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(errno));
}

#[test]
fn subprocess_facility() {
    let Ok(facility) = std::env::var("BOTWORK_FACILITY") else {
        return;
    };
    let mode = std::env::var("BOTWORK_FACILITY_MODE").unwrap();
    let errno: i32 = std::env::var("BOTWORK_FACILITY_ERRNO")
        .unwrap()
        .parse()
        .unwrap();
    let directory = PathBuf::from(std::env::var_os("BOTWORK_FACILITY_DIRECTORY").unwrap());
    let limits = WorkerLimits {
        max_in_flight: NonZeroUsize::new(1).unwrap(),
        timeout: Duration::from_secs(3),
        cleanup_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_botwork"));
    let pool = match mode.as_str() {
        "group" => WorkerPool::new(limits),
        "guardian" => WorkerPool::with_process_tree(limits, helper),
        "namespace" => WorkerPool::with_pid_namespace(limits, helper, None),
        _ => panic!("unknown mode {mode}"),
    }
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    if facility != "none" {
        deny(&facility, errno);
    }
    let refuses = required(&facility, &mode);
    for attempt in 0..2 {
        let marker = directory.join(format!("entered-{attempt}"));
        let command = WorkerCommand {
            executable: "/usr/bin/touch".into(),
            arguments: vec![marker.clone().into_os_string()],
            directory: directory.clone(),
            environment: Default::default(),
        };
        match pool.start(command, vec![], OperationControl::default()) {
            Err(error) => {
                // A global clone3 EPERM can prevent creating the supervisor
                // thread itself. This is still refusal before worker effects.
                assert!(
                    refuses && facility == "clone3" && errno == libc::EPERM,
                    "{error}"
                );
                assert_eq!(error.code(), DiagnosticCode::AsyncRuntime);
                assert!(error
                    .to_string()
                    .contains(&io::Error::from_raw_os_error(errno).to_string()));
            }
            Ok(handle) => {
                let report = runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(5), handle.wait())
                        .await
                        .unwrap()
                });
                assert!(report.progress_complete, "{report:?}");
                if refuses {
                    assert_eq!(report.outcome, WorkerOutcome::Failed, "{report:?}");
                    if facility == "pdeathsig" {
                        // Exit before consuming the startup byte can reset the
                        // socket; only owned namespace reaping is then proven.
                        assert!(
                            matches!(
                                report.cleanup,
                                WorkerCleanup::NotStarted | WorkerCleanup::NamespaceReaped
                            ),
                            "{report:?}"
                        );
                    } else {
                        assert_eq!(report.cleanup, WorkerCleanup::NotStarted, "{report:?}");
                    }
                    assert!(report.exit_status.is_none());
                    let error = report.diagnostic.unwrap();
                    assert_eq!(error.code(), DiagnosticCode::AsyncRuntime);
                    // PDEATHSIG setup precedes the mapping gate. Its refusal
                    // can race the parent's map writes/socket release, which
                    // then report their own kernel error instead of the frame.
                    if facility != "pdeathsig" {
                        assert!(
                            error
                                .to_string()
                                .contains(&io::Error::from_raw_os_error(errno).to_string()),
                            "{error}"
                        );
                    }
                } else {
                    assert_eq!(report.outcome, WorkerOutcome::Succeeded, "{report:?}");
                    assert_eq!(
                        report.cleanup,
                        if mode == "group" {
                            WorkerCleanup::Reaped
                        } else {
                            WorkerCleanup::TreeReaped
                        }
                    );
                    assert!(report.exit_status.unwrap().success());
                    assert!(report.io_complete);
                    assert!(report.diagnostic.is_none());
                }
            }
        }
        assert_eq!(marker.exists(), !refuses);
        assert!(pool.snapshot().active.is_empty());
        // Exclusive ownership leaves neither a live child nor a zombie here.
        assert_eq!(
            unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
    assert!(pool
        .shutdown_wait(Duration::ZERO)
        .unwrap()
        .active
        .is_empty());
    fs::write(directory.join("verified"), b"2").unwrap();
}
