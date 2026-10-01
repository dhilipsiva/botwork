//! The Windows backend: each worker runs in its own Job Object, assigned while
//! the worker is still suspended, so termination reaches every descendant and a
//! lost host closes the job and takes them with it.
use super::observation::Observation;
use super::*;
use std::{
    ffi::c_void,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
        process::CommandExt,
    },
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_NO_MORE_FILES, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    },
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Pipes::{CreatePipe, SetNamedPipeHandleState, PIPE_NOWAIT},
        Threading::{
            OpenThread, ResumeThread, WaitForSingleObject, CREATE_NEW_PROCESS_GROUP,
            CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
        },
    },
};

pub(super) type Stdin = Pipe;
pub(super) type Output = Pipe;

/// The exit code of a worker the supervisor terminated.
const TERMINATED: u32 = 1;

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

fn raw(handle: &impl AsRawHandle) -> HANDLE {
    handle.as_raw_handle() as HANDLE
}

/// One end of an anonymous pipe, read and written without waiting. `std`'s
/// readers take `ERROR_NO_DATA`, which an empty pipe reports in this mode, for
/// the end of the stream, so the calls are made here.
pub(super) struct Pipe(OwnedHandle);

/// Bytes a pipe holds before a writer must wait.
const PIPE_BUFFER: u32 = 64 * 1024;

/// A new anonymous pipe, as its read and write ends. Neither end is
/// inheritable; `std` duplicates the child's end for it. These pipes, unlike the
/// overlapped ones `std` creates, can be switched to not waiting.
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write) = (null_mut(), null_mut());
    // SAFETY: both outputs are valid; default security makes them private.
    if unsafe { CreatePipe(&mut read, &mut write, null(), PIPE_BUFFER) } == 0 {
        return Err(last_error());
    }
    // SAFETY: two new handles that nothing else owns.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read as RawHandle),
            OwnedHandle::from_raw_handle(write as RawHandle),
        )
    })
}

impl Read for Pipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let mut count = 0;
        // SAFETY: the handle is open, and the buffer holds `length` bytes.
        let read = unsafe {
            ReadFile(
                raw(&self.0),
                buffer.as_mut_ptr(),
                length,
                &mut count,
                null_mut(),
            )
        };
        if read != 0 {
            return Ok(count as usize);
        }
        // SAFETY: reads the calling thread's last error.
        match unsafe { GetLastError() } {
            ERROR_NO_DATA => Err(io::ErrorKind::WouldBlock.into()),
            ERROR_BROKEN_PIPE => Ok(0),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
}

impl Write for Pipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let mut count = 0;
        // SAFETY: the handle is open, and the buffer holds `length` bytes.
        let written = unsafe {
            WriteFile(
                raw(&self.0),
                buffer.as_ptr(),
                length,
                &mut count,
                null_mut(),
            )
        };
        if written == 0 {
            // SAFETY: reads the calling thread's last error.
            return Err(match unsafe { GetLastError() } {
                // The reader closed its end.
                ERROR_NO_DATA => io::ErrorKind::BrokenPipe.into(),
                code => io::Error::from_raw_os_error(code as i32),
            });
        }
        // A full pipe accepts nothing rather than waiting.
        if count == 0 && !buffer.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        Ok(count as usize)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Stop the pipe from waiting: reads and writes return at once.
pub(super) fn nonblocking(pipe: &Pipe) -> io::Result<()> {
    let mode = PIPE_NOWAIT;
    // SAFETY: the handle is open; the other settings stay as they are.
    if unsafe { SetNamedPipeHandleState(raw(&pipe.0), &mode, null(), null()) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// A Job Object that kills its processes when its last handle closes.
struct Job(OwnedHandle);

impl Job {
    fn new() -> io::Result<Self> {
        // SAFETY: no name and default security; the result is checked.
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(last_error());
        }
        // SAFETY: a new handle that nothing else owns.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) });
        // SAFETY: zero is a valid value for every field of this plain struct.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the buffer is the structure this information class expects.
        let set = unsafe {
            SetInformationJobObject(
                raw(&job.0),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast::<c_void>(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(last_error());
        }
        Ok(job)
    }

    fn assign(&self, process: &impl AsRawHandle) -> io::Result<()> {
        // SAFETY: both handles are open.
        if unsafe { AssignProcessToJobObject(raw(&self.0), raw(process)) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }

    /// Terminate every process in the job; an empty job is no error.
    fn terminate(&self) -> io::Result<()> {
        // SAFETY: the handle is open.
        if unsafe { TerminateJobObject(raw(&self.0), TERMINATED) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }
}

/// Resume the threads of a process created suspended: just its first one.
fn resume(process: u32) -> io::Result<()> {
    // SAFETY: a snapshot of every thread; the result is checked.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    // SAFETY: a new handle that nothing else owns.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot as RawHandle) };
    // SAFETY: zero is valid for this plain struct; its size is set before use.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut resumed = 0;
    // SAFETY: the snapshot is open and the entry carries its size.
    let mut more = unsafe { Thread32First(raw(&snapshot), &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == process {
            // SAFETY: opens the thread by ID; the result is checked.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(last_error());
            }
            // SAFETY: a new handle that nothing else owns.
            let thread = unsafe { OwnedHandle::from_raw_handle(thread as RawHandle) };
            // SAFETY: the thread handle is open with resume access.
            if unsafe { ResumeThread(raw(&thread)) } == u32::MAX {
                return Err(last_error());
            }
            resumed += 1;
        }
        // SAFETY: as for Thread32First.
        more = unsafe { Thread32Next(raw(&snapshot), &mut entry) } != 0;
    }
    // SAFETY: reads the calling thread's last error.
    let ended = unsafe { GetLastError() };
    if ended != ERROR_NO_MORE_FILES {
        return Err(io::Error::from_raw_os_error(ended as i32));
    }
    if resumed == 0 {
        return Err(io::Error::other(
            "the suspended worker had no thread to resume",
        ));
    }
    Ok(())
}

pub(in crate::core::worker) struct ChildOwner {
    child: Child,
    job: Job,
    stdin: Option<Pipe>,
    stdout: Option<Pipe>,
    stderr: Option<Pipe>,
    pub(super) owned: bool,
    #[cfg(test)]
    pub(super) observer: Option<Arc<Observation>>,
}

impl ChildOwner {
    pub(super) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(super) fn take_pipes(&mut self) -> (Option<Stdin>, Option<Output>, Option<Output>) {
        (self.stdin.take(), self.stdout.take(), self.stderr.take())
    }

    /// Windows workers have no control channel.
    pub(super) fn close_control(&mut self, _observation: &Observation, _pid: u32) {}

    /// Whether the worker has exited. The open process handle keeps its ID
    /// reserved until [`Self::reap`] records the status.
    pub(super) fn exited(&self) -> io::Result<bool> {
        // SAFETY: the process handle stays open while the child is owned.
        Ok(unsafe { WaitForSingleObject(raw(&self.child), 0) } == WAIT_OBJECT_0)
    }

    /// Terminate the worker and every process in its job.
    pub(super) fn terminate(&mut self) -> io::Result<()> {
        if !self.owned {
            return Ok(());
        }
        self.job.terminate()
    }

    pub(super) fn reap(&mut self) -> io::Result<Option<Completion>> {
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        self.owned = false;
        Ok(Some(Completion {
            status: Some(status),
            cleanup: WorkerCleanup::Reaped,
            error: None,
        }))
    }
}

impl Drop for ChildOwner {
    fn drop(&mut self) {
        if self.owned {
            #[cfg(test)]
            if let Some(observer) = self.observer.take() {
                observer.hook(Point::Drop, self.child.id());
            }
            let _ = self.terminate();
            let _ = self.child.wait();
        }
        // Closing the job afterwards ends anything left in it.
    }
}

pub(super) fn launch_worker(
    specification: WorkerCommand,
    _observation: &Observation,
) -> io::Result<ChildOwner> {
    spawn(specification)
}

/// Start the worker suspended, place it in a new job, then let it run, so that
/// no descendant can start outside the job.
pub(super) fn spawn(specification: WorkerCommand) -> io::Result<ChildOwner> {
    let job = Job::new()?;
    let (stdin, input) = pipe()?;
    let (output, stdout) = pipe()?;
    let (errors, stderr) = pipe()?;
    // The command, and with it this process's copy of each child end, is
    // dropped once spawned, so the pipes report their end when the child exits.
    let mut child = Command::new(&specification.executable)
        .args(&specification.arguments)
        .current_dir(&specification.directory)
        .env_clear()
        .envs(&specification.environment)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        // Its own group: a console interrupt does not reach it, as on Unix.
        .creation_flags(CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    if let Err(error) = job.assign(&child).and_then(|()| resume(child.id())) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(ChildOwner {
        stdin: Some(Pipe(input)),
        stdout: Some(Pipe(output)),
        stderr: Some(Pipe(errors)),
        child,
        job,
        owned: true,
        #[cfg(test)]
        observer: None,
    })
}
