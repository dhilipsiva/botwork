# Browser automation with WebDriver

`Import |"botwork:webdriver"| As |web|` gives a script statements that drive a
browser through a [W3C WebDriver](https://www.w3.org/TR/webdriver2/) server:
chromedriver for Chrome, geckodriver for Firefox, or a Selenium Grid. The
client is part of Botwork, in Rust ([D11](decisions.md#d11-browser-and-mobile-integrations)).

```
Import |"botwork:webdriver"| As |web|
|chrome| = |{browserName: "chrome", "goog:chromeOptions": {args: ["--headless=new"]}}|
|browser| = web::Open Browser |{driver: "chromedriver", capabilities: chrome}|
Try {
    web::Navigate |browser| To |"https://example.com"|
    Assert |@{ web::Title Of |browser| }| Equals |"Example Domain"|
    web::Click |@{ web::Find Element |"a"| In |browser| }|
    web::Take Screenshot Of |browser| To |"more.png"|
} Finally {
    web::Close Browser |browser|
}
```

## What it needs

- **A WebDriver server and its browser.** For Chrome, chromedriver of the
  same major version as the browser. Botwork starts the driver when `driver`
  names its executable, or connects to one already listening when `driver` is
  its `http` URL.
- **Asynchronous execution**, which the CLI uses. Embedded runs need
  `Engine::run_source_async`, as for other asynchronous statements.

Nothing else: the client speaks WebDriver's JSON over HTTP/1.1 itself.

## Statements

The namespace is the import's alias; the table writes `web`.

| Statement | Returns | Does |
| --- | --- | --- |
| `web::Open Browser \|options\|` | browser | Start a session; see [options](#opening-a-browser) |
| `web::Close Browser \|browser\|` | None | Delete the session, which closes the browser, and end the driver Botwork started for it |
| `web::Navigate \|browser\| To \|url\|` | None | Load a URL; the driver waits for the page to load |
| `web::Current URL Of \|browser\|` | String | The current page's URL |
| `web::Title Of \|browser\|` | String | The current page's title |
| `web::Find Element \|selector\| In \|scope\|` | element | The first match in a browser, or within an element |
| `web::Find Elements \|selector\| In \|scope\|` | Array | Every match, in document order; empty when none matches |
| `web::Wait For Element \|selector\| In \|scope\| Within \|milliseconds\|` | element | Look every 100 ms until a match appears; see [waits](#waits) |
| `web::Click \|element\|` | None | Click an element |
| `web::Type \|text\| Into \|element\|` | None | Type into an element |
| `web::Clear \|element\|` | None | Clear an editable element |
| `web::Text Of \|element\|` | String | The element's rendered text |
| `web::Attribute \|name\| Of \|element\|` | String or None | An attribute, or None when the element has none |
| `web::Is Displayed \|element\|` | Bool | Whether the element is displayed |
| `web::Execute Script \|script\| With \|arguments\| In \|browser\|` | any | Run JavaScript in the page as a function body with `arguments`, and return its result |
| `web::Take Screenshot Of \|browser\| To \|path\|` | String | Save a PNG of the viewport and return its absolute path; see [screenshots](#screenshots) |

`botwork --check` and the language server know these statements, so a
misspelled one is an error before the run.

## Opening a browser

`Open Browser` takes a Map:

| Option | Kind | Default | Meaning |
| --- | --- | --- | --- |
| `driver` | String | required | An `http` URL of a running WebDriver server, such as `http://127.0.0.1:9515` or a Grid's `http://grid:4444/wd/hub`; or the driver executable to start: a path, relative to the run's directory, or a name on the run's `PATH` |
| `capabilities` | Map | `{}` | The [W3C capabilities](https://www.w3.org/TR/webdriver2/#capabilities) the session must match, such as `browserName` and `goog:chromeOptions` |
| `timeout_ms` | Int, 1–600000 | 30000 | Each command's bound, and the started driver's bound to answer |
| `failure_artifacts` | String | | A directory, from the run's directory, where the session saves a screenshot and the page source when it fails, and a driver Botwork starts writes its log; see [failure artifacts](browser-matrix.md#failure-artifacts) |

Botwork starts a driver on a free loopback port, in the run's directory and
environment, passing `--port=N` as chromedriver and geckodriver accept. It
returns once the driver answers, or fails when the driver exits first or does
not answer within `timeout_ms`. Starting the browser is the session's first
command, and the slowest: on a busy machine, such as a shared CI runner, it
has taken over 30 seconds, so give `timeout_ms` room there.

## Handles

A browser is a Map with the session's ID, browser name, and version:
`{session: "3d34…", browser: "chrome", version: "153.0.8010.12"}`. An element
is a Map with its session and the driver's reference: `{session: "3d34…",
element: "f.17…"}`. Pass them back as they came; Log shows them.

A browser belongs to the run that opened it. A handle from another run, or one
already closed, fails with BW3003 and does not reach the driver. Runs in
parallel each open their own browsers.

## Selectors

A String selector is CSS. A Map names one strategy:

| Map | Matches |
| --- | --- |
| `{css: "form > input"}` | A CSS selector, as a String selector does |
| `{xpath: "//button[text()='Go']"}` | An XPath expression |
| `{link_text: "More"}` | Links whose whole text is this |
| `{partial_link_text: "Mor"}` | Links whose text contains this |
| `{tag_name: "h1"}` | Elements by tag |

With [Appium](appium.md#selectors), a Map can also name `accessibility_id`,
`id`, `class_name`, `android_uiautomator`, `ios_predicate`, or
`ios_class_chain`. The [Playwright statements](playwright.md) take the same
Maps, except Appium's; see [browser and device conventions](browser-conventions.md).

`Find Element` and `Find Elements` with an element as the scope search inside
it.

## Waits

`Wait For Element` looks for a match, then every 100 ms, until one appears or
`milliseconds` (0–600000) pass. When none appears it fails with BW9004, a
condition not met, which `Try`/`Catch` can catch. Other statements do not wait
or retry on their own. To wait for something else, wrap it in
[`Eventually`](polling.md):

```
Eventually |{timeout_ms: 5000}| {
    Assert |@{ web::Text Of |status| }| Equals |"Saved"|
}
```

## Assertions

Botwork's own assertions apply to what the statements return:

```
Assert |@{ web::Current URL Of |browser| }| Equals |"https://example.com/done"|
Assert |@{ web::Is Displayed |banner| }|
```

A failure is BW9001 with the compared values, as in any script.

## Screenshots

`Take Screenshot Of` writes the viewport as a PNG. A relative path is from the
run's directory, and the file's directory must exist. The screenshot becomes an
artifact of the run: it appears in the run's record with `kind` `screenshot`,
in the [JSON report](json-report.md) and the [HTML report](html-report.md),
and in a listener's `artifact` event. A file that cannot be written fails with
BW4001.

## Sessions and cleanup

`Close Browser` deletes the session, so the driver closes the browser, then
ends the driver Botwork started. A run that ends with browsers open, by a
failure, a stop, or a forgotten `Close Browser`, closes them as it ends: it
deletes each session, waiting up to 2 seconds for each driver, and ends the
drivers it started. On Linux and macOS a started driver runs in a process group
of its own, and ending it ends the group, so a browser it launched cannot
outlive it. On Windows Botwork ends the driver process; the browser closes when
its session is deleted.

## Errors

| Code | When |
| --- | --- |
| BW3003 | A handle, selector, option, or script argument that does not fit, or a browser this run does not have open |
| BW4002 | The driver failed the command (`WebDriver: no such element: …`, with its W3C error and message), could not start, could not be reached, or did not answer within `timeout_ms` |
| BW4001 | A screenshot could not be written |
| BW9004 | `Wait For Element` found no match in time |
| BW5001, BW5002 | The run was cancelled or reached its deadline; the command in flight is dropped |
| BW5003 | The run is not asynchronous |

Errors carry the statement's call site, as other statements' do.

## Values

Script arguments and results cross as JSON. An element handle crosses as the
page's element, and an element in a result comes back as a handle. A whole
number becomes an Int when it fits 32 bits, and a Float when a 32-bit Float
holds it exactly; any other whole number fails, rather than rounding. A
fraction becomes the nearest 32-bit Float, and `null` and `undefined` become
None. Results nest at most 64 levels.

## Limits

- Driver URLs use plain `http`, as WebDriver servers do; put a TLS-terminating
  Grid behind a plain endpoint the run can reach.
- Each command opens its own connection and is bounded by `timeout_ms`; a stop
  drops it at once.
- A driver's answer holds at most 32 MiB, enough for a full-screen
  screenshot's base64.
- WebDriver BiDi, frames and windows, cookies, alerts, file uploads, and
  low-level actions are not statements yet; `Execute Script` reaches the page.

## Trust

The driver and browser run with the user's authority, like [other trusted
extensions](trust.md). A page the browser loads runs in the browser's own
sandbox.

## Tests

`tests/webdriver.rs` runs every statement against a fake W3C server, with no
browser, on Linux, macOS, and Windows: the requests sent, errors and bad values,
waits, a hanging driver bounded by `timeout_ms` and by a run deadline, sessions
closed at the run's end and kept apart between parallel cases, screenshots as
artifacts, script values, `--check`, and, on Unix, a started driver that takes
its child processes with it. When `BOTWORK_WEBDRIVER` names chromedriver (and
`BOTWORK_CHROME` the browser, when it is not where chromedriver looks), a
further test fills a form in headless Chrome, waits for the result, and takes a
screenshot. CI's Linux jobs set them and `BOTWORK_REQUIRE_WEBDRIVER`, so the
real browser test cannot be skipped there.
