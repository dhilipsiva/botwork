# Browser and device examples

Each example signs in on a sample page, or opens a device's Settings, checks
what it sees with assertions, saves a screenshot, and closes its browser or
session in `Finally`, so the run cleans up however it ends. Run them from this
directory; `inputs.json` holds their inputs, which `--var` overrides.

| Example | Drives | Prerequisites |
| --- | --- | --- |
| `webdriver.botwork` | Chrome, through chromedriver ([WebDriver](../../docs/webdriver.md)) | Chrome, and the chromedriver of its major version, from [Chrome for Testing](https://googlechromelabs.github.io/chrome-for-testing/), on `PATH` |
| `playwright.botwork` | Chromium, with [Playwright](../../docs/playwright.md) | Node.js, then `npm install playwright` and `npx playwright install chromium` here |
| `appium.botwork` | Android's Settings app, through [Appium](../../docs/appium.md) | Node.js, then `npm install appium`, `npx appium driver install uiautomator2` here, and an Android device or emulator that `adb devices` lists |
| `acceptance.suite.botwork` | Chrome, once per person in its dataset, through `sign-in.botwork`'s statements: the [W5 reference workflow](../../docs/reference-workflows.md#w5-browser-acceptance-against-a-local-page) | As for `webdriver.botwork` |

```sh
botwork --file webdriver.botwork --vars-file inputs.json
botwork --file playwright.botwork --vars-file inputs.json
botwork --file appium.botwork --vars-file inputs.json --var 'appium="node_modules/.bin/appium"'
botwork --suite acceptance.suite.botwork --vars-file inputs.json --jobs 2
```

`webdriver.botwork` and `playwright.botwork` log `Hello, Ada` and write
`signed-in.png`; `appium.botwork` logs that Settings shows network settings and
writes `settings.png`; `acceptance.suite.botwork` logs each person's greeting,
and both its rows pass. A missing prerequisite fails the run with a message that
names it.

## Inputs

| Input | Default | Meaning |
| --- | --- | --- |
| `driver` | `"chromedriver"` | The chromedriver to start: a name on `PATH`, a path, or the `http` URL of one already listening |
| `chrome` | `""` | A Chrome binary for chromedriver to start, when it should not find its own |
| `browser_args` | `[]` | More arguments for the browser |
| `appium` | `"appium"` | The Appium to start: a name on `PATH`, a path, or the `http` URL of a running server |
| `expected_greeting` | `"Hello"` | The greeting the acceptance suite expects; any other makes each row fail |

`page.botwork` defines the sample page, as a `data:` URL, so the examples need no
web server.

## Diagnosing a failure

The acceptance suite opens each browser with `failure_artifacts: "artifacts"`
and leaves it to the run's end, so a failing row keeps what it showed. Plant a
failure with a wrong expectation, and record the failed rows:

```sh
botwork --suite acceptance.suite.botwork --vars-file inputs.json --var 'expected_greeting="Hi"' --failures failed.json --report-json report.json
```

Each row fails with BW9001: the expected and actual greetings, the assertion's
line, and the case and row. `artifacts/` holds, for each row, a screenshot, the
page source, and chromedriver's log, which `report.json` lists under each run's
`artifacts`; `failed.json` names both rows. Repair the expectation, and rerun
only those rows:

```sh
botwork --suite acceptance.suite.botwork --vars-file inputs.json --rerun-failed failed.json
```

See [browser and device matrix](../../docs/browser-matrix.md#failure-artifacts).

## Teardown

Each example closes what it opened in `Finally`. When a run stops before that,
by an interrupt or a deadline, Botwork still closes the run's browsers and
sessions and ends the drivers, Node host, and Appium server it started; see
[browser and device conventions](../../docs/browser-conventions.md#session-ownership).
The screenshots are the only files the examples leave.

`tests/browser_examples.rs` checks every example with `botwork --check`, and runs
each one where its prerequisites are configured: the WebDriver example in CI's
Linux jobs, the Playwright example in its Playwright job, and the Appium example
on its Android emulator. Each run must log what the example promises, write its
screenshot, and leave no process behind.
