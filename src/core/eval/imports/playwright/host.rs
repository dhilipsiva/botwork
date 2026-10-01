//! A run's Playwright host: one Node process, started at the run's first
//! Playwright command and kept until the run ends. Commands and answers are
//! JSON lines, matched by ID, so a dropped command never confuses the next.
use super::*;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// The Node side, run with `node --input-type=module -e`.
const SCRIPT: &str = include_str!("../playwright_host.mjs");
/// How long the run's end lets the host close its browsers before ending it.
const CLOSE_GRACE: Duration = Duration::from_secs(5);
/// The longest answer line: a full-page screenshot is written to a file, so
/// answers stay small.
const MAX_LINE: usize = 16 * 1024 * 1024;

/// What the host said went wrong.
#[derive(Debug)]
pub(super) struct Failure {
    pub(super) kind: String,
    pub(super) message: String,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, Failure>>>>>;

struct Process {
    child: Mutex<Child>,
    /// Lines are short and the host reads them at once, so a write does not
    /// block for long.
    stdin: Mutex<Option<ChildStdin>>,
    pending: Pending,
}

/// A run's host, started by its first command.
pub(in crate::core::eval) struct Host {
    node: Option<PathBuf>,
    directory: PathBuf,
    variables: BTreeMap<OsString, OsString>,
    process: tokio::sync::Mutex<Option<Arc<Process>>>,
    next: AtomicU64,
}

impl Host {
    pub(in crate::core::eval) fn new(
        directory: PathBuf,
        variables: BTreeMap<OsString, OsString>,
    ) -> Self {
        let node = node(&variables);
        Self {
            node,
            directory,
            variables,
            process: tokio::sync::Mutex::new(None),
            next: AtomicU64::new(1),
        }
    }

    /// The host, started if this is the run's first command.
    async fn process(&self) -> Result<Arc<Process>, Failure> {
        let mut slot = self.process.lock().await;
        if let Some(process) = &*slot {
            return Ok(Arc::clone(process));
        }
        let node = self.node.as_ref().ok_or_else(|| Failure {
            kind: "setup".into(),
            message: "Playwright needs Node.js on the run's PATH".into(),
        })?;
        let mut command = std::process::Command::new(node);
        command
            .args(["--input-type=module", "-e", SCRIPT])
            .current_dir(&self.directory)
            .env_clear()
            .envs(&self.variables)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|error| Failure {
            kind: "setup".into(),
            message: format!("could not start Node for Playwright: {error}"),
        })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let pending: Pending = Arc::default();
        {
            let pending = Arc::clone(&pending);
            std::thread::Builder::new()
                .name("botwork-playwright".into())
                .spawn(move || read(stdout, &pending))
                .map_err(|error| Failure {
                    kind: "setup".into(),
                    message: format!("could not read the Playwright host: {error}"),
                })?;
        }
        let process = Arc::new(Process {
            child: Mutex::new(child),
            stdin: Mutex::new(Some(stdin)),
            pending,
        });
        *slot = Some(Arc::clone(&process));
        Ok(process)
    }

    /// Send `command` with `args` and wait for its answer, at most `timeout`.
    pub(super) async fn call(
        &self,
        command: &str,
        args: Vec<Value>,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        let process = self.process().await?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        process
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id, sender);
        // Forget the command if this call is dropped, as a stop drops it.
        struct Forget<'a>(&'a Pending, u64);
        impl Drop for Forget<'_> {
            fn drop(&mut self) {
                self.0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&self.1);
            }
        }
        let _forget = Forget(&process.pending, id);
        let line = format!(
            "{}\n",
            json!({ "id": id, "command": command, "args": args })
        );
        let exited = || Failure {
            kind: "error".into(),
            message: "the Playwright host exited".into(),
        };
        process
            .stdin
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .ok_or_else(exited)?
            .write_all(line.as_bytes())
            .map_err(|_| exited())?;
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(exited()),
            Err(_) => Err(Failure {
                kind: "error".into(),
                message: format!(
                    "the Playwright host did not answer {command} within {} ms",
                    timeout.as_millis()
                ),
            }),
        }
    }
}

/// Route each answer to the command waiting for it, until the host exits.
fn read(stdout: ChildStdout, pending: &Pending) {
    let mut lines = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match lines.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) if line.len() > MAX_LINE => continue,
            Ok(_) => {}
        }
        let Ok(answer) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = answer["id"].as_u64() else {
            continue;
        };
        let result = match answer.get("error") {
            Some(error) => Err(Failure {
                kind: error["kind"].as_str().unwrap_or("error").to_owned(),
                message: error["message"].as_str().unwrap_or_default().to_owned(),
            }),
            None => Ok(answer.get("value").cloned().unwrap_or(Value::Null)),
        };
        if let Some(sender) = pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id)
        {
            let _ = sender.send(result);
        }
    }
    // Commands still waiting learn that the host exited.
    pending
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}

impl Drop for Host {
    /// End the host with the run: close its input, so it closes its browsers,
    /// then end it and, on Unix, its process group.
    fn drop(&mut self) {
        let Some(process) = self.process.get_mut().take() else {
            return;
        };
        drop(
            process
                .stdin
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take(),
        );
        let mut child = process
            .child
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let deadline = Instant::now() + CLOSE_GRACE;
        while Instant::now() < deadline && !exited(&mut child) {
            std::thread::sleep(Duration::from_millis(20));
        }
        #[cfg(unix)]
        if let Ok(group) = libc::pid_t::try_from(child.id()) {
            // SAFETY: signals the host's own process group, which it leads; the
            // host is not yet reaped, so its ID cannot have been reused.
            unsafe { libc::killpg(group, libc::SIGKILL) };
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Whether the host has exited. On Unix it is left unreaped, so its process
/// group can still be signalled by its ID.
fn exited(child: &mut Child) -> bool {
    #[cfg(unix)]
    {
        let pid: libc::id_t = child.id();
        // SAFETY: an all-zero siginfo_t is valid, and waitid only writes it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waits on our own child without reaping it (WNOWAIT).
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        // SAFETY: si_pid is set by waitid when a child exited.
        result != 0 || unsafe { info.si_pid() } != 0
    }
    #[cfg(not(unix))]
    {
        !matches!(child.try_wait(), Ok(None))
    }
}

/// Node, on the run's `PATH`.
fn node(variables: &BTreeMap<OsString, OsString>) -> Option<PathBuf> {
    on_path(
        variables,
        "node",
        if cfg!(windows) { &[".exe"] } else { &[""] },
    )
}
