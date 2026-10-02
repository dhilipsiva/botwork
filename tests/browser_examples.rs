//! The browser and device examples (examples/browser): every one passes
//! `--check`, its README states its prerequisites, and each runs end to end
//! where they are configured: the WebDriver example with BOTWORK_WEBDRIVER
//! (CI's Linux jobs), the Playwright example with BOTWORK_PLAYWRIGHT (its
//! Playwright job), and the Appium example with BOTWORK_APPIUM (its emulator
//! job). A run must log what the example promises, write its screenshot, and
//! leave no process behind.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/browser")
}

/// Each runnable example, and the option that runs it.
const RUNNABLE: [(&str, &str); 4] = [
    ("webdriver.botwork", "--file"),
    ("playwright.botwork", "--file"),
    ("appium.botwork", "--file"),
    ("acceptance.suite.botwork", "--suite"),
];

fn copy_examples(to: &Path) {
    for entry in fs::read_dir(examples()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            fs::copy(&path, to.join(path.file_name().unwrap())).unwrap();
        }
    }
}

fn botwork(directory: &Path, arguments: &[&str], limit: Duration) -> Output {
    let (stdout, stderr) = (directory.join(".stdout"), directory.join(".stderr"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(arguments)
        .current_dir(directory)
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
        std::thread::sleep(Duration::from_millis(20));
    };
    Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}

#[test]
fn every_example_checks_clean_and_reads_only_its_documented_inputs() {
    let readme = fs::read_to_string(examples().join("README.md")).unwrap();
    let inputs: serde_json::Value =
        serde_json::from_slice(&fs::read(examples().join("inputs.json")).unwrap()).unwrap();
    let mut scripts: Vec<String> = fs::read_dir(examples())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".botwork"))
        .collect();
    scripts.sort();
    assert_eq!(
        scripts,
        [
            "acceptance.suite.botwork",
            "appium.botwork",
            "page.botwork",
            "playwright.botwork",
            "sign-in.botwork",
            "webdriver.botwork"
        ]
    );
    for (name, option) in RUNNABLE {
        assert!(
            readme.contains(&format!("| `{name}` |"))
                && readme.contains(&format!("botwork {option} {name} --vars-file inputs.json")),
            "README.md must state {name}'s prerequisites and how to run it"
        );
        let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(["--check", option, name])
            .current_dir(examples())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{name}: {text}");
        assert!(text.contains(": 0 errors,"), "{name}: {text}");
        // Its only warnings are input variables, each documented and defaulted.
        for line in text.lines().filter(|line| line.contains("warning[")) {
            let variable = line
                .split('`')
                .nth(1)
                .unwrap_or_else(|| panic!("{name}: {line}"));
            assert!(
                line.contains("warning[undefined-variable]"),
                "{name}: {line}"
            );
            assert!(
                inputs.get(variable).is_some(),
                "{name}: `{variable}` has no default in inputs.json"
            );
            assert!(
                readme.contains(&format!("| `{variable}` |")),
                "{name}: README.md does not describe `{variable}`"
            );
        }
    }
}

/// The processes still running whose working directory is `directory`, or
/// whose command line holds `marker`: a driver, host, server, or browser the
/// example left behind.
#[cfg(target_os = "linux")]
fn left_behind(directory: &Path, marker: &str) -> Vec<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let process = entry.path();
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let command = fs::read(process.join("cmdline")).unwrap_or_default();
        let command = String::from_utf8_lossy(&command).replace('\0', " ");
        let name = fs::read_to_string(process.join("comm")).unwrap_or_default();
        // adb's server outlives every client by design, and is not the run's.
        if name.trim() == "adb" {
            continue;
        }
        let inside = fs::read_link(process.join("cwd")).is_ok_and(|cwd| cwd.starts_with(directory));
        if inside || command.contains(marker) {
            found.push(format!("{} {}", name.trim(), command.trim()));
        }
    }
    found
}

/// Run an example in `directory` with `overrides`, and check its output, its
/// screenshot, and that it left nothing running.
fn run_example(
    directory: &Path,
    name: &str,
    overrides: &[(&str, serde_json::Value)],
    logged: &str,
    screenshot: &str,
) {
    copy_examples(directory);
    let marker = format!("--botwork-example={}", std::process::id());
    let mut arguments: Vec<String> = ["--file", name, "--vars-file", "inputs.json"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut overrides = overrides.to_vec();
    overrides.push(("browser_args", serde_json::json!([marker])));
    for (input, value) in &overrides {
        arguments.push("--var".into());
        arguments.push(format!("{input}={value}"));
    }
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let output = botwork(directory, &arguments, Duration::from_secs(600));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{name}: {}{stdout}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout, logged, "{name}");
    let png = fs::read(directory.join(screenshot)).unwrap();
    assert_eq!(&png[..4], b"\x89PNG", "{name}");
    #[cfg(target_os = "linux")]
    {
        let directory = botwork::core::paths::canonicalize(directory).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut remaining = left_behind(&directory, &marker);
        while !remaining.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            remaining = left_behind(&directory, &marker);
        }
        assert!(
            remaining.is_empty(),
            "{name} left processes running: {remaining:#?}"
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = marker;
}

/// The value of `variable`, failing when `required` is set without it.
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

#[test]
fn the_webdriver_example_signs_in_with_chrome() {
    let Some(driver) = configured("BOTWORK_WEBDRIVER", "BOTWORK_REQUIRE_WEBDRIVER") else {
        return;
    };
    let chrome = std::env::var("BOTWORK_CHROME").unwrap_or_default();
    let directory = tempfile::tempdir().unwrap();
    run_example(
        directory.path(),
        "webdriver.botwork",
        &[("driver", driver.into()), ("chrome", chrome.into())],
        "Hello, Ada\n",
        "signed-in.png",
    );
}

#[test]
fn the_playwright_example_signs_in_with_chromium() {
    let Some(installed) = configured("BOTWORK_PLAYWRIGHT", "BOTWORK_REQUIRE_PLAYWRIGHT") else {
        return;
    };
    // Inside the directory that has Playwright, so the run finds it.
    let directory = tempfile::tempdir_in(installed).unwrap();
    run_example(
        directory.path(),
        "playwright.botwork",
        &[],
        "Hello, Ada\n",
        "signed-in.png",
    );
}

#[test]
fn the_appium_example_opens_settings_on_android() {
    let Some(appium) = configured("BOTWORK_APPIUM", "BOTWORK_REQUIRE_APPIUM") else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    run_example(
        directory.path(),
        "appium.botwork",
        &[("appium", appium.into())],
        "Settings shows network settings\n",
        "settings.png",
    );
}

/// The acceptance suite (W5) with Chrome: every row signs in and passes.
#[test]
fn the_acceptance_suite_signs_in_each_person() {
    let Some(driver) = configured("BOTWORK_WEBDRIVER", "BOTWORK_REQUIRE_WEBDRIVER") else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    copy_examples(directory.path());
    let output = acceptance(directory.path(), &driver, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut greetings: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    greetings.sort();
    assert_eq!(greetings, ["Hello, Ada", "Hello, Grace"]);
    // Rows that pass keep no failure artifacts.
    let left: Vec<_> = fs::read_dir(directory.path().join("artifacts"))
        .unwrap()
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// The acceptance suite's failure diagnosis (W5): a wrong expectation fails
/// each row with BW9001, the run's end captures each row's browser, the
/// failed-case record names the rows, and rerunning them after the repair
/// passes.
#[test]
fn a_failing_acceptance_row_is_captured_recorded_and_rerun() {
    let Some(driver) = configured("BOTWORK_WEBDRIVER", "BOTWORK_REQUIRE_WEBDRIVER") else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    copy_examples(directory.path());
    let output = acceptance(
        directory.path(),
        &driver,
        &[
            "--var",
            "expected_greeting=\"Hi\"",
            "--failures",
            "failed.json",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("[BW9001]") && text.contains("Expected \"Hi, Ada\""),
        "{text}"
    );
    let failed: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("failed.json")).unwrap()).unwrap();
    assert_eq!(
        failed["failed"],
        serde_json::json!(["sign-in/greets/ada", "sign-in/greets/grace"])
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("report.json")).unwrap()).unwrap();
    for run in report["runs"].as_array().unwrap() {
        let kinds: Vec<&str> = run["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|artifact| artifact["kind"].as_str().unwrap())
            .collect();
        for kind in ["failure-screenshot", "failure-source", "driver-log"] {
            assert!(
                kinds.contains(&kind),
                "{}: {kinds:?}",
                run["identity"]["id"]
            );
        }
        for artifact in run["artifacts"].as_array().unwrap() {
            assert!(
                Path::new(artifact["path"].as_str().unwrap()).is_file(),
                "{artifact}"
            );
        }
    }
    // The repair is the right expectation; the rerun selects only the failed rows.
    let output = acceptance(
        directory.path(),
        &driver,
        &["--rerun-failed", "failed.json"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("[cases] 2 selected: 2 succeeded"));
    #[cfg(target_os = "linux")]
    {
        let directory = botwork::core::paths::canonicalize(directory.path()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut remaining = left_behind(&directory, "--botwork-example-never");
        while !remaining.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
            remaining = left_behind(&directory, "--botwork-example-never");
        }
        assert!(remaining.is_empty(), "left running: {remaining:#?}");
    }
}

/// Run the acceptance suite in `directory` with Chrome through `driver`.
fn acceptance(directory: &Path, driver: &str, more: &[&str]) -> Output {
    let chrome = std::env::var("BOTWORK_CHROME").unwrap_or_default();
    let driver = format!("driver={}", serde_json::json!(driver));
    let chrome = format!("chrome={}", serde_json::json!(chrome));
    let mut arguments = vec![
        "--suite",
        "acceptance.suite.botwork",
        "--vars-file",
        "inputs.json",
        "--var",
        &driver,
        "--var",
        &chrome,
        "--jobs",
        "2",
    ];
    arguments.extend(more);
    botwork(directory, &arguments, Duration::from_secs(600))
}
