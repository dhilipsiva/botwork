# Browser and device matrix

Each integration is verified where it is advertised, against every supported
failure path ([D11](decisions.md#d11-browser-and-mobile-integrations)). A
failure path ends with a diagnostic that says what failed, failure artifacts
that show it, and nothing left open.

## What is verified where

| Integration | Verified against | Where |
| --- | --- | --- |
| [WebDriver](webdriver.md) | Chrome and its chromedriver, as hosted Ubuntu runners ship them | CI's four Linux x86_64 Rust jobs (GNU and musl, debug and release) |
| [Playwright](playwright.md) | Playwright 1.63.0 with its Chromium | CI's Playwright job, on Linux x86_64 |
| [Appium](appium.md) | Appium 3.8.0 with UiAutomator2 8.7.0, on an Android 14 (API 34) x86_64 emulator | CI's Appium job, on Linux x86_64 |
| All three | Fakes: a W3C WebDriver server and a `playwright` package that record what they are sent | Every CI job: Linux, macOS, and Windows, debug and release |

The fakes check what Botwork sends and how it handles every answer, on every
platform Botwork supports. The real browsers and the emulator check that the
same scripts work against the software users run. macOS and Windows run the
fakes only: CI's macOS and Windows runners have no browser driver or Android
emulator that the tests set up.

## Failure paths

| Path | WebDriver and Appium | Playwright | Artifacts | Verified |
| --- | --- | --- | --- | --- |
| Session creation fails | BW4002, `the session could not start:` and the driver's reason | BW4002, with Playwright's launch log | The driver's log, when Botwork started the driver | Fakes; Chrome; Chromium; Android |
| An element never appears | BW9004 from `Wait For Element`, or BW4002 `no such element` | BW9004 from `Wait For`, or BW4002 when an action times out | A screenshot and the page source, named in the error | Fakes; Chrome; Chromium; Android |
| An assertion fails | BW9001 from `Assert` | BW9001 from `Assert`, or from an `Expect` statement | Captured as the failure happens (`Expect`), or as the run ends with the browser open (`Assert`) | Fakes; Chrome; Chromium; Android |
| The run is cancelled or reaches its deadline | BW5001 or BW5002; the command in flight is dropped | The same | Captured as the run ends | Fakes; Chrome; Chromium; Android |
| Runs in parallel | Each case's session, captures, and cleanup are its own | The same | Each run records only its own | Fakes; Chrome; Chromium |

Android is verified with one session at a time, since one emulator serves one
session. In every path, the run's end closes the sessions and browsers it left
open and ends the drivers, Node host, and Appium server it started; on Linux,
the tests check that no process the run started is left running.

## Failure artifacts

`Open Browser` and `Launch Browser` take `failure_artifacts`, a directory
relative to the run's directory, which Botwork creates:

```
|browser| = web::Open Browser |{driver: "chromedriver", failure_artifacts: "artifacts"}|
|playwright| = pw::Launch Browser |{failure_artifacts: "artifacts"}|
```

With it, a session or page saves what it shows when something fails:

- **A statement fails in it**, other than with a wrong handle or value: a
  screenshot (`failure-screenshot`) and the page source (`failure-source`; for
  an app, its view hierarchy), and the error names their paths after
  `; failure artifacts:`.
- **The run ends with it open**, after an assertion fails, a stop, or a
  forgotten close: the same capture, before Botwork closes it. Leave the
  browser to the run's end, rather than closing it in `Finally`, to capture
  what an `Assert` saw.
- **Botwork started the driver**: the driver writes its log there. The log is
  recorded (`driver-log`) when the session cannot start or a capture is taken,
  and deleted when the session closes without a failure.

Each artifact is in the run's record, the [JSON](json-report.md) and
[HTML](html-report.md) reports, and a listener's `artifact` events. Files are
named by their session or page and a count, so parallel runs never share one.
A session that closes without a failure leaves the directory empty.

## Tests

`tests/browser_failures.rs` runs each path through both modules against the
fakes on every platform, and against Chrome (`BOTWORK_WEBDRIVER`), Chromium
(`BOTWORK_PLAYWRIGHT`), and an Android emulator (`BOTWORK_APPIUM`) where they
are configured; the CI jobs above set them, with `BOTWORK_REQUIRE_*`, so the
tests cannot be skipped there.
