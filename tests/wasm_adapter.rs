//! WebAssembly modules through Wasmtime (decision D8): loading, value
//! conversion, failures and traps, instances of their own, denied
//! capabilities, limits, and stops. The tests call tests/wasm/statements.wasm,
//! built from tests/wasm/guest, or the component `BOTWORK_WASM_GUEST` names.
#![cfg(feature = "wasm")]

use botwork::core::{
    diagnostic::{Diagnostic, DiagnosticCode},
    grammar::Literal,
    run::{Engine, RunOptions, RunResult},
};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

/// The component under test.
fn guest() -> Vec<u8> {
    let path = std::env::var_os("BOTWORK_WASM_GUEST")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/wasm/statements.wasm")
        });
    fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

struct Workspace(tempfile::TempDir);

impl Workspace {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("statements.wasm"), guest()).unwrap();
        Self(directory)
    }

    fn run(&self, source: &str) -> RunResult {
        self.run_with(source, RunOptions::default())
    }

    /// WASM statements are blocking operations, so they run in the engine's
    /// asynchronous mode, as the CLI runs scripts.
    fn run_with(&self, source: &str, options: RunOptions) -> RunResult {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(Engine::default().run_source_async(
            "main.botwork",
            &format!("Import |\"statements.wasm\"| As |wasm|\n{source}"),
            RunOptions {
                working_directory: Some(self.0.path().into()),
                ..options
            },
        ));
        runtime.shutdown_background();
        result
    }
}

fn failure(result: &RunResult) -> &Diagnostic {
    result.result.as_ref().expect_err("the run fails")
}

fn value(result: &RunResult, name: &str) -> Literal {
    assert!(result.result.is_ok(), "{:?}", result.result);
    result.variables[name].clone()
}

fn with_wasm(change: impl FnOnce(&mut botwork::core::run::WasmLimits)) -> RunOptions {
    let mut options = RunOptions::default();
    change(&mut options.limits.wasm);
    options
}

#[test]
fn values_cross_into_wasm_and_back_unchanged() {
    let workspace = Workspace::new();
    let result = workspace.run(
        r#"|nothing| = wasm::Env |"BOTWORK_UNSET"|
|values| = |[nothing, 2147483647, -2147483648, 1.5, true, "Grüße, 世界", [1, [2]], {"b": 1, "a": {"é": [false]}}, [], {}]|
|back| = wasm::Echo |values|
Assert |back| Equals |values|
|laid| = wasm::Make |"depth-first"|"#,
    );
    assert_eq!(
        value(&result, "back").to_string(),
        value(&result, "values").to_string()
    );
    // The guest lays values out breadth first and the host depth first; each
    // accepts the other's layout.
    let Literal::Map(laid) = value(&result, "laid") else {
        panic!("a map");
    };
    assert_eq!(laid.len(), 2);
    assert_eq!(laid["b"].to_string(), "[\"deep\"]");
    assert_eq!(laid["a"].to_string(), "true");
}

#[test]
fn values_that_are_not_trees_or_not_botwork_values_are_refused() {
    let workspace = Workspace::new();
    for (kind, reason) in [
        ("empty", "a value with no nodes"),
        ("cycle", "node 0 names node 0, which is not after it"),
        ("dangling", "node 0 names node 5, which is not after it"),
        ("shared", "names node 1 twice"),
        ("orphan", "node 1 is not in its tree"),
        ("nan", "the float NaN, which is not finite"),
        ("infinity", "the float inf, which is not finite"),
        ("duplicate", "a map with the key \"a\" twice"),
        ("deep", "a value nested more than 64 levels deep"),
        ("wide", "a value of more than 65536 nodes"),
        ("long", "a value beyond the value limits"),
    ] {
        let result = workspace.run(&format!("|value| = wasm::Make |\"{kind}\"|"));
        let error = failure(&result);
        assert_eq!(error.code(), DiagnosticCode::Native, "{kind}: {error}");
        let text = error.to_string();
        assert!(
            text.contains("The WASM statement returned") && text.contains(reason),
            "{kind}: {text}"
        );
    }
}

#[test]
fn failures_become_catchable_diagnostics() {
    let workspace = Workspace::new();
    let result = workspace.run(r#"|outcome| = wasm::Fail with |"bad input"|"#);
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Native);
    assert!(error.to_string().contains("bad input"), "{error}");
    let result = workspace.run("|held| = wasm::Insist |false|");
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Assertion);
    assert!(
        error.to_string().contains("the condition does not hold"),
        "{error}"
    );
    let result = workspace.run(
        r#"|caught| = |"no"|
Try {
    wasm::Fail with |"bad input"|
} Catch |error| {
    |caught| = |"yes"|
}
|held| = wasm::Insist |true|"#,
    );
    assert_eq!(value(&result, "caught").to_string(), "yes");
    assert_eq!(value(&result, "held").to_string(), "true");
}

#[test]
fn traps_report_what_the_module_wrote_to_stderr_and_where_it_was() {
    let workspace = Workspace::new();
    let result = workspace.run(r#"|outcome| = wasm::Panic with |"the gears slipped"|"#);
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Native);
    let text = error.to_string();
    // The trap, the panic message Rust wrote to stderr, and the backtrace.
    assert!(text.contains("WebAssembly trap: "), "{text}");
    assert!(text.contains("panicked at"), "{text}");
    assert!(text.contains("the gears slipped"), "{text}");
    assert!(text.contains("botwork_test_statements"), "{text}");
}

#[test]
fn each_call_runs_in_an_instance_of_its_own() {
    let workspace = Workspace::new();
    let result = workspace.run("|first| = wasm::Count\n|second| = wasm::Count");
    assert_eq!(value(&result, "first").to_string(), "1");
    assert_eq!(value(&result, "second").to_string(), "1");
}

#[test]
fn modules_get_clocks_but_no_files_network_or_environment() {
    let workspace = Workspace::new();
    let secret = workspace.0.path().join("secret.txt");
    fs::write(&secret, "hidden").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let result = workspace.run_with(
        &format!(
            r#"|now| = wasm::Clock
|home| = wasm::Env |"BOTWORK_CHOSEN"|
|read| = |"no"|
Try {{
    |text| = wasm::Read file |"{}"|
    |read| = |text|
}} Catch |error| {{
    |read| = |"denied"|
}}
|connected| = |"no"|
Try {{
    |connected| = wasm::Connect |"{address}"|
}} Catch |error| {{
    |connected| = |"denied"|
}}"#,
            secret.display().to_string().replace('\\', "\\\\")
        ),
        RunOptions {
            environment: BTreeMap::from([("BOTWORK_CHOSEN".into(), Some("yes".into()))]),
            ..RunOptions::default()
        },
    );
    let Literal::Int(now) = value(&result, "now") else {
        panic!("the clock is an Int");
    };
    // Seconds since 1970, after this test was written.
    assert!(now > 1_790_000_000, "{now}");
    assert!(matches!(value(&result, "home"), Literal::None));
    assert_eq!(value(&result, "read").to_string(), "denied");
    assert_eq!(value(&result, "connected").to_string(), "denied");
}

#[test]
fn fuel_memory_and_output_bound_a_call() {
    let workspace = Workspace::new();
    let limited = |change: fn(&mut botwork::core::run::WasmLimits), source: &str| {
        workspace.run_with(source, with_wasm(change))
    };
    // Within each limit, calls succeed.
    let result = limited(|limits| limits.fuel = 1_000_000, "|n| = wasm::Burn |1000|");
    assert!(result.result.is_ok(), "{:?}", result.result);
    let result = limited(
        |limits| limits.memory_bytes = 16 << 20,
        "|n| = wasm::Grow |1|",
    );
    assert_eq!(value(&result, "n").to_string(), "1");
    let result = limited(
        |limits| limits.output_bytes = 1024,
        r#"|n| = wasm::Print |"short"|"#,
    );
    assert_eq!(value(&result, "n").to_string(), "short");
    // Beyond each, the call stops with BW8001 naming the limit.
    for (change, source, resource) in [
        (
            (|limits| limits.fuel = 1_000_000) as fn(&mut botwork::core::run::WasmLimits),
            "|n| = wasm::Burn |100000000|",
            "WASM fuel (limit 1000000)",
        ),
        (
            |limits| limits.memory_bytes = 16 << 20,
            "|n| = wasm::Grow |64|",
            "WASM memory bytes (limit 16777216)",
        ),
        (
            |limits| limits.output_bytes = 1024,
            &format!("|n| = wasm::Print |\"{}\"|", "x".repeat(2000)),
            "WASM stdout bytes (limit 1024)",
        ),
    ] {
        let result = limited(change, source);
        let error = failure(&result);
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit, "{error}");
        assert!(error.to_string().contains(resource), "{error}");
    }
}

#[test]
fn a_stop_interrupts_a_busy_call() {
    let workspace = Workspace::new();
    let start = Instant::now();
    let result = workspace.run_with(
        "wasm::Spin",
        RunOptions {
            timeout: Some(Duration::from_secs(1)),
            ..RunOptions::default()
        },
    );
    let elapsed = start.elapsed();
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    // The epoch interrupt ended the call within the grace: nothing was
    // abandoned, and the deadline is the only failure.
    assert!(!error.to_string().contains("abandoned"), "{error}");
    assert!(error.causes.is_empty(), "{error}");
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

#[test]
fn loading_failures_are_import_errors() {
    let workspace = Workspace::new();
    let directory = workspace.0.path();
    fs::write(directory.join("text.wasm"), "not wasm").unwrap();
    // A core module, not a component.
    fs::write(directory.join("core.wasm"), b"\0asm\x01\0\0\0").unwrap();
    // A component that exports nothing.
    fs::write(directory.join("empty.wasm"), b"\0asm\x0d\0\x01\0").unwrap();
    for (file, reason) in [
        ("text.wasm", "not a WebAssembly component"),
        ("core.wasm", "not a WebAssembly component"),
        ("empty.wasm", "botwork:statements/statements"),
        ("missing.wasm", "missing.wasm"),
        (
            "https://example.com/tools.wasm",
            "must name a local WebAssembly file",
        ),
    ] {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(Engine::default().run_source_async(
            "main.botwork",
            &format!("Import |\"{file}\"| As |broken|"),
            RunOptions {
                working_directory: Some(directory.into()),
                ..RunOptions::default()
            },
        ));
        runtime.shutdown_background();
        let error = failure(&result);
        assert_eq!(error.code(), DiagnosticCode::ImportRead, "{file}: {error}");
        assert!(error.to_string().contains(reason), "{file}: {error}");
    }
}

#[test]
fn wasm_limits_are_checked_before_a_run() {
    let result = Workspace::new().run_with(
        "|n| = 1",
        with_wasm(|limits| limits.memory_bytes = (5_u64 << 30) as usize),
    );
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    assert!(
        error.to_string().contains("WASM memory cannot exceed"),
        "{error}"
    );
}

#[test]
fn the_cli_runs_wasm_statements() {
    let workspace = Workspace::new();
    fs::write(
        workspace.0.path().join("main.botwork"),
        "Import |\"statements.wasm\"| As |wasm|\n|n| = wasm::Echo |[1, \"two\"]|\nLog |n|\n",
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(["--file", "main.botwork"])
        .current_dir(workspace.0.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[1, \"two\"]\n");
}
