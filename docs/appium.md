# Mobile automation with Appium

[Appium](https://appium.io) serves the W3C WebDriver protocol for Android and
iOS apps, so the [WebDriver statements](webdriver.md) drive devices too: with
Appium's capabilities, Appium's selector strategies, and its `mobile:`
commands ([D11](decisions.md#d11-browser-and-mobile-integrations)).

```
Import |"botwork:webdriver"| As |web|
|settings| = |{platformName: "Android", "appium:automationName": "UiAutomator2", "appium:appPackage": "com.android.settings", "appium:appActivity": ".Settings"}|
|device| = web::Open Browser |{driver: "appium", capabilities: settings, timeout_ms: 300000}|
Try {
    |network| = web::Wait For Element |{android_uiautomator: "new UiSelector().textContains(\"Network\")"}| In |device| Within |60000|
    web::Click |network|
    web::Take Screenshot Of |device| To |"network.png"|
} Finally {
    web::Close Browser |device|
}
```

## What it needs

- **Appium and a driver for the platform:** `npm install appium`, then
  `npx appium driver install uiautomator2` for Android (or `xcuitest` for iOS).
  Botwork starts Appium when `driver` names its executable, as `appium` on the
  run's `PATH` or a path such as `node_modules/.bin/appium`, passing it a free
  port; or it connects to an Appium server already listening, by its `http`
  URL, such as `http://127.0.0.1:4723`.
- **A device:** an emulator, a simulator, or a real device that the driver
  reaches, with the platform's tools (the Android SDK, or Xcode) that the
  driver needs.

Botwork is tested with Appium 3.8, which `tests/appium/package.json` pins, and
its UiAutomator2 driver 8.7 on an Android 14 emulator.

## Device and session configuration

`Open Browser`'s `capabilities` are the session's
[Appium capabilities](https://appium.io/docs/en/latest/guides/caps/):
`platformName`, and Appium's own under the `appium:` prefix, such as
`appium:automationName`, `appium:app`, `appium:appPackage`,
`appium:appActivity`, `appium:udid`, or `appium:newCommandTimeout`. Quote the
prefixed keys: `"appium:automationName": "UiAutomator2"`.

A device session's handle names its platform where a browser's names the
browser: `{session: "…", browser: "Android", version: ""}`.

Starting a session can take a minute or more the first time, while the driver
installs its helpers on the device; give `timeout_ms` room, such as 300000.

## Selectors

Besides the [W3C strategies](webdriver.md#selectors), a selector Map can name
one of Appium's:

| Map | Matches |
| --- | --- |
| `{accessibility_id: "Search settings"}` | An element's accessibility label (content description on Android) |
| `{id: "com.android.settings:id/title"}` | An Android resource ID, or an iOS element name |
| `{class_name: "android.widget.TextView"}` | Elements of a class |
| `{android_uiautomator: "new UiSelector().text(\"Wi-Fi\")"}` | An Android UiAutomator expression |
| `{ios_predicate: "label == 'OK'"}` | An iOS predicate string |
| `{ios_class_chain: "**/XCUIElementTypeButton"}` | An iOS class chain |

`{xpath: …}` works on Appium too, over the app's view hierarchy. Browsers'
drivers refuse Appium's strategies with `invalid argument`.

## Element operations, waits, and screenshots

Every [WebDriver statement](webdriver.md#statements) applies: `Find Element`,
`Click`, `Type`, `Clear`, `Text Of`, `Attribute`, `Is Displayed`,
`Wait For Element`, `Take Screenshot Of` (the device's screen, recorded as an
artifact), and `Close Browser`, which ends the session and the Appium server
Botwork started. A run that ends with a session open closes it, as for
[browsers](webdriver.md#sessions-and-cleanup).

Gestures and device actions are Appium's
[`mobile:` commands](https://appium.io/docs/en/latest/guides/execute-methods/),
run through `Execute Script`:

```
web::Execute Script |"mobile: swipeGesture"| With |[{left: 100, top: 500, width: 600, height: 800, direction: "up", percent: 0.75}]| In |device|
web::Execute Script |"mobile: pressKey"| With |[{keycode: 4}]| In |device|
```

## Tests

`tests/webdriver.rs` checks against a fake server that Appium's capabilities,
each of its strategies, and a `mobile:` command reach the driver unchanged, and
that a device session's handle names its platform. `tests/appium.rs` opens the
Settings app on an Android emulator through the Appium Botwork starts, waits
for an element by UiAutomator, finds others by class name and XPath, reads
them, and takes a screenshot; CI's Appium job runs it on an Android 14
emulator, with `BOTWORK_REQUIRE_APPIUM` so that it cannot be skipped. No iOS
device is tested.
