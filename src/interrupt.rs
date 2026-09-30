//! SIGINT and SIGTERM. The first signal cancels every run and fixture through the
//! invocation's root control and stops admission, so started runs still end with
//! one terminal outcome; a second signal exits at once, leaving any report journal
//! for `--reconcile-report`. Linux only for now: elsewhere an interrupt keeps the
//! operating system's default and ends the process at once.
use botwork::core::operation::OperationControl;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

/// Exit status after a second signal, as a shell reports SIGINT.
#[cfg(target_os = "linux")]
const FORCED: i32 = 130;

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

#[cfg(target_os = "linux")]
fn stop() {
    INTERRUPTED.store(true, Ordering::SeqCst);
    root().cancel();
}

#[cfg(target_os = "linux")]
mod signals {
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

    static RECEIVED: AtomicUsize = AtomicUsize::new(0);
    static NOTIFY: AtomicI32 = AtomicI32::new(-1);

    /// Async-signal-safe: count, then wake the watcher or exit on the second signal.
    extern "C" fn handle(_: libc::c_int) {
        if RECEIVED.fetch_add(1, Ordering::SeqCst) != 0 {
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
        if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(std::io::Error::last_os_error());
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
                        let _ = super::super::Context::default().write_output(
                            &mut std::io::stderr().lock(),
                            format_args!(
                                "[interrupted] stopping runs; interrupt again to exit at once\n"
                            ),
                        );
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

/// Handle SIGINT and SIGTERM for the rest of the process, giving the root control
/// its stop grace. Call it before anything reads [`root`].
pub(super) fn install(stop_grace: std::time::Duration) -> std::io::Result<()> {
    ROOT.get_or_init(|| OperationControl::default().with_stop_grace(stop_grace));
    #[cfg(target_os = "linux")]
    signals::install()?;
    Ok(())
}
