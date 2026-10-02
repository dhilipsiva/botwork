//! Every supported failure path of the browser and device integrations
//! (docs/browser-matrix.md): session creation failure, element timeout,
//! assertion failure, cancellation, and parallel session isolation. Each ends
//! with its diagnostic, the failure artifacts that show it, and nothing left
//! open. They run against fakes on every platform, and against Chrome,
//! Chromium, and an Android emulator where `BOTWORK_WEBDRIVER`,
//! `BOTWORK_PLAYWRIGHT`, and `BOTWORK_APPIUM` configure them, as CI does.
#[path = "support/webdriver_fake.rs"]
#[allow(dead_code)] // Shared fake: these tests read only its answers.
mod webdriver_fake;

use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use webdriver_fake::Fake;

/// One way a module reaches a browser or a device.
#[derive(Clone)]
enum Target {
    /// The fake WebDriver server, by URL.
    FakeWebDriver(String),
    /// The fake `playwright` package, installed in each run directory.
    FakePlaywright,
    /// chromedriver, which Botwork starts, and the Chrome binary it should use.
    Chrome { driver: String, binary: String },
    /// A directory with Playwright and Chromium installed.
    Chromium(PathBuf),
    /// Appium, which Botwork starts, with an Android device attached.
    Android(String),
}

impl Target {
    fn webdriver(&self) -> bool {
        !matches!(self, Self::FakePlaywright | Self::Chromium(_))
    }

    fn real(&self) -> bool {
        matches!(
            self,
            Self::Chrome { .. } | Self::Chromium(_) | Self::Android(_)
        )
    }

    /// The statements that open a browser as `browser` (and, for Playwright,
    /// a page as `page`), keeping failure artifacts, with `extra` options.
    fn open(&self, extra: &str) -> String {
        let common = format!("failure_artifacts: \"artifacts\"{extra}");
        match self {
            Self::FakeWebDriver(url) => format!(
                "Import |\"botwork:webdriver\"| As |web|\n|browser| = web::Open Browser |{{driver: \"{url}\", {common}}}|\n"
            ),
            Self::Chrome { driver, binary } => {
                let mut options = json!({ "args": ["--headless=new", "--no-sandbox", "--disable-dev-shm-usage"] });
                if !binary.is_empty() {
                    options["binary"] = json!(binary);
                }
                format!(
                    "Import |\"botwork:webdriver\"| As |web|\n|browser| = web::Open Browser |{{driver: {driver}, capabilities: {{browserName: \"chrome\", \"goog:chromeOptions\": {options}}}, timeout_ms: 120000, {common}}}|\n",
                    driver = json!(driver)
                )
            }
            Self::Android(appium) => format!(
                "Import |\"botwork:webdriver\"| As |web|\n|browser| = web::Open Browser |{{driver: {appium}, capabilities: {{platformName: \"Android\", \"appium:automationName\": \"UiAutomator2\", \"appium:appPackage\": \"com.android.settings\", \"appium:appActivity\": \".Settings\"}}, timeout_ms: 300000, {common}}}|\n",
                appium = json!(appium)
            ),
            Self::FakePlaywright | Self::Chromium(_) => format!(
                "Import |\"botwork:playwright\"| As |pw|\n|browser| = pw::Launch Browser |{{timeout_ms: 2000, {common}}}|\n|page| = pw::New Page In |@{{ pw::New Context In |browser| With |{{}}| }}|\n"
            ),
        }
    }

    /// A statement that loads the test page.
    fn load(&self) -> &'static str {
        match self {
            Self::FakeWebDriver(_) | Self::Chrome { .. } => {
                "web::Navigate |browser| To |\"data:text/html,<title>Form</title><h1>Form</h1>\"|\n"
            }
            Self::Android(_) => "",
            Self::FakePlaywright | Self::Chromium(_) => {
                "pw::Go To |\"data:text/html,<title>Form</title><h1>Form</h1>\"| In |page|\n"
            }
        }
    }

    /// A statement that waits `milliseconds` for an element that never appears.
    fn wait_for_nothing(&self, milliseconds: u32) -> String {
        if let Self::Android(_) = self {
            // Native apps have no CSS; a resource ID that no view has.
            format!("web::Wait For Element |{{id: \"android:id/botwork_never\"}}| In |browser| Within |{milliseconds}|\n")
        } else if self.webdriver() {
            format!("web::Wait For Element |\"#never\"| In |browser| Within |{milliseconds}|\n")
        } else {
            format!("pw::Wait For |\"#never\"| In |page| Within |{milliseconds}|\n")
        }
    }

    /// A statement that closes what `open` opened.
    fn close(&self) -> &'static str {
        if self.webdriver() {
            "web::Close Browser |browser|\n"
        } else {
            "pw::Close Browser |browser|\n"
        }
    }

    /// A statement that never finishes by itself.
    fn hang(&self) -> String {
        match self {
            Self::FakeWebDriver(_) => "web::Navigate |browser| To |\"http://hang.test/\"|\n".into(),
            Self::FakePlaywright => "pw::Go To |\"http://hang.test/\"| In |page|\n".into(),
            _ => self.wait_for_nothing(600_000),
        }
    }
}

/// A run directory for `target`, inside the Playwright installation for
/// Chromium so the run finds it.
fn workspace(target: &Target) -> tempfile::TempDir {
    let directory = match target {
        Target::Chromium(installed) => tempfile::tempdir_in(installed).unwrap(),
        _ => tempfile::tempdir().unwrap(),
    };
    if matches!(target, Target::FakePlaywright) {
        let package = directory.path().join("node_modules/playwright");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"name": "playwright", "main": "index.js"}"#,
        )
        .unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/playwright_fake.js"),
            package.join("index.js"),
        )
        .unwrap();
    }
    directory
}

fn botwork(directory: &Path, arguments: &[&str]) -> Output {
    let (stdout, stderr) = (directory.join(".stdout"), directory.join(".stderr"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(600);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            break child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}

/// Run `source` in `directory` with a JSON report, and return the CLI's stderr
/// and each run's record.
fn run(directory: &Path, source: &str, more: &[&str]) -> (Output, Vec<Value>) {
    fs::write(directory.join("main.botwork"), source).unwrap();
    let mut arguments = vec!["--file", "main.botwork", "--report-json", "report.json"];
    arguments.extend(more);
    let output = botwork(directory, &arguments);
    let report: Value =
        serde_json::from_slice(&fs::read(directory.join("report.json")).unwrap()).unwrap();
    (output, report["runs"].as_array().unwrap().clone())
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The paths of a run's artifacts of `kind`.
fn artifacts(record: &Value, kind: &str) -> Vec<PathBuf> {
    record["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|artifact| artifact["kind"] == kind)
        .map(|artifact| PathBuf::from(artifact["path"].as_str().unwrap()))
        .collect()
}

/// A capture's files: a screenshot that is an image, and the page's HTML.
fn assert_captured(target: &Target, record: &Value, at_least: usize) {
    let screenshots = artifacts(record, "failure-screenshot");
    let sources = artifacts(record, "failure-source");
    assert!(
        screenshots.len() >= at_least && sources.len() >= at_least,
        "{record:#}"
    );
    for path in &screenshots {
        let bytes = fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        if target.real() {
            assert_eq!(&bytes[..4], b"\x89PNG", "{}", path.display());
        }
    }
    for path in &sources {
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains('<'), "{}: {text}", path.display());
    }
}

/// Nothing the run started is still running: on Linux, no process has the run
/// directory as its working directory. (adb's server outlives its clients.)
fn assert_closed(directory: &Path) {
    #[cfg(target_os = "linux")]
    {
        let directory = botwork::core::paths::canonicalize(directory).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left: Vec<String> = fs::read_dir("/proc")
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .bytes()
                        .all(|byte| byte.is_ascii_digit())
                })
                .filter(|entry| {
                    fs::read_link(entry.path().join("cwd"))
                        .is_ok_and(|cwd| cwd.starts_with(&directory))
                })
                .map(|entry| {
                    fs::read_to_string(entry.path().join("comm"))
                        .unwrap_or_default()
                        .trim()
                        .to_owned()
                })
                .filter(|name| name != "adb")
                .collect();
            if left.is_empty() {
                return;
            }
            assert!(Instant::now() < deadline, "left running: {left:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = directory;
}

/// An element that never appears: the wait fails catchably with BW9004 and
/// names the capture it saved, and the browser closes in Finally.
fn element_timeout(target: &Target) {
    let directory = workspace(target);
    let source = format!(
        "{open}Try {{\n    {load}    {wait}}} Catch |error| {{\n    Log |error.code|\n    Log |error.message|\n}} Finally {{\n    {close}}}\n",
        open = target.open(""),
        load = target.load(),
        wait = target.wait_for_nothing(300),
        close = target.close(),
    );
    let (output, runs) = run(directory.path(), &source, &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("BW9004"), "{stdout}");
    assert!(
        lines.next().unwrap().contains("; failure artifacts: "),
        "{stdout}"
    );
    assert_captured(target, &runs[0], 1);
    assert_closed(directory.path());
}

/// An assertion that fails while the browser is open: the run fails with
/// BW9001, and its end captures the browser before closing it.
fn assertion_failure(target: &Target) {
    let directory = workspace(target);
    let read = match target {
        // Native apps have no title; the Settings list's first entry has text.
        Target::Android(_) => "web::Text Of |@{ web::Wait For Element |{android_uiautomator: \"new UiSelector().textContains(\\\"Network\\\")\"}| In |browser| Within |120000| }|",
        _ if target.webdriver() => "web::Title Of |browser|",
        _ => "pw::Title Of |page|",
    };
    let source = format!(
        "{open}{load}Assert |@{{ {read} }}| Equals |\"Not the title\"|\n{close}",
        open = target.open(""),
        load = target.load(),
        close = target.close(),
    );
    let (output, runs) = run(directory.path(), &source, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("[BW9001]"), "{}", stderr(&output));
    assert_captured(target, &runs[0], 1);
    assert_closed(directory.path());
}

/// A run stopped by its deadline mid-command: BW5002, a capture, and every
/// session closed.
fn cancellation(target: &Target) {
    let directory = workspace(target);
    let deadline = if target.real() { "20000" } else { "2000" };
    let source = format!("{}{}{}", target.open(""), target.load(), target.hang());
    let started = Instant::now();
    let (output, runs) = run(directory.path(), &source, &["--timeout-ms", deadline]);
    assert!(stderr(&output).contains("[BW5002]"), "{}", stderr(&output));
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "{:?}",
        started.elapsed()
    );
    assert_captured(target, &runs[0], 1);
    assert_closed(directory.path());
}

/// Two cases in parallel, each failing in its own session: each run records
/// only its own captures, in files of their own.
fn parallel_isolation(target: &Target) {
    let directory = workspace(target);
    let case = |id: &str| {
        let body = format!(
            "{}{}{}",
            target.open(""),
            target.load(),
            target.wait_for_nothing(200)
        );
        let body: String = body
            .lines()
            .map(|line| format!("        {line}\n"))
            .collect();
        format!("    Case |\"{id}\"| {{\n{body}    }}\n")
    };
    // Imports belong in the suite's Library.
    let (imports, cases): (Vec<String>, Vec<String>) = (case("one") + &case("two"))
        .lines()
        .map(str::to_owned)
        .partition(|line| line.trim_start().starts_with("Import"));
    let mut imports: Vec<String> = imports
        .iter()
        .map(|line| format!("        {}", line.trim()))
        .collect();
    imports.dedup();
    let suite = format!(
        "Suite |\"parallel\"| {{\n    Library {{\n{}\n    }}\n{}\n}}\n",
        imports.join("\n"),
        cases.join("\n")
    );
    fs::write(directory.path().join("parallel.suite.botwork"), suite).unwrap();
    let output = botwork(
        directory.path(),
        &[
            "--suite",
            "parallel.suite.botwork",
            "--jobs",
            "2",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let report: Value =
        serde_json::from_slice(&fs::read(directory.path().join("report.json")).unwrap()).unwrap();
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    let mut all = Vec::new();
    for record in runs {
        assert_eq!(record["status"], "failed", "{record:#}");
        assert_captured(target, record, 1);
        let mine = artifacts(record, "failure-screenshot");
        all.extend(mine.iter().cloned());
        // A capture names the session or page it came from, and only one.
        let stems: std::collections::BTreeSet<String> = mine
            .iter()
            .map(|path| {
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                name.rsplit_once('-').unwrap().0.to_owned()
            })
            .collect();
        assert_eq!(stems.len(), 1, "{mine:?}");
    }
    let unique: std::collections::BTreeSet<_> = all.iter().collect();
    assert_eq!(unique.len(), all.len(), "{all:?}");
    let stems: std::collections::BTreeSet<String> = all
        .iter()
        .map(|path| {
            path.file_stem()
                .unwrap()
                .to_string_lossy()
                .rsplit_once('-')
                .unwrap()
                .0
                .to_owned()
        })
        .collect();
    assert_eq!(stems.len(), 2, "the two cases share a session: {all:?}");
    assert_closed(directory.path());
}

/// A session that cannot start: BW4002 naming why, with the driver's log
/// when Botwork started the driver, and nothing left running.
fn session_creation_failure(target: &Target) {
    let directory = workspace(target);
    let (source, log) = match target {
        Target::FakeWebDriver(url) => (
            format!("Import |\"botwork:webdriver\"| As |web|\nweb::Open Browser |{{driver: \"{url}\", capabilities: {{browserName: \"refused\"}}, failure_artifacts: \"artifacts\"}}|\n"),
            false,
        ),
        Target::Chrome { driver, .. } => (
            format!(
                "Import |\"botwork:webdriver\"| As |web|\nweb::Open Browser |{{driver: {}, capabilities: {{browserName: \"chrome\", \"goog:chromeOptions\": {{binary: \"/no/such/chrome\"}}}}, timeout_ms: 120000, failure_artifacts: \"artifacts\"}}|\n",
                json!(driver)
            ),
            true,
        ),
        Target::Android(appium) => (
            format!(
                "Import |\"botwork:webdriver\"| As |web|\nweb::Open Browser |{{driver: {}, capabilities: {{platformName: \"Android\", \"appium:automationName\": \"UiAutomator2\", \"appium:appPackage\": \"com.botwork.missing\", \"appium:appActivity\": \".Missing\"}}, timeout_ms: 300000, failure_artifacts: \"artifacts\"}}|\n",
                json!(appium)
            ),
            true,
        ),
        Target::FakePlaywright | Target::Chromium(_) => (
            "Import |\"botwork:playwright\"| As |pw|\npw::Launch Browser |{executable_path: \"/no/such/browser\", failure_artifacts: \"artifacts\"}|\n".into(),
            false,
        ),
    };
    let (output, runs) = run(directory.path(), &source, &[]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(text.contains("[BW4002]"), "{text}");
    if target.webdriver() {
        assert!(text.contains("the session could not start"), "{text}");
    }
    if log {
        let logs = artifacts(&runs[0], "driver-log");
        assert_eq!(logs.len(), 1, "{:#}", runs[0]);
        assert!(text.contains(&logs[0].display().to_string()), "{text}");
        assert!(
            fs::metadata(&logs[0]).unwrap().len() > 0,
            "an empty driver log"
        );
    }
    assert_closed(directory.path());
}

fn node() -> bool {
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let found = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(name).is_file())
    });
    assert!(
        found || std::env::var_os("BOTWORK_REQUIRE_NODE").is_none(),
        "BOTWORK_REQUIRE_NODE is set, but Node is not on PATH"
    );
    found
}

fn configured(variable: &str, required: &str) -> Option<String> {
    match std::env::var(variable) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => {
            assert!(
                std::env::var_os(required).is_none(),
                "{required} is set, but {variable} is not"
            );
            None
        }
    }
}

/// The fakes, on every platform.
fn fakes(fake: &Fake) -> Vec<Target> {
    let mut targets = vec![Target::FakeWebDriver(fake.url.clone())];
    if node() {
        targets.push(Target::FakePlaywright);
    }
    targets
}

#[test]
fn fakes_report_session_creation_failures() {
    let fake = Fake::start();
    fakes(&fake).iter().for_each(session_creation_failure);
}

#[test]
fn fakes_capture_element_timeouts() {
    let fake = Fake::start();
    fakes(&fake).iter().for_each(element_timeout);
}

#[test]
fn fakes_capture_assertion_failures_as_the_run_ends() {
    let fake = Fake::start();
    fakes(&fake).iter().for_each(assertion_failure);
}

#[test]
fn fakes_capture_and_close_cancelled_runs() {
    let fake = Fake::start();
    fakes(&fake).iter().for_each(cancellation);
}

#[test]
fn fakes_keep_parallel_sessions_apart() {
    let fake = Fake::start();
    fakes(&fake).iter().for_each(parallel_isolation);
}

/// A session that closes without a failure leaves no artifacts behind.
#[test]
fn a_session_that_closes_cleanly_keeps_nothing() {
    let fake = Fake::start();
    for target in fakes(&fake) {
        let directory = workspace(&target);
        let source = format!("{}{}{}", target.open(""), target.load(), target.close());
        let (output, runs) = run(directory.path(), &source, &[]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(runs[0]["artifacts"], json!([]));
        let left: Vec<_> = fs::read_dir(directory.path().join("artifacts"))
            .unwrap()
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }
}

fn chrome() -> Option<Target> {
    let driver = configured("BOTWORK_WEBDRIVER", "BOTWORK_REQUIRE_WEBDRIVER")?;
    Some(Target::Chrome {
        driver,
        binary: std::env::var("BOTWORK_CHROME").unwrap_or_default(),
    })
}

fn chromium() -> Option<Target> {
    configured("BOTWORK_PLAYWRIGHT", "BOTWORK_REQUIRE_PLAYWRIGHT")
        .map(|directory| Target::Chromium(directory.into()))
}

fn android() -> Option<Target> {
    configured("BOTWORK_APPIUM", "BOTWORK_REQUIRE_APPIUM").map(Target::Android)
}

#[test]
fn chrome_covers_every_failure_path() {
    let Some(target) = chrome() else {
        return;
    };
    session_creation_failure(&target);
    element_timeout(&target);
    assertion_failure(&target);
    cancellation(&target);
    parallel_isolation(&target);
}

#[test]
fn chromium_covers_every_failure_path() {
    let Some(target) = chromium() else {
        return;
    };
    session_creation_failure(&target);
    element_timeout(&target);
    assertion_failure(&target);
    cancellation(&target);
    parallel_isolation(&target);
}

/// One emulator serves one session at a time, so Android has no parallel case.
#[test]
fn android_covers_every_failure_path_but_parallel_sessions() {
    let Some(target) = android() else {
        return;
    };
    session_creation_failure(&target);
    element_timeout(&target);
    assertion_failure(&target);
    cancellation(&target);
}

/// A driver Botwork starts writes its log among the failure artifacts: the
/// log is recorded and named when the session cannot start, and deleted when
/// a session closes without a failure.
#[cfg(unix)]
#[test]
fn a_started_drivers_log_is_kept_only_for_failures() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let driver = directory.path().join("fake-driver");
    fs::write(
        &driver,
        r#"#!/usr/bin/env python3
import http.server, json, sys
port = int([a for a in sys.argv if a.startswith("--port=")][0].split("=")[1])
print("fake driver listening on", port, file=sys.stderr, flush=True)
class Handler(http.server.BaseHTTPRequestHandler):
    def answer(self, status, value):
        body = json.dumps({"value": value}).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self.answer(200, {"ready": True})
    def do_POST(self):
        wanted = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
        if wanted["capabilities"]["alwaysMatch"].get("browserName") == "refused":
            print("refusing the session", file=sys.stderr, flush=True)
            self.answer(500, {"error": "session not created", "message": "refused"})
        else:
            self.answer(200, {"sessionId": "started", "capabilities": {"browserName": "fake"}})
    def do_DELETE(self):
        self.answer(200, None)
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()
"#,
    )
    .unwrap();
    fs::set_permissions(&driver, fs::Permissions::from_mode(0o755)).unwrap();
    let open = |browser: &str| {
        // A cold first start of Python, as on a fresh macOS runner, can take
        // longer than the default 30 seconds.
        format!("Import |\"botwork:webdriver\"| As |web|\n|browser| = web::Open Browser |{{driver: \"./fake-driver\", capabilities: {{browserName: \"{browser}\"}}, failure_artifacts: \"artifacts\", timeout_ms: 180000}}|\n")
    };
    let (output, runs) = run(directory.path(), &open("refused"), &[]);
    let text = stderr(&output);
    let logs: String = fs::read_dir(directory.path().join("artifacts"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| fs::read_to_string(entry.path()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        text.contains("[BW4002]") && text.contains("the session could not start"),
        "{text}\ndriver log:\n{logs}"
    );
    let logs = artifacts(&runs[0], "driver-log");
    assert_eq!(logs.len(), 1, "{:#}", runs[0]);
    assert!(text.contains(&logs[0].display().to_string()), "{text}");
    let log = fs::read_to_string(&logs[0]).unwrap();
    assert!(
        log.contains("fake driver listening") && log.contains("refusing the session"),
        "{log}"
    );
    assert_closed(directory.path());
    // A session that closes cleanly leaves no log.
    fs::remove_dir_all(directory.path().join("artifacts")).unwrap();
    let (output, runs) = run(
        directory.path(),
        &format!("{}web::Close Browser |browser|\n", open("fake")),
        &[],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(runs[0]["artifacts"], json!([]));
    let left: Vec<_> = fs::read_dir(directory.path().join("artifacts"))
        .unwrap()
        .collect();
    assert!(left.is_empty(), "{left:?}");
    assert_closed(directory.path());
}

/// A wrong handle or value never reaches the browser, so it captures nothing.
#[test]
fn wrong_handles_and_values_capture_nothing() {
    let fake = Fake::start();
    for target in fakes(&fake) {
        let directory = workspace(&target);
        let wrong = if target.webdriver() {
            "web::Find Element |{css: \"a\", xpath: \"b\"}| In |browser|"
        } else {
            "pw::Click |{css: \"a\", xpath: \"b\"}| In |page|"
        };
        let source = format!(
            "{open}Try {{\n    {wrong}\n}} Catch |error| {{\n    Log |error.code|\n}}\n{close}",
            open = target.open(""),
            close = target.close(),
        );
        let (output, runs) = run(directory.path(), &source, &[]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "BW3003\n");
        assert_eq!(runs[0]["artifacts"], json!([]));
    }
}
