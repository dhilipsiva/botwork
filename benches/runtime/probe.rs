//! Native parent avoids inheriting Python's RSS high-water mark at child fork.
#[cfg(target_os = "linux")]
pub(super) fn main() -> i32 {
    use std::{
        fs, io,
        os::unix::process::{CommandExt, ExitStatusExt},
        process::{Command, ExitStatus},
        time::Instant,
    };

    // `--probe REPORT [--preload LIBRARY] COMMAND...`: the library is preloaded
    // into the measured command only, never into this supervisor.
    let mut args = std::env::args_os().skip(2);
    let report = args.next().expect("probe report path");
    let mut executable = args.next().expect("probe executable");
    let mut preload = None;
    if executable == "--preload" {
        preload = Some(args.next().expect("preloaded library"));
        executable = args.next().expect("probe executable");
    }
    let mut command = Command::new(executable);
    command.args(args);
    if let Some(library) = preload {
        command.env("LD_PRELOAD", library);
    }
    let parent = std::process::id() as libc::pid_t;
    // Only async-signal-safe syscalls in the post-fork callback. All measured
    // commands are ordinary non-setuid programs and do not launch subprocesses.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
    let start = Instant::now();
    // Reaped by wait4 below so the measurement retains per-child rusage.
    #[allow(clippy::zombie_processes)]
    let child = command.spawn().expect("spawn measured process");
    let mut status = 0;
    // wait4 fills the two output records; zero initialization is valid for the
    // integer/timeval fields in Linux rusage, including unused fields.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        let waited = unsafe { libc::wait4(child.id() as libc::pid_t, &mut status, 0, &mut usage) };
        if waited >= 0 {
            break;
        }
        let error = io::Error::last_os_error();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted, "{error}");
    }
    let elapsed_ns = u64::try_from(start.elapsed().as_nanos()).unwrap();
    let status = ExitStatus::from_raw(status);
    let code = status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap());
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0;
    let report_data = serde_json::json!({
        "schema": 1, "returncode": code,
        "process_elapsed_ns": elapsed_ns, "peak_rss_kib": usage.ru_maxrss,
        "user_seconds": seconds(usage.ru_utime), "system_seconds": seconds(usage.ru_stime),
        "voluntary_switches": usage.ru_nvcsw, "involuntary_switches": usage.ru_nivcsw,
    });
    fs::write(report, format!("{report_data}\n")).expect("write probe report");
    code
}

#[cfg(not(target_os = "linux"))]
pub(super) fn main() -> i32 {
    eprintln!("The measurement probe requires Linux wait4 and parent-death signals");
    1
}
