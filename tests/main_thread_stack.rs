//! The headroom checks on a process's main thread, whose stack the platform
//! sizes: Linux grows it on demand up to the soft stack limit, and musl reports
//! only the part mapped so far; macOS gives it 8 MiB and Windows 1 MiB. A plain
//! main function, since the test harness runs tests on spawned threads. Each
//! case runs in its own process, because a thread's stack extent is found once.

use botwork::core::run::{Engine, RunLimits, RunOptions, RunOutcome};
use std::process::Command;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some(case) => run(case),
        None => {
            for case in ["generous", "limited"] {
                let output = Command::new(std::env::current_exe().unwrap())
                    .arg(case)
                    .output()
                    .unwrap();
                print!("{}", String::from_utf8_lossy(&output.stdout));
                assert!(
                    output.status.success(),
                    "{case}: {:?}\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

fn run(case: &str) {
    // An 8 MiB limit holds evaluation depth 96 in every build profile; 512
    // KiB cannot also hold the reserve below it.
    #[cfg(target_os = "linux")]
    set_stack_limit(if case == "generous" { 8192 } else { 512 } * 1024);
    let source = format!(
        "Recurse {{{}Recurse{}}}\nRecurse",
        "If |true| {".repeat(16),
        "}".repeat(16)
    );
    let run = Engine::default().run_source(
        "deep",
        &source,
        RunOptions {
            limits: RunLimits {
                call_depth: 10_000,
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded, "{:?}", run.result);
    let message = run.result.unwrap_err().to_string();
    // Elsewhere the platform fixes the main thread's size, so either limit
    // may stop the run; what matters is that one does.
    let expected: &[&str] = match (cfg!(target_os = "linux"), case) {
        (true, "generous") => &["evaluation depth"],
        (true, _) => &["stack headroom bytes"],
        (false, _) => &["stack headroom bytes", "evaluation depth"],
    };
    assert!(
        expected.iter().any(|limit| message.contains(limit)),
        "{case}: {message}"
    );
    println!("{case}: {}", message.lines().next().unwrap());
}

#[cfg(target_os = "linux")]
fn set_stack_limit(bytes: u64) {
    // SAFETY: getrlimit fills the zeroed rlimit that setrlimit then reads.
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        assert_eq!(libc::getrlimit(libc::RLIMIT_STACK, &mut limit), 0);
        limit.rlim_cur = bytes.min(limit.rlim_max);
        assert_eq!(libc::setrlimit(libc::RLIMIT_STACK, &limit), 0);
    }
}
