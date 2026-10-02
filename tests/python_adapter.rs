//! Python modules in builds with the `python` feature (decision D9): loading,
//! value conversion, exceptions, run isolation, async statements, and stops.
#![cfg(feature = "python")]

use botwork::core::{
    diagnostic::{Diagnostic, DiagnosticCode},
    grammar::Literal,
    run::{Engine, RunOptions, RunResult},
};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const HELPERS: &str = r#"
import botwork

@botwork.statement("Echo |value|")
def echo(value):
    return value

@botwork.statement("Nothing")
def nothing():
    return None

@botwork.statement("Describe |value|")
def describe(value):
    return type(value).__name__

def nested(depth):
    value = 0
    for _ in range(depth):
        value = [value]
    return value

@botwork.statement("Make |kind|")
def make(kind):
    return {
        "bytes": b"raw",
        "huge": 2 ** 40,
        "nan": float("nan"),
        "wide": 1e39,
        "keyed": {1: "one"},
        "set": {1, 2},
        "object": object(),
        "deep": nested(1000),
    }[kind]

@botwork.statement("Fail with |message|")
def fail_with(message):
    raise ValueError(message)

@botwork.statement("Insist |condition|")
def insist(condition):
    assert condition, "the condition does not hold"
    return True

COUNT = 0

@botwork.statement("Count")
def count():
    global COUNT
    COUNT += 1
    return COUNT

@botwork.statement("Later |x|")
async def later(x):
    import asyncio
    await asyncio.sleep(0.01)
    return x + 1

@botwork.statement("Spin")
def spin():
    while True:
        pass

@botwork.statement("Quick")
def quick():
    return "done"
"#;

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

    /// Python statements are blocking operations, so they run in the
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
        // Do not wait for an abandoned call, as the CLI does not: a statement
        // that never stops must fail its test rather than hang it.
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
fn values_cross_into_python_and_back_unchanged() {
    let workspace = Workspace::new(&[("helpers.py", HELPERS)]);
    let source = r#"Import |"helpers.py"| As |py|
|nothing| = py::Nothing
|values| = |[nothing, 2147483647, -2147483648, 1.5, true, "Grüße, 世界", [1, [2]], {"a": {"b": [false]}}]|
|back| = py::Echo |values|
Assert |back| Equals |values|
|kinds| = |[]|
For |item| In |values| {
    |kind| = py::Describe |item|
    |kinds| = |kinds + [kind]|
}
"#;
    let result = workspace.run(source);
    assert_eq!(
        value(&result, "kinds").to_string(),
        r#"["NoneType", "int", "int", "float", "bool", "str", "list", "dict"]"#
    );
}

#[test]
fn values_without_an_exact_botwork_equivalent_are_refused() {
    let workspace = Workspace::new(&[("helpers.py", HELPERS)]);
    for (kind, expected) in [
        ("bytes", "a bytes, which has no Botwork equivalent"),
        (
            "huge",
            "the int 1099511627776, outside Botwork's 32-bit integers",
        ),
        ("nan", "the float nan, which no finite 32-bit float holds"),
        (
            "wide",
            "the float 1e+39, which no finite 32-bit float holds",
        ),
        ("keyed", "a dict with a key that is not a str"),
        ("set", "a set, which has no Botwork equivalent"),
        ("object", "a object, which has no Botwork equivalent"),
        ("deep", "a value beyond the value limits"),
    ] {
        let result = workspace.run(&format!(
            "Import |\"helpers.py\"| As |py|\npy::Make |\"{kind}\"|"
        ));
        let error = failure(&result);
        assert_eq!(error.code(), DiagnosticCode::Native, "{kind}: {error}");
        assert!(
            error
                .to_string()
                .contains(&format!("The Python statement returned {expected}")),
            "{kind}: {error}"
        );
    }
}

#[test]
fn exceptions_become_catchable_diagnostics_with_their_traceback() {
    let workspace = Workspace::new(&[("helpers.py", HELPERS)]);
    let result = workspace.run("Import |\"helpers.py\"| As |py|\npy::Fail with |\"bad input\"|");
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Native);
    let text = error.to_string();
    assert!(text.contains("Python ValueError: bad input"), "{text}");
    assert!(text.contains("helpers.py\", line"), "{text}");
    assert!(text.contains("in fail_with"), "{text}");
    // An AssertionError is an assertion failure.
    let result = workspace.run("Import |\"helpers.py\"| As |py|\npy::Insist |false|");
    let error = failure(&result);
    assert_eq!(error.code(), DiagnosticCode::Assertion);
    assert!(error.to_string().contains("the condition does not hold"));
    let result = workspace.run(
        "Import |\"helpers.py\"| As |py|\n|caught| = |false|\nTry {\n    py::Fail with |\"x\"|\n} Catch |error| {\n    |caught| = |true|\n}",
    );
    assert_eq!(value(&result, "caught").to_string(), "true");
}

const CHAINS: &str = r#"
import botwork

@botwork.statement("Implicit")
def implicit():
    try:
        {}["missing"]
    except KeyError:
        raise ValueError("while handling")

@botwork.statement("Suppressed")
def suppressed():
    try:
        {}["missing"]
    except KeyError:
        raise ValueError("on its own") from None
"#;

/// The file and text of the line a failure says raised it.
fn raised(error: &Diagnostic) -> Option<(String, String)> {
    error
        .related
        .iter()
        .find(|related| related.message == "raised here")
        .map(|related| {
            (
                related.span.source().name().to_owned(),
                related.span.text().to_owned(),
            )
        })
}

#[test]
fn exceptions_raised_while_handling_others_keep_them_as_causes_unless_suppressed() {
    // A line more than 256 KiB into its file has no location.
    let far = format!(
        "{}import botwork\n\n@botwork.statement(\"Far\")\ndef far():\n    raise ValueError(\"far\")\n",
        "# padding\n".repeat(300 * 1024 / 10)
    );
    let workspace = Workspace::new(&[("chains.py", CHAINS), ("far.py", &far)]);
    let result = workspace.run("Import |\"chains.py\"| As |py|\npy::Implicit");
    let error = failure(&result);
    let (file, text) = raised(error).expect("a raise site");
    assert!(file.ends_with("chains.py"), "{file}");
    assert_eq!(text, "raise ValueError(\"while handling\")");
    assert_eq!(error.causes.len(), 1, "{error}");
    let cause = &error.causes[0];
    assert_eq!(cause.code(), DiagnosticCode::Native);
    assert!(cause.to_string().contains("Python KeyError"), "{cause}");
    assert_eq!(raised(cause).expect("a raise site").1, "{}[\"missing\"]");
    let result = workspace.run("Import |\"chains.py\"| As |py|\npy::Suppressed");
    let error = failure(&result);
    assert!(error.causes.is_empty(), "{error}");
    assert!(raised(error).is_some(), "{error}");
    let result = workspace.run("Import |\"far.py\"| As |py|\npy::Far");
    let error = failure(&result);
    assert!(
        error.to_string().contains("Python ValueError: far"),
        "{error}"
    );
    assert_eq!(raised(error), None, "{error}");
    assert!(
        error
            .related
            .iter()
            .any(|related| related.message == "imported here"),
        "{error}"
    );
}

#[test]
fn module_globals_stay_within_one_run() {
    let workspace = Workspace::new(&[("helpers.py", HELPERS)]);
    let source = "Import |\"helpers.py\"| As |py|\nImport |\"helpers.py\"| As |again|\n|first| = py::Count\n|second| = again::Count\n";
    for _ in 0..2 {
        let result = workspace.run(source);
        // Both imports share this run's module object; the next run starts afresh.
        assert_eq!(value(&result, "first").to_string(), "1");
        assert_eq!(value(&result, "second").to_string(), "2");
    }
}

#[test]
fn async_statements_run_their_coroutines() {
    let workspace = Workspace::new(&[("helpers.py", HELPERS)]);
    let result = workspace.run("Import |\"helpers.py\"| As |py|\n|n| = py::Later |41|");
    assert_eq!(value(&result, "n").to_string(), "42");
}

/// Run `source` with a 300 ms deadline after starting the interpreter, and
/// return the failure and how long the stopped run took.
fn stopped(workspace: &Workspace, source: &str) -> (Diagnostic, Duration) {
    // Start the interpreter first, which can take seconds on a cold host, so
    // that the time measured is the stop's alone.
    let warm = workspace.run("Import |\"helpers.py\"| As |py|\n|done| = py::Quick");
    assert_eq!(value(&warm, "done").to_string(), "done");
    let start = Instant::now();
    let result = workspace.run_with(
        source,
        RunOptions {
            timeout: Some(Duration::from_millis(300)),
            ..RunOptions::default()
        },
    );
    (failure(&result).clone(), start.elapsed())
}

#[test]
fn a_stop_interrupts_python_and_leaves_the_interpreter_usable() {
    let workspace = Workspace::new(&[("helpers.py", HELPERS)]);
    let (error, took) = stopped(&workspace, "Import |\"helpers.py\"| As |py|\npy::Spin");
    assert_eq!(error.code(), DiagnosticCode::Timeout);
    // The interruption ended the loop well inside the stop grace, and is the
    // only failure reported.
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(error.causes.is_empty(), "{error}");
    let result = workspace.run("Import |\"helpers.py\"| As |py|\n|done| = py::Quick");
    assert_eq!(value(&result, "done").to_string(), "done");
}

#[test]
fn a_stop_interrupts_a_file_while_it_loads() {
    let workspace = Workspace::new(&[
        ("helpers.py", HELPERS),
        ("endless.py", "while True:\n    pass\n"),
    ]);
    let (error, took) = stopped(&workspace, "Import |\"endless.py\"| As |endless|");
    assert_eq!(error.code(), DiagnosticCode::Timeout, "{error}");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(error.causes.is_empty(), "{error}");
}

#[test]
fn files_import_their_neighbours_while_loading() {
    let workspace = Workspace::new(&[
        ("neighbour.py", "GREETING = 'Hello'\n"),
        (
            "greeter.py",
            "import botwork\nfrom neighbour import GREETING\n\n@botwork.statement(\"Greet |name|\")\ndef greet(name):\n    return f\"{GREETING}, {name}\"\n",
        ),
    ]);
    let result = workspace.run("Import |\"greeter.py\"| As |g|\n|text| = g::Greet |\"Ada\"|");
    assert_eq!(value(&result, "text").to_string(), "Hello, Ada");
}

#[test]
fn modules_read_the_botwork_version_running_them() {
    let workspace = Workspace::new(&[(
        "version.py",
        "import botwork\n\n@botwork.statement(\"Botwork Version\")\ndef version():\n    return botwork.__version__\n",
    )]);
    let result = workspace.run("Import |\"version.py\"| As |v|\n|version| = v::Botwork Version");
    assert_eq!(
        value(&result, "version").to_string(),
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn loading_failures_are_import_errors() {
    let workspace = Workspace::new(&[
        ("helpers.py", HELPERS),
        ("broken.py", "def broken(:\n"),
        ("raising.py", "raise RuntimeError('no')\n"),
        (
            "arity.py",
            "import botwork\n\n@botwork.statement(\"Pair |a| and |b|\")\ndef pair(a):\n    return a\n",
        ),
        (
            "header.py",
            "import botwork\n\n@botwork.statement(42)\ndef numbered():\n    return 1\n",
        ),
    ]);
    for (file, expected) in [
        ("broken.py", "Python SyntaxError"),
        ("raising.py", "Python RuntimeError: no"),
        (
            "arity.py",
            "`pair` does not take the 2 argument(s) of `Pair |a| and |b|`",
        ),
        ("header.py", "Python TypeError: a statement header is a str"),
        ("missing.py", "missing.py"),
    ] {
        let result = workspace.run(&format!("Import |\"{file}\"| As |m|"));
        let error = failure(&result);
        assert_eq!(error.code(), DiagnosticCode::ImportRead, "{file}: {error}");
        assert!(error.to_string().contains(expected), "{file}: {error}");
    }
    // A namespace already in use is refused, as for Botwork modules.
    let result = workspace.run("Import |\"helpers.py\"| As |m|\nImport |\"helpers.py\"| As |m|");
    assert_eq!(failure(&result).code(), DiagnosticCode::DuplicateNamespace);
    // An unknown statement in the namespace is undefined.
    let result = workspace.run("Import |\"helpers.py\"| As |m|\nm::Absent");
    assert_eq!(failure(&result).code(), DiagnosticCode::UndefinedStatement);
}

#[test]
fn the_cli_runs_python_statements() {
    let workspace = Workspace::new(&[
        ("helpers.py", HELPERS),
        (
            "main.botwork",
            "Import |\"helpers.py\"| As |py|\n|n| = py::Later |1|\nLog |n|\n",
        ),
    ]);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(["--file", "main.botwork"])
        .current_dir(workspace.0.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"2\n");
    assert!(Path::new(&workspace.0.path().join("helpers.py")).exists());
}
