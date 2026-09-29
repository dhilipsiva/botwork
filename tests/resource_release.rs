#![cfg(target_os = "linux")]
//! Every run releases its descriptors, threads, and child processes after a
//! failure, a cancellation, or a timeout. This binary holds one test, so the
//! process-wide counts it reads belong to the scenarios alone.
use botwork::core::{
    diagnostic::DiagnosticCode,
    operation::OperationControl,
    run::{Engine, RunOptions},
};
use std::{
    collections::BTreeSet,
    fs,
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

type Accepted = Arc<Mutex<Vec<TcpStream>>>;

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
        let path = task.unwrap().path().join("children");
        if let Ok(text) = fs::read_to_string(path) {
            pids.extend(
                text.split_whitespace()
                    .map(|pid| pid.parse::<u32>().unwrap()),
            );
        }
    }
    pids
}

/// Wait up to five seconds for descriptors, threads, and children to return
/// to at most their earlier levels.
fn assert_released(scenario: &str, before: (usize, usize, BTreeSet<u32>)) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = (descriptors(), threads(), children());
        if now.0 <= before.0 && now.1 <= before.1 && now.2.is_subset(&before.2) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{scenario}: descriptors {} -> {}, threads {} -> {}, children {:?} -> {:?}",
            before.0,
            now.0,
            before.1,
            now.1,
            before.2,
            now.2
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_no_children(scenario: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !children().is_empty() {
        assert!(
            Instant::now() < deadline,
            "{scenario}: children {:?}",
            children()
        );
        thread::sleep(Duration::from_millis(20));
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

struct Scenario {
    name: &'static str,
    source: String,
    /// Cancel once this returns true, or rely on the timeout when None.
    cancel_when: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    timeout: Option<Duration>,
    code: DiagnosticCode,
}

/// Run a scenario once and return how long the stop took to arrive.
fn run(runtime: &tokio::runtime::Runtime, directory: &Path, scenario: &Scenario) -> Duration {
    let control = OperationControl::default();
    let options = RunOptions {
        working_directory: Some(directory.to_path_buf()),
        control: control.clone(),
        timeout: scenario.timeout,
        ..RunOptions::default()
    };
    let cancel_when = scenario.cancel_when.as_ref();
    let stopped = Arc::new(Mutex::new(None));
    thread::scope(|scope| {
        if let Some(ready) = cancel_when {
            let stopped = Arc::clone(&stopped);
            let control = control.clone();
            scope.spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(20);
                while !ready() {
                    assert!(Instant::now() < deadline, "the scenario never became ready");
                    thread::sleep(Duration::from_millis(5));
                }
                *stopped.lock().unwrap() = Some(Instant::now());
                control.cancel();
            });
        }
        let start = Instant::now();
        let result = runtime.block_on(Engine::default().run_source_async(
            "scenario.botwork",
            &scenario.source,
            options,
        ));
        let finished = Instant::now();
        let error = result.result.expect_err(scenario.name);
        assert_eq!(error.code(), scenario.code, "{}: {error}", scenario.name);
        let stopped = stopped.lock().unwrap().unwrap_or(start);
        finished.duration_since(stopped)
    })
}

fn scenarios(directory: &Path, server: &str, accepted: &Accepted) -> Vec<Scenario> {
    let connected = {
        let accepted = Arc::clone(accepted);
        move || !accepted.lock().unwrap().is_empty()
    };
    let marker = directory.join("started");
    let process_started = {
        let marker = directory.join("child.pid");
        move || marker.exists()
    };
    let cleanup_started = {
        let marker = marker.clone();
        move || marker.exists()
    };
    vec![
        Scenario {
            name: "nested statements and cleanup",
            source: r#"Deep |depth| {
    For |item| In |[1, 2, 3]| {
        Try {
            Eventually |{timeout_ms: 60000, interval_ms: 10}| {
                Write File |"started"| Text |"yes"|
                Sleep |60000|
            }
        } Finally {
            Write File |"cleanup.txt"| Text |@{ Read File |"started"| }|
        }
    }
}
Deep |1|"#
                .into(),
            cancel_when: Some(Box::new(cleanup_started)),
            timeout: None,
            code: DiagnosticCode::Cancelled,
        },
        Scenario {
            name: "process statement",
            source: r#"Run Process |"/bin/sh"| With Arguments |["-c", "echo $$ > child.pid; exec sleep 60"]|"#
                .into(),
            cancel_when: Some(Box::new(process_started)),
            timeout: None,
            code: DiagnosticCode::Cancelled,
        },
        Scenario {
            name: "HTTP request",
            source: format!(r#"HTTP Request |"GET"| To |"{server}"|"#),
            cancel_when: Some(Box::new(connected)),
            timeout: None,
            code: DiagnosticCode::Cancelled,
        },
        Scenario {
            name: "deadline in nested custom calls",
            source: "Inner { Sleep |60000| }\nOuter { While |true| { Inner } }\nOuter".into(),
            cancel_when: None,
            timeout: Some(Duration::from_millis(100)),
            code: DiagnosticCode::Timeout,
        },
        Scenario {
            name: "failure after file effects",
            source: r#"For |index| In |[1, 2, 3]| {
    Write File |"effect.txt"| Text |"x"|
    |text| = Read File |"effect.txt"|
}
Fail |"stop"|"#
                .into(),
            cancel_when: None,
            timeout: None,
            code: DiagnosticCode::ExplicitFailure,
        },
    ]
}

#[test]
fn stopped_and_failed_runs_release_descriptors_threads_and_children() {
    let directory = tempfile::tempdir().unwrap();
    // A server that accepts and never answers, so a request can only be stopped.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}/", listener.local_addr().unwrap());
    let accepted: Accepted = Arc::new(Mutex::new(Vec::new()));
    {
        let accepted = Arc::clone(&accepted);
        thread::spawn(move || {
            for stream in listener.incoming() {
                accepted.lock().unwrap().push(stream.unwrap());
            }
        });
    }
    let runtime = runtime();
    for scenario in scenarios(directory.path(), &server, &accepted) {
        // A first run warms pools and caches; later runs must not grow them.
        run(&runtime, directory.path(), &scenario);
        let _ = fs::remove_file(directory.path().join("started"));
        let _ = fs::remove_file(directory.path().join("child.pid"));
        accepted.lock().unwrap().clear();
        wait_for_no_children(scenario.name);
        thread::sleep(Duration::from_millis(300));
        let before = (descriptors(), threads(), children());
        for _ in 0..5 {
            let latency = run(&runtime, directory.path(), &scenario);
            assert!(
                latency < Duration::from_secs(5),
                "{}: stop took {latency:?}",
                scenario.name
            );
            if scenario.name == "process statement" {
                let pid: u32 = fs::read_to_string(directory.path().join("child.pid"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                assert!(!alive(pid), "the cancelled child {pid} is stopped");
            }
            if scenario.name == "nested statements and cleanup" {
                assert_eq!(
                    fs::read_to_string(directory.path().join("cleanup.txt")).unwrap(),
                    "yes",
                    "Finally ran after the cancellation"
                );
            }
            let _ = fs::remove_file(directory.path().join("started"));
            let _ = fs::remove_file(directory.path().join("child.pid"));
            accepted.lock().unwrap().clear();
        }
        assert_released(scenario.name, before);
    }
}
