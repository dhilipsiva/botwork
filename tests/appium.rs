//! Appium (decision D11): the WebDriver statements with Appium's capabilities
//! and strategies, on an Android emulator. `BOTWORK_APPIUM` names the Appium
//! executable, which Botwork starts; CI's Appium job sets it, and
//! `BOTWORK_REQUIRE_APPIUM`, with an emulator running. The same statements'
//! behaviour against a fake server is in tests/webdriver.rs.
use std::{
    fs,
    process::{Command, Stdio},
};

/// The Appium executable, from BOTWORK_APPIUM.
fn appium() -> Option<String> {
    match std::env::var("BOTWORK_APPIUM") {
        Ok(appium) if !appium.is_empty() => Some(appium),
        _ => {
            assert!(
                std::env::var_os("BOTWORK_REQUIRE_APPIUM").is_none(),
                "BOTWORK_REQUIRE_APPIUM is set, but BOTWORK_APPIUM names no Appium"
            );
            None
        }
    }
}

#[test]
fn an_android_emulator_opens_settings_and_finds_reads_and_captures_it() {
    let Some(appium) = appium() else {
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    fs::write(
        workspace.path().join("main.botwork"),
        format!(
            r#"Import |"botwork:webdriver"| As |web|
|capabilities| = |{{platformName: "Android", "appium:automationName": "UiAutomator2", "appium:appPackage": "com.android.settings", "appium:appActivity": ".Settings", "appium:newCommandTimeout": 300}}|
|device| = web::Open Browser |{{driver: {appium}, capabilities: capabilities, timeout_ms: 300000}}|
Log |device.browser|
|network| = web::Wait For Element |{{android_uiautomator: "new UiSelector().textContains(\"Network\")"}}| In |device| Within |120000|
Log |@{{ web::Is Displayed |network| }}|
Log |@{{ web::Text Of |network| }}|
|texts| = web::Find Elements |{{class_name: "android.widget.TextView"}}| In |device|
Log |@{{ Length Of |texts| }} > 3|
|same| = web::Find Element |{{xpath: "//android.widget.TextView[contains(@text, 'Network')]"}}| In |device|
Log |@{{ web::Text Of |same| }} == @{{ web::Text Of |network| }}|
web::Take Screenshot Of |device| To |"device.png"|
web::Close Browser |device|
"#,
            appium = serde_json::json!(appium),
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(["--file", "main.botwork"])
        .current_dir(workspace.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}{stdout}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "Android");
    assert_eq!(lines[1], "true");
    assert!(lines[2].contains("Network"), "{stdout}");
    assert_eq!(&lines[3..], ["true", "true"]);
    let png = fs::read(workspace.path().join("device.png")).unwrap();
    assert_eq!(&png[..4], b"\x89PNG");
}

#[test]
fn the_tested_appium_is_the_documented_one() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |path: &str| fs::read_to_string(root.join(path)).unwrap();
    let package: serde_json::Value =
        serde_json::from_str(&read("tests/appium/package.json")).unwrap();
    let pinned = package["dependencies"]["appium"].as_str().unwrap();
    let lock: serde_json::Value =
        serde_json::from_str(&read("tests/appium/package-lock.json")).unwrap();
    assert_eq!(lock["packages"]["node_modules/appium"]["version"], pinned);
    let workflow = read(".github/workflows/ci.yml");
    let driver = workflow
        .split("appium-uiautomator2-driver@")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("CI installs a pinned UiAutomator2 driver");
    let minor = |version: &str| version.rsplit_once('.').unwrap().0.to_owned();
    assert!(read("docs/compatibility.md").contains(&format!(
        "| Appium, for [mobile automation](appium.md) | {} | {pinned}, with UiAutomator2 {driver} on an Android 14 emulator, in CI |",
        minor(pinned)
    )));
    assert!(read("docs/appium.md").contains(&format!(
        "Botwork is tested with Appium {}, which `tests/appium/package.json` pins, and\nits UiAutomator2 driver {} on an Android 14 emulator.",
        minor(pinned),
        minor(driver)
    )));
    assert!(
        workflow.contains("api-level: 34"),
        "Android 14 is API level 34"
    );
}
