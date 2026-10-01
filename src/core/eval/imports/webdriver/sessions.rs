//! The browser sessions a run opened, and the drivers Botwork started for them.
//! A session belongs to the run that opened it; the run's end deletes any still
//! open and ends their drivers.
use super::*;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

/// How long the run's end waits on each session's driver to close it.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// How often a starting driver is asked whether it is ready.
const POLL: Duration = Duration::from_millis(50);

/// A run's open sessions, by the driver's session ID.
#[derive(Default)]
pub(in crate::core::eval) struct Sessions {
    open: Mutex<HashMap<String, Arc<Session>>>,
}

pub(super) struct Session {
    pub(super) endpoint: Endpoint,
    /// Each command's bound.
    pub(super) timeout: Duration,
    /// The driver Botwork started for this session, ended with it.
    driver: Mutex<Option<Driver>>,
}

impl Session {
    pub(super) fn new(endpoint: Endpoint, timeout: Duration, driver: Option<Driver>) -> Self {
        Self {
            endpoint,
            timeout,
            driver: Mutex::new(driver),
        }
    }

    /// End the driver Botwork started, if it did.
    pub(super) fn end_driver(&self) {
        drop(
            self.driver
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take(),
        );
    }
}

impl Sessions {
    fn open(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Session>>> {
        self.open.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn insert(&self, id: String, session: Session) {
        self.open().insert(id, Arc::new(session));
    }

    pub(super) fn get(&self, id: &str) -> Result<Arc<Session>, String> {
        self.open().get(id).cloned().ok_or_else(|| {
            format!("`{id}` is not a browser this run has open: it was closed, or another run opened it")
        })
    }

    pub(super) fn remove(&self, id: &str) -> Option<Arc<Session>> {
        self.open().remove(id)
    }
}

impl Drop for Sessions {
    /// Close what the run left open: delete each session, so its driver closes
    /// the browser, then end the drivers Botwork started.
    fn drop(&mut self) {
        let open = std::mem::take(&mut *self.open());
        for (id, session) in open {
            session.endpoint.delete_blocking(&id, CLOSE_TIMEOUT);
            session.end_driver();
        }
    }
}

/// A driver process Botwork started, such as chromedriver, listening on a
/// loopback port of its own. Dropping it ends it and, on Unix, its process
/// group, so a browser it started cannot outlive it.
pub(super) struct Driver {
    child: Child,
}

impl Driver {
    /// Start `executable` on a free loopback port, in the run's directory and
    /// environment, and wait until it answers.
    pub(super) async fn start(
        executable: &Path,
        directory: &Path,
        variables: &BTreeMap<OsString, OsString>,
        timeout: Duration,
    ) -> Result<(Self, Endpoint), String> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map_err(|error| format!("no loopback port for the driver: {error}"))?
            .port();
        let mut command = Command::new(executable);
        command
            .arg(format!("--port={port}"))
            .current_dir(directory)
            .env_clear()
            .envs(variables)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn().map_err(|error| {
            format!(
                "could not start the driver `{}`: {error}",
                executable.display()
            )
        })?;
        let mut driver = Self { child };
        let endpoint = Endpoint::loopback(port);
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = driver.child.try_wait() {
                return Err(format!(
                    "the driver `{}` exited with {status} before it listened",
                    executable.display()
                ));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!(
                    "the driver `{}` did not answer on port {port} within {} ms",
                    executable.display(),
                    timeout.as_millis()
                ));
            }
            if endpoint
                .command(
                    hyper::Method::GET,
                    "/status",
                    None,
                    left.min(Duration::from_secs(1)),
                )
                .await
                .is_ok()
            {
                return Ok((driver, endpoint));
            }
            tokio::time::sleep(POLL.min(left)).await;
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Ok(group) = libc::pid_t::try_from(self.child.id()) {
            // SAFETY: signals the driver's own process group, which it leads.
            unsafe { libc::killpg(group, libc::SIGKILL) };
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
