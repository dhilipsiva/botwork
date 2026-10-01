//! WebDriver (decision D11). `Import |"botwork:webdriver"| As |web|` publishes
//! statements that drive a browser through a W3C WebDriver server: one Botwork
//! starts, such as chromedriver, or one already listening. A session belongs to
//! the run that opened it, and the run's end closes any it left open.
use super::*;
use crate::core::{
    diagnostic::DiagnosticCode as Code,
    operation::OperationControl,
    signature::{StatementSignature, ValueKind as Kind, ValueKinds},
};
use hyper::Method;
use serde_json::{json, Value};
use std::fmt;
use std::{collections::BTreeMap, ffi::OsString, sync::OnceLock, time::Duration};

mod client;
mod sessions;
pub(in crate::core::eval) mod values;

use client::{segment, Endpoint, Failure};
pub(in crate::core::eval) use sessions::Sessions;
use sessions::{Driver, Session};

/// The import path that names this module.
pub(crate) const PATH: &str = "botwork:webdriver";
/// Each command's bound unless `timeout_ms` says otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest `timeout_ms`.
const MAX_TIMEOUT_MS: i32 = 600_000;
/// How often Wait For Element looks again.
const WAIT_INTERVAL: Duration = Duration::from_millis(100);

/// The statements, each with what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Open,
    Close,
    Navigate,
    Url,
    Title,
    Find,
    FindAll,
    Wait,
    Click,
    Type,
    Clear,
    Text,
    Attribute,
    Displayed,
    Script,
    Screenshot,
}

impl Command {
    const ALL: [Self; 16] = [
        Self::Open,
        Self::Close,
        Self::Navigate,
        Self::Url,
        Self::Title,
        Self::Find,
        Self::FindAll,
        Self::Wait,
        Self::Click,
        Self::Type,
        Self::Clear,
        Self::Text,
        Self::Attribute,
        Self::Displayed,
        Self::Script,
        Self::Screenshot,
    ];

    fn header(self) -> &'static str {
        match self {
            Self::Open => "Open Browser |options|",
            Self::Close => "Close Browser |browser|",
            Self::Navigate => "Navigate |browser| To |url|",
            Self::Url => "Current URL Of |browser|",
            Self::Title => "Title Of |browser|",
            Self::Find => "Find Element |selector| In |scope|",
            Self::FindAll => "Find Elements |selector| In |scope|",
            Self::Wait => "Wait For Element |selector| In |scope| Within |milliseconds|",
            Self::Click => "Click |element|",
            Self::Type => "Type |text| Into |element|",
            Self::Clear => "Clear |element|",
            Self::Text => "Text Of |element|",
            Self::Attribute => "Attribute |name| Of |element|",
            Self::Displayed => "Is Displayed |element|",
            Self::Script => "Execute Script |script| With |arguments| In |browser|",
            Self::Screenshot => "Take Screenshot Of |browser| To |path|",
        }
    }

    fn signature(self) -> StatementSignature {
        let (parameters, returns, description): (&[(&str, ValueKinds)], ValueKinds, &str) = match self {
            Self::Open => (
                &[("options", Kind::Map.into())],
                Kind::Map.into(),
                "Open a browser session through a WebDriver server and return its handle. Options: `driver`, an http URL of a running server or the path of a driver executable to start (required); `capabilities`, the W3C capabilities to match; `timeout_ms`, each command's bound (default 30000).",
            ),
            Self::Close => (&[("browser", Kind::Map.into())], Kind::None.into(), "Close the browser session, and end the driver Botwork started for it."),
            Self::Navigate => (
                &[("browser", Kind::Map.into()), ("url", Kind::String.into())],
                Kind::None.into(),
                "Load a URL and wait for the page to load.",
            ),
            Self::Url => (&[("browser", Kind::Map.into())], Kind::String.into(), "The URL of the current page."),
            Self::Title => (&[("browser", Kind::Map.into())], Kind::String.into(), "The title of the current page."),
            Self::Find => (
                &[("selector", Kind::String.into_union(Kind::Map)), ("scope", Kind::Map.into())],
                Kind::Map.into(),
                "The first element matching a selector, in a browser or within an element. A String selector is CSS; a Map names one of css, xpath, link_text, partial_link_text, or tag_name.",
            ),
            Self::FindAll => (
                &[("selector", Kind::String.into_union(Kind::Map)), ("scope", Kind::Map.into())],
                Kind::Array.into(),
                "Every element matching a selector, in document order; empty when none does.",
            ),
            Self::Wait => (
                &[
                    ("selector", Kind::String.into_union(Kind::Map)),
                    ("scope", Kind::Map.into()),
                    ("milliseconds", Kind::Int.into()),
                ],
                Kind::Map.into(),
                "Look for a matching element every 100 ms until one appears, and return it.",
            ),
            Self::Click => (&[("element", Kind::Map.into())], Kind::None.into(), "Click an element."),
            Self::Type => (
                &[("text", Kind::String.into()), ("element", Kind::Map.into())],
                Kind::None.into(),
                "Type text into an element.",
            ),
            Self::Clear => (&[("element", Kind::Map.into())], Kind::None.into(), "Clear an editable element."),
            Self::Text => (&[("element", Kind::Map.into())], Kind::String.into(), "An element's rendered text."),
            Self::Attribute => (
                &[("name", Kind::String.into()), ("element", Kind::Map.into())],
                Kind::String.into_union(Kind::None),
                "An element's attribute, or None when it has none.",
            ),
            Self::Displayed => (&[("element", Kind::Map.into())], Kind::Bool.into(), "Whether an element is displayed."),
            Self::Script => (
                &[
                    ("script", Kind::String.into()),
                    ("arguments", Kind::Array.into()),
                    ("browser", Kind::Map.into()),
                ],
                ValueKinds::ANY,
                "Run JavaScript in the page as a function body with `arguments`, and return its result. Element handles cross as elements.",
            ),
            Self::Screenshot => (
                &[("browser", Kind::Map.into()), ("path", Kind::String.into())],
                Kind::String.into(),
                "Save a PNG of the browser's viewport, record it as the run's artifact, and return its absolute path. A relative path is from the run's directory.",
            ),
        };
        let mut signature = StatementSignature::native_at("<webdriver>", self.header())
            .expect("fixed WebDriver signature")
            .returns(returns)
            .description(description);
        for (name, kinds) in parameters {
            signature = signature.parameter(name, *kinds).expect("fixed parameter");
        }
        let mut errors = vec![
            (
                Code::IncompatibleType,
                "A handle, selector, option, or value that does not fit.",
            ),
            (
                Code::Native,
                "The driver failed the command, could not start, or could not be reached.",
            ),
            (
                Code::AsyncRuntime,
                "Requires asynchronous execution, as the CLI runs scripts.",
            ),
        ];
        if self == Self::Wait {
            errors.push((
                Code::ConditionNotMet,
                "No element matched before the time was up.",
            ));
        }
        if self == Self::Screenshot {
            errors.push((Code::Output, "The file could not be written."));
        }
        for (code, reason) in errors {
            signature = signature
                .documents_error(code, reason)
                .expect("fixed error");
        }
        signature
    }
}

trait Union {
    fn into_union(self, other: Kind) -> ValueKinds;
}
impl Union for Kind {
    fn into_union(self, other: Kind) -> ValueKinds {
        ValueKinds::one(self).union(ValueKinds::one(other))
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

/// The statements this module exports, as analysis lists a module's: by
/// normalized signature, with the header to show.
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
    sessions: Arc<Sessions>,
    recorder: Option<crate::core::report::Recorder>,
    directory: PathBuf,
    variables: BTreeMap<OsString, OsString>,
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
    let run = Arc::new(Run {
        sessions: context.modules.webdriver_sessions(),
        recorder: environment
            .as_ref()
            .and_then(|environment| environment.recorder.clone()),
        directory,
        variables: environment.as_ref().map_or_else(
            || std::env::vars_os().collect(),
            |environment| environment.variables().clone(),
        ),
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

fn native(reason: impl fmt::Display) -> Diagnostic {
    Diagnostic::new(BWErr::NativeError(format!("WebDriver: {reason}")))
}

fn failed(failure: Failure) -> Diagnostic {
    native(failure)
}

fn string(value: &Value, what: &str) -> DiagnosticResult<String> {
    value.as_str().map(str::to_owned).ok_or_else(|| {
        native(format_args!(
            "the driver returned {what} that is not a String"
        ))
    })
}

impl Command {
    async fn run(
        self,
        run: &Run,
        values: Vec<Literal>,
        _control: OperationControl,
    ) -> DiagnosticResult<Literal> {
        if self == Self::Open {
            return open(run, &values[0]).await;
        }
        let browser = match self {
            Self::Find | Self::FindAll | Self::Wait => &values[1],
            Self::Type | Self::Attribute => &values[1],
            Self::Script => &values[2],
            _ => &values[0],
        };
        let (id, element) = values::handle(browser, "The handle").map_err(incompatible)?;
        let session = run.sessions.get(&id).map_err(incompatible)?;
        let base = format!("/session/{}", segment(&id));
        let element_path = |element: &Option<String>| -> DiagnosticResult<String> {
            element
                .as_ref()
                .map(|element| format!("{base}/element/{}", segment(element)))
                .ok_or_else(|| {
                    incompatible("The handle is a browser; pass an element from Find Element")
                })
        };
        let send = |method: Method, path: String, body: Option<Value>| {
            let session = Arc::clone(&session);
            async move {
                session
                    .endpoint
                    .command(method, &path, body.as_ref(), session.timeout)
                    .await
                    .map_err(failed)
            }
        };
        match self {
            Self::Open => unreachable!("handled above"),
            Self::Close => {
                send(Method::DELETE, base, None).await?;
                if let Some(session) = run.sessions.remove(&id) {
                    session.end_driver();
                }
                Ok(Literal::None)
            }
            Self::Navigate => {
                let Literal::String(url) = &values[1] else {
                    unreachable!("checked kind")
                };
                send(
                    Method::POST,
                    format!("{base}/url"),
                    Some(json!({ "url": url })),
                )
                .await?;
                Ok(Literal::None)
            }
            Self::Url => Ok(Literal::String(string(
                &send(Method::GET, format!("{base}/url"), None).await?,
                "a URL",
            )?)),
            Self::Title => Ok(Literal::String(string(
                &send(Method::GET, format!("{base}/title"), None).await?,
                "a title",
            )?)),
            Self::Find | Self::FindAll | Self::Wait => {
                let (using, value) = values::selector(&values[0]).map_err(incompatible)?;
                let scope = match &element {
                    Some(element) => format!("{base}/element/{}", segment(element)),
                    None => base.clone(),
                };
                let query = json!({ "using": using, "value": value });
                let find_all = || {
                    send(
                        Method::POST,
                        format!("{scope}/elements"),
                        Some(query.clone()),
                    )
                };
                match self {
                    Self::Find => {
                        let found = send(
                            Method::POST,
                            format!("{scope}/element"),
                            Some(query.clone()),
                        )
                        .await?;
                        reference(&found, &id)
                    }
                    Self::FindAll => {
                        let found = find_all().await?;
                        let items = found.as_array().ok_or_else(|| {
                            native("the driver returned elements that are not an Array")
                        })?;
                        Ok(Literal::Array(
                            items
                                .iter()
                                .map(|item| reference(item, &id))
                                .collect::<Result<_, _>>()?,
                        ))
                    }
                    _ => {
                        let Literal::Int(milliseconds) = values[2] else {
                            unreachable!("checked kind")
                        };
                        let limit = u64::try_from(milliseconds)
                            .ok()
                            .filter(|milliseconds| *milliseconds <= MAX_TIMEOUT_MS as u64)
                            .ok_or_else(|| incompatible(format_args!("Wait For Element waits 0 to {MAX_TIMEOUT_MS} ms, not {milliseconds}")))?;
                        let deadline = tokio::time::Instant::now() + Duration::from_millis(limit);
                        let mut attempts = 0u32;
                        loop {
                            attempts += 1;
                            let found = find_all().await?;
                            if let Some(first) = found.as_array().and_then(|items| items.first()) {
                                return reference(first, &id);
                            }
                            if tokio::time::Instant::now() >= deadline {
                                return Err(Diagnostic::new(BWErr::ConditionNotMet {
                                    reason: format!(
                                        "no element matched {} within {limit} ms",
                                        values::describe(using, &value)
                                    ),
                                    attempts: attempts.to_string(),
                                    history: "[]".into(),
                                }));
                            }
                            tokio::time::sleep_until(
                                deadline.min(tokio::time::Instant::now() + WAIT_INTERVAL),
                            )
                            .await;
                        }
                    }
                }
            }
            Self::Click => {
                send(
                    Method::POST,
                    format!("{}/click", element_path(&element)?),
                    Some(json!({})),
                )
                .await?;
                Ok(Literal::None)
            }
            Self::Type => {
                let Literal::String(text) = &values[0] else {
                    unreachable!("checked kind")
                };
                send(
                    Method::POST,
                    format!("{}/value", element_path(&element)?),
                    Some(json!({ "text": text })),
                )
                .await?;
                Ok(Literal::None)
            }
            Self::Clear => {
                send(
                    Method::POST,
                    format!("{}/clear", element_path(&element)?),
                    Some(json!({})),
                )
                .await?;
                Ok(Literal::None)
            }
            Self::Text => Ok(Literal::String(string(
                &send(
                    Method::GET,
                    format!("{}/text", element_path(&element)?),
                    None,
                )
                .await?,
                "text",
            )?)),
            Self::Attribute => {
                let Literal::String(name) = &values[0] else {
                    unreachable!("checked kind")
                };
                let value = send(
                    Method::GET,
                    format!("{}/attribute/{}", element_path(&element)?, segment(name)),
                    None,
                )
                .await?;
                if value.is_null() {
                    Ok(Literal::None)
                } else {
                    Ok(Literal::String(string(&value, "an attribute")?))
                }
            }
            Self::Displayed => send(
                Method::GET,
                format!("{}/displayed", element_path(&element)?),
                None,
            )
            .await?
            .as_bool()
            .map(Literal::Bool)
            .ok_or_else(|| native("the driver returned a displayed state that is not a Bool")),
            Self::Script => {
                let (Literal::String(script), Literal::Array(arguments)) = (&values[0], &values[1])
                else {
                    unreachable!("checked kinds")
                };
                let arguments = arguments
                    .iter()
                    .map(values::to_json)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(incompatible)?;
                let result = send(
                    Method::POST,
                    format!("{base}/execute/sync"),
                    Some(json!({ "script": script, "args": arguments })),
                )
                .await?;
                values::from_json(&result, &id)
                    .map_err(|reason| native(format_args!("the script's result: {reason}")))
            }
            Self::Screenshot => {
                let Literal::String(path) = &values[1] else {
                    unreachable!("checked kind")
                };
                let encoded = send(Method::GET, format!("{base}/screenshot"), None).await?;
                let png = values::base64(&string(&encoded, "a screenshot")?).map_err(native)?;
                let path = run.directory.join(path);
                let written = std::fs::write(&path, &png)
                    .and_then(|()| crate::core::paths::canonicalize(&path));
                let path = written.map_err(|error| {
                    Diagnostic::new(BWErr::OutputError(format!(
                        "Writing the screenshot {} failed: {error}",
                        path.display()
                    )))
                })?;
                let path = path.display().to_string();
                if let Some(recorder) = &run.recorder {
                    recorder.artifact("screenshot", &path);
                }
                Ok(Literal::String(path))
            }
        }
    }
}

/// An element handle for the driver's element reference.
fn reference(found: &Value, session: &str) -> DiagnosticResult<Literal> {
    found
        .get(values::ELEMENT)
        .and_then(Value::as_str)
        .map(|id| values::element(session, id))
        .ok_or_else(|| native("the driver returned an element without a W3C reference"))
}

/// Open a session, starting the driver when `driver` names an executable.
async fn open(run: &Run, options: &Literal) -> DiagnosticResult<Literal> {
    let Literal::Map(options) = options else {
        unreachable!("checked kind")
    };
    for key in options.keys() {
        if !matches!(key.as_str(), "driver" | "capabilities" | "timeout_ms") {
            return Err(incompatible(format_args!(
                "Unknown Open Browser option `{key}`; use driver, capabilities, or timeout_ms"
            )));
        }
    }
    let timeout = match options.get("timeout_ms") {
        None => DEFAULT_TIMEOUT,
        Some(Literal::Int(milliseconds)) if (1..=MAX_TIMEOUT_MS).contains(milliseconds) => {
            Duration::from_millis(*milliseconds as u64)
        }
        Some(other) => {
            return Err(incompatible(format_args!(
                "`timeout_ms` is an Int from 1 to {MAX_TIMEOUT_MS}, not {other}"
            )))
        }
    };
    let capabilities = match options.get("capabilities") {
        None => json!({}),
        Some(capabilities @ Literal::Map(_)) => {
            values::to_json(capabilities).map_err(incompatible)?
        }
        Some(other) => {
            return Err(incompatible(format_args!(
                "`capabilities` is a Map, not a {}",
                other.kind().as_str()
            )))
        }
    };
    let Some(Literal::String(driver)) = options.get("driver") else {
        return Err(incompatible(
            "Open Browser needs `driver`: the http URL of a WebDriver server, or the path of a driver such as chromedriver",
        ));
    };
    let (driver, endpoint) = if driver.starts_with("http://") || driver.starts_with("https://") {
        (None, Endpoint::parse(driver).map_err(incompatible)?)
    } else {
        let executable =
            executable(driver, &run.directory, &run.variables).map_err(incompatible)?;
        let (process, endpoint) =
            Driver::start(&executable, &run.directory, &run.variables, timeout)
                .await
                .map_err(native)?;
        (Some(process), endpoint)
    };
    let created = endpoint
        .command(
            Method::POST,
            "/session",
            Some(&json!({ "capabilities": { "alwaysMatch": capabilities } })),
            timeout,
        )
        .await
        .map_err(|failure| native(format_args!("the session could not start: {failure}")))?;
    let id = string(&created["sessionId"], "a session ID")?;
    let browser = created["capabilities"]["browserName"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let version = created["capabilities"]["browserVersion"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    // Registered before any other await, so a stop cannot lose the session.
    run.sessions
        .insert(id.clone(), Session::new(endpoint, timeout, driver));
    Ok(Literal::Map(HashMap::from([
        ("session".to_owned(), Literal::String(id)),
        ("browser".to_owned(), Literal::String(browser)),
        ("version".to_owned(), Literal::String(version)),
    ])))
}

/// A driver executable: a path, relative to the run's directory, or a name on
/// the run's `PATH`.
fn executable(
    driver: &str,
    directory: &Path,
    variables: &BTreeMap<OsString, OsString>,
) -> Result<PathBuf, String> {
    let named = Path::new(driver);
    if named.components().count() > 1 || named.is_absolute() {
        return Ok(directory.join(named));
    }
    let suffixes: &[&str] = if cfg!(windows) { &["", ".exe"] } else { &[""] };
    variables
        .get(std::ffi::OsStr::new("PATH"))
        .into_iter()
        .flat_map(std::env::split_paths)
        .flat_map(|directory| suffixes.iter().map(move |suffix| directory.join(format!("{driver}{suffix}"))))
        .find(|path| path.is_absolute() && path.is_file())
        .ok_or_else(|| format!("No driver `{driver}` on the run's PATH; name its path, or start it and pass its http URL"))
}
