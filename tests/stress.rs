#![cfg(target_os = "linux")]
//! Deterministic isolation and cancellation scenarios, repeated
//! `BOTWORK_STRESS_ITERATIONS` times (10 by default; the stress campaign runs
//! 1,000). Stops arrive at handshakes rather than after sleeps, deadlines run on
//! a paused Tokio clock, and every service is deterministic in-process code. A
//! failure is therefore attributed to one class: `setup` failures come from the
//! environment (temporary files, sockets, spawning the CLI); a `handshake`
//! failure means the run never reached a controlled point; `outcome`, `release`,
//! and `bound` failures are wrong results, leaked resources, and slow stops.
//! `BOTWORK_STRESS_REPORT` names a JSON report to write.
//!
//! This binary holds one test, so the process-wide counts it reads belong to the
//! scenarios alone.
use botwork::core::{
    diagnostic::DiagnosticCode,
    grammar::Literal,
    operation::{
        NativeOperation, OperationBudget, OperationControl, OperationOwnershipLimits,
        OperationUsage,
    },
    run::{Engine, RunOptions, RunResult},
    signature::StatementSignature,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    net::TcpListener,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Longest a stop may take to end a run, and longest any handshake may wait.
const BOUND: Duration = Duration::from_secs(5);
const HANDSHAKE: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct Failure {
    class: &'static str,
    message: String,
}

type Attempt = Result<Duration, Failure>;

fn failure(class: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        class,
        message: message.into(),
    }
}

macro_rules! ensure {
    ($condition:expr, $class:literal, $($message:tt)+) => {
        if !$condition {
            return Err(failure($class, format!($($message)+)));
        }
    };
}

fn descriptors() -> usize {
    fs::read_dir("/proc/self/fd").unwrap().count()
}

fn threads() -> usize {
    fs::read_dir("/proc/self/task").unwrap().count()
}

/// Every child process of this test process, zombies included.
fn children() -> BTreeSet<u32> {
    let mut pids = BTreeSet::new();
    for task in fs::read_dir("/proc/self/task").unwrap() {
        if let Ok(text) = fs::read_to_string(task.unwrap().path().join("children")) {
            pids.extend(
                text.split_whitespace()
                    .map(|pid| pid.parse::<u32>().unwrap()),
            );
        }
    }
    pids
}

fn resident_kib() -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or(0)
}

fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}

/// Poll a condition from outside the runtime.
fn wait_until(
    scenario: &'static str,
    what: &str,
    mut ready: impl FnMut() -> bool,
) -> Result<(), Failure> {
    let deadline = Instant::now() + HANDSHAKE;
    while !ready() {
        ensure!(
            Instant::now() < deadline,
            "handshake",
            "{scenario}: {what} never happened"
        );
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// Receive a handshake from a controlled service or operation.
async fn handshake(receiver: &mut UnboundedReceiver<()>, what: &str) -> Result<Instant, Failure> {
    match tokio::time::timeout(HANDSHAKE, receiver.recv()).await {
        Ok(Some(())) => Ok(Instant::now()),
        _ => Err(failure("handshake", format!("{what} never happened"))),
    }
}

/// Await a run, failing rather than hanging when a stop is not bounded.
async fn bounded<F: std::future::Future>(run: F) -> Result<F::Output, Failure> {
    tokio::time::timeout(HANDSHAKE, run)
        .await
        .map_err(|_| failure("bound", "the run did not end"))
}

fn expect_code(result: &RunResult, code: DiagnosticCode) -> Result<String, Failure> {
    match &result.result {
        Err(error) if error.code() == code => Ok(error.to_string()),
        other => Err(failure(
            "outcome",
            format!("expected {code:?}, got {other:?}"),
        )),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    // Idle blocking threads exit quickly, so settled counts are deterministic.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_keep_alive(Duration::from_millis(50))
        .build()
        .unwrap()
}

fn signature(header: &str) -> StatementSignature {
    StatementSignature::native(header).unwrap()
}

/// `Note |text|` appends to a shared log, so tests observe cleanup directly.
fn note(notes: &Arc<Mutex<Vec<String>>>) -> NativeOperation {
    let notes = Arc::clone(notes);
    NativeOperation::asynchronous(signature("Note |text|"), move |values, _| {
        notes.lock().unwrap().push(values[0].to_string());
        async { Ok(Literal::Bool(true)) }
    })
    .unwrap()
}

/// An operation that signals entry and never finishes on its own.
fn hold(header: &str, entered: UnboundedSender<()>, budget: &OperationBudget) -> NativeOperation {
    NativeOperation::asynchronous(signature(header), move |_, _| {
        let _ = entered.send(());
        std::future::pending()
    })
    .unwrap()
    .with_ownership_budget(budget.clone())
}

fn budget() -> OperationBudget {
    OperationBudget::new(OperationOwnershipLimits::default())
}

fn statistics(latencies: &mut [Duration]) -> Value {
    if latencies.is_empty() {
        return Value::Null;
    }
    latencies.sort();
    let at = |fraction: f64| {
        let index = ((latencies.len() as f64 * fraction).ceil() as usize).clamp(1, latencies.len());
        latencies[index - 1].as_secs_f64() * 1000.0
    };
    json!({"p50_ms": at(0.50), "p95_ms": at(0.95), "max_ms": at(1.0)})
}

/// Warm up once, settle, then repeat `once` and check that descriptors,
/// threads, and children return to their settled levels.
fn exercise(name: &str, iterations: usize, mut once: impl FnMut(usize) -> Attempt) -> Value {
    let started = Instant::now();
    let mut failures = Vec::new();
    if let Err(error) = once(0) {
        failures.push(json!({"iteration": 0, "class": error.class, "message": error.message}));
    }
    let _ = wait_until("settle", "children exiting", || children().is_empty());
    thread::sleep(Duration::from_millis(300));
    let before = (descriptors(), threads(), children());
    let resident_before = resident_kib();
    let mut latencies = Vec::with_capacity(iterations);
    let mut completed = 0;
    if failures.is_empty() {
        for iteration in 1..=iterations {
            match once(iteration) {
                Ok(latency) if latency < BOUND => {
                    latencies.push(latency);
                    completed += 1;
                }
                Ok(latency) => {
                    failures.push(json!({"iteration": iteration, "class": "bound", "message": format!("the stop took {latency:?}")}));
                    break;
                }
                Err(error) => {
                    // Later iterations could only repeat the consequences of this one.
                    failures.push(json!({"iteration": iteration, "class": error.class, "message": error.message}));
                    break;
                }
            }
        }
    }
    let settled = Instant::now() + BOUND;
    let after = loop {
        let now = (descriptors(), threads(), children());
        if (now.0 <= before.0 && now.1 <= before.1 && now.2.is_subset(&before.2))
            || Instant::now() >= settled
        {
            break now;
        }
        thread::sleep(Duration::from_millis(20));
    };
    if after.0 > before.0 || after.1 > before.1 || !after.2.is_subset(&before.2) {
        failures.push(
            json!({"iteration": completed, "class": "release", "message": format!(
                "descriptors {} -> {}, threads {} -> {}, children {:?} -> {:?}",
                before.0, after.0, before.1, after.1, before.2, after.2
            )}),
        );
    }
    json!({
        "scenario": name,
        "iterations": iterations,
        "completed": completed,
        "failures": failures,
        "latency": statistics(&mut latencies),
        "seconds": started.elapsed().as_secs_f64(),
        "descriptors": [before.0, after.0],
        "threads": [before.1, after.1],
        "children": [before.2.len(), after.2.len()],
        "resident_kib": [resident_before, resident_kib()],
    })
}

/// Await every future concurrently on the current task.
async fn join_all<F: std::future::Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut futures: Vec<_> = futures
        .into_iter()
        .map(|future| Some(Box::pin(future)))
        .collect();
    let mut outputs: Vec<Option<F::Output>> = futures.iter().map(|_| None).collect();
    std::future::poll_fn(|context| {
        let mut pending = false;
        for (index, slot) in futures.iter_mut().enumerate() {
            if let Some(future) = slot {
                match future.as_mut().poll(context) {
                    std::task::Poll::Ready(output) => {
                        outputs[index] = Some(output);
                        *slot = None;
                    }
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    outputs.into_iter().map(Option::unwrap).collect()
}

/// Eight concurrent runs import a module that reads the environment, and write
/// and read a file in their own directory. The module parse is shared.
fn isolation<'a>(
    runtime: &'a tokio::runtime::Runtime,
    root: &Path,
) -> impl FnMut(usize) -> Attempt + 'a {
    let engine = Engine::default();
    let directories: Vec<PathBuf> = (0..8)
        .map(|index| {
            let directory = root.join(format!("isolation-{index}"));
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("mode.botwork"),
                "|mode| = Get Environment Variable |\"MODE\"|\nMode { Return |mode| }\n",
            )
            .unwrap();
            directory
        })
        .collect();
    let source = r#"Import |"mode.botwork"| As |settings|
|seen| = |@{ settings::Mode }|
|name| = |name + "!"|
Write File |"owned.txt"| Text |name|
|read| = Read File |"owned.txt"|
"#;
    move |iteration| {
        let before = engine.compiled_modules().statistics();
        let start = Instant::now();
        let runs = directories
            .iter()
            .enumerate()
            .map(|(index, directory)| {
                let options = RunOptions {
                    working_directory: Some(directory.clone()),
                    inherit_environment: false,
                    environment: BTreeMap::from([(
                        OsString::from("MODE"),
                        Some(OsString::from(format!("mode{index}-{iteration}"))),
                    )]),
                    variables: BTreeMap::from([(
                        "name".to_owned(),
                        Literal::String(format!("run{index}-{iteration}")),
                    )]),
                    ..RunOptions::default()
                };
                engine.run_source_async("isolation.botwork", source, options)
            })
            .collect();
        let results = runtime.block_on(async { bounded(join_all(runs)).await })?;
        let elapsed = start.elapsed();
        for (index, result) in results.iter().enumerate() {
            ensure!(
                result.result.is_ok(),
                "outcome",
                "run {index}: {:?}",
                result.result
            );
            let value = |name: &str| result.variables.get(name).map(ToString::to_string);
            let expected = format!("run{index}-{iteration}!");
            ensure!(
                value("seen") == Some(format!("mode{index}-{iteration}"))
                    && value("name") == Some(expected.clone())
                    && value("read") == Some(expected.clone()),
                "outcome",
                "run {index} saw another run's state: {:?}",
                result.variables
            );
            let written = fs::read_to_string(directories[index].join("owned.txt"))
                .map_err(|error| failure("outcome", error.to_string()))?;
            ensure!(
                written == expected,
                "outcome",
                "run {index} file holds {written:?}"
            );
        }
        let after = engine.compiled_modules().statistics();
        if iteration > 0 {
            ensure!(
                after.misses == before.misses && after.hits == before.hits + 8,
                "outcome",
                "module parses were not shared: {before:?} -> {after:?}"
            );
        }
        Ok(elapsed)
    }
}

/// Cancellation arrives while an operation is suspended inside `Try`; cleanup
/// runs, the run stops, and the operation's ownership is returned.
fn cancellation(runtime: &tokio::runtime::Runtime) -> impl FnMut(usize) -> Attempt + '_ {
    let notes = Arc::new(Mutex::new(Vec::new()));
    let (entered, mut held) = unbounded_channel();
    let budget = budget();
    let mut engine = Engine::default();
    engine.register_operation(note(&notes)).unwrap();
    engine
        .register_operation(hold("Hold", entered, &budget))
        .unwrap();
    let source = "Note |\"body\"|\nTry {\n    Hold\n} Finally {\n    Note |\"cleanup\"|\n}\nNote |\"after\"|\n";
    move |_| {
        notes.lock().unwrap().clear();
        let control = OperationControl::default();
        let options = RunOptions {
            control: control.clone(),
            ..RunOptions::default()
        };
        let (result, cancelled) = runtime.block_on(async {
            let run = bounded(engine.run_source_async("cancel.botwork", source, options));
            let trigger = async {
                let at = handshake(&mut held, "Hold entering").await?;
                control.cancel();
                Ok::<_, Failure>(at)
            };
            tokio::join!(run, trigger)
        });
        let cancelled = cancelled?;
        let result = result?;
        let latency = cancelled.elapsed();
        expect_code(&result, DiagnosticCode::Cancelled)?;
        let seen = notes.lock().unwrap().clone();
        ensure!(seen == ["body", "cleanup"], "outcome", "notes {seen:?}");
        ensure!(
            budget.usage() == OperationUsage::default(),
            "release",
            "ownership {:?}",
            budget.usage()
        );
        Ok(latency)
    }
}

/// A run deadline one hour away is reached on a paused clock: 59 one-minute
/// sleeps complete, the 60th meets the deadline, and cleanup runs.
fn deadline(runtime: &tokio::runtime::Runtime) -> impl FnMut(usize) -> Attempt + '_ {
    let notes = Arc::new(Mutex::new(Vec::new()));
    let (entered, mut ready) = unbounded_channel();
    let mut engine = Engine::default();
    engine.register_operation(note(&notes)).unwrap();
    engine
        .register_operation(
            NativeOperation::asynchronous(signature("Enter"), move |_, _| {
                let _ = entered.send(());
                async { Ok(Literal::Bool(true)) }
            })
            .unwrap(),
        )
        .unwrap();
    let source = "Enter\n|count| = |0|\nTry {\n    While |true| {\n        Sleep |60000|\n        |count| = |count + 1|\n    }\n} Finally {\n    Note |\"cleanup\"|\n}\n";
    move |_| {
        notes.lock().unwrap().clear();
        let options = RunOptions {
            timeout: Some(Duration::from_secs(3600)),
            ..RunOptions::default()
        };
        let start = Instant::now();
        let (result, paused) = runtime.block_on(async {
            let run = engine.run_source_async("deadline.botwork", source, options);
            let pause = async {
                // Pause once the run has entered, after its setup finished.
                let entered = tokio::time::timeout(HANDSHAKE, ready.recv()).await;
                tokio::time::pause();
                entered
            };
            // Virtual time reaches this bound only if the deadline is missed.
            let bounded = tokio::time::timeout(Duration::from_secs(7200), run);
            let joined = tokio::join!(bounded, pause);
            tokio::time::resume();
            joined
        });
        let elapsed = start.elapsed();
        ensure!(
            matches!(paused, Ok(Some(()))),
            "handshake",
            "the run never entered"
        );
        let result = result.map_err(|_| failure("bound", "the paused deadline never fired"))?;
        expect_code(&result, DiagnosticCode::Timeout)?;
        let count = result.variables.get("count").map(ToString::to_string);
        ensure!(count.as_deref() == Some("59"), "outcome", "count {count:?}");
        let seen = notes.lock().unwrap().clone();
        ensure!(seen == ["cleanup"], "outcome", "notes {seen:?}");
        Ok(elapsed)
    }
}

/// A blocking operation that ignores its control is abandoned after the stop
/// grace, then returns its permit and ownership once released.
fn abandonment(runtime: &tokio::runtime::Runtime) -> impl FnMut(usize) -> Attempt + '_ {
    let (entered, mut held) = unbounded_channel();
    let (release, released) = mpsc::channel::<()>();
    let released = Arc::new(Mutex::new(released));
    let returned = Arc::new(AtomicUsize::new(0));
    let budget = budget();
    let mut engine = Engine::default();
    {
        let returned = Arc::clone(&returned);
        engine
            .register_operation(
                NativeOperation::blocking(
                    signature("Block"),
                    NonZeroUsize::new(1).unwrap(),
                    move |_, _| {
                        let _ = entered.send(());
                        let _ = released.lock().unwrap().recv();
                        returned.fetch_add(1, Ordering::SeqCst);
                        Ok(Literal::Bool(true))
                    },
                )
                .unwrap()
                .with_ownership_budget(budget.clone()),
            )
            .unwrap();
    }
    let mut calls = 0;
    move |_| {
        let control = OperationControl::default().with_stop_grace(Duration::from_millis(20));
        let options = RunOptions {
            control: control.clone(),
            ..RunOptions::default()
        };
        let (result, cancelled) = runtime.block_on(async {
            let run = bounded(engine.run_source_async("block.botwork", "Block", options));
            let trigger = async {
                let at = handshake(&mut held, "Block entering").await?;
                control.cancel();
                Ok::<_, Failure>(at)
            };
            tokio::join!(run, trigger)
        });
        calls += 1;
        let outcome = (|| {
            let cancelled = cancelled?;
            let result = result?;
            let latency = cancelled.elapsed();
            let text = expect_code(&result, DiagnosticCode::Cancelled)?;
            ensure!(
                text.contains("was abandoned"),
                "outcome",
                "no abandonment cause: {text}"
            );
            ensure!(
                latency >= Duration::from_millis(20),
                "outcome",
                "abandoned before the grace: {latency:?}"
            );
            ensure!(
                returned.load(Ordering::SeqCst) + 1 == calls,
                "outcome",
                "the callback returned early"
            );
            Ok(latency)
        })();
        // Release the callback whatever happened, so no worker stays blocked.
        let _ = release.send(());
        let latency = outcome?;
        wait_until("abandonment", "the released callback returning", || {
            returned.load(Ordering::SeqCst) == calls && budget.usage() == OperationUsage::default()
        })
        .map_err(|error| failure("release", error.message))?;
        Ok(latency)
    }
}

/// An HTTP request to an in-process server that never answers is cancelled
/// once the server has read it, and the connection is closed.
fn http(runtime: &tokio::runtime::Runtime) -> impl FnMut(usize) -> Attempt + '_ {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let (received_sender, mut received) = unbounded_channel();
    let (closed_sender, mut closed) = unbounded_channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let (received, closed) = (received_sender.clone(), closed_sender.clone());
            thread::spawn(move || {
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => return,
                        Ok(count) => request.extend_from_slice(&buffer[..count]),
                    }
                }
                let _ = received.send(());
                // Never answer; report when the client closes the connection.
                while matches!(stream.read(&mut buffer), Ok(count) if count > 0) {}
                let _ = closed.send(());
            });
        }
    });
    let engine = Engine::default();
    let source = format!("HTTP Request |\"GET\"| To |\"{url}\"|");
    move |_| {
        let control = OperationControl::default();
        let options = RunOptions {
            control: control.clone(),
            ..RunOptions::default()
        };
        let (result, cancelled, released) = runtime.block_on(async {
            let run = bounded(engine.run_source_async("http.botwork", &source, options));
            let trigger = async {
                let at = handshake(&mut received, "the request arriving").await?;
                control.cancel();
                Ok::<_, Failure>(at)
            };
            let (result, cancelled) = tokio::join!(run, trigger);
            let released = handshake(&mut closed, "the connection closing").await;
            (result, cancelled, released)
        });
        let cancelled = cancelled?;
        let result = result?;
        let latency = cancelled.elapsed();
        expect_code(&result, DiagnosticCode::Cancelled)?;
        released.map_err(|error| failure("release", error.message))?;
        Ok(latency)
    }
}

/// A process statement is cancelled once its shell has recorded its PID; the
/// child is stopped and reaped before the run returns.
fn process<'a>(
    runtime: &'a tokio::runtime::Runtime,
    root: &Path,
) -> impl FnMut(usize) -> Attempt + 'a {
    let directory = root.join("process");
    fs::create_dir_all(&directory).unwrap();
    let engine = Engine::default();
    let source =
        r#"Run Process |"/bin/sh"| With Arguments |["-c", "echo $$ > child.pid; exec sleep 60"]|"#;
    move |_| {
        let marker = directory.join("child.pid");
        let _ = fs::remove_file(&marker);
        let control = OperationControl::default();
        let options = RunOptions {
            working_directory: Some(directory.clone()),
            control: control.clone(),
            ..RunOptions::default()
        };
        let (result, cancelled) = runtime.block_on(async {
            let run = bounded(engine.run_source_async("process.botwork", source, options));
            let trigger = async {
                let deadline = Instant::now() + HANDSHAKE;
                let pid = loop {
                    if let Some(pid) = fs::read_to_string(&marker)
                        .ok()
                        .filter(|text| text.ends_with('\n'))
                        .and_then(|text| text.trim().parse::<u32>().ok())
                    {
                        break pid;
                    }
                    ensure!(
                        Instant::now() < deadline,
                        "handshake",
                        "the child never started"
                    );
                    tokio::time::sleep(Duration::from_millis(1)).await;
                };
                control.cancel();
                Ok::<_, Failure>((Instant::now(), pid))
            };
            tokio::join!(run, trigger)
        });
        let (cancelled, pid) = cancelled?;
        let result = result?;
        let latency = cancelled.elapsed();
        expect_code(&result, DiagnosticCode::Cancelled)?;
        ensure!(!alive(pid), "release", "the child {pid} is still running");
        ensure!(
            !children().contains(&pid),
            "release",
            "the child {pid} was not reaped"
        );
        Ok(latency)
    }
}

/// The CLI stops at its first SIGINT once the script is inside `Try`, runs
/// `Finally`, and reports the cancellation. The marker is written inside `Try`:
/// a file's existence does not mean the statement writing it has finished.
fn interrupt(root: &Path) -> impl FnMut(usize) -> Attempt {
    let directory = root.join("interrupt");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("interrupt.botwork"),
        "Try {\n    Write File |\"started\"| Text |\"yes\"|\n    Sleep |60000|\n} Finally {\n    Write File |\"cleaned\"| Text |\"yes\"|\n}\n",
    )
    .unwrap();
    move |_| {
        let (started, cleaned, stderr) = (
            directory.join("started"),
            directory.join("cleaned"),
            directory.join("stderr.txt"),
        );
        let _ = fs::remove_file(&started);
        let _ = fs::remove_file(&cleaned);
        let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(["--file", "interrupt.botwork"])
            .current_dir(&directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&stderr).map_err(|error| failure("setup", error.to_string()))?)
            .spawn()
            .map_err(|error| failure("setup", error.to_string()))?;
        let result = (|| {
            wait_until("interrupt", "the script starting", || started.exists())?;
            ensure!(
                unsafe { libc::kill(child.id() as i32, libc::SIGINT) } == 0,
                "setup",
                "SIGINT failed"
            );
            let signalled = Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                ensure!(
                    signalled.elapsed() < HANDSHAKE,
                    "bound",
                    "the CLI did not stop"
                );
                thread::sleep(Duration::from_millis(1));
            };
            let latency = signalled.elapsed();
            let text = fs::read_to_string(&stderr).unwrap_or_default();
            ensure!(
                status.code() == Some(1) && text.contains("[BW5001]"),
                "outcome",
                "status {status:?}: {text}"
            );
            ensure!(cleaned.exists(), "outcome", "Finally did not run: {text}");
            Ok(latency)
        })();
        if result.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        result
    }
}

#[test]
fn isolation_and_cancellation_scenarios_repeat_without_flakes_or_leaks() {
    let iterations = std::env::var("BOTWORK_STRESS_ITERATIONS")
        .ok()
        .map(|value| value.parse::<usize>().expect("an iteration count"))
        .unwrap_or(10);
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime();
    let started = Instant::now();
    let scenarios = vec![
        exercise("isolation", iterations, isolation(&runtime, root.path())),
        exercise("cancellation", iterations, cancellation(&runtime)),
        exercise("paused-clock-deadline", iterations, deadline(&runtime)),
        exercise("blocking-abandonment", iterations, abandonment(&runtime)),
        exercise("http-cancellation", iterations, http(&runtime)),
        exercise(
            "process-cancellation",
            iterations,
            process(&runtime, root.path()),
        ),
        exercise("cli-interrupt", iterations, interrupt(root.path())),
    ];
    let failures: Vec<&Value> = scenarios
        .iter()
        .flat_map(|scenario| scenario["failures"].as_array().unwrap())
        .collect();
    let report = json!({
        "iterations": iterations,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "target_env": if cfg!(target_env = "musl") { "musl" } else { "gnu" },
        "seconds": started.elapsed().as_secs_f64(),
        "failures": failures.len(),
        "scenarios": scenarios,
    });
    if let Some(path) = std::env::var_os("BOTWORK_STRESS_REPORT") {
        fs::write(path, serde_json::to_string_pretty(&report).unwrap() + "\n").unwrap();
    }
    assert!(failures.is_empty(), "{report:#}");
}
