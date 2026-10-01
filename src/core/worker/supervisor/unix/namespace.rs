//! clone3 creates the namespace init as a direct, exclusively reaped child.
//! The post-clone branch uses only stack data and async-signal-safe syscalls,
//! then execve/_exit: no allocator, Rust destructors, or inherited host locks.
use super::*;
use std::{
    ffi::CString,
    fs::File,
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
};

fn cstring(value: &[u8]) -> io::Result<CString> {
    CString::new(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Namespace command contains NUL",
        )
    })
}
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut descriptors = [0; 2];
    // SAFETY: descriptors has two valid outputs; pipe2 returns fresh owned FDs.
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe {
        (
            OwnedFd::from_raw_fd(descriptors[0]),
            OwnedFd::from_raw_fd(descriptors[1]),
        )
    })
}

pub(super) fn spawn(
    executable: &Path,
    specification: WorkerCommand,
    remote: i32,
    host: i32,
    record: Option<i32>,
    control: UnixStream,
) -> io::Result<ChildOwner> {
    let executable = cstring(executable.as_os_str().as_bytes())?;
    let directory = cstring(specification.directory.as_os_str().as_bytes())?;
    let mut arguments = vec![
        executable.clone(),
        cstring(if record.is_some() {
            guardian::NAMESPACE_JOURNAL_ARGUMENT.as_bytes()
        } else {
            guardian::NAMESPACE_ARGUMENT.as_bytes()
        })?,
        cstring(specification.executable.as_os_str().as_bytes())?,
    ];
    for argument in &specification.arguments {
        arguments.push(cstring(argument.as_bytes())?);
    }
    let mut environment = Vec::with_capacity(specification.environment.len());
    for (name, value) in &specification.environment {
        if name.as_bytes().contains(&b'=') || name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid namespace environment name",
            ));
        }
        let mut entry = name.as_bytes().to_vec();
        entry.push(b'=');
        entry.extend_from_slice(value.as_bytes());
        environment.push(cstring(&entry)?);
    }
    let argv: Vec<_> = arguments
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let envp: Vec<_> = environment
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let (input, input_writer) = pipe()?;
    let (output_reader, output) = pipe()?;
    let (error_reader, error) = pipe()?;
    // All sources survive dup2 onto 0..=5, including hosts with closed stdio.
    let input = guardian::duplicate(input.as_raw_fd())?;
    let output = guardian::duplicate(output.as_raw_fd())?;
    let error = guardian::duplicate(error.as_raw_fd())?;
    let bootstrap = Bootstrap {
        executable: executable.as_ptr(),
        directory: directory.as_ptr(),
        argv: argv.as_ptr(),
        envp: envp.as_ptr(),
        input: input.as_raw_fd(),
        output: output.as_raw_fd(),
        error: error.as_raw_fd(),
        parent_control: control.as_raw_fd(),
        remote,
        host,
        record,
    };
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    // SAFETY: clone_args contains only integers/pointers; zero initializes unused fields.
    let mut args: libc::clone_args = unsafe { std::mem::zeroed() };
    args.flags = (libc::CLONE_NEWUSER | libc::CLONE_NEWPID | libc::CLONE_NEWNS) as u64;
    args.exit_signal = libc::SIGCHLD as u64;
    let signals = BlockSignals::new()?;
    // No CLONE_VM/CLONE_FILES: the child has private address and descriptor tables.
    let pid = unsafe { libc::syscall(libc::SYS_clone3, &args, std::mem::size_of_val(&args)) };
    if pid == -1 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe { bootstrap.enter() }
    }
    // Establish a kill/reap guard before any mapping I/O can fail or unwind.
    let child = ChildOwner {
        child: process::Process::namespace(
            pid as u32,
            File::from(input_writer),
            File::from(output_reader),
            File::from(error_reader),
        ),
        owned: true,
        guardian: Some(control),
        #[cfg(test)]
        observer: None,
    };
    drop(signals);
    // This exclusively owned, unreaped PID cannot be recycled during these opens.
    // Map only the invoking identity; never grant host-root identity or groups.
    std::fs::write(format!("/proc/{pid}/setgroups"), b"deny")?;
    std::fs::write(format!("/proc/{pid}/uid_map"), format!("{uid} {uid} 1\n"))?;
    std::fs::write(format!("/proc/{pid}/gid_map"), format!("{gid} {gid} 1\n"))?;
    release_gate(child.guardian.as_ref().expect("namespace control"))?;
    Ok(child)
}

fn release_gate(control: &UnixStream) -> io::Result<()> {
    let byte = 1u8;
    loop {
        // A failed bootstrap must not signal the embedding host. The socket is
        // nonblocking and only one byte is sent before any protocol traffic.
        let sent = unsafe {
            libc::send(
                control.as_raw_fd(),
                (&byte as *const u8).cast(),
                1,
                libc::MSG_NOSIGNAL,
            )
        };
        if sent == 1 {
            return Ok(());
        }
        if sent == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

struct BlockSignals {
    previous: [u8; std::mem::size_of::<libc::sigset_t>()],
    bytes: usize,
    armed: bool,
}
impl BlockSignals {
    fn new() -> io::Result<Self> {
        let mut mask = Self {
            previous: [0; std::mem::size_of::<libc::sigset_t>()],
            bytes: (libc::SIGRTMAX() as usize).div_ceil(8),
            armed: false,
        };
        let all = [0xffu8; std::mem::size_of::<libc::sigset_t>()];
        if mask.bytes > all.len() {
            return Err(io::Error::other("Unsupported kernel signal mask size"));
        }
        // Use the kernel mask size and include libc's reserved thread signals.
        // The child must not invoke any inherited handler before exec. SIGKILL
        // and SIGSTOP cannot be blocked; kernel parent-death cleanup still works.
        if unsafe {
            libc::syscall(
                libc::SYS_rt_sigprocmask,
                libc::SIG_SETMASK,
                all.as_ptr(),
                mask.previous.as_mut_ptr(),
                mask.bytes,
            )
        } == -1
        {
            return Err(io::Error::last_os_error());
        }
        mask.armed = true;
        Ok(mask)
    }
}
impl Drop for BlockSignals {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Parent only: the child's branch always execs or _exits. These are the
        // same valid arguments/size that successfully captured this thread mask.
        unsafe {
            libc::syscall(
                libc::SYS_rt_sigprocmask,
                libc::SIG_SETMASK,
                self.previous.as_ptr(),
                std::ptr::null_mut::<u8>(),
                self.bytes,
            );
        }
    }
}

struct Bootstrap {
    executable: *const libc::c_char,
    directory: *const libc::c_char,
    argv: *const *const libc::c_char,
    envp: *const *const libc::c_char,
    input: i32,
    output: i32,
    error: i32,
    parent_control: i32,
    remote: i32,
    host: i32,
    record: Option<i32>,
}
impl Bootstrap {
    /// Called only in clone3's child, before any Rust runtime reinitialization.
    unsafe fn enter(&self) -> ! {
        // The creating OS-owner thread outlives the invocation. Kernel death
        // signalling also handles a stopped guardian when that thread/host dies.
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) == -1 {
            self.fail(*libc::__errno_location());
        }
        libc::close(self.parent_control);
        // Close the race where the whole host died before PDEATHSIG was armed.
        let mut host = libc::pollfd {
            fd: self.host,
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let result = libc::poll(&mut host, 1, 0);
            if result == 0 {
                break;
            }
            if result < 0 && *libc::__errno_location() == libc::EINTR {
                continue;
            }
            self.fail(libc::ECANCELED);
        }
        let mut byte = 0u8;
        loop {
            let count = libc::read(self.remote, (&mut byte as *mut u8).cast(), 1);
            if count == 1 && byte == 1 {
                break;
            }
            if count < 0 && *libc::__errno_location() == libc::EINTR {
                continue;
            }
            self.fail(libc::ECANCELED);
        }
        if libc::setsid() == -1
            || libc::mount(
                std::ptr::null(),
                c"/".as_ptr(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ) == -1
            || libc::mount(
                c"proc".as_ptr(),
                c"/proc".as_ptr(),
                c"proc".as_ptr(),
                libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                std::ptr::null(),
            ) == -1
            || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == -1
            || libc::chdir(self.directory) == -1
            || libc::dup2(self.input, 0) == -1
            || libc::dup2(self.output, 1) == -1
            || libc::dup2(self.error, 2) == -1
            || libc::dup2(self.remote, 3) == -1
            || libc::dup2(self.host, 4) == -1
        {
            self.fail(*libc::__errno_location());
        }
        if let Some(record) = self.record {
            if libc::dup2(record, 5) == -1 {
                self.fail(*libc::__errno_location());
            }
        }
        libc::execve(self.executable, self.argv, self.envp);
        self.fail(*libc::__errno_location());
    }

    unsafe fn fail(&self, errno: i32) -> ! {
        // The private endpoint is a stable source above every dup2 target, even
        // when bootstrap failed before descriptor placement. Fixed stack frame;
        // no formatting, allocation, unwinding, or inherited host locks.
        let frame = guardian::frame(1, 0, errno);
        let mut offset = 0;
        while offset < frame.len() {
            let count = libc::write(
                self.remote,
                frame.as_ptr().add(offset).cast(),
                frame.len() - offset,
            );
            if count > 0 {
                offset += count as usize;
            } else if count < 0 && *libc::__errno_location() == libc::EINTR {
                continue;
            } else {
                break;
            }
        }
        libc::_exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_startup_gate_does_not_deliver_sigpipe_to_the_host() {
        const MARKER: &str = "BOTWORK_TEST_NAMESPACE_SIGPIPE";
        if std::env::var_os(MARKER).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "core::worker::supervisor::unix::namespace::tests::failed_startup_gate_does_not_deliver_sigpipe_to_the_host"])
                .env(MARKER, "1").stdout(Stdio::null()).status().unwrap();
            assert!(
                status.success(),
                "startup gate terminated the host: {status}"
            );
            return;
        }
        // Isolated fixture process: do not change the test runner's dispositions.
        assert_ne!(
            unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) },
            libc::SIG_ERR
        );
        let (control, peer) = UnixStream::pair().unwrap();
        drop(peer);
        assert_eq!(
            release_gate(&control).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}
