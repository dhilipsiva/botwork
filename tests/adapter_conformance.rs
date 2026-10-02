//! One conformance suite for every language adapter a build advertises
//! (decisions D6 to D9): WebAssembly in builds with `wasm`, JavaScript where
//! Node is installed, and Python in builds with `python`. Each adapter loads a
//! module with the same statements, and each scenario runs against all of
//! them: Unicode, numeric boundaries, nested values, None, async results,
//! errors, cancellation, ownership, cleanup, and values without a Botwork
//! equivalent, which must fail explicitly.

use botwork::core::{
    diagnostic::DiagnosticCode,
    grammar::Literal,
    run::{Engine, RunOptions, RunResult},
};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

const JAVASCRIPT: &str = r#"
import assert from "node:assert";
class Point {}
export const statements = {
    "Echo |value|": (value) => value,
    "Fail with |message|": (message) => { throw new RangeError(message); },
    "Insist |condition|": (condition) => { assert.ok(condition, "the condition does not hold"); return true; },
    "Wrap |message|": (message) => {
        try {
            try {
                assert.fail(message);
            } catch (error) {
                throw new TypeError("the value was rejected", { cause: error });
            }
        } catch (error) {
            throw new Error("the wrapped step failed", { cause: error });
        }
    },
    "Spin": () => { for (;;) {} },
    "Later |x|": async (x) => { await new Promise((resolve) => setTimeout(resolve, 10)); return x + 1; },
    "Append |items|": (items) => { items.push(1); return items; },
    "Make |kind|": (kind) => ({
        big: () => 2 ** 31 + 1,
        nan: () => NaN,
        surrogate: () => "\ud800",
        object: () => new Point(),
    })[kind](),
};
"#;

#[cfg(feature = "python")]
const PYTHON: &str = r#"
import asyncio
import botwork

class Point:
    pass

@botwork.statement("Echo |value|")
def echo(value):
    return value

@botwork.statement("Fail with |message|")
def fail(message):
    raise ValueError(message)

@botwork.statement("Insist |condition|")
def insist(condition):
    assert condition, "the condition does not hold"
    return True

@botwork.statement("Wrap |message|")
def wrap(message):
    try:
        try:
            assert False, message
        except AssertionError as error:
            raise TypeError("the value was rejected") from error
    except TypeError as error:
        raise RuntimeError("the wrapped step failed") from error

@botwork.statement("Spin")
def spin():
    while True:
        pass

@botwork.statement("Later |x|")
async def later(x):
    await asyncio.sleep(0.01)
    return x + 1

@botwork.statement("Append |items|")
def append(items):
    items.append(1)
    return items

@botwork.statement("Make |kind|")
def make(kind):
    return {
        "big": lambda: 2 ** 40,
        "nan": lambda: float("nan"),
        "surrogate": lambda: "\ud800",
        "object": lambda: Point(),
    }[kind]()
"#;

/// An adapter under test: the module file a script imports, whether its
/// language has asynchronous functions, and the values without a Botwork
/// equivalent its `Make` returns, with what the failure says.
struct Adapter {
    file: &'static str,
    asynchronous: bool,
    unsupported: &'static [(&'static str, &'static str)],
    /// The line where `Fail with` raises, without its indentation, for an
    /// adapter whose failures say where they were raised and what they chain;
    /// the module also has `Wrap`.
    raised: Option<&'static str>,
}

/// The adapters this build and machine advertise, with their modules
/// written into `directory`.
fn adapters(directory: &Path) -> Vec<Adapter> {
    let mut adapters = Vec::new();
    #[cfg(feature = "wasm")]
    {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wasm/statements.wasm"),
            directory.join("conformance.wasm"),
        )
        .unwrap();
        adapters.push(Adapter {
            file: "conformance.wasm",
            // WebAssembly calls are synchronous; a component returns a value.
            asynchronous: false,
            unsupported: &[
                ("nan", "the float NaN, which is not finite"),
                ("cycle", "names node 0, which is not after it"),
                ("duplicate", "a map with the key \"a\" twice"),
            ],
            // A component's failure is a kind and a message alone.
            raised: None,
        });
    }
    if node() {
        fs::write(directory.join("conformance.mjs"), JAVASCRIPT).unwrap();
        adapters.push(Adapter {
            file: "conformance.mjs",
            asynchronous: true,
            unsupported: &[
                (
                    "big",
                    "a whole number beyond 32 bits that no 32-bit float holds exactly",
                ),
                ("nan", "the number NaN, which no finite 32-bit float holds"),
                ("surrogate", "a string with an unpaired surrogate"),
                ("object", "a Point, which has no Botwork equivalent"),
            ],
            raised: Some(
                r#""Fail with |message|": (message) => { throw new RangeError(message); },"#,
            ),
        });
    }
    #[cfg(feature = "python")]
    {
        fs::write(directory.join("conformance.py"), PYTHON).unwrap();
        adapters.push(Adapter {
            file: "conformance.py",
            asynchronous: true,
            unsupported: &[
                (
                    "big",
                    "the int 1099511627776, outside Botwork's 32-bit integers",
                ),
                ("nan", "the float nan, which no finite 32-bit float holds"),
                ("surrogate", "a str that is not valid Unicode"),
                ("object", "a Point, which has no Botwork equivalent"),
            ],
            raised: Some("raise ValueError(message)"),
        });
    }
    adapters
}

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

/// Run `source` after importing the adapter's module as `m`, as the CLI runs
/// scripts.
fn run(directory: &Path, adapter: &Adapter, source: &str, options: RunOptions) -> RunResult {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    // A run that hangs fails here, naming its adapter, rather than holding the
    // test binary until CI's job limit.
    let result = runtime.block_on(async {
        tokio::time::timeout(
            HANG,
            Engine::default().run_source_async(
                "main.botwork",
                &format!("Import |\"{}\"| As |m|\n{source}", adapter.file),
                RunOptions {
                    working_directory: Some(directory.into()),
                    ..options
                },
            ),
        )
        .await
    });
    runtime.shutdown_background();
    result.unwrap_or_else(|_| {
        panic!(
            "{}: the run did not finish within {HANG:?}:\n{source}",
            adapter.file
        )
    })
}

/// Far longer than any scenario takes, even with Node starting cold.
const HANG: Duration = Duration::from_secs(180);

fn value(result: &RunResult, adapter: &Adapter, name: &str) -> Literal {
    assert!(
        result.result.is_ok(),
        "{}: {:?}",
        adapter.file,
        result.result
    );
    result.variables[name].clone()
}

/// Values, with each kind spelled out so that Float and Int differ, and map
/// keys in order.
fn typed(value: &Literal) -> String {
    match value {
        Literal::Map(entries) => {
            let mut keys: Vec<_> = entries.keys().collect();
            keys.sort();
            let entries: Vec<_> = keys
                .into_iter()
                .map(|key| format!("{key:?}: {}", typed(&entries[key])))
                .collect();
            format!("Map {{{}}}", entries.join(", "))
        }
        Literal::Array(items) => {
            let items: Vec<_> = items.iter().map(typed).collect();
            format!("Array [{}]", items.join(", "))
        }
        other => format!("{other:?}"),
    }
}

/// Every adapter, in a workspace of its own; an empty build is an error.
fn each(scenario: impl Fn(&Path, &Adapter)) {
    let directory = tempfile::tempdir().unwrap();
    let adapters = adapters(directory.path());
    if adapters.is_empty() {
        // A build without wasm or python, on a machine without Node, has no
        // adapter to hold to this suite.
        return;
    }
    for adapter in &adapters {
        scenario(directory.path(), adapter);
    }
}

#[test]
fn unicode_text_and_numeric_boundaries_cross_every_adapter_exactly() {
    each(|directory, adapter| {
        let result = run(
            directory,
            adapter,
            concat!(
                "|values| = |[\"\", \"Grüße\", \"世界\", \"😀👍🏽\", \"e\u{301}\", \"עברית\", \"\u{7f}\u{80}\u{ffff}\u{10ffff}\", ",
                "2147483647, -2147483648, 0, 1.5, 0.1, ",
                "340282346638528859811704183484516925440.0, ",
                "0.000000000000000000000000000000000000011754944, ",
                "0.000000000000000000000000000000000000000000001401298]|\n",
                "|back| = m::Echo |values|",
            ),
            RunOptions::default(),
        );
        let (values, back) = (
            value(&result, adapter, "values"),
            value(&result, adapter, "back"),
        );
        assert_eq!(typed(&back), typed(&values), "{}", adapter.file);
    });
}

#[test]
fn nested_values_and_none_cross_every_adapter_unchanged() {
    each(|directory, adapter| {
        let result = run(
            directory,
            adapter,
            "Nothing {\n}\n|nothing| = Nothing\n|values| = |{\"empty\": [], \"none\": nothing, \"map\": {}, \"deep\": [[[[[[[[[[1]]]]]]]]]], \"rows\": [{\"a\": [1, 2.5, \"x\", true]}, {\"b\": {\"c\": nothing}}]}|\n|back| = m::Echo |values|\n|alone| = m::Echo |nothing|",
            RunOptions::default(),
        );
        let (values, back) = (
            value(&result, adapter, "values"),
            value(&result, adapter, "back"),
        );
        assert_eq!(typed(&back), typed(&values), "{}", adapter.file);
        assert!(
            matches!(value(&result, adapter, "alone"), Literal::None),
            "{}",
            adapter.file
        );
    });
}

#[test]
fn javascript_returns_whole_floats_as_ints_because_it_has_one_number_type() {
    let directory = tempfile::tempdir().unwrap();
    if !node() {
        return;
    }
    fs::write(directory.path().join("conformance.mjs"), JAVASCRIPT).unwrap();
    let adapter = Adapter {
        file: "conformance.mjs",
        asynchronous: true,
        unsupported: &[],
        raised: None,
    };
    let result = run(
        directory.path(),
        &adapter,
        "|back| = m::Echo |3.0|",
        RunOptions::default(),
    );
    assert!(matches!(value(&result, &adapter, "back"), Literal::Int(3)));
}

#[test]
fn async_results_are_awaited_where_the_language_has_them() {
    each(|directory, adapter| {
        if !adapter.asynchronous {
            return;
        }
        let result = run(
            directory,
            adapter,
            "|n| = m::Later |41|",
            RunOptions::default(),
        );
        assert!(
            matches!(value(&result, adapter, "n"), Literal::Int(42)),
            "{}",
            adapter.file
        );
    });
}

#[test]
fn failures_are_native_errors_or_assertions_and_cleanup_still_runs() {
    each(|directory, adapter| {
        let result = run(
            directory,
            adapter,
            "m::Fail with |\"bad input\"|",
            RunOptions::default(),
        );
        let error = result.result.as_ref().expect_err("the call fails");
        assert_eq!(
            error.code(),
            DiagnosticCode::Native,
            "{}: {error}",
            adapter.file
        );
        assert!(
            error.to_string().contains("bad input"),
            "{}: {error}",
            adapter.file
        );
        let result = run(
            directory,
            adapter,
            "m::Insist |false|",
            RunOptions::default(),
        );
        let error = result.result.as_ref().expect_err("the call fails");
        assert_eq!(
            error.code(),
            DiagnosticCode::Assertion,
            "{}: {error}",
            adapter.file
        );
        assert!(
            error.to_string().contains("the condition does not hold"),
            "{}: {error}",
            adapter.file
        );
        // A script catches both, and its cleanup runs after either.
        let result = run(
            directory,
            adapter,
            "|caught| = |0|\n|cleaned| = |0|\nTry {\n    Try {\n        m::Fail with |\"bad input\"|\n    } Finally {\n        |cleaned| = |cleaned + 1|\n    }\n} Catch |error| {\n    |caught| = |caught + 1|\n}\nTry {\n    m::Insist |false|\n} Catch |error| {\n    |caught| = |caught + 1|\n}",
            RunOptions::default(),
        );
        assert!(
            matches!(value(&result, adapter, "caught"), Literal::Int(2)),
            "{}",
            adapter.file
        );
        assert!(
            matches!(value(&result, adapter, "cleaned"), Literal::Int(1)),
            "{}",
            adapter.file
        );
    });
}

fn field<'a>(map: &'a Literal, key: &str) -> &'a Literal {
    match map {
        Literal::Map(entries) => entries
            .get(key)
            .unwrap_or_else(|| panic!("no {key} in {map}")),
        other => panic!("not a map: {other}"),
    }
}

fn text<'a>(map: &'a Literal, key: &str) -> &'a str {
    match field(map, key) {
        Literal::String(text) => text,
        other => panic!("{key} is not a String: {other}"),
    }
}

fn items<'a>(map: &'a Literal, key: &str) -> &'a [Literal] {
    match field(map, key) {
        Literal::Array(items) => items,
        other => panic!("{key} is not an Array: {other}"),
    }
}

/// The source of the related location with `message`, if there is one.
fn related<'a>(error: &'a Literal, message: &str) -> Option<&'a Literal> {
    items(error, "related")
        .iter()
        .find(|related| text(related, "message") == message)
        .map(|related| field(related, "source"))
}

#[test]
fn failures_keep_their_codes_locations_callers_and_causes_through_a_module() {
    each(|directory, adapter| {
        // A Botwork module between the script and the adapter.
        fs::write(
            directory.join("steps.botwork"),
            format!(
                "Import |\"{}\"| As |m|\nFail Through |message| {{\n    m::Fail with |message|\n}}\nInsist Through |condition| {{\n    m::Insist |condition|\n}}\nWrap Through |message| {{\n    m::Wrap |message|\n}}\n",
                adapter.file
            ),
        )
        .unwrap();
        let mut source = "Import |\"steps.botwork\"| As |s|\nTry {\n    s::Fail Through |\"bad input\"|\n} Catch |error| {\n    |failed| = |error|\n}\nTry {\n    s::Insist Through |false|\n} Catch |error| {\n    |insisted| = |error|\n}".to_owned();
        if adapter.raised.is_some() {
            source.push_str("\nTry {\n    s::Wrap Through |\"bad input\"|\n} Catch |error| {\n    |wrapped| = |error|\n}");
        }
        let result = run(directory, adapter, &source, RunOptions::default());
        for (name, code) in [("failed", "BW4002"), ("insisted", "BW9001")] {
            let error = value(&result, adapter, name);
            let error = &error;
            assert_eq!(text(error, "code"), code, "{}: {error}", adapter.file);
            // The adapter's call inside the module, then the module's inside
            // the script, which the run names `main.botwork`.
            let callers: Vec<_> = items(error, "call_stack")
                .iter()
                .map(|frame| {
                    let site = field(frame, "call_site");
                    (text(site, "file").to_owned(), text(site, "line").to_owned())
                })
                .collect();
            assert_eq!(callers.len(), 2, "{}: {error}", adapter.file);
            assert!(
                callers[0].0.ends_with("steps.botwork") && callers[1].0 == "main.botwork",
                "{}: {callers:?}",
                adapter.file
            );
            // The statement came in through the module's own import.
            let imported = related(error, "imported here")
                .unwrap_or_else(|| panic!("{}: no import site in {error}", adapter.file));
            assert!(
                text(imported, "file").ends_with("steps.botwork") && text(imported, "line") == "1",
                "{}: {imported}",
                adapter.file
            );
            match adapter.raised {
                Some(_) => {
                    let raised = related(error, "raised here")
                        .unwrap_or_else(|| panic!("{}: no raise site in {error}", adapter.file));
                    assert!(
                        text(raised, "file").ends_with(adapter.file),
                        "{}: {raised}",
                        adapter.file
                    );
                }
                None => assert!(related(error, "raised here").is_none(), "{error}"),
            }
        }
        let failed = value(&result, adapter, "failed");
        if let Some(line) = adapter.raised {
            let raised = related(&failed, "raised here").unwrap();
            assert_eq!(text(raised, "text"), line, "{}: {raised}", adapter.file);
            // Each failure the chain holds is the cause of the one it led to,
            // with its own code and where it was raised.
            let mut failure = value(&result, adapter, "wrapped");
            for (code, message) in [
                ("BW4002", "the wrapped step failed"),
                ("BW4002", "the value was rejected"),
                ("BW9001", "bad input"),
            ] {
                assert_eq!(text(&failure, "code"), code, "{}: {failure}", adapter.file);
                assert!(
                    text(&failure, "message").contains(message),
                    "{}: {failure}",
                    adapter.file
                );
                let raised = related(&failure, "raised here")
                    .unwrap_or_else(|| panic!("{}: no raise site in {failure}", adapter.file));
                assert!(
                    text(raised, "file").ends_with(adapter.file),
                    "{}: {raised}",
                    adapter.file
                );
                let causes = items(&failure, "causes");
                if code == "BW9001" {
                    assert!(causes.is_empty(), "{}: {failure}", adapter.file);
                } else {
                    assert_eq!(causes.len(), 1, "{}: {failure}", adapter.file);
                    failure = causes[0].clone();
                }
            }
        }
    });
}

#[test]
fn a_deadline_stops_a_busy_call_and_the_adapter_works_afterwards() {
    each(|directory, adapter| {
        let start = Instant::now();
        let result = run(
            directory,
            adapter,
            "|cleaned| = |false|\nTry {\n    m::Spin\n} Finally {\n    |cleaned| = |true|\n}",
            RunOptions {
                timeout: Some(Duration::from_secs(1)),
                ..RunOptions::default()
            },
        );
        let elapsed = start.elapsed();
        let error = result
            .result
            .as_ref()
            .expect_err("the deadline stops the run");
        assert_eq!(
            error.code(),
            DiagnosticCode::Timeout,
            "{}: {error}",
            adapter.file
        );
        // The call ended within the grace: nothing was abandoned.
        assert!(error.causes.is_empty(), "{}: {error}", adapter.file);
        assert!(
            elapsed < Duration::from_secs(4),
            "{}: {elapsed:?}",
            adapter.file
        );
        // The adapter serves the next run.
        let result = run(
            directory,
            adapter,
            "|back| = m::Echo |[1]|",
            RunOptions::default(),
        );
        assert_eq!(
            value(&result, adapter, "back").to_string(),
            "[1]",
            "{}",
            adapter.file
        );
    });
}

#[test]
fn adapters_receive_copies_and_cannot_change_the_callers_values() {
    each(|directory, adapter| {
        let result = run(
            directory,
            adapter,
            "|items| = |[1, 2]|\n|back| = m::Append |items|\n|again| = m::Append |items|",
            RunOptions::default(),
        );
        assert_eq!(
            value(&result, adapter, "items").to_string(),
            "[1, 2]",
            "{}",
            adapter.file
        );
        assert_eq!(
            value(&result, adapter, "back").to_string(),
            "[1, 2, 1]",
            "{}",
            adapter.file
        );
        assert_eq!(
            value(&result, adapter, "again").to_string(),
            "[1, 2, 1]",
            "{}",
            adapter.file
        );
    });
}

#[test]
fn values_without_a_botwork_equivalent_fail_explicitly_in_every_adapter() {
    each(|directory, adapter| {
        assert!(!adapter.unsupported.is_empty(), "{}", adapter.file);
        for (kind, reason) in adapter.unsupported {
            let result = run(
                directory,
                adapter,
                &format!("|value| = m::Make |\"{kind}\"|"),
                RunOptions::default(),
            );
            let error = result.result.as_ref().expect_err("the call fails");
            assert_eq!(
                error.code(),
                DiagnosticCode::Native,
                "{} {kind}: {error}",
                adapter.file
            );
            let text = error.to_string();
            assert!(
                text.contains("statement returned") && text.contains(reason),
                "{} {kind}: {text}",
                adapter.file
            );
            // Nothing reaches the script.
            assert!(
                !result.variables.contains_key("value"),
                "{} {kind}",
                adapter.file
            );
        }
    });
}
