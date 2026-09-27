//! A standalone subreaper owns one invocation, never unrelated host children.
use super::super::journal::{self, GuardianReceipt};
use super::*;
use std::{
    fs::File,
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::{net::UnixStream, process::ExitStatusExt},
    },
    path::Path,
};

const ARGUMENT: &str = "--botwork-worker-guardian-v1";
const JOURNAL_ARGUMENT: &str = "--botwork-worker-guardian-journal-v1";
const CONTROL_FD: i32 = 3;
const HOST_FD: i32 = 4;
const JOURNAL_FD: i32 = 5;
const FRAME_BYTES: usize = 13;
const MAGIC: &[u8; 4] = b"BWG1";

pub(super) fn spawn(
    executable: &Path,
    specification: WorkerCommand,
    record: Option<&File>,
) -> io::Result<ChildOwner> {
    // Open our own process identity while it cannot have been recycled.
    let raw_host = unsafe { libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0) };
    if raw_host == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pidfd_open returned a new descriptor.
    let host = unsafe { OwnedFd::from_raw_fd(raw_host as i32) };
    let passed_host = duplicate(host.as_raw_fd())?;
    drop(host);
    let (control, remote) = UnixStream::pair()?;
    let passed_remote = duplicate(remote.as_raw_fd())?;
    drop(remote);
    control.set_nonblocking(true)?;
    // Sources are above stdio and both reserved targets, even if the host closed stdio.
    let remote_fd = passed_remote.as_raw_fd();
    let host_fd = passed_host.as_raw_fd();
    let passed_record = record.map(|file| duplicate(file.as_raw_fd())).transpose()?;
    let record_fd = passed_record.as_ref().map(AsRawFd::as_raw_fd);
    let mut command = Command::new(executable);
    command
        .arg(if record.is_some() {
            JOURNAL_ARGUMENT
        } else {
            ARGUMENT
        })
        .arg(&specification.executable)
        .args(&specification.arguments)
        .current_dir(&specification.directory)
        .env_clear()
        .envs(&specification.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // SAFETY: after fork this closure uses only async-signal-safe descriptor calls.
    // Both passed descriptors remain owned until spawn returns, including on failure.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(remote_fd, CONTROL_FD) == -1 || libc::dup2(host_fd, HOST_FD) == -1 {
                return Err(io::Error::last_os_error());
            }
            if record_fd.is_some_and(|fd| libc::dup2(fd, JOURNAL_FD) == -1) {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    Ok(ChildOwner {
        child,
        owned: true,
        guardian: Some(control),
        #[cfg(test)]
        observer: None,
    })
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Guardian completion was not verified",
    )
}

pub(super) fn completion(control: &mut UnixStream, status: ExitStatus) -> io::Result<Completion> {
    if !status.success() {
        return Err(io::Error::other(format!("Guardian exited with {status}")));
    }
    let mut frame = [0; FRAME_BYTES];
    control.read_exact(&mut frame)?;
    let mut trailing = [0];
    if control.read(&mut trailing)? != 0 || &frame[..4] != MAGIC {
        return Err(invalid());
    }
    decode(&frame)
}

fn decode(frame: &[u8; FRAME_BYTES]) -> io::Result<Completion> {
    if &frame[..4] != MAGIC {
        return Err(invalid());
    }
    let raw = i32::from_le_bytes(frame[5..9].try_into().unwrap());
    let errno = i32::from_le_bytes(frame[9..13].try_into().unwrap());
    let valid_status = (0..=0xff00).contains(&raw)
        && ((libc::WIFEXITED(raw) && raw & 0xff == 0)
            || (raw <= 0xff && libc::WIFSIGNALED(raw) && libc::WTERMSIG(raw) <= libc::SIGRTMAX()));
    match frame[4] {
        0 | 2
            if valid_status && ((frame[4] == 0 && errno == 0) || (frame[4] == 2 && errno > 0)) =>
        {
            Ok(Completion {
                status: Some(ExitStatus::from_raw(raw)),
                cleanup: WorkerCleanup::TreeReaped,
                error: (frame[4] == 2).then(|| {
                    runtime(format_args!(
                        "Worker tree cleanup encountered an error: {}",
                        io::Error::from_raw_os_error(errno)
                    ))
                }),
            })
        }
        1 if raw == 0 && errno > 0 => Ok(Completion {
            status: None,
            cleanup: WorkerCleanup::NotStarted,
            error: Some(runtime(format_args!(
                "Starting guarded worker failed: {}",
                io::Error::from_raw_os_error(errno)
            ))),
        }),
        _ => Err(invalid()),
    }
}

// Duplicate the inherited descriptor into a fresh Rust owner. Do not construct
// an OwnedFd from a descriptor that another embedding caller might already own.
fn duplicate(source: i32) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::fcntl(source, libc::F_DUPFD_CLOEXEC, 6) };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl returned a new, uniquely owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn control() -> io::Result<(UnixStream, OwnedFd)> {
    let owned = duplicate(CONTROL_FD)?;
    let host = duplicate(HOST_FD)?;
    let fd = owned.as_raw_fd();
    let mut domain: libc::c_int = 0;
    let mut size = std::mem::size_of_val(&domain) as libc::socklen_t;
    // SAFETY: owned is live, and domain/size are valid getsockopt outputs.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_DOMAIN,
            (&mut domain as *mut libc::c_int).cast(),
            &mut size,
        )
    } == -1
        || domain != libc::AF_UNIX
    {
        return Err(invalid());
    }
    // No worker exec may inherit either copy of the guardian's liveness channel.
    if unsafe { libc::fcntl(CONTROL_FD, libc::F_SETFD, libc::FD_CLOEXEC) } == -1
        || unsafe { libc::fcntl(HOST_FD, libc::F_SETFD, libc::FD_CLOEXEC) } == -1
    {
        return Err(io::Error::last_os_error());
    }
    let control = UnixStream::from(owned);
    control.set_nonblocking(true)?;
    Ok((control, host))
}

pub(crate) fn entry() -> Option<u8> {
    let mut args = std::env::args_os().skip(1);
    let journaled = match args.next().as_deref() {
        Some(arg) if arg == std::ffi::OsStr::new(ARGUMENT) => false,
        Some(arg) if arg == std::ffi::OsStr::new(JOURNAL_ARGUMENT) => true,
        _ => return None,
    };
    Some(match control() {
        Ok((mut control, host)) => {
            let record = if journaled {
                let result = duplicate(JOURNAL_FD).and_then(|owned| {
                    if unsafe { libc::fcntl(JOURNAL_FD, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    let file = File::from(owned);
                    journal::guardian_file(&file).map(|id| (file, id))
                });
                match result {
                    Ok(record) => Some(record),
                    Err(_) => return Some(2),
                }
            } else {
                None
            };
            let result = run(&mut control, &host, args);
            match result {
                Ok(frame) => {
                    if let Some((file, id)) = record {
                        let errno = i32::from_le_bytes(frame[9..13].try_into().unwrap());
                        let receipt = if frame[4] == 1 {
                            GuardianReceipt::NotStarted { errno }
                        } else {
                            GuardianReceipt::TreeSettled {
                                exit_status: i32::from_le_bytes(frame[5..9].try_into().unwrap()),
                                errno,
                            }
                        };
                        if journal::receipt(&file, id, receipt).is_err() {
                            return Some(1);
                        }
                    }
                    let _ = control.write_all(&frame);
                    0
                }
                Err(_) => 1, // Missing acknowledgment quarantines the host's slot.
            }
        }
        Err(_) => 2,
    })
}

fn frame(kind: u8, status: i32, errno: i32) -> [u8; FRAME_BYTES] {
    let mut frame = [0; FRAME_BYTES];
    frame[..4].copy_from_slice(MAGIC);
    frame[4] = kind;
    frame[5..9].copy_from_slice(&status.to_le_bytes());
    frame[9..].copy_from_slice(&errno.to_le_bytes());
    frame
}
fn errno(error: &io::Error) -> i32 {
    error.raw_os_error().unwrap_or(libc::EIO)
}

fn stopped(control: &mut UnixStream, host: &OwnedFd) -> bool {
    let mut process = libc::pollfd {
        fd: host.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: host remains live and process is a valid poll descriptor.
    let ready = unsafe { libc::poll(&mut process, 1, 0) };
    if ready > 0 || (ready == -1 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted)
    {
        return true;
    }
    let mut byte = [0];
    match control.read(&mut byte) {
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            false
        }
        _ => true, // EOF is cancellation/host death; unexpected data also stops entry.
    }
}

fn prepare() -> io::Result<File> {
    // SAFETY: no pointers are passed to prctl. The standalone guardian has not
    // created any children or threads; resetting SIGCHLD preserves wait ownership.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::signal(libc::SIGCHLD, libc::SIG_DFL) } == libc::SIG_ERR {
        return Err(io::Error::last_os_error());
    }
    File::open("/proc/thread-self/children")
}

fn run(
    control: &mut UnixStream,
    host: &OwnedFd,
    mut args: impl Iterator<Item = OsString>,
) -> io::Result<[u8; FRAME_BYTES]> {
    let Some(executable) = args.next() else {
        return Ok(frame(1, 0, libc::EINVAL));
    };
    if !Path::new(&executable).is_absolute() {
        return Ok(frame(1, 0, libc::EINVAL));
    }
    if let Err(error) = prepare() {
        return Ok(frame(1, 0, errno(&error)));
    }
    if stopped(control, host) {
        return Ok(frame(1, 0, libc::ECANCELED));
    }
    // The child inherits only the configured environment/cwd and data streams.
    // Control descriptors are close-on-exec. The guardian remains its subreaper.
    let child = match Command::new(executable)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return Ok(frame(1, 0, errno(&error))),
    };
    let primary = child.id() as libc::pid_t;
    drop(child); // waitpid below is the exclusive reaper, including orphaned descendants.
    let mut primary_status = None;
    let mut cleaning = false;
    let mut failure = None;
    loop {
        cleaning |= stopped(control, host);
        if cleaning {
            if let Err(error) = kill_children() {
                failure.get_or_insert(errno(&error));
            }
        }
        // Bound each reap batch so a stream of short-lived children cannot starve liveness checks.
        for _ in 0..64 {
            let mut status = 0;
            // SAFETY: status is a valid output; this standalone process owns all its children.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::__WALL) };
            if pid > 0 {
                if pid == primary && primary_status.is_none() {
                    primary_status = Some(status);
                    cleaning = true;
                }
            } else if pid == 0 {
                break;
            } else {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    // Only this kernel result proves the entire adopted tree has settled.
                    let status = primary_status.ok_or_else(invalid)?;
                    return Ok(frame(
                        if failure.is_some() { 2 } else { 0 },
                        status,
                        failure.unwrap_or(0),
                    ));
                }
                if error.kind() != io::ErrorKind::Interrupted {
                    failure.get_or_insert(errno(&error));
                }
                break;
            }
        }
        std::thread::sleep(QUANTUM);
    }
}

fn kill_children() -> io::Result<()> {
    visit_children(File::open("/proc/thread-self/children")?, kill_child)
}

fn visit_children(
    mut children: impl Read,
    mut signal: impl FnMut(i32) -> io::Result<()>,
) -> io::Result<()> {
    let mut failure = None;
    let mut attempt = |pid| {
        if let Err(error) = signal(pid) {
            failure.get_or_insert(error);
        }
    };
    let mut buffer = [0u8; CHUNK];
    let mut pid = 0i32;
    loop {
        let count = match children.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            other => other?,
        };
        for &byte in &buffer[..count] {
            if byte.is_ascii_whitespace() {
                if pid != 0 {
                    attempt(pid);
                    pid = 0;
                }
            } else if byte.is_ascii_digit() {
                pid = pid
                    .checked_mul(10)
                    .and_then(|value| value.checked_add(i32::from(byte - b'0')))
                    .ok_or_else(invalid)?;
            } else {
                return Err(invalid());
            }
        }
        if count == 0 {
            if pid != 0 {
                attempt(pid);
            }
            return failure.map_or(Ok(()), Err);
        }
    }
}
fn kill_child(pid: libc::pid_t) -> io::Result<()> {
    // Validate current child ownership without reaping. PID identity remains
    // reserved until our later waitpid batch; no stale process-list PID is signalled.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let observed = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as u32,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT | libc::__WALL,
        )
    };
    if observed == -1 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ECHILD) {
            Ok(())
        } else {
            Err(error)
        };
    }
    if unsafe { libc::kill(pid, libc::SIGKILL) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
