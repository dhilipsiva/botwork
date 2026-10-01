//! JavaScript modules through Node (decision D7): loading, value conversion,
//! exceptions, calls in processes of their own, and stops. The tests need Node
//! on PATH; without it they are skipped unless `BOTWORK_REQUIRE_NODE` is set,
//! as it is in continuous integration.

use botwork::core::{
    diagnostic::{Diagnostic, DiagnosticCode},
    grammar::Literal,
    run::{Engine, RunOptions, RunResult},
};
use std::{
    collections::BTreeMap,
    fs,
    time::{Duration, Instant},
};

const HELPERS: &str = r#"
import assert from "node:assert";

let count = 0;
// Printing while loading, as while calling, leaves the response intact.
console.log("loading helpers");

class Point {
    constructor(x) { this.x = x; }
}

function nested(depth) {
    let value = 0;
    for (let i = 0; i < depth; i++) value = [value];
    return value;
}

export const statements = {
    "Echo |value|": (value) => value,
    "Nothing": () => null,
    // UTF-16 order puts the emoji before the fullwidth z; UTF-8 byte order,
    // which the protocol requires, puts it after.
    "Unsorted": () => ({ "z": 1, "a": 2, "😀": 4, "ｚ": 3 }),
    "Describe |value|": (value) =>
        value === null ? "null" : Array.isArray(value) ? "array" : typeof value,
    "Make |kind|": (kind) => ({
        bigint: () => 10n,
        map: () => new Map(),
        set: () => new Set(),
        date: () => new Date(0),
        function: () => () => 1,
        nan: () => NaN,
        infinity: () => Infinity,
        wide: () => 1e39,
        point: () => new Point(1),
        deep: () => nested(100),
    })[kind](),
    "Fail with |message|": (message) => { throw new RangeError(message); },
    "Throw text": () => { throw "plain text"; },
    "Insist |condition|": (condition) => { assert.ok(condition, "the condition does not hold"); return true; },
    "Count": () => ++count,
    "Later |x|": async (x) => { await new Promise((resolve) => setTimeout(resolve, 10)); return x + 1; },
    "Spin": () => { for (;;) {} },
    "Quick": () => "done",
    "Env |name|": (name) => process.env[name] ?? null,
    "Chatty |x|": (x) => {
        console.log("calling with", x);
        console.info({ x });
        process.stdout.write("raw output\n");
        return x + 1;
    },
    "Print |bytes|": (bytes) => { process.stdout.write("x".repeat(bytes)); return bytes; },
};
"#;

/// Whether Node is on PATH, failing when CI requires it.
fn node() -> bool {
    let found = std::process::Command::new(if cfg!(windows) { "node.exe" } else { "node" })
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    assert!(
        found || std::env::var_os("BOTWORK_REQUIRE_NODE").is_none(),
        "BOTWORK_REQUIRE_NODE is set but Node was not found"
    );
    found
}

struct Workspace(tempfile::TempDir);

impl Workspace {
    fn new(files: &[(&str, &str)]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        for (name, text) in files {
            fs::write(directory.path().join(name), text).unwrap();
        }
        Self(directory)
    }

    fn run(&self, source: &str) -> RunResult {
        self.run_with(source, RunOptions::default())
    }

    /// JavaScript statements are isolated operations, so they run in the
    /// engine's asynchronous mode, as the CLI runs scripts.
    fn run_with(&self, source: &str, options: RunOptions) -> RunResult {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(Engine::default().run_source_async(
            "main.botwork",
            source,
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

#[test]
fn values_cross_into_javascript_and_back_unchanged() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[("helpers.mjs", HELPERS)]);
    let source = r#"Import |"helpers.mjs"| As |js|
|nothing| = js::Nothing
|values| = |[nothing, 2147483647, -2147483648, 1.5, true, "Grüße, 世界", [1, [2]], {"b": 1, "a": {"é": [false]}}]|
|back| = js::Echo |values|
Assert |back| Equals |values|
|mixed| = js::Unsorted
Assert |mixed| Equals |{"a": 2, "z": 1, "ｚ": 3, "😀": 4}|
|kinds| = |[]|
For |item| In |values| {
    |kind| = js::Describe |item|
    |kinds| = |kinds + [kind]|
}
"#;
    let result = workspace.run(source);
    assert_eq!(
        value(&result, "kinds").to_string(),
        r#"["null", "number", "number", "number", "boolean", "string", "array", "object"]"#
    );
}

#[test]
fn values_without_an_exact_botwork_equivalent_are_refused() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[("helpers.mjs", HELPERS)]);
    for (kind, expected) in [
        ("bigint", "the BigInt 10, which has no Botwork equivalent"),
        ("map", "a Map, which has no Botwork equivalent"),
        ("set", "a Set, which has no Botwork equivalent"),
        ("date", "a Date, which has no Botwork equivalent"),
        ("function", "a function, which has no Botwork equivalent"),
        ("nan", "the number NaN, which no finite 32-bit float holds"),
        (
            "infinity",
            "the number Infinity, which no finite 32-bit float holds",
        ),
        (
            "wide",
            "the number 1e+39, which no finite 32-bit float holds",
        ),
        ("point", "a Point, which has no Botwork equivalent"),
        ("deep", "a value nested more than 64 levels deep"),
    ] {
        let result = workspace.run(&format!(
            "Import |\"helpers.mjs\"| As |js|\njs::Make |\"{kind}\"|"
        ));
        let error = failure(&result);
        assert_eq!(error.code(), DiagnosticCode::Native, "{kind}: {error}");
        assert!(
            error
                .to_string()
                .contains(&format!("The JavaScript statement returned {expected}")),
            "{kind}: {error}"
        );
    }
}

#[test]
fn exceptions_become_catchable_diagnostics_with_their_stack() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[("helpers.mjs", HELPERS)]);
    let result = workspace.run("Import |\"helpers.mjs\"| As |js|\njs::Fail with |\"bad input\"|");
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Native);
    let text = error.to_string();
    assert!(text.contains("JavaScript RangeError: bad input"), "{text}");
    assert!(text.contains("helpers.mjs"), "{text}");
    let result = workspace.run("Import |\"helpers.mjs\"| As |js|\njs::Throw text");
    assert!(
        failure(&result)
            .to_string()
            .contains("JavaScript threw a String: plain text"),
        "{}",
        failure(&result)
    );
    // An AssertionError is an assertion failure.
    let result = workspace.run("Import |\"helpers.mjs\"| As |js|\njs::Insist |false|");
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Assertion);
    assert!(error.to_string().contains("the condition does not hold"));
    let result = workspace.run(
        "Import |\"helpers.mjs\"| As |js|\n|caught| = |false|\nTry {\n    js::Fail with |\"x\"|\n} Catch |error| {\n    |caught| = |true|\n}",
    );
    assert_eq!(value(&result, "caught").to_string(), "true");
}

#[test]
fn each_call_runs_in_a_process_of_its_own() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[("helpers.mjs", HELPERS)]);
    // Module state lasts one call.
    let result = workspace
        .run("Import |\"helpers.mjs\"| As |js|\n|first| = js::Count\n|second| = js::Count");
    assert_eq!(value(&result, "first").to_string(), "1");
    assert_eq!(value(&result, "second").to_string(), "1");
}

#[test]
fn printing_leaves_the_response_intact() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[("helpers.mjs", HELPERS)]);
    let result = workspace.run("Import |\"helpers.mjs\"| As |js|\n|n| = js::Chatty |1|");
    assert_eq!(value(&result, "n").to_string(), "2");
    // Printed output is bounded like any worker's stderr.
    let result = workspace.run("Import |\"helpers.mjs\"| As |js|\n|n| = js::Print |2000000|");
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(error.to_string().contains("worker stderr bytes"), "{error}");
}

#[test]
fn calls_see_the_runs_environment() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[("helpers.mjs", HELPERS)]);
    let result = workspace.run_with(
        "Import |\"helpers.mjs\"| As |js|\n|chosen| = js::Env |\"BOTWORK_CHOSEN\"|",
        RunOptions {
            environment: BTreeMap::from([("BOTWORK_CHOSEN".into(), Some("yes".into()))]),
            ..RunOptions::default()
        },
    );
    assert_eq!(value(&result, "chosen").to_string(), "yes");
}

#[test]
fn async_functions_and_commonjs_modules_work() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[
        ("helpers.mjs", HELPERS),
        (
            "common.cjs",
            "module.exports.statements = { \"Double |x|\": (x) => x * 2 };\n",
        ),
    ]);
    let result = workspace.run(
        "Import |\"helpers.mjs\"| As |js|\nImport |\"common.cjs\"| As |cjs|\n|n| = js::Later |41|\n|d| = cjs::Double |21|",
    );
    assert_eq!(value(&result, "n").to_string(), "42");
    assert_eq!(value(&result, "d").to_string(), "42");
}

#[test]
fn a_stop_ends_the_call_and_its_process() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[("helpers.mjs", HELPERS)]);
    let start = Instant::now();
    let result = workspace.run_with(
        "Import |\"helpers.mjs\"| As |js|\njs::Spin",
        RunOptions {
            timeout: Some(Duration::from_secs(1)),
            ..RunOptions::default()
        },
    );
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Timeout, "{error}");
    // The supervisor kills the process at the deadline, well inside the
    // stop grace, and the stop is the only failure reported.
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
    assert!(error.causes.is_empty(), "{error}");
    let result = workspace.run("Import |\"helpers.mjs\"| As |js|\n|done| = js::Quick");
    assert_eq!(value(&result, "done").to_string(), "done");
}

#[test]
fn loading_failures_are_import_errors() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[
        ("broken.mjs", "export const statements = {\n"),
        ("empty.mjs", "export const other = 1;\n"),
        ("value.mjs", "export const statements = { \"Seven\": 7 };\n"),
        (
            "arity.mjs",
            "export const statements = { \"Pair |a|\": (a, b) => a };\n",
        ),
        (
            "header.mjs",
            "export const statements = { \"Bad |\": () => 1 };\n",
        ),
    ]);
    for (file, code, expected) in [
        ("broken.mjs", DiagnosticCode::ImportRead, "SyntaxError"),
        (
            "empty.mjs",
            DiagnosticCode::ImportRead,
            "exports no `statements` object",
        ),
        ("value.mjs", DiagnosticCode::ImportRead, "is not a function"),
        (
            "arity.mjs",
            DiagnosticCode::ImportRead,
            "the function for `Pair |a|` takes 2 argument(s), but the header passes 1",
        ),
        ("header.mjs", DiagnosticCode::Syntax, ""),
        ("missing.mjs", DiagnosticCode::ImportRead, "missing.mjs"),
    ] {
        let result = workspace.run(&format!("Import |\"{file}\"| As |m|"));
        let error = failure(&result);
        assert_eq!(error.code(), code, "{file}: {error}");
        assert!(error.to_string().contains(expected), "{file}: {error}");
    }
}

#[test]
fn the_cli_runs_javascript_statements_and_needs_node() {
    if !node() {
        return;
    }
    let workspace = Workspace::new(&[
        ("helpers.mjs", HELPERS),
        (
            "main.botwork",
            "Import |\"helpers.mjs\"| As |js|\n|n| = js::Later |1|\nLog |n|\n",
        ),
    ]);
    let run = |path: Option<std::ffi::OsString>| {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_botwork"));
        command
            .args(["--file", "main.botwork"])
            .current_dir(workspace.0.path());
        if let Some(path) = path {
            command.env("PATH", path);
        }
        command.output().unwrap()
    };
    let output = run(None);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"2\n");
    // Without Node on PATH the import says what it needs. The other
    // directories stay, since a build with Python finds its library there.
    let without_node = std::env::join_paths(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|directory| {
            !["node", "node.exe"]
                .iter()
                .any(|name| directory.join(name).is_file())
        }),
    )
    .unwrap();
    let output = run(Some(without_node));
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("`helpers.mjs` is a JavaScript module, which needs Node.js on PATH"),
        "{output:?}"
    );
}
