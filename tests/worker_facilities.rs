//! Real, inherited syscall refusals; each filter lives in a disposable process.
#![cfg(target_os = "linux")]
use botwork::core::{
    diagnostic::DiagnosticCode,
    operation::OperationControl,
    worker::{
        journal::WorkerJournal, WorkerCleanup, WorkerCommand, WorkerLimits, WorkerOutcome,
        WorkerPool,
    },
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
    let executable = std::env::current_exe().unwrap();
    let mut command = if facility.starts_with("proc-") || facility == "namespace-quota" {
        // unshare configures the user namespace while still single-threaded,
        // before the Rust test harness starts its fixture thread.
        let mut command = Command::new("/usr/bin/unshare");
        command
            .args([
                "--user",
                "--map-current-user",
                "--keep-caps",
                "--mount",
                "--propagation",
                "private",
                "--",
            ])
            .arg(executable);
        command.env(
            "BOTWORK_PARENT_MOUNT",
            fs::read_link("/proc/self/ns/mnt").unwrap(),
        );
        command.env(
            "BOTWORK_PARENT_USER",
            fs::read_link("/proc/self/ns/user").unwrap(),
        );
        command
    } else {
        Command::new(executable)
    };
    let mut fixture = Fixture(
        command
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

#[test]
fn hidden_procfs_refuses_tree_modes_before_entry() {
    for mode in MODES {
        check("proc-hidden", mode, libc::ENOENT);
    }
}

#[test]
fn read_only_procfs_refuses_namespace_mapping_before_entry() {
    for mode in MODES {
        check("proc-read-only", mode, libc::EROFS);
    }
}

#[test]
fn an_exhausted_namespace_quota_refuses_the_namespace_mode_before_entry() {
    for mode in MODES {
        check("namespace-quota", mode, libc::ENOSPC);
    }
}

#[test]
fn exhausted_descriptors_refuse_all_modes_without_leaking_ownership() {
    for mode in MODES {
        check("descriptors", mode, libc::EMFILE);
    }
}

fn required(facility: &str, mode: &str) -> bool {
    match facility {
        "none" => false,
        "execve" | "descriptors" => true,
        "pidfd" | "socketpair" | "subreaper" | "proc-hidden" => mode != "group",
        _ => mode == "namespace",
    }
}

struct DescriptorLimit(libc::rlimit);
impl DescriptorLimit {
    fn exhaust() -> Self {
        let mut previous = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut previous) },
            0
        );
        let limited = libc::rlimit {
            rlim_cur: 0,
            rlim_max: previous.rlim_max,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limited) }, 0);
        Self(previous)
    }
}
impl Drop for DescriptorLimit {
    fn drop(&mut self) {
        // Restore this process's soft limit before writing its verification file.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &self.0) }, 0);
    }
}

/// Allow this process's descendants no user namespaces: a real quota, as a
/// host sets with `user.max_user_namespaces`. The process is the root of a user
/// namespace unshare made, so the limit binds only what it starts.
fn exhaust_namespace_quota() {
    let parent = PathBuf::from(std::env::var_os("BOTWORK_PARENT_USER").unwrap());
    assert_ne!(fs::read_link("/proc/self/ns/user").unwrap(), parent);
    let quota = "/proc/sys/user/max_user_namespaces";
    fs::write(quota, "0").unwrap();
    assert_eq!(fs::read_to_string(quota).unwrap().trim(), "0");
}

fn restrict_proc(facility: &str) {
    let parent_mount = PathBuf::from(std::env::var_os("BOTWORK_PARENT_MOUNT").unwrap());
    assert_ne!(fs::read_link("/proc/self/ns/mnt").unwrap(), parent_mount);
    // The executable was started by unshare with private propagation. Bind
    // remount flags affect only this mount, never the proc superblock globally.
    let result = if facility == "proc-hidden" {
        unsafe {
            libc::mount(
                c"tmpfs".as_ptr(),
                c"/proc".as_ptr(),
                c"tmpfs".as_ptr(),
                libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                c"size=4096".as_ptr().cast(),
            )
        }
    } else {
        assert_eq!(
            unsafe {
                libc::mount(
                    c"/proc".as_ptr(),
                    c"/proc".as_ptr(),
                    std::ptr::null(),
                    libc::MS_BIND | libc::MS_REC,
                    std::ptr::null(),
                )
            },
            0,
            "{}",
            io::Error::last_os_error()
        );
        unsafe {
            libc::mount(
                std::ptr::null(),
                c"/proc".as_ptr(),
                std::ptr::null(),
                libc::MS_REMOUNT
                    | libc::MS_BIND
                    | libc::MS_RDONLY
                    | libc::MS_NOSUID
                    | libc::MS_NODEV
                    | libc::MS_NOEXEC,
                std::ptr::null(),
            )
        }
    };
    assert_eq!(result, 0, "{}", io::Error::last_os_error());
    if facility == "proc-hidden" {
        assert_eq!(
            fs::read("/proc/thread-self/children")
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOENT)
        );
    } else {
        // Open only: do not attempt to alter the already installed identity map.
        assert_eq!(
            fs::OpenOptions::new()
                .write(true)
                .open("/proc/self/uid_map")
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EROFS)
        );
        assert!(fs::read("/proc/thread-self/children").unwrap().is_empty());
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
    let descriptor_limit = if facility == "descriptors" {
        Some(DescriptorLimit::exhaust())
    } else {
        if facility.starts_with("proc-") {
            restrict_proc(&facility);
        } else if facility == "namespace-quota" {
            exhaust_namespace_quota();
        } else if facility != "none" {
            deny(&facility, errno);
        }
        None
    };
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
    drop(descriptor_limit);
    fs::write(directory.join("verified"), b"2").unwrap();
}

/// A journal needs a writable local filesystem: on a read-only one it is
/// refused when opened, whether its directory exists or not, so no worker
/// starts without the records it was promised.
#[test]
fn a_journal_on_a_read_only_filesystem_is_refused_when_opened() {
    let workspace = Workspace::new();
    // A journal directory as the journal makes one: private to its owner.
    std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700)
        .create(workspace.0.join("existing"))
        .unwrap();
    let status = Command::new("/usr/bin/unshare")
        .args([
            "--user",
            "--map-current-user",
            "--keep-caps",
            "--mount",
            "--propagation",
            "private",
            "--",
        ])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "subprocess_read_only_journal", "--nocapture"])
        .env("BOTWORK_JOURNAL_DIRECTORY", &workspace.0)
        .env(
            "BOTWORK_PARENT_MOUNT",
            fs::read_link("/proc/self/ns/mnt").unwrap(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .unwrap();
    assert!(status.success(), "{status}");
    assert_eq!(fs::read(workspace.0.join("verified")).unwrap(), b"2");
}

#[test]
fn subprocess_read_only_journal() {
    let Some(directory) = std::env::var_os("BOTWORK_JOURNAL_DIRECTORY") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let parent_mount = PathBuf::from(std::env::var_os("BOTWORK_PARENT_MOUNT").unwrap());
    assert_ne!(fs::read_link("/proc/self/ns/mnt").unwrap(), parent_mount);
    let target = std::ffi::CString::new(directory.as_os_str().as_encoded_bytes()).unwrap();
    // A read-only bind of the workspace, in this process's private mounts only.
    unsafe {
        assert_eq!(
            libc::mount(
                target.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            ),
            0,
            "{}",
            io::Error::last_os_error()
        );
        // A user namespace may not clear the flags its mount inherited
        // locked, so the remount keeps them. musl's bindings lack
        // ST_RELATIME, which Linux defines as 0x1000.
        const ST_RELATIME: libc::c_ulong = 0x1000;
        let mut status: libc::statvfs = std::mem::zeroed();
        assert_eq!(libc::statvfs(target.as_ptr(), &mut status), 0);
        let mut flags = libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY;
        for (kept, flag) in [
            (libc::ST_NOSUID, libc::MS_NOSUID),
            (libc::ST_NODEV, libc::MS_NODEV),
            (libc::ST_NOEXEC, libc::MS_NOEXEC),
            (libc::ST_NOATIME, libc::MS_NOATIME),
            (libc::ST_NODIRATIME, libc::MS_NODIRATIME),
            (ST_RELATIME, libc::MS_RELATIME),
        ] {
            if status.f_flag & kept != 0 {
                flags |= flag;
            }
        }
        assert_eq!(
            libc::mount(
                std::ptr::null(),
                target.as_ptr(),
                std::ptr::null(),
                flags,
                std::ptr::null(),
            ),
            0,
            "{}",
            io::Error::last_os_error()
        );
    }
    let mut refused = 0;
    for name in ["missing", "existing"] {
        match WorkerJournal::open(&directory.join(name), NonZeroUsize::new(4).unwrap()) {
            Ok(_) => panic!("a journal opened on a read-only filesystem at {name}"),
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(libc::EROFS), "{name}: {error}");
                refused += 1;
            }
        }
    }
    // Written beside the read-only bind, through the parent's view of it.
    unsafe {
        assert_eq!(libc::umount2(target.as_ptr(), 0), 0);
    }
    fs::write(directory.join("verified"), refused.to_string()).unwrap();
}
