//! WebDriver (decision D11): `Import |"botwork:webdriver"|` statements against a
//! fake W3C server everywhere, and against headless Chrome and chromedriver
//! where `BOTWORK_WEBDRIVER` names chromedriver (CI requires it on Linux).
#[path = "support/webdriver_fake.rs"]
mod webdriver_fake;

use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use webdriver_fake::{Fake, ELEMENT, PNG};

struct Workspace(tempfile::TempDir);

impl Workspace {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.path().join(name), text).unwrap();
    }

    fn botwork(&self, arguments: &[&str]) -> Output {
        self.botwork_within(arguments, Duration::from_secs(40))
    }

    /// Run the CLI, ending it after `limit`, so a run that hangs fails its
    /// test instead of outlasting it.
    fn botwork_within(&self, arguments: &[&str], limit: Duration) -> Output {
        let (stdout, stderr) = (self.path().join(".stdout"), self.path().join(".stderr"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(arguments)
            .current_dir(self.path())
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + limit;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                break child.wait().unwrap();
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Output {
            status,
            stdout: fs::read(stdout).unwrap(),
            stderr: fs::read(stderr).unwrap(),
        }
    }

    /// Run `source` as main.botwork, after importing the module as `web` and
    /// opening a browser through `driver` as `browser`.
    fn run(&self, driver: &str, source: &str, more: &[&str]) -> Output {
        self.write(
            "main.botwork",
            &format!(
                "Import |\"botwork:webdriver\"| As |web|\n|browser| = web::Open Browser |{{driver: \"{driver}\"}}|\n{source}"
            ),
        );
        let mut arguments = vec!["--file", "main.botwork"];
        arguments.extend(more);
        self.botwork(&arguments)
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn browsers_open_navigate_find_act_and_close_through_a_driver() {
    let fake = Fake::start();
    let workspace = Workspace::new();
    let output = workspace.run(
        &fake.url,
        r#"Log |browser|
web::Navigate |browser| To |"http://fake.test/"|
Log |@{ web::Current URL Of |browser| }|
Log |@{ web::Title Of |browser| }|
|heading| = web::Find Element |"h1"| In |browser|
Log |heading|
Log |@{ web::Text Of |heading| }|
|field| = web::Find Element |{xpath: "//input"}| In |heading|
web::Clear |field|
web::Type |"Ada"| Into |field|
web::Click |field|
Log |@{ web::Attribute |"name"| Of |field| }|
Log |@{ web::Attribute |"missing"| Of |field| }|
Log |@{ web::Is Displayed |field| }|
Log |@{ web::Find Elements |{link_text: "More"}| In |browser| }|
Log |@{ web::Find Elements |"p"| In |browser| }|
web::Close Browser |browser|
"#,
        &[],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        r#"{"browser": "fake", "session": "s1", "version": "1.0"}
http://fake.test/
Fake page
{"element": "e-h1", "session": "s1"}
text of e-h1
value of name
none
true
[{"element": "e-More", "session": "s1"}, {"element": "e-More-2", "session": "s1"}]
[]
"#
    );
    let sent: Vec<(String, String, Value)> = fake
        .received()
        .into_iter()
        .map(|request| (request.method, request.path, request.body))
        .collect();
    let expected = [
        (
            "POST",
            "/session",
            json!({ "capabilities": { "alwaysMatch": {} } }),
        ),
        (
            "POST",
            "/session/s1/url",
            json!({ "url": "http://fake.test/" }),
        ),
        ("GET", "/session/s1/url", Value::Null),
        ("GET", "/session/s1/title", Value::Null),
        (
            "POST",
            "/session/s1/element",
            json!({ "using": "css selector", "value": "h1" }),
        ),
        ("GET", "/session/s1/element/e-h1/text", Value::Null),
        (
            "POST",
            "/session/s1/element/e-h1/element",
            json!({ "using": "xpath", "value": "//input" }),
        ),
        ("POST", "/session/s1/element/e-%2F%2Finput/clear", json!({})),
        (
            "POST",
            "/session/s1/element/e-%2F%2Finput/value",
            json!({ "text": "Ada" }),
        ),
        ("POST", "/session/s1/element/e-%2F%2Finput/click", json!({})),
        (
            "GET",
            "/session/s1/element/e-%2F%2Finput/attribute/name",
            Value::Null,
        ),
        (
            "GET",
            "/session/s1/element/e-%2F%2Finput/attribute/missing",
            Value::Null,
        ),
        (
            "GET",
            "/session/s1/element/e-%2F%2Finput/displayed",
            Value::Null,
        ),
        (
            "POST",
            "/session/s1/elements",
            json!({ "using": "link text", "value": "More" }),
        ),
        (
            "POST",
            "/session/s1/elements",
            json!({ "using": "css selector", "value": "p" }),
        ),
        ("DELETE", "/session/s1", Value::Null),
    ];
    assert_eq!(sent.len(), expected.len(), "{sent:#?}");
    for ((method, path, body), (want_method, want_path, want_body)) in sent.iter().zip(expected) {
        assert_eq!(
            (method.as_str(), path.as_str(), body),
            (want_method, want_path, &want_body)
        );
    }
}

#[test]
fn driver_failures_bad_values_and_foreign_handles_are_typed_errors() {
    let fake = Fake::start();
    let workspace = Workspace::new();
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    for (driver, source, code, message) in [
        (
            fake.url.as_str(),
            "web::Find Element |\"#missing\"| In |browser|",
            "BW4002",
            "WebDriver: no such element: Unable to locate element: #missing",
        ),
        (
            fake.url.as_str(),
            "web::Find Element |{css: \"a\", xpath: \"b\"}| In |browser|",
            "BW3003",
            "A selector Map names exactly one strategy",
        ),
        (
            fake.url.as_str(),
            "web::Find Element |{name: \"a\"}| In |browser|",
            "BW3003",
            "Unknown selector strategy `name`",
        ),
        (
            fake.url.as_str(),
            "web::Click |browser|",
            "BW3003",
            "The handle is a browser; pass an element from Find Element",
        ),
        (
            fake.url.as_str(),
            "web::Click |{session: \"elsewhere\", element: \"e\"}|",
            "BW3003",
            "`elsewhere` is not a browser this run has open",
        ),
        (
            fake.url.as_str(),
            "web::Close Browser |browser|\nweb::Title Of |browser|",
            "BW3003",
            "is not a browser this run has open: it was closed",
        ),
        (
            fake.url.as_str(),
            "web::Execute Script |\"huge\"| With |[]| In |browser|",
            "BW4002",
            "the whole number 2147483649 is beyond 32 bits",
        ),
        (
            fake.url.as_str(),
            "web::Wait For Element |\"a\"| In |browser| Within |-1|",
            "BW3003",
            "Wait For Element waits 0 to 600000 ms, not -1",
        ),
    ] {
        let output = workspace.run(driver, source, &[]);
        assert_eq!(output.status.code(), Some(1), "{source}");
        let text = stderr(&output);
        assert!(
            text.contains(&format!("[{code}]")) && text.contains(message),
            "{source}: {text}"
        );
    }
    for (options, code, message) in [
        (
            format!("{{driver: \"http://{closed}\"}}"),
            "BW4002",
            "the session could not start: could not reach the driver at http://",
        ),
        (
            format!(
                "{{driver: \"{}\", capabilities: {{browserName: \"refused\"}}}}",
                fake.url
            ),
            "BW4002",
            "the session could not start: session not created: no such browser",
        ),
        (
            "{driver: \"https://grid.test/wd/hub\"}".into(),
            "BW3003",
            "must use http",
        ),
        (
            format!("{{driver: \"{}\", timeout: 5}}", fake.url),
            "BW3003",
            "Unknown Open Browser option `timeout`",
        ),
        (
            format!("{{driver: \"{}\", timeout_ms: 0}}", fake.url),
            "BW3003",
            "`timeout_ms` is an Int from 1 to 600000",
        ),
        (
            "{capabilities: {}}".into(),
            "BW3003",
            "Open Browser needs `driver`",
        ),
        (
            "{driver: \"no-such-driver-anywhere\"}".into(),
            "BW3003",
            "No driver `no-such-driver-anywhere` on the run's PATH",
        ),
    ] {
        workspace.write(
            "open.botwork",
            &format!("Import |\"botwork:webdriver\"| As |web|\nweb::Open Browser |{options}|\n"),
        );
        let output = workspace.botwork(&["--file", "open.botwork"]);
        assert_eq!(output.status.code(), Some(1), "{options}");
        let text = stderr(&output);
        assert!(
            text.contains(&format!("[{code}]")) && text.contains(message),
            "{options}: {text}"
        );
    }
}

#[test]
fn appium_strategies_and_capabilities_reach_the_driver() {
    let fake = Fake::start();
    let workspace = Workspace::new();
    workspace.write(
        "main.botwork",
        &format!(
            r#"Import |"botwork:webdriver"| As |web|
|device| = web::Open Browser |{{driver: "{url}", capabilities: {{platformName: "Android", "appium:automationName": "UiAutomator2", "appium:appPackage": "com.android.settings"}}}}|
Log |device.browser|
web::Find Element |{{accessibility_id: "Search settings"}}| In |device|
web::Find Element |{{id: "android:id/title"}}| In |device|
web::Find Elements |{{class_name: "android.widget.TextView"}}| In |device|
web::Find Element |{{android_uiautomator: "new UiSelector().text(\"Wi-Fi\")"}}| In |device|
web::Find Element |{{ios_predicate: "label == 'OK'"}}| In |device|
web::Find Element |{{ios_class_chain: "**/XCUIElementTypeButton"}}| In |device|
web::Execute Script |"mobile: swipeGesture"| With |[{{direction: "up", percent: 0.5}}]| In |device|
web::Close Browser |device|
"#,
            url = fake.url
        ),
    );
    let output = workspace.botwork(&["--file", "main.botwork"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "Android\n");
    let sent: Vec<Value> = fake
        .received()
        .into_iter()
        .map(|request| request.body)
        .collect();
    assert_eq!(
        sent[0],
        json!({ "capabilities": { "alwaysMatch": { "platformName": "Android", "appium:automationName": "UiAutomator2", "appium:appPackage": "com.android.settings" } } })
    );
    let strategies: Vec<&str> = sent[1..7]
        .iter()
        .map(|body| body["using"].as_str().unwrap())
        .collect();
    assert_eq!(
        strategies,
        [
            "accessibility id",
            "id",
            "class name",
            "-android uiautomator",
            "-ios predicate string",
            "-ios class chain"
        ]
    );
    assert_eq!(sent[4]["value"], "new UiSelector().text(\"Wi-Fi\")");
    assert_eq!(
        sent[7],
        json!({ "script": "mobile: swipeGesture", "args": [{ "direction": "up", "percent": 0.5 }] })
    );
}

#[test]
fn waiting_for_an_element_looks_again_until_it_appears_or_fails_catchably() {
    let fake = Fake::start();
    let workspace = Workspace::new();
    let output = workspace.run(
        &fake.url,
        r##"Log |@{ web::Wait For Element |"#late"| In |browser| Within |5000| }|
Try {
    web::Wait For Element |"#never"| In |browser| Within |250|
} Catch |error| {
    Log |error.code|
    Log |error.details.reason|
}
"##,
        &[],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "{\"element\": \"e-#late\", \"session\": \"s1\"}\nBW9004\nno element matched css selector `#never` within 250 ms\n"
    );
    let looks = fake
        .paths("POST")
        .iter()
        .filter(|path| path.ends_with("/elements"))
        .count();
    // Three looks for #late, and at least three for #never in 250 ms.
    assert!(looks >= 6, "{looks}");
}

#[test]
fn a_hanging_driver_is_bounded_by_the_command_timeout_and_by_a_stop() {
    let fake = Fake::start();
    let workspace = Workspace::new();
    let started = Instant::now();
    workspace.write(
        "main.botwork",
        &format!("Import |\"botwork:webdriver\"| As |web|\n|browser| = web::Open Browser |{{driver: \"{}\", timeout_ms: 300}}|\nweb::Navigate |browser| To |\"http://hang.test/\"|\n", fake.url),
    );
    let output = workspace.botwork_within(&["--file", "main.botwork"], Duration::from_secs(15));
    let text = stderr(&output);
    assert!(
        text.contains("[BW4002]")
            && text.contains("did not answer POST /session/s1/url within 300 ms"),
        "{text}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    // A run deadline drops a command that would wait longer.
    let started = Instant::now();
    let output = workspace.run(
        &fake.url,
        "web::Navigate |browser| To |\"http://hang.test/\"|\n",
        &["--timeout-ms", "500"],
    );
    assert!(stderr(&output).contains("[BW5002]"), "{}", stderr(&output));
    assert!(started.elapsed() < Duration::from_secs(20));
    // Both runs closed their sessions on the way out.
    assert_eq!(fake.paths("DELETE"), ["/session/s1", "/session/s2"]);
}

#[test]
fn a_run_closes_the_sessions_it_left_open_and_only_its_own() {
    let fake = Fake::start();
    let workspace = Workspace::new();
    let output = workspace.run(&fake.url, "Fail |\"left open\"|\n", &[]);
    assert!(stderr(&output).contains("[BW9002]"));
    assert_eq!(fake.paths("DELETE"), ["/session/s1"]);
    // Parallel cases each open their own session and close only it.
    workspace.write(
        "parallel.suite.botwork",
        &format!(
            r#"Suite |"browsers"| {{
    Library {{
        Import |"botwork:webdriver"| As |web|
    }}
    Case |"one"| {{
        |browser| = web::Open Browser |{{driver: "{url}"}}|
        Log |browser.session|
    }}
    Case |"two"| {{
        |browser| = web::Open Browser |{{driver: "{url}"}}|
        Log |browser.session|
        web::Close Browser |browser|
    }}
}}
"#,
            url = fake.url
        ),
    );
    let output = workspace.botwork(&["--suite", "parallel.suite.botwork", "--jobs", "2"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let mut deleted = fake.paths("DELETE");
    deleted.sort();
    assert_eq!(deleted, ["/session/s1", "/session/s2", "/session/s3"]);
}

#[test]
fn screenshots_are_written_and_recorded_as_run_artifacts() {
    let fake = Fake::start();
    let workspace = Workspace::new();
    fs::create_dir(workspace.path().join("shots")).unwrap();
    #[allow(unused_mut)]
    let mut arguments = vec!["--report-json", "report.json"];
    #[cfg(unix)]
    arguments.extend([
        "--listener",
        "sh",
        "--listener-arg",
        "-c",
        "--listener-arg",
        "cat > events.jsonl",
    ]);
    let output = workspace.run(
        &fake.url,
        "Log |@{ web::Take Screenshot Of |browser| To |\"shots/page.png\"| }|\n",
        &arguments,
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let written = workspace.path().join("shots/page.png");
    assert_eq!(fs::read(&written).unwrap(), PNG);
    let path = botwork::core::paths::canonicalize(&written)
        .unwrap()
        .display()
        .to_string();
    assert_eq!(stdout(&output), format!("{path}\n"));
    let report: Value =
        serde_json::from_slice(&fs::read(workspace.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(
        report["runs"][0]["artifacts"],
        json!([{ "kind": "screenshot", "path": path }])
    );
    // A listener sees it as it is taken.
    #[cfg(unix)]
    {
        let events = fs::read_to_string(workspace.path().join("events.jsonl")).unwrap();
        let artifacts: Vec<Value> = events
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| event["event"] == "artifact")
            .collect();
        assert_eq!(artifacts.len(), 1, "{events}");
        assert_eq!(
            (&artifacts[0]["kind"], &artifacts[0]["path"]),
            (&json!("screenshot"), &json!(path))
        );
    }
    // A screenshot that cannot be written is an output error.
    let output = workspace.run(
        &fake.url,
        "web::Take Screenshot Of |browser| To |\"missing/page.png\"|\n",
        &[],
    );
    assert!(
        stderr(&output).contains("[BW4001]") && stderr(&output).contains("Writing the screenshot"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn scripts_receive_and_return_values_and_elements_exactly() {
    let fake = Fake::start();
    let workspace = Workspace::new();
    let output = workspace.run(
        &fake.url,
        r#"|heading| = web::Find Element |"h1"| In |browser|
Log |@{ web::Execute Script |"echo"| With |[1, 2.5, "text", true, [heading], {key: heading}]| In |browser| }|
Log |@{ web::Execute Script |"nothing"| With |[]| In |browser| }|
"#,
        &[],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "[1, 2.5, \"text\", true, [{\"element\": \"e-h1\", \"session\": \"s1\"}], {\"key\": {\"element\": \"e-h1\", \"session\": \"s1\"}}]\nnone\n"
    );
    let script = fake
        .received()
        .into_iter()
        .find(|request| request.path.ends_with("/execute/sync"))
        .unwrap();
    assert_eq!(
        script.body["args"],
        json!([1, 2.5, "text", true, [{ ELEMENT: "e-h1" }], { "key": { ELEMENT: "e-h1" } }])
    );
}

#[test]
fn check_knows_the_modules_statements() {
    let workspace = Workspace::new();
    workspace.write(
        "main.botwork",
        "Import |\"botwork:webdriver\"| As |web|\n|browser| = web::Open Browser |{driver: \"chromedriver\"}|\nweb::Navigate |browser| To |\"https://example.com\"|\nweb::Opne Browser |{}|\n",
    );
    let output = workspace.botwork(&["--check", "--file", "main.botwork"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(
        text.contains("main.botwork:4:1") && text.contains("[BW2002]"),
        "{text}"
    );
    assert!(
        text.contains("The module imported as `web` does not define this statement"),
        "{text}"
    );
    assert!(!text.contains("not checked"), "{text}");
    assert!(text.contains("1 error"), "{text}");
}

/// A fake driver executable: it serves sessions on the port it is given and
/// starts a child, standing in for a browser, in its process group.
#[cfg(unix)]
#[test]
fn a_driver_botwork_starts_ends_with_its_session_and_takes_its_browser_with_it() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = Workspace::new();
    let driver = workspace.path().join("fake-driver");
    fs::write(
        &driver,
        r#"#!/usr/bin/env python3
import http.server, json, subprocess, sys, os
port = int([a for a in sys.argv if a.startswith("--port=")][0].split("=")[1])
browser = subprocess.Popen(["sleep", "60"])
open("browser.pid", "w").write(str(browser.pid))
class Handler(http.server.BaseHTTPRequestHandler):
    def answer(self, value):
        body = json.dumps({"value": value}).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self.answer({"ready": True})
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.answer({"sessionId": "owned", "capabilities": {"browserName": "fake"}})
    def do_DELETE(self):
        self.answer(None)
    def log_message(self, *args):
        pass
http.server.HTTPServer(("127.0.0.1", port), Handler).serve_forever()
"#,
    )
    .unwrap();
    fs::set_permissions(&driver, fs::Permissions::from_mode(0o755)).unwrap();
    let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
    for source in ["web::Close Browser |browser|\n", "Fail |\"left open\"|\n"] {
        let output = workspace.run("./fake-driver", source, &[]);
        assert!(
            stdout(&output).is_empty() || output.status.success(),
            "{}",
            stderr(&output)
        );
        let browser: i32 = fs::read_to_string(workspace.path().join("browser.pid"))
            .unwrap()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(browser) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !alive(browser),
            "the driver's child outlived it after {source:?}"
        );
    }
}

/// chromedriver, from `BOTWORK_WEBDRIVER`, and optionally the browser binary
/// it should start, from `BOTWORK_CHROME`.
fn chrome() -> Option<(String, Option<String>)> {
    match std::env::var("BOTWORK_WEBDRIVER") {
        Ok(driver) if !driver.is_empty() => Some((
            driver,
            std::env::var("BOTWORK_CHROME")
                .ok()
                .filter(|binary| !binary.is_empty()),
        )),
        _ => {
            assert!(
                std::env::var_os("BOTWORK_REQUIRE_WEBDRIVER").is_none(),
                "BOTWORK_REQUIRE_WEBDRIVER is set, but BOTWORK_WEBDRIVER names no chromedriver"
            );
            None
        }
    }
}

/// A page server for the real browser: `/` is a form whose button reveals a
/// result later.
fn serve_page() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            use std::io::{BufRead, BufReader, Write};
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                line.clear();
            }
            let page = r#"<!doctype html><title>Form</title><h1>Sign in</h1>
<input id="name" value="x"><button id="go" onclick="setTimeout(() => { const p = document.createElement('p'); p.id = 'done'; p.textContent = 'Hello, ' + document.getElementById('name').value; document.body.append(p); }, 300)">Go</button>"#;
            let mut stream = stream;
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
        }
    });
    url
}

#[test]
fn a_real_browser_fills_a_form_waits_and_takes_a_screenshot() {
    let Some((driver, binary)) = chrome() else {
        return;
    };
    let page = serve_page();
    let workspace = Workspace::new();
    let mut options =
        json!({ "args": ["--headless=new", "--no-sandbox", "--disable-dev-shm-usage"] });
    if let Some(binary) = binary {
        options["binary"] = json!(binary);
    }
    workspace.write(
        "main.botwork",
        &format!(
            r##"Import |"botwork:webdriver"| As |web|
|browser| = web::Open Browser |{{driver: {driver}, capabilities: {{browserName: "chrome", "goog:chromeOptions": {options}}}, timeout_ms: 120000}}|
web::Navigate |browser| To |"{page}"|
Log |@{{ web::Title Of |browser| }}|
|name| = web::Find Element |"#name"| In |browser|
web::Clear |name|
web::Type |"Ada"| Into |name|
web::Click |@{{ web::Find Element |{{xpath: "//button"}}| In |browser| }}|
|done| = web::Wait For Element |"#done"| In |browser| Within |10000|
Log |@{{ web::Text Of |done| }}|
Log |@{{ web::Execute Script |"return arguments[0].tagName"| With |[done]| In |browser| }}|
web::Take Screenshot Of |browser| To |"page.png"|
web::Close Browser |browser|
"##,
            driver = json!(driver),
        ),
    );
    let output = workspace.botwork(&["--file", "main.botwork"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "Form\nHello, Ada\nP\n");
    let png = fs::read(workspace.path().join("page.png")).unwrap();
    assert_eq!(&png[..8], &PNG[..8], "a PNG");
}
