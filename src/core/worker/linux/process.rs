//! Direct-child ownership for std::process and clone3 namespace launches.
use super::*;
use std::{
    fs::File,
    os::{fd::OwnedFd, unix::process::ExitStatusExt},
};

pub(super) struct Process {
    id: u32,
    standard: Option<Child>,
    status: Option<ExitStatus>,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}
impl From<Child> for Process {
    fn from(mut child: Child) -> Self {
        Self {
            id: child.id(),
            stdin: child
                .stdin
                .take()
                .map(|pipe| File::from(OwnedFd::from(pipe))),
            stdout: child
                .stdout
                .take()
                .map(|pipe| File::from(OwnedFd::from(pipe))),
            stderr: child
                .stderr
                .take()
                .map(|pipe| File::from(OwnedFd::from(pipe))),
            standard: Some(child),
            status: None,
        }
    }
}
impl Process {
    pub fn namespace(id: u32, stdin: File, stdout: File, stderr: File) -> Self {
        Self {
            id,
            standard: None,
            status: None,
            stdin: Some(stdin),
            stdout: Some(stdout),
            stderr: Some(stderr),
        }
    }
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn namespaced(&self) -> bool {
        self.standard.is_none()
    }
    pub fn kill(&mut self) -> io::Result<()> {
        if let Some(child) = &mut self.standard {
            return child.kill();
        }
        if self.status.is_some() {
            return Ok(());
        }
        // SAFETY: the direct child remains unreaped, so its PID cannot be reused.
        if unsafe { libc::kill(self.id as i32, libc::SIGKILL) } == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(child) = &mut self.standard {
            return child.try_wait();
        }
        self.wait_raw(libc::WNOHANG)
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(child) = &mut self.standard {
            return child.wait();
        }
        self.wait_raw(0)?
            .ok_or_else(|| io::Error::other("Namespace wait returned without child status"))
    }
    fn wait_raw(&mut self, flags: i32) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        loop {
            let mut raw = 0;
            // SAFETY: raw is a valid output; only this owner waits for this child.
            match unsafe { libc::waitpid(self.id as i32, &mut raw, flags) } {
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                0 => return Ok(None),
                _ => {
                    self.status = Some(ExitStatus::from_raw(raw));
                    return Ok(self.status);
                }
            }
        }
    }
}
