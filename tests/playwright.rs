//! Playwright (decision D11): `Import |"botwork:playwright"|` statements through
//! the run's Node host, against a fake `playwright` package wherever Node is, and
//! against the real one with Chromium where `BOTWORK_PLAYWRIGHT` names a
//! directory that has it installed (CI's Playwright job).
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

/// Node, from PATH; tests are skipped without it unless BOTWORK_REQUIRE_NODE.
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

struct Workspace(tempfile::TempDir);

impl Workspace {
    /// A run directory with the fake `playwright` package installed.
    fn fake() -> Self {
        let workspace = Self(tempfile::tempdir().unwrap());
        let package = workspace.path().join("node_modules/playwright");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            r##"{"name": "playwright", "main": "index.js"}"##,
        )
        .unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/playwright_fake.js"),
            package.join("index.js"),
        )
        .unwrap();
        workspace
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.path().join(name), text).unwrap();
    }

    /// Run the CLI, ending it after a minute so a hang fails the test.
    fn botwork(&self, arguments: &[&str]) -> Output {
        let (stdout, stderr) = (self.path().join(".stdout"), self.path().join(".stderr"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(arguments)
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

    /// Run `source` after importing the module as `pw` and opening a page as
    /// `page` in a context `context` of a browser `browser`.
    fn run(&self, source: &str, more: &[&str]) -> Output {
        self.write(
            "main.botwork",
            &format!(
                "Import |\"botwork:playwright\"| As |pw|\n|browser| = pw::Launch Browser |{{timeout_ms: 1000}}|\n|context| = pw::New Context In |browser| With |{{}}|\n|page| = pw::New Page In |context|\n{source}"
            ),
        );
        let mut arguments = vec!["--file", "main.botwork"];
        arguments.extend(more);
        self.botwork(&arguments)
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
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
fn pages_open_act_read_and_close_through_the_runs_host() {
    if !node() {
        return;
    }
    let workspace = Workspace::fake();
    let output = workspace.run(
        r##"Log |browser.browser|
Log |browser.version|
pw::Go To |"https://example.test/"| In |page|
Log |@{ pw::URL Of |page| }|
Log |@{ pw::Title Of |page| }|
pw::Fill |"#name"| With |"Ada"| In |page|
pw::Press |"Enter"| On |"#name"| In |page|
pw::Click |"text=Go"| In |page|
pw::Check |"#agree"| In |page|
pw::Uncheck |"#agree"| In |page|
pw::Hover |"nav"| In |page|
Log |@{ pw::Select |["a", "b"]| In |"#pick"| Of |page| }|
Log |@{ pw::Text Of |"h1"| In |page| }|
Log |@{ pw::Attribute |"href"| Of |"a"| In |page| }|
Log |@{ pw::Attribute |"missing"| Of |"a"| In |page| }|
Log |@{ pw::Count |"li"| In |page| }|
Log |@{ pw::Is Visible |"#never"| In |page| }|
pw::Close Page |page|
pw::Close Context |context|
pw::Close Browser |browser|
"##,
        &[],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "chromium\n1.0-fake\nhttps://example.test/\nFake page\n[\"a\", \"b\"]\ntext of h1\nhref of a\nnone\n2\nfalse\n"
    );
    assert_eq!(
        workspace.calls(),
        [
            json!(["launch", "chromium", { "headless": true }]),
            json!(["newContext", {}]),
            json!(["setDefaultTimeout", 1000]),
            json!(["goto", "https://example.test/"]),
            json!(["fill", "#name", "Ada"]),
            json!(["press", "#name", "Enter"]),
            json!(["click", "text=Go"]),
            json!(["setChecked", "#agree", true]),
            json!(["setChecked", "#agree", false]),
            json!(["hover", "nav"]),
            json!(["selectOption", "#pick", ["a", "b"]]),
            json!(["closePage"]),
            json!(["closeContext"]),
            json!(["closeBrowser"]),
        ]
    );
}

#[test]
fn expectations_retry_until_they_hold_and_fail_as_assertions() {
    if !node() {
        return;
    }
    let workspace = Workspace::fake();
    let output = workspace.run(
        r##"pw::Go To |"https://example.test/"| In |page|
pw::Expect |"#status"| In |page| To Have Text |"Saved"|
pw::Expect |"#late"| In |page| To Be Visible
pw::Expect Title Of |page| To Be |"Fake page"|
pw::Wait For |"#late"| In |page| Within |5000|
Try {
    pw::Expect |"#status"| In |page| To Have Text |"Lost"|
} Catch |error| {
    Log |[error.code, error.message]|
}
Try {
    pw::Expect |"#never"| In |page| To Have Text |"x"|
} Catch |error| {
    Log |error.message|
}
Try {
    pw::Expect Title Of |page| To Be |"Other"|
} Catch |error| {
    Log |error.message|
}
Try {
    pw::Wait For |"#never"| In |page| Within |200|
} Catch |error| {
    Log |[error.code, error.message]|
}
"##,
        &[],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        r##"["BW9001", "Assertion failed: `#status` should have the text \"Lost\", but it has \"Saved\""]
Assertion failed: `#never` should have the text "x", but no element matched
Assertion failed: the page should have the title "Other", but it has "Fake page"
["BW9004", "Condition not met: no element matched `#never` within 200 ms"]
"##
    );
}

#[test]
fn handles_belong_to_their_run_and_their_kind() {
    if !node() {
        return;
    }
    let workspace = Workspace::fake();
    for (source, code, message) in [
        (
            "pw::Go To |\"x\"| In |browser|",
            "BW3003",
            "not a page this run has open",
        ),
        (
            "pw::Go To |\"x\"| In |{playwright: \"page\", id: \"elsewhere-p1\"}|",
            "BW3003",
            "not a page this run has open",
        ),
        (
            "pw::Close Page |page|\npw::Title Of |page|",
            "BW3003",
            "it was closed, or another run opened it",
        ),
        (
            "pw::Go To |\"http://fail.test/\"| In |page|",
            "BW4002",
            "Playwright: page.goto: net::ERR_NAME_NOT_RESOLVED",
        ),
        (
            "pw::Click |\"#never\"| In |page|\npw::Text Of |\"#never\"| In |page|",
            "BW4002",
            "Playwright: waiting for #never",
        ),
        (
            "pw::Evaluate |\"huge\"| With |[]| In |page|",
            "BW4002",
            "the whole number 2147483649 is beyond 32 bits",
        ),
        (
            "pw::Wait For |\"a\"| In |page| Within |-1|",
            "BW3003",
            "Wait For's time is an Int from 0 to 600000",
        ),
    ] {
        let output = workspace.run(source, &[]);
        assert_eq!(output.status.code(), Some(1), "{source}");
        let text = stderr(&output);
        assert!(
            text.contains(&format!("[{code}]")) && text.contains(message),
            "{source}: {text}"
        );
    }
    for (options, message) in [
        ("{browser: \"opera\"}", "Unknown browser `opera`"),
        ("{headed: true}", "Unknown Launch Browser option `headed`"),
        (
            "{timeout_ms: -5}",
            "`timeout_ms` is an Int from 0 to 600000",
        ),
    ] {
        workspace.write(
            "open.botwork",
            &format!("Import |\"botwork:playwright\"| As |pw|\npw::Launch Browser |{options}|\n"),
        );
        let output = workspace.botwork(&["--file", "open.botwork"]);
        let text = stderr(&output);
        assert!(
            text.contains("[BW3003]") && text.contains(message),
            "{options}: {text}"
        );
    }
}

#[test]
fn scripts_receive_and_return_values_exactly() {
    if !node() {
        return;
    }
    let workspace = Workspace::fake();
    let output = workspace.run(
        "Log |@{ pw::Evaluate |\"echo\"| With |[1, 2.5, \"text\", true, [1], {key: \"value\"}]| In |page| }|\n",
        &[],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "[1, 2.5, \"text\", true, [1], {\"key\": \"value\"}]\n"
    );
    let evaluated = workspace
        .calls()
        .into_iter()
        .find(|call| call[0] == "evaluate")
        .unwrap();
    assert_eq!(
        evaluated,
        json!(["evaluate", "echo", [1, 2.5, "text", true, [1], { "key": "value" }]])
    );
}

#[test]
fn screenshots_traces_and_videos_are_run_artifacts() {
    if !node() {
        return;
    }
    let workspace = Workspace::fake();
    let output = workspace.run(
        r##"|recorded| = pw::New Context In |browser| With |{recordVideo: {dir: "videos"}}|
|filmed| = pw::New Page In |recorded|
Log |@{ pw::Screenshot Of |page| To |"view.png"| }|
Log |@{ pw::Full Page Screenshot Of |page| To |"full.png"| }|
pw::Start Tracing |context|
Log |@{ pw::Stop Tracing |context| To |"trace.zip"| }|
pw::Close Context |recorded|
"##,
        &["--report-json", "report.json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let absolute = |name: &str| {
        botwork::core::paths::canonicalize(workspace.path().join(name))
            .unwrap()
            .display()
            .to_string()
    };
    assert_eq!(
        stdout(&output),
        format!(
            "{}\n{}\n{}\n",
            absolute("view.png"),
            absolute("full.png"),
            absolute("trace.zip")
        )
    );
    let report: Value =
        serde_json::from_slice(&fs::read(workspace.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(
        report["runs"][0]["artifacts"],
        json!([
            { "kind": "screenshot", "path": absolute("view.png") },
            { "kind": "screenshot", "path": absolute("full.png") },
            { "kind": "trace", "path": absolute("trace.zip") },
            { "kind": "video", "path": absolute("videos/page.webm") },
        ])
    );
    let screenshots: Vec<Value> = workspace
        .calls()
        .into_iter()
        .filter(|call| call[0] == "screenshot")
        .collect();
    assert_eq!(
        screenshots,
        [json!(["screenshot", false]), json!(["screenshot", true])]
    );
}

#[test]
fn a_run_closes_its_browsers_and_ends_its_host_however_it_ends() {
    if !node() {
        return;
    }
    let workspace = Workspace::fake();
    workspace.write(
        "main.botwork",
        "Import |\"botwork:playwright\"| As |pw|\n|browser| = pw::Launch Browser |{args: [\"--child\"]}|\nFail |\"left open\"|\n",
    );
    let output = workspace.botwork(&["--file", "main.botwork"]);
    assert!(stderr(&output).contains("[BW9002]"), "{}", stderr(&output));
    // The host closed the browser it launched when its input ended.
    assert_eq!(workspace.calls().last().unwrap(), &json!(["closeBrowser"]));
    // On Unix, ending the host's process group ends what the browser started.
    #[cfg(unix)]
    {
        let child: i32 = fs::read_to_string(workspace.path().join("browser.pid"))
            .unwrap()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while unsafe { libc::kill(child, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(
            unsafe { libc::kill(child, 0) },
            0,
            "the browser's child outlived the run"
        );
    }
}

#[test]
fn a_missing_package_or_node_says_what_to_install() {
    if !node() {
        return;
    }
    let workspace = Workspace(tempfile::tempdir().unwrap());
    workspace.write(
        "main.botwork",
        "Import |\"botwork:playwright\"| As |pw|\npw::Launch Browser |{}|\n",
    );
    let output = workspace.botwork(&["--file", "main.botwork"]);
    let text = stderr(&output);
    assert!(
        text.contains("[BW4002]") && text.contains("the `playwright` package is not installed where the run is; install it with `npm install playwright`"),
        "{text}"
    );
    // Without Node on the run's PATH.
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let path = std::env::var_os("PATH").unwrap();
    let without: Vec<_> = std::env::split_paths(&path)
        .filter(|directory| !directory.join(name).is_file())
        .collect();
    let mut command = Command::new(env!("CARGO_BIN_EXE_botwork"));
    command
        .args(["--file", "main.botwork"])
        .current_dir(workspace.path())
        .env("PATH", std::env::join_paths(without).unwrap());
    let text = String::from_utf8_lossy(&command.output().unwrap().stderr).into_owned();
    assert!(
        text.contains("[BW4002]") && text.contains("Playwright needs Node.js on the run's PATH"),
        "{text}"
    );
}

#[test]
fn check_knows_the_modules_statements() {
    let workspace = Workspace(tempfile::tempdir().unwrap());
    workspace.write(
        "main.botwork",
        "Import |\"botwork:playwright\"| As |pw|\n|browser| = pw::Launch Browser |{}|\npw::Lunch Browser |{}|\n",
    );
    let output = workspace.botwork(&["--check", "--file", "main.botwork"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(
        text.contains("main.botwork:3:1") && text.contains("[BW2002]"),
        "{text}"
    );
    assert!(!text.contains("not checked"), "{text}");
    assert!(text.contains("1 error"), "{text}");
}

/// The directory with the real `playwright` installed, from BOTWORK_PLAYWRIGHT.
fn playwright() -> Option<String> {
    match std::env::var("BOTWORK_PLAYWRIGHT") {
        Ok(directory) if !directory.is_empty() => Some(directory),
        _ => {
            assert!(
                std::env::var_os("BOTWORK_REQUIRE_PLAYWRIGHT").is_none(),
                "BOTWORK_REQUIRE_PLAYWRIGHT is set, but BOTWORK_PLAYWRIGHT names no directory with Playwright"
            );
            None
        }
    }
}

#[test]
fn a_real_browser_fills_a_form_and_records_a_trace() {
    let Some(directory) = playwright() else {
        return;
    };
    let workspace = Workspace(tempfile::tempdir_in(&directory).unwrap());
    workspace.write(
        "form.html",
        r##"<!doctype html><title>Form</title><input id="name"><button onclick="setTimeout(() => { document.querySelector('p').textContent = 'Hello, ' + document.querySelector('#name').value }, 300)">Go</button><p></p>"##,
    );
    let page = url::Url::from_file_path(workspace.path().join("form.html")).unwrap();
    workspace.write(
        "main.botwork",
        &format!(
            r##"Import |"botwork:playwright"| As |pw|
|browser| = pw::Launch Browser |{{}}|
|context| = pw::New Context In |browser| With |{{trace: true}}|
|page| = pw::New Page In |context|
pw::Go To |"{page}"| In |page|
pw::Expect Title Of |page| To Be |"Form"|
pw::Fill |"#name"| With |"Ada"| In |page|
pw::Click |"text=Go"| In |page|
pw::Expect |"p"| In |page| To Have Text |"Hello, Ada"|
Log |@{{ pw::Evaluate |"return document.querySelector('p').tagName"| With |[]| In |page| }}|
pw::Screenshot Of |page| To |"page.png"|
pw::Stop Tracing |context| To |"trace.zip"|
pw::Close Browser |browser|
"##
        ),
    );
    let output = workspace.botwork(&["--file", "main.botwork"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "P\n");
    let png = fs::read(workspace.path().join("page.png")).unwrap();
    assert_eq!(&png[..4], b"\x89PNG");
    assert!(
        fs::metadata(workspace.path().join("trace.zip"))
            .unwrap()
            .len()
            > 0
    );
}

#[test]
fn the_tested_playwright_is_the_documented_one() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let package: Value =
        serde_json::from_slice(&fs::read(root.join("tests/playwright/package.json")).unwrap())
            .unwrap();
    let pinned = package["dependencies"]["playwright"].as_str().unwrap();
    let lock: Value =
        serde_json::from_slice(&fs::read(root.join("tests/playwright/package-lock.json")).unwrap())
            .unwrap();
    assert_eq!(
        lock["packages"]["node_modules/playwright"]["version"],
        pinned
    );
    let minor = pinned.rsplit_once('.').unwrap().0;
    let compatibility = fs::read_to_string(root.join("docs/compatibility.md")).unwrap();
    assert!(
        compatibility.contains(&format!("| Playwright, for [Playwright statements](playwright.md) | {minor} | {pinned} with Chromium in CI |")),
        "docs/compatibility.md names another Playwright than {pinned}"
    );
    let page = fs::read_to_string(root.join("docs/playwright.md")).unwrap();
    assert!(page.contains(&format!("Botwork is tested with Playwright {minor}")));
}
