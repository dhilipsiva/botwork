//! The conventions WebDriver and Playwright share (docs/browser-conventions.md):
//! selectors, timeouts, waits, no implicit retries, and session ownership,
//! checked through both modules against their fakes.
#[path = "support/webdriver_fake.rs"]
mod webdriver_fake;

use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use webdriver_fake::Fake;

/// Node, for the Playwright side; skipped without it unless BOTWORK_REQUIRE_NODE.
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

/// A run directory with the fake `playwright` package, and a fake WebDriver
/// server; each script imports both modules and opens a browser in each.
struct Both {
    directory: tempfile::TempDir,
    fake: Fake,
}

impl Both {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
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
        Self {
            directory,
            fake: Fake::start(),
        }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    /// Run `source` with `web::` and `pw::` imported, a WebDriver `browser`
    /// with an element `heading`, and a Playwright `page`.
    fn run(&self, source: &str) -> Output {
        fs::write(
            self.path().join("main.botwork"),
            format!(
                r#"Import |"botwork:webdriver"| As |web|
Import |"botwork:playwright"| As |pw|
|browser| = web::Open Browser |{{driver: "{url}"}}|
|heading| = web::Find Element |"h1"| In |browser|
|playwright| = pw::Launch Browser |{{timeout_ms: 1000}}|
|page| = pw::New Page In |@{{ pw::New Context In |playwright| With |{{}}| }}|
pw::Go To |"https://example.test/"| In |page|
{source}"#,
                url = self.fake.url
            ),
        )
        .unwrap();
        let (stdout, stderr) = (self.path().join(".stdout"), self.path().join(".stderr"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(["--file", "main.botwork"])
            .current_dir(self.path())
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
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

    /// The Playwright fake's calls named `name`.
    fn playwright_calls(&self, name: &str) -> Vec<Value> {
        fs::read_to_string(self.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|call| call[0] == name)
            .collect()
    }

    /// The WebDriver fake's requests whose path ends with `suffix`.
    fn webdriver_requests(&self, method: &str, suffix: &str) -> Vec<Value> {
        self.fake
            .received()
            .into_iter()
            .filter(|request| request.method == method && request.path.ends_with(suffix))
            .map(|request| request.body)
            .collect()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn one_selector_map_reaches_each_modules_engine() {
    if !node() {
        return;
    }
    let both = Both::new();
    let selectors = [
        "\"h1\"",
        "{css: \"main h1\"}",
        "{xpath: \"//h1\"}",
        "{link_text: \"More\"}",
        "{partial_link_text: \"Mor\"}",
        "{tag_name: \"h1\"}",
    ];
    let source: String = selectors
        .iter()
        .map(|selector| {
            format!(
                "web::Find Element |{selector}| In |browser|\npw::Click |{selector}| In |page|\n"
            )
        })
        .collect();
    let output = both.run(&source);
    assert!(output.status.success(), "{}", stderr(&output));
    let webdriver: Vec<Value> = both.webdriver_requests("POST", "/element")[1..].to_vec();
    assert_eq!(
        webdriver,
        [
            json!({ "using": "css selector", "value": "h1" }),
            json!({ "using": "css selector", "value": "main h1" }),
            json!({ "using": "xpath", "value": "//h1" }),
            json!({ "using": "link text", "value": "More" }),
            json!({ "using": "partial link text", "value": "Mor" }),
            json!({ "using": "tag name", "value": "h1" }),
        ]
    );
    let playwright: Vec<Value> = both
        .playwright_calls("click")
        .into_iter()
        .map(|call| call[1].clone())
        .collect();
    assert_eq!(
        playwright,
        [
            json!("h1"),
            json!("css=main h1"),
            json!("xpath=//h1"),
            json!("a:text-is(\"More\")"),
            json!("a:has-text(\"Mor\")"),
            json!("css=h1"),
        ]
    );
    // Appium's strategies are WebDriver's alone, and say so.
    let output = both.run("pw::Click |{accessibility_id: \"Search\"}| In |page|\n");
    let text = stderr(&output);
    assert!(
        text.contains("[BW3003]") && text.contains("`accessibility_id` is an Appium strategy"),
        "{text}"
    );
}

#[test]
fn statements_never_retry_on_their_own_and_explicit_retries_keep_each_failure() {
    if !node() {
        return;
    }
    let both = Both::new();
    let output = both.run(
        r##"Try {
    Assert |@{ web::Text Of |heading| }| Equals |"other"|
} Catch |error| {
    Log |error.code|
}
Try {
    Assert |@{ pw::Text Of |"h1"| In |page| }| Equals |"other"|
} Catch |error| {
    Log |error.code|
}
"##,
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "BW9001\nBW9001\n");
    // Each failed assertion read its text once: nothing retried it.
    assert_eq!(both.webdriver_requests("GET", "/text").len(), 1);
    assert_eq!(both.playwright_calls("innerText").len(), 1);

    // An explicit retry runs again, and its failure keeps every attempt's.
    let both = Both::new();
    let output = both.run(
        r##"Try {
    Eventually |{timeout_ms: 1000, interval_ms: 100}| {
        Assert |@{ web::Text Of |heading| }| Equals |"other"|
    }
} Catch |error| {
    Log |[error.code, error.details.attempts]|
    Log |@{ Parse JSON |error.details.history| }|
}
"##,
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    // Count from what the run reports, as load changes how many fit.
    let attempts: usize = lines[0]
        .trim_start_matches("[\"BW9004\", \"")
        .trim_end_matches("\"]")
        .parse()
        .unwrap_or_else(|_| panic!("{}", lines[0]));
    assert!(attempts >= 2, "Eventually retried: {}", lines[0]);
    // Each attempt read once; the deadline may end the last before it reads.
    let reads = both.webdriver_requests("GET", "/text").len();
    assert!(
        reads == attempts || reads + 1 == attempts,
        "{reads} reads, {attempts} attempts"
    );
    // Every attempt's own failure is kept. The deadline may end the last
    // attempt, as BW5002, rather than its assertion failing.
    let kept = attempts.min(16);
    let failures = lines[1].matches("BW9001").count();
    let cut_short = lines[1].matches("BW5002").count();
    assert_eq!(
        lines[1].matches("\"outcome\"").count(),
        kept,
        "{}",
        lines[1]
    );
    assert!(
        cut_short <= 1 && failures + cut_short == kept,
        "{}",
        lines[1]
    );
}

#[test]
fn waits_and_timeouts_have_one_shape() {
    if !node() {
        return;
    }
    let both = Both::new();
    let output = both.run(
        r##"Try {
    web::Wait For Element |"#never"| In |browser| Within |200|
} Catch |error| {
    Log |[error.code, error.details.reason]|
}
Try {
    pw::Wait For |"#never"| In |page| Within |200|
} Catch |error| {
    Log |[error.code, error.details.reason]|
}
"##,
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "[\"BW9004\", \"no element matched css selector `#never` within 200 ms\"]\n[\"BW9004\", \"no element matched `#never` within 200 ms\"]\n"
    );
    // The same bounds for each command's timeout and for waits.
    for (source, message) in [
        (
            format!(
                "web::Open Browser |{{driver: \"{}\", timeout_ms: 0}}|",
                both.fake.url
            ),
            "`timeout_ms` is an Int from 1 to 600000, not 0",
        ),
        (
            "pw::Launch Browser |{timeout_ms: 0}|".to_owned(),
            "`timeout_ms` is an Int from 1 to 600000, not 0",
        ),
        (
            format!(
                "web::Open Browser |{{driver: \"{}\", timeout_ms: 600001}}|",
                both.fake.url
            ),
            "`timeout_ms` is an Int from 1 to 600000, not 600001",
        ),
        (
            "pw::Launch Browser |{timeout_ms: 600001}|".to_owned(),
            "`timeout_ms` is an Int from 1 to 600000, not 600001",
        ),
        (
            "web::Wait For Element |\"h1\"| In |browser| Within |600001|".to_owned(),
            "600001",
        ),
        (
            "pw::Wait For |\"h1\"| In |page| Within |600001|".to_owned(),
            "600001",
        ),
    ] {
        let output = both.run(&format!("{source}\n"));
        let text = stderr(&output);
        assert!(
            text.contains("[BW3003]") && text.contains(message),
            "{source}: {text}"
        );
    }
}

#[test]
fn sessions_belong_to_their_module_and_run_and_close_with_it() {
    if !node() {
        return;
    }
    let both = Both::new();
    for (source, message) in [
        (
            "pw::Go To |\"x\"| In |browser|",
            "not a page this run has open",
        ),
        ("web::Title Of |page|", "has no `session`"),
    ] {
        let output = both.run(&format!("{source}\n"));
        let text = stderr(&output);
        assert!(
            text.contains("[BW3003]") && text.contains(message),
            "{source}: {text}"
        );
    }
    // A run that fails with both open closes both.
    let both = Both::new();
    let output = both.run("Fail |\"left open\"|\n");
    assert!(stderr(&output).contains("[BW9002]"));
    assert_eq!(both.fake.paths("DELETE"), ["/session/s1"]);
    assert_eq!(both.playwright_calls("closeBrowser").len(), 1);
}
