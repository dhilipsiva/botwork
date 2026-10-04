//! The Unix backend: a child process group, with a Linux-only guardian or PID
//! namespace when the pool asks for one.
use super::observation::Observation;
use super::*;
use std::{
    os::{fd::AsRawFd, unix::process::CommandExt},
    sync::atomic::{AtomicI32, Ordering},
};

// Process-tree guardians and PID namespaces need Linux (decision D12).
#[cfg(target_os = "linux")]
pub(in crate::core::worker) mod guardian;
mod launch;
#[cfg(target_os = "linux")]
mod namespace;
mod process;

pub(super) type Stdin = std::fs::File;
pub(super) type Output = std::fs::File;

/// Slots for the process groups of the default pools' live workers, which a
/// host exiting without waiting can end at once; zero marks a free slot. A
/// worker that finds every slot taken goes unlisted, and only it outlives a
/// forced exit.
const GROUP_SLOTS: usize = 1024;
static GROUPS: [AtomicI32; GROUP_SLOTS] = [const { AtomicI32::new(0) }; GROUP_SLOTS];

/// List a new worker's process group, whose leader it is.
fn register_group(leader: u32) -> Option<usize> {
    let leader = i32::try_from(leader).ok().filter(|leader| *leader > 0)?;
    GROUPS.iter().position(|slot| {
        slot.compare_exchange(0, leader, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    })
}

/// Send SIGKILL to every listed process group. It only loads atomics and
/// calls kill(2), so a signal handler may call it.
pub(in crate::core::worker) fn kill_groups() {
    for slot in &GROUPS {
        let group = slot.load(Ordering::SeqCst);
        if group > 0 {
            // SAFETY: kill has no memory effects. A listed group's leader is
            // not yet reaped, so its ID names no other process group.
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
    }
}

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
    /// The slot listing this worker's process group, for a forced exit.
    group: Option<usize>,
    pub(super) guardian: Option<std::os::unix::net::UnixStream>,
    #[cfg(test)]
    pub(super) observer: Option<Arc<observation::Observation>>,
}
impl ChildOwner {
    pub(super) fn id(&self) -> u32 {
        self.child.id()
    }

    /// Unlist the worker's process group. Callers do so while its exited
    /// leader still reserves the ID, before reaping it.
    fn release_group(&mut self) {
        if let Some(slot) = self.group.take() {
            GROUPS[slot].store(0, Ordering::SeqCst);
        }
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
        // Unlist the group while its exited leader still reserves the ID, so a
        // forced exit never signals an ID reused since.
        if self.group.is_some() {
            if !self.exited()? {
                return Ok(None);
            }
            self.release_group();
        }
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
            self.release_group();
            // Only the owned OS thread reaches this fallback.
            // Its capacity stays retained if the kernel cannot finish reaping.
            let _ = self.child.wait();
        }
        self.release_group();
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
