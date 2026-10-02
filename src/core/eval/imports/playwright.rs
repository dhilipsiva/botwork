//! Playwright (decision D11). `Import |"botwork:playwright"| As |pw|` publishes
//! statements that drive browsers with the official Playwright library, which
//! the run's directory provides, through a Node host the run owns: started at
//! its first command and ended, with its browsers, when the run ends.
use super::*;
use crate::core::{
    diagnostic::DiagnosticCode as Code,
    operation::OperationControl,
    signature::{StatementSignature, ValueKind as Kind, ValueKinds},
};
use serde_json::{json, Value};
use std::{fmt, sync::OnceLock, time::Duration};

mod host;

pub(in crate::core::eval) use host::Host;

/// The import path that names this module.
pub(crate) const PATH: &str = "botwork:playwright";
/// How long Botwork waits for the host to answer a command. Playwright bounds
/// its own waits by each browser's `timeout_ms`; this catches a host that
/// stops answering, and a run's deadline or stop ends a command sooner.
const ANSWER: Duration = Duration::from_secs(600);
/// The longest wait or timeout a script may give.
const MAX_TIMEOUT_MS: i32 = 600_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Launch,
    CloseBrowser,
    NewContext,
    CloseContext,
    NewPage,
    ClosePage,
    Goto,
    Url,
    Title,
    Click,
    Fill,
    Press,
    Check,
    Uncheck,
    Hover,
    Select,
    Text,
    Attribute,
    Count,
    Visible,
    Wait,
    ExpectText,
    ExpectVisible,
    ExpectTitle,
    Evaluate,
    Screenshot,
    FullScreenshot,
    StartTracing,
    StopTracing,
}

impl Command {
    const ALL: [Self; 29] = [
        Self::Launch,
        Self::CloseBrowser,
        Self::NewContext,
        Self::CloseContext,
        Self::NewPage,
        Self::ClosePage,
        Self::Goto,
        Self::Url,
        Self::Title,
        Self::Click,
        Self::Fill,
        Self::Press,
        Self::Check,
        Self::Uncheck,
        Self::Hover,
        Self::Select,
        Self::Text,
        Self::Attribute,
        Self::Count,
        Self::Visible,
        Self::Wait,
        Self::ExpectText,
        Self::ExpectVisible,
        Self::ExpectTitle,
        Self::Evaluate,
        Self::Screenshot,
        Self::FullScreenshot,
        Self::StartTracing,
        Self::StopTracing,
    ];

    fn header(self) -> &'static str {
        match self {
            Self::Launch => "Launch Browser |options|",
            Self::CloseBrowser => "Close Browser |browser|",
            Self::NewContext => "New Context In |browser| With |options|",
            Self::CloseContext => "Close Context |context|",
            Self::NewPage => "New Page In |context|",
            Self::ClosePage => "Close Page |page|",
            Self::Goto => "Go To |url| In |page|",
            Self::Url => "URL Of |page|",
            Self::Title => "Title Of |page|",
            Self::Click => "Click |selector| In |page|",
            Self::Fill => "Fill |selector| With |text| In |page|",
            Self::Press => "Press |key| On |selector| In |page|",
            Self::Check => "Check |selector| In |page|",
            Self::Uncheck => "Uncheck |selector| In |page|",
            Self::Hover => "Hover |selector| In |page|",
            Self::Select => "Select |values| In |selector| Of |page|",
            Self::Text => "Text Of |selector| In |page|",
            Self::Attribute => "Attribute |name| Of |selector| In |page|",
            Self::Count => "Count |selector| In |page|",
            Self::Visible => "Is Visible |selector| In |page|",
            Self::Wait => "Wait For |selector| In |page| Within |milliseconds|",
            Self::ExpectText => "Expect |selector| In |page| To Have Text |text|",
            Self::ExpectVisible => "Expect |selector| In |page| To Be Visible",
            Self::ExpectTitle => "Expect Title Of |page| To Be |title|",
            Self::Evaluate => "Evaluate |script| With |arguments| In |page|",
            Self::Screenshot => "Screenshot Of |page| To |path|",
            Self::FullScreenshot => "Full Page Screenshot Of |page| To |path|",
            Self::StartTracing => "Start Tracing |context|",
            Self::StopTracing => "Stop Tracing |context| To |path|",
        }
    }

    fn signature(self) -> StatementSignature {
        let map: ValueKinds = Kind::Map.into();
        let string: ValueKinds = Kind::String.into();
        let none: ValueKinds = Kind::None.into();
        let selector = string.union(map);
        let (parameters, returns, description): (Vec<(&str, ValueKinds)>, ValueKinds, &str) = match self {
            Self::Launch => (vec![("options", map)], map, "Launch a browser and return its handle. Options: browser (chromium, firefox, or webkit; default chromium), headless (default true), args, executable_path, and timeout_ms, the bound on each action and expectation in it (default 30000)."),
            Self::CloseBrowser => (vec![("browser", map)], none, "Close a browser, its contexts, and their pages."),
            Self::NewContext => (vec![("browser", map), ("options", map)], map, "Open an isolated browser context with Playwright's context options, such as viewport, locale, or recordVideo; trace: true also starts tracing."),
            Self::CloseContext => (vec![("context", map)], none, "Close a context and its pages, recording any videos as artifacts."),
            Self::NewPage => (vec![("context", map)], map, "Open a page in a context."),
            Self::ClosePage => (vec![("page", map)], none, "Close a page."),
            Self::Goto => (vec![("url", string), ("page", map)], none, "Load a URL and wait for the page to load."),
            Self::Url => (vec![("page", map)], string, "The page's URL."),
            Self::Title => (vec![("page", map)], string, "The page's title."),
            Self::Click => (vec![("selector", selector), ("page", map)], none, "Click the first element matching a Playwright selector, once it is actionable."),
            Self::Fill => (vec![("selector", selector), ("text", string), ("page", map)], none, "Fill an input with text."),
            Self::Press => (vec![("key", string), ("selector", selector), ("page", map)], none, "Press a key, such as Enter or Control+A, on an element."),
            Self::Check => (vec![("selector", selector), ("page", map)], none, "Check a checkbox or radio button."),
            Self::Uncheck => (vec![("selector", selector), ("page", map)], none, "Uncheck a checkbox."),
            Self::Hover => (vec![("selector", selector), ("page", map)], none, "Hover over an element."),
            Self::Select => (vec![("values", ValueKinds::one(Kind::String).union(Kind::Array.into())), ("selector", selector), ("page", map)], Kind::Array.into(), "Select options of a select element by value or label, and return the values selected."),
            Self::Text => (vec![("selector", selector), ("page", map)], string, "An element's rendered text."),
            Self::Attribute => (vec![("name", string), ("selector", selector), ("page", map)], string.union(none), "An element's attribute, or None when it has none."),
            Self::Count => (vec![("selector", selector), ("page", map)], Kind::Int.into(), "How many elements match a selector now."),
            Self::Visible => (vec![("selector", selector), ("page", map)], Kind::Bool.into(), "Whether the first match is visible now."),
            Self::Wait => (vec![("selector", selector), ("page", map), ("milliseconds", Kind::Int.into())], none, "Wait until an element matching a selector is visible."),
            Self::ExpectText => (vec![("selector", selector), ("page", map), ("text", string)], none, "Assert that the first match has exactly this text, looking again until the browser's timeout."),
            Self::ExpectVisible => (vec![("selector", selector), ("page", map)], none, "Assert that the first match is visible, looking again until the browser's timeout."),
            Self::ExpectTitle => (vec![("page", map), ("title", string)], none, "Assert the page's title, looking again until the browser's timeout."),
            Self::Evaluate => (vec![("script", string), ("arguments", Kind::Array.into()), ("page", map)], ValueKinds::ANY, "Run JavaScript in the page as a function body with `arguments`, and return its result."),
            Self::Screenshot => (vec![("page", map), ("path", string)], string, "Save a PNG of the viewport, record it as the run's artifact, and return its absolute path."),
            Self::FullScreenshot => (vec![("page", map), ("path", string)], string, "Save a PNG of the whole page, record it as the run's artifact, and return its absolute path."),
            Self::StartTracing => (vec![("context", map)], none, "Start recording a Playwright trace of a context."),
            Self::StopTracing => (vec![("context", map), ("path", string)], string, "Stop the context's trace, save it, record it as the run's artifact, and return its absolute path."),
        };
        let mut signature = StatementSignature::native_at("<playwright>", self.header())
            .expect("fixed Playwright signature")
            .returns(returns)
            .description(description);
        for (name, kinds) in parameters {
            signature = signature.parameter(name, kinds).expect("fixed parameter");
        }
        let mut errors = vec![
            (Code::IncompatibleType, "A handle, option, or value that does not fit, or a handle this run does not have open."),
            (Code::Native, "Playwright failed, timed out, or is not installed, or Node.js is missing."),
            (Code::AsyncRuntime, "Requires asynchronous execution, as the CLI runs scripts."),
        ];
        match self {
            Self::ExpectText | Self::ExpectVisible | Self::ExpectTitle => errors.push((
                Code::Assertion,
                "The expectation did not hold before the browser's timeout.",
            )),
            Self::Wait => {
                errors.push((Code::ConditionNotMet, "No element became visible in time."))
            }
            Self::Screenshot | Self::FullScreenshot | Self::StopTracing => {
                errors.push((Code::Output, "The file could not be written."))
            }
            _ => {}
        }
        for (code, reason) in errors {
            signature = signature
                .documents_error(code, reason)
                .expect("fixed error");
        }
        signature
    }
}

fn signatures() -> &'static [(Command, StatementSignature)] {
    static SIGNATURES: OnceLock<Vec<(Command, StatementSignature)>> = OnceLock::new();
    SIGNATURES.get_or_init(|| {
        Command::ALL
            .iter()
            .map(|command| (*command, command.signature()))
            .collect()
    })
}

/// The statements this module exports, as analysis lists a module's.
pub(crate) fn exports() -> impl Iterator<Item = (String, String)> {
    signatures().iter().map(|(_, signature)| {
        (
            signature.normalized().to_owned(),
            signature.header().text().to_owned(),
        )
    })
}

/// What every statement of one run shares.
struct Run {
    host: Arc<Host>,
    recorder: Option<crate::core::report::Recorder>,
    directory: PathBuf,
}

pub(super) async fn evaluate_import(
    namespace: &Name,
    normalized: &str,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    let environment = context.environment.clone();
    let directory = match &environment {
        Some(environment) => environment.working_directory().to_owned(),
        None => context
            .working_directory
            .clone()
            .unwrap_or_else(|_| PathBuf::from(".")),
    };
    let variables = environment.as_ref().map_or_else(
        || std::env::vars_os().collect(),
        |environment| environment.variables().clone(),
    );
    let recorder = environment
        .as_ref()
        .and_then(|environment| environment.recorder.clone());
    let run = Arc::new(Run {
        host: context
            .modules
            .playwright_host(&directory, variables, recorder.clone()),
        recorder,
        directory,
    });
    let statements = signatures().iter().map(|(command, signature)| {
        let (command, run) = (*command, Arc::clone(&run));
        (signature, move |qualified| {
            NativeOperation::asynchronous(qualified, move |values, control| {
                let run = Arc::clone(&run);
                async move { command.run(&run, values, control).await }
            })
        })
    });
    publish_operations(statements, namespace, normalized, import_site, context)
}

fn incompatible(reason: impl fmt::Display) -> Diagnostic {
    Diagnostic::new(BWErr::OperationIncompatibleError(reason.to_string()))
}

/// A Botwork value as JSON for the host.
fn value(literal: &Literal) -> DiagnosticResult<Value> {
    webdriver::values::to_json(literal).map_err(incompatible)
}

/// The diagnostic for what the host reported.
fn failed(failure: host::Failure, run: &Run) -> Diagnostic {
    // What the host captured of the page is the run's, and the error names it.
    let mut message = failure.message;
    if !failure.artifacts.is_empty() {
        let mut saved = Vec::new();
        for (kind, path) in &failure.artifacts {
            let path = crate::core::paths::canonicalize(Path::new(path))
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| path.clone());
            if let Some(recorder) = &run.recorder {
                recorder.artifact(kind, &path);
            }
            saved.push(path);
        }
        message = format!("{message}; failure artifacts: {}", saved.join(", "));
    }
    match failure.kind.as_str() {
        "handle" | "value" => incompatible(message),
        "assertion" => Diagnostic::new(BWErr::AssertionFailed(message)),
        "wait" => Diagnostic::new(BWErr::ConditionNotMet {
            reason: message,
            attempts: "1".into(),
            history: "[]".into(),
        }),
        _ => Diagnostic::new(BWErr::NativeError(format!("Playwright: {message}"))),
    }
}

fn options_map<'a>(options: &'a Literal, key: &str) -> Option<&'a Literal> {
    match options {
        Literal::Map(map) => map.get(key),
        _ => None,
    }
}

/// The directory `failure_artifacts` names, from the run's directory, made if
/// it is missing.
fn failure_directory(run: &Path, option: Option<&Literal>) -> DiagnosticResult<Option<PathBuf>> {
    match option {
        None => Ok(None),
        Some(Literal::String(path)) => {
            let directory = run.join(path);
            std::fs::create_dir_all(&directory)
                .and_then(|()| crate::core::paths::canonicalize(&directory))
                .map(Some)
                .map_err(|error| {
                    Diagnostic::new(BWErr::OutputError(format!(
                        "The failure artifacts directory {} could not be made: {error}",
                        directory.display()
                    )))
                })
        }
        Some(other) => Err(incompatible(format_args!(
            "`failure_artifacts` is a directory's path, not a {}",
            other.kind().as_str()
        ))),
    }
}

/// A wait in milliseconds, from `least` to the longest.
fn milliseconds(value: &Literal, what: &str, least: i32) -> DiagnosticResult<i32> {
    match value {
        Literal::Int(milliseconds) if (least..=MAX_TIMEOUT_MS).contains(milliseconds) => {
            Ok(*milliseconds)
        }
        other => Err(incompatible(format_args!(
            "{what} is an Int from {least} to {MAX_TIMEOUT_MS}, not {other}"
        ))),
    }
}

/// A Playwright selector: a String as it is (CSS, or one of Playwright's own
/// engines, such as `text=`), or a Map naming one of the strategies WebDriver
/// statements take, so one selector works with both.
fn selector(value: &Literal) -> DiagnosticResult<String> {
    let quoted = |text: &str| serde_json::to_string(text).expect("a String serializes");
    match value {
        Literal::String(text) => Ok(text.clone()),
        Literal::Map(map) if map.len() == 1 => {
            let (key, value) = map.iter().next().expect("one entry");
            let Literal::String(text) = value else {
                return Err(incompatible(format_args!(
                    "The `{key}` selector must be a String, not a {}",
                    value.kind().as_str()
                )));
            };
            Ok(match key.as_str() {
                "css" | "tag_name" => format!("css={text}"),
                "xpath" => format!("xpath={text}"),
                "link_text" => format!("a:text-is({})", quoted(text)),
                "partial_link_text" => format!("a:has-text({})", quoted(text)),
                "accessibility_id" | "id" | "class_name" | "android_uiautomator" | "ios_predicate" | "ios_class_chain" => {
                    return Err(incompatible(format_args!(
                        "`{key}` is an Appium strategy; Playwright takes css, xpath, link_text, partial_link_text, or tag_name, or a Playwright selector String"
                    )))
                }
                _ => {
                    return Err(incompatible(format_args!(
                        "Unknown selector strategy `{key}`; use css, xpath, link_text, partial_link_text, or tag_name, or a Playwright selector String"
                    )))
                }
            })
        }
        Literal::Map(_) => Err(incompatible(
            "A selector Map names exactly one strategy, such as {xpath: \"//h1\"}",
        )),
        other => Err(incompatible(format_args!(
            "A selector is a String or a Map, not a {}",
            other.kind().as_str()
        ))),
    }
}

impl Command {
    /// The host's name for this command.
    fn name(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::CloseBrowser => "closeBrowser",
            Self::NewContext => "newContext",
            Self::CloseContext => "closeContext",
            Self::NewPage => "newPage",
            Self::ClosePage => "closePage",
            Self::Goto => "goto",
            Self::Url => "url",
            Self::Title => "title",
            Self::Click => "click",
            Self::Fill => "fill",
            Self::Press => "press",
            Self::Check | Self::Uncheck => "check",
            Self::Hover => "hover",
            Self::Select => "select",
            Self::Text => "text",
            Self::Attribute => "attribute",
            Self::Count => "count",
            Self::Visible => "visible",
            Self::Wait => "wait",
            Self::ExpectText => "expectText",
            Self::ExpectVisible => "expectVisible",
            Self::ExpectTitle => "expectTitle",
            Self::Evaluate => "evaluate",
            Self::Screenshot | Self::FullScreenshot => "screenshot",
            Self::StartTracing => "startTracing",
            Self::StopTracing => "stopTracing",
        }
    }

    async fn run(
        self,
        run: &Run,
        values: Vec<Literal>,
        _control: OperationControl,
    ) -> DiagnosticResult<Literal> {
        let v = |index: usize| value(&values[index]);
        let s = |index: usize| selector(&values[index]).map(Value::String);
        // A path the host writes: from the run's directory, absolute.
        let file = |index: usize| -> DiagnosticResult<String> {
            let Literal::String(path) = &values[index] else {
                unreachable!("checked kind")
            };
            Ok(run.directory.join(path).display().to_string())
        };
        let args: Vec<Value> = match self {
            Self::Launch => {
                let Literal::Map(options) = &values[0] else {
                    unreachable!("checked kind")
                };
                for key in options.keys() {
                    if !matches!(
                        key.as_str(),
                        "browser"
                            | "headless"
                            | "args"
                            | "executable_path"
                            | "timeout_ms"
                            | "failure_artifacts"
                    ) {
                        return Err(incompatible(format_args!(
                            "Unknown Launch Browser option `{key}`; use browser, headless, args, executable_path, timeout_ms, or failure_artifacts"
                        )));
                    }
                }
                if let Some(timeout) = options.get("timeout_ms") {
                    milliseconds(timeout, "`timeout_ms`", 1)?;
                }
                let mut options = v(0)?;
                // The host saves failures where the run's directory says.
                if let Some(directory) =
                    failure_directory(&run.directory, options_map(&values[0], "failure_artifacts"))?
                {
                    options["failure_artifacts"] = json!(directory.display().to_string());
                }
                vec![options]
            }
            Self::CloseBrowser
            | Self::CloseContext
            | Self::NewPage
            | Self::ClosePage
            | Self::Url
            | Self::Title
            | Self::StartTracing => vec![v(0)?],
            Self::NewContext => vec![v(0)?, v(1)?],
            Self::Goto => vec![v(1)?, v(0)?],
            Self::Click
            | Self::Hover
            | Self::Text
            | Self::Count
            | Self::Visible
            | Self::ExpectVisible => vec![v(1)?, s(0)?],
            Self::Check => vec![v(1)?, s(0)?, json!(true)],
            Self::Uncheck => vec![v(1)?, s(0)?, json!(false)],
            Self::Fill => vec![v(2)?, s(0)?, v(1)?],
            Self::Press | Self::Select | Self::Attribute => vec![v(2)?, s(1)?, v(0)?],
            Self::Wait => vec![
                v(1)?,
                s(0)?,
                json!(milliseconds(&values[2], "Wait For's time", 0)?),
            ],
            Self::ExpectText => vec![v(1)?, s(0)?, v(2)?],
            Self::ExpectTitle => vec![v(0)?, v(1)?],
            Self::Evaluate => vec![v(2)?, v(0)?, v(1)?],
            Self::Screenshot => vec![v(0)?, json!(file(1)?), json!(false)],
            Self::FullScreenshot => vec![v(0)?, json!(file(1)?), json!(true)],
            Self::StopTracing => vec![v(0)?, json!(file(1)?)],
        };
        let answer = run
            .host
            .call(self.name(), args, ANSWER)
            .await
            .map_err(|failure| failed(failure, run))?;
        let artifact = |kind: &str, path: &str| -> DiagnosticResult<Literal> {
            let path = crate::core::paths::canonicalize(Path::new(path))
                .map_err(|error| {
                    Diagnostic::new(BWErr::OutputError(format!(
                        "Playwright wrote no file at {path}: {error}"
                    )))
                })?
                .display()
                .to_string();
            if let Some(recorder) = &run.recorder {
                recorder.artifact(kind, &path);
            }
            Ok(Literal::String(path))
        };
        match self {
            Self::Screenshot | Self::FullScreenshot => {
                artifact("screenshot", answer.as_str().unwrap_or_default())
            }
            Self::StopTracing => artifact("trace", answer.as_str().unwrap_or_default()),
            Self::CloseContext => {
                for video in answer
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    artifact("video", video)?;
                }
                Ok(Literal::None)
            }
            _ => webdriver::values::from_json(&answer, "").map_err(|reason| {
                Diagnostic::new(BWErr::NativeError(format!(
                    "Playwright: the result: {reason}"
                )))
            }),
        }
    }
}
