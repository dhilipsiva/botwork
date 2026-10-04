//! Interrupts: SIGINT and SIGTERM on Unix, Ctrl-C and Ctrl-Break on Windows. The
//! first cancels every run and fixture through the invocation's root control and
//! stops admission, so started runs still end with one terminal outcome; a
//! second exits at once, leaving any report journal for `--reconcile-report`.
use botwork::core::operation::OperationControl;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

/// Exit status after a second interrupt, as a shell reports SIGINT.
#[cfg(any(unix, windows))]
const FORCED: i32 = 130;

/// What the console shows when the first interrupt arrives.
#[cfg(any(unix, windows))]
fn announce() {
    let _ = super::Context::default().write_output(
        &mut std::io::stderr().lock(),
        format_args!("[interrupted] stopping runs; interrupt again to exit at once\n"),
    );
}

static ROOT: OnceLock<OperationControl> = OnceLock::new();
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// The control every run, case, and fixture control descends from.
pub(super) fn root() -> &'static OperationControl {
    ROOT.get_or_init(OperationControl::default)
}

/// Whether a signal asked the invocation to stop.
pub(super) fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

#[cfg(any(unix, windows))]
fn stop() {
    INTERRUPTED.store(true, Ordering::SeqCst);
    root().cancel();
}

#[cfg(unix)]
mod signals {
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

    static RECEIVED: AtomicUsize = AtomicUsize::new(0);
    static NOTIFY: AtomicI32 = AtomicI32::new(-1);

    /// Async-signal-safe: count, then wake the watcher or exit on the second signal.
    extern "C" fn handle(_: libc::c_int) {
        if RECEIVED.fetch_add(1, Ordering::SeqCst) != 0 {
            // The processes runs started go with the invocation.
            botwork::core::worker::kill_groups_for_exit();
            unsafe { libc::_exit(super::FORCED) };
        }
        let descriptor = NOTIFY.load(Ordering::SeqCst);
        if descriptor >= 0 {
            let byte = 1u8;
            unsafe { libc::write(descriptor, (&byte as *const u8).cast(), 1) };
        }
    }

    pub(super) fn install() -> std::io::Result<()> {
        let mut descriptors = [0; 2];
        // SAFETY: a two-descriptor array for the pipe's ends.
        #[cfg(target_os = "linux")]
        let created = unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) };
        // macOS has no pipe2; this runs before the CLI starts any process.
        #[cfg(not(target_os = "linux"))]
        let created = unsafe { libc::pipe(descriptors.as_mut_ptr()) };
        if created != 0 {
            return Err(std::io::Error::last_os_error());
        }
        #[cfg(not(target_os = "linux"))]
        for descriptor in descriptors {
            // SAFETY: both descriptors were just created.
            if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
                return Err(std::io::Error::last_os_error());
            }
        }
        let [read, write] = descriptors;
        NOTIFY.store(write, Ordering::SeqCst);
        std::thread::Builder::new()
            .name("botwork-signals".into())
            .spawn(move || {
                let mut byte = 0u8;
                loop {
                    let count = unsafe { libc::read(read, (&mut byte as *mut u8).cast(), 1) };
                    if count == 1 {
                        super::stop();
                        super::announce();
                        return;
                    }
                    if count == 0
                        || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                    {
                        return;
                    }
                }
            })?;
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = handle as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            unsafe { libc::sigemptyset(&mut action.sa_mask) };
            if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod console {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::{
        Foundation::{FALSE, TRUE},
        System::{
            Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT},
            Threading::{GetCurrentProcess, TerminateProcess},
        },
    };

    static RECEIVED: AtomicUsize = AtomicUsize::new(0);

    /// Windows runs this on a thread of its own, so it may do the work itself.
    /// Other events, such as closing the console, keep their default.
    unsafe extern "system" fn handle(event: u32) -> BOOL {
        if event != CTRL_C_EVENT && event != CTRL_BREAK_EVENT {
            return FALSE;
        }
        if RECEIVED.fetch_add(1, Ordering::SeqCst) != 0 {
            // Like `_exit`: no destructors or buffered output, which another
            // thread may hold locked.
            // SAFETY: terminates this process.
            unsafe { TerminateProcess(GetCurrentProcess(), super::FORCED as u32) };
        }
        super::stop();
        super::announce();
        TRUE
    }

    pub(super) fn install() -> std::io::Result<()> {
        // SAFETY: registers a handler that lives as long as the process.
        if unsafe { SetConsoleCtrlHandler(Some(handle), TRUE) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Handle interrupts for the rest of the process, giving the root control
/// its stop grace. Call it before anything reads [`root`].
pub(super) fn install(stop_grace: std::time::Duration) -> std::io::Result<()> {
    ROOT.get_or_init(|| OperationControl::default().with_stop_grace(stop_grace));
    #[cfg(unix)]
    signals::install()?;
    #[cfg(windows)]
    console::install()?;
    Ok(())
}
