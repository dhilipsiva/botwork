//! Deep work stops with a limit error before any thread's stack runs out,
//! whatever its size, the target, or the build profile. See
//! docs/embedded-runs.md#stack-headroom.

use botwork::core::{
    diagnostic::DiagnosticCode,
    grammar::Literal,
    run::{Engine, RunLimits, RunOptions, RunOutcome, RunResult},
};
use std::{collections::BTreeMap, process::Command};

const HEADROOM: &str = "stack headroom bytes";
const KIB: usize = 1024;

/// Work at each recursion ceiling: evaluation depth, parser nesting in three
/// grammar paths, and JSON nesting within the value depth limit.
#[derive(Clone, Copy, Debug)]
enum Shape {
    Evaluation,
    Arrays,
    Maps,
    Calls,
    Json,
}

impl Shape {
    const ALL: [Shape; 5] = [
        Shape::Evaluation,
        Shape::Arrays,
        Shape::Maps,
        Shape::Calls,
        Shape::Json,
    ];

    fn source(self) -> String {
        match self {
            Shape::Evaluation => format!(
                "Recurse {{{}Recurse{}}}\nRecurse",
                "If |true| {".repeat(16),
                "}".repeat(16)
            ),
            Shape::Arrays => format!("|x| = |{}1{}|", "[".repeat(31), "]".repeat(31)),
            Shape::Maps => format!("|x| = |{}1{}|", "{\"a\": ".repeat(31), "}".repeat(31)),
            Shape::Calls => format!(
                "|x| = |{}[1]{}|",
                "@{ Enumerate |".repeat(15),
                "| }".repeat(15)
            ),
            Shape::Json => "|x| = Parse JSON |text|".to_owned(),
        }
    }

    fn options(self) -> RunOptions {
        let text = format!("{}1{}", "[".repeat(63), "]".repeat(63));
        RunOptions {
            limits: RunLimits {
                call_depth: 10_000,
                ..RunLimits::default()
            },
            variables: BTreeMap::from([("text".to_owned(), Literal::String(text))]),
            ..RunOptions::default()
        }
    }

    fn run(self) -> RunResult {
        Engine::default().run_source("deep", &self.source(), self.options())
    }
}

fn on_stack<T: Send>(bytes: usize, work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(bytes)
            .spawn_scoped(scope, work)
            .unwrap()
            .join()
            .unwrap()
    })
}

/// The limit a stopped run names, or None when it succeeded.
fn stopped_by(shape: Shape, run: RunResult) -> Option<String> {
    match run.outcome() {
        RunOutcome::Succeeded => None,
        RunOutcome::LimitExceeded => {
            let error = run.result.unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit, "{shape:?}");
            Some(error.to_string())
        }
        outcome => panic!("{shape:?}: {outcome:?}: {:?}", run.result),
    }
}

#[test]
fn deep_work_ends_with_a_result_or_a_limit_error_on_any_thread_stack() {
    // Every size from well below the reserves to well above them: reaching the
    // end of this loop at all shows no size overflowed.
    for bytes in (160 * KIB..=4096 * KIB).step_by(96 * KIB) {
        for shape in Shape::ALL {
            let stopped = on_stack(bytes, || stopped_by(shape, shape.run()));
            match (shape, stopped) {
                (Shape::Evaluation, Some(message)) => assert!(
                    message.contains(HEADROOM) || message.contains("evaluation depth"),
                    "{bytes}: {message}"
                ),
                (Shape::Evaluation, None) => panic!("{bytes}: unbounded recursion succeeded"),
                (_, Some(message)) => {
                    assert!(message.contains(HEADROOM), "{shape:?} {bytes}: {message}")
                }
                (_, None) => (),
            }
        }
    }
}

#[test]
fn small_stacks_stop_deep_evaluation_on_headroom() {
    // Even the stack glibc may substitute from its cache, up to four times the
    // request, cannot hold evaluation depth 96 and the reserve.
    let message = on_stack(160 * KIB, || {
        stopped_by(Shape::Evaluation, Shape::Evaluation.run())
    });
    let message = message.expect("stopped");
    assert!(message.contains(HEADROOM), "{message}");
}

#[test]
fn large_stacks_reach_the_depth_limits() {
    for shape in Shape::ALL {
        let stopped = on_stack(16 * 1024 * KIB, || stopped_by(shape, shape.run()));
        match shape {
            Shape::Evaluation => {
                let message = stopped.expect("stopped");
                assert!(message.contains("evaluation depth"), "{message}");
            }
            _ => assert_eq!(stopped, None, "{shape:?}"),
        }
    }
}

#[test]
fn asynchronous_runs_check_the_stack_of_the_thread_polling_them() {
    // An asynchronous run needs a base of about 200 KiB unoptimized and 100 KiB
    // optimized before any recursion; the checks cover everything deeper.
    let small = if cfg!(debug_assertions) { 256 } else { 128 } * KIB;
    for (bytes, limit) in [(small, HEADROOM), (16 * 1024 * KIB, "evaluation depth")] {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_stack_size(bytes)
            .build()
            .unwrap();
        let run = runtime
            .block_on(runtime.spawn(async {
                let engine = Engine::default();
                let shape = Shape::Evaluation;
                engine
                    .run_source_async("deep", &shape.source(), shape.options())
                    .await
            }))
            .unwrap();
        let message = stopped_by(Shape::Evaluation, run).expect("stopped");
        assert!(message.contains(limit), "{bytes}: {message}");
    }
}

#[test]
fn asynchronous_deep_evaluation_ends_with_a_limit_error_on_any_worker_stack() {
    let smallest = if cfg!(debug_assertions) { 256 } else { 128 } * KIB;
    for bytes in (smallest..=4096 * KIB).step_by(192 * KIB) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_stack_size(bytes)
            .build()
            .unwrap();
        let run = runtime
            .block_on(runtime.spawn(async {
                let shape = Shape::Evaluation;
                Engine::default()
                    .run_source_async("deep", &shape.source(), shape.options())
                    .await
            }))
            .unwrap();
        let message = stopped_by(Shape::Evaluation, run).expect("stopped");
        assert!(
            message.contains(HEADROOM) || message.contains("evaluation depth"),
            "{bytes}: {message}"
        );
    }
}

#[test]
fn the_cli_reaches_the_depth_limit_on_its_own_stack() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("deep.botwork"),
        Shape::Evaluation.source(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(["--file", "deep.botwork", "--max-call-depth", "10000"])
        .current_dir(directory.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("[BW8001] Resource limit exceeded: evaluation depth (limit 96)"),
        "{stderr}"
    );
}
