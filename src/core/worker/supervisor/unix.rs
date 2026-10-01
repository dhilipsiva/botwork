//! The Unix backend: a child process group, with a Linux-only guardian or PID
//! namespace when the pool asks for one.
use super::observation::Observation;
use super::*;
use std::os::{fd::AsRawFd, unix::process::CommandExt};

// Process-tree guardians and PID namespaces need Linux (decision D12).
#[cfg(target_os = "linux")]
pub(in crate::core::worker) mod guardian;
mod launch;
#[cfg(target_os = "linux")]
mod namespace;
mod process;

pub(super) type Stdin = std::fs::File;
pub(super) type Output = std::fs::File;

pub(super) fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY: the borrowed pipe owns a live descriptor throughout both calls.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(in crate::core::worker) struct ChildOwner {
    child: process::Process,
    pub(super) owned: bool,
    pub(super) guardian: Option<std::os::unix::net::UnixStream>,
    #[cfg(test)]
    pub(super) observer: Option<Arc<observation::Observation>>,
}
impl ChildOwner {
    pub(super) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(super) fn take_pipes(&mut self) -> (Option<Stdin>, Option<Output>, Option<Output>) {
        (
            self.child.stdin.take(),
            self.child.stdout.take(),
            self.child.stderr.take(),
        )
    }

    /// Close a guardian's control channel, which tells it to finish.
    pub(super) fn close_control(&mut self, observation: &Observation, pid: u32) {
        #[cfg(test)]
        if self.guardian.is_some() {
            observation.hook(Point::ControlClose, pid);
        }
        super::owner::close_pipe(&mut self.guardian, observation, pid);
    }

    pub(super) fn exited(&self) -> io::Result<bool> {
        // WNOWAIT keeps the PID reserved until process-group cleanup is requested.
        // SAFETY: zero is a valid empty siginfo_t; waitid initializes the result.
        let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut information,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { information.si_pid() } != 0)
    }

    pub(super) fn terminate(&mut self) -> io::Result<()> {
        if !self.owned {
            return Ok(());
        }
        if self.child.namespaced() {
            // Killing this known namespace init asks the kernel to terminate
            // its entire namespace, even when the guardian cannot cooperate.
            return self.child.kill().or_else(|error| {
                if error.raw_os_error() == Some(libc::ESRCH) {
                    Ok(())
                } else {
                    Err(error)
                }
            });
        }
        if let Some(control) = &self.guardian {
            // Keep the guardian alive to reap its descendants, including detached ones.
            return control
                .shutdown(std::net::Shutdown::Write)
                .or_else(|error| {
                    if error.kind() == io::ErrorKind::NotConnected {
                        Ok(())
                    } else {
                        Err(error)
                    }
                });
        }
        // SAFETY: this is our unreaped child and initial process-group leader.
        // Never signal by this numeric PID after releasing child ownership.
        let result = unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
        let group_error = (result == -1)
            .then(io::Error::last_os_error)
            .filter(|error| !self.nothing_left(error));
        // Also stop the direct child if it moved itself out of the initial group.
        let direct = self.child.kill().or_else(|error| {
            if self.nothing_left(&error) {
                Ok(())
            } else {
                Err(error)
            }
        });
        match group_error {
            Some(error) => Err(error),
            None => direct,
        }
    }

    /// Whether a failed signal found nothing left to stop: no such process, or
    /// on macOS, which refuses to signal zombies, only the exited child that
    /// [`Self::exited`] holds for reaping.
    fn nothing_left(&self, error: &io::Error) -> bool {
        match error.raw_os_error() {
            Some(libc::ESRCH) => true,
            Some(libc::EPERM) if cfg!(target_os = "macos") => self.exited().unwrap_or(false),
            _ => false,
        }
    }

    pub(super) fn reap(&mut self) -> io::Result<Option<Completion>> {
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        self.owned = false;
        #[cfg(target_os = "linux")]
        if let Some(control) = self.guardian.as_mut() {
            return match guardian::completion(control, status) {
                Ok(completion) => Ok(Some(completion)),
                Err(error) if self.child.namespaced() => Ok(Some(Completion {
                    status: None,
                    cleanup: WorkerCleanup::NamespaceReaped,
                    error: Some(runtime(format_args!(
                        "Namespace guardian ended without verified worker completion: {error}"
                    ))),
                })),
                Err(error) => Err(error),
            };
        }
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
            // Only the owned OS thread reaches this fallback.
            // Its capacity stays retained if the kernel cannot finish reaping.
            let _ = self.child.wait();
        }
    }
}

pub(super) fn launch_worker(
    specification: WorkerCommand,
    observation: &Observation,
) -> io::Result<ChildOwner> {
    #[cfg(target_os = "linux")]
    if let Some(executable) = &observation.shared.guardian {
        let record = observation
            .request
            .journal
            .as_ref()
            .map(|ticket| ticket.file())
            .transpose()?;
        return guardian::spawn(
            executable,
            specification,
            record.as_deref(),
            observation.shared.namespaced,
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = observation;
    launch::spawn(specification)
}
