# Browser automation with Playwright

`Import |"botwork:playwright"| As |pw|` gives a script statements that drive
Chromium, Firefox, or WebKit with the official [Playwright](https://playwright.dev)
library, through a Node.js process the run owns
([D11](decisions.md#d11-browser-and-mobile-integrations)).

```
Import |"botwork:playwright"| As |pw|
|browser| = pw::Launch Browser |{browser: "chromium"}|
Try {
    |context| = pw::New Context In |browser| With |{viewport: {width: 1280, height: 720}}|
    |page| = pw::New Page In |context|
    pw::Go To |"https://example.com"| In |page|
    pw::Expect Title Of |page| To Be |"Example Domain"|
    pw::Click |"text=More information"| In |page|
    pw::Screenshot Of |page| To |"more.png"|
} Finally {
    pw::Close Browser |browser|
}
```

## What it needs

- **Node.js** on the run's `PATH`.
- **The `playwright` package**, installed where the run is, so that Node finds
  it from the run's directory: `npm install playwright` there, then
  `npx playwright install chromium` (or `firefox`, `webkit`) for its browsers.
  Botwork does not bundle Playwright; the project chooses its version.
- **Asynchronous execution**, which the CLI uses.

Botwork is tested with Playwright 1.63, which `tests/playwright/package.json`
pins.

## How it runs

A run's first Playwright statement starts one Node process for the run, in its
directory and with its environment. That process loads the project's
`playwright` and keeps the run's browsers, contexts, and pages; each statement
is one command to it. When the run ends, however it ends, Botwork closes the
process's input, so it closes every browser it launched within 5 seconds, then
ends the process. On Linux and macOS it runs in a process group of its own,
which Botwork ends with it, so no browser outlives the run.

## Statements

The namespace is the import's alias; the table writes `pw`.

| Statement | Returns | Does |
| --- | --- | --- |
| `pw::Launch Browser \|options\|` | browser | Launch a browser; see [options](#launching) |
| `pw::Close Browser \|browser\|` | None | Close a browser, its contexts, and their pages |
| `pw::New Context In \|browser\| With \|options\|` | context | Open an isolated context, with Playwright's [context options](https://playwright.dev/docs/api/class-browser#browser-new-context), such as `viewport`, `locale`, or `recordVideo`; `trace: true` also starts tracing |
| `pw::Close Context \|context\|` | None | Close a context and its pages; its videos become artifacts |
| `pw::New Page In \|context\|` | page | Open a page |
| `pw::Close Page \|page\|` | None | Close a page |
| `pw::Go To \|url\| In \|page\|` | None | Load a URL and wait for the page to load |
| `pw::URL Of \|page\|`, `pw::Title Of \|page\|` | String | The page's URL, or title |
| `pw::Click \|selector\| In \|page\|` | None | Click the first match, once it is actionable |
| `pw::Fill \|selector\| With \|text\| In \|page\|` | None | Fill an input |
| `pw::Press \|key\| On \|selector\| In \|page\|` | None | Press a key, such as `Enter` or `Control+A` |
| `pw::Check \|selector\| In \|page\|`, `pw::Uncheck …` | None | Check or uncheck a checkbox |
| `pw::Hover \|selector\| In \|page\|` | None | Hover over an element |
| `pw::Select \|values\| In \|selector\| Of \|page\|` | Array | Select options by value or label (a String or an Array), and return the values selected |
| `pw::Text Of \|selector\| In \|page\|` | String | The first match's rendered text |
| `pw::Attribute \|name\| Of \|selector\| In \|page\|` | String or None | An attribute, or None when the element has none |
| `pw::Count \|selector\| In \|page\|` | Int | How many elements match now |
| `pw::Is Visible \|selector\| In \|page\|` | Bool | Whether the first match is visible now |
| `pw::Wait For \|selector\| In \|page\| Within \|milliseconds\|` | None | Wait until a match is visible; see [waits](#waits-and-expectations) |
| `pw::Expect \|selector\| In \|page\| To Have Text \|text\|` | None | Assert the first match's text |
| `pw::Expect \|selector\| In \|page\| To Be Visible` | None | Assert the first match is visible |
| `pw::Expect Title Of \|page\| To Be \|title\|` | None | Assert the page's title |
| `pw::Evaluate \|script\| With \|arguments\| In \|page\|` | any | Run JavaScript in the page as a function body with `arguments`, and return its result |
| `pw::Screenshot Of \|page\| To \|path\|`, `pw::Full Page Screenshot Of …` | String | Save a PNG of the viewport, or of the whole page, and return its absolute path |
| `pw::Start Tracing \|context\|` | None | Start a [trace](https://playwright.dev/docs/trace-viewer) of a context |
| `pw::Stop Tracing \|context\| To \|path\|` | String | Save the trace and return its absolute path |

Selectors are [Playwright selectors](https://playwright.dev/docs/selectors):
CSS by default, and `text=`, `xpath=`, `role=`, and the others it supports.
A selector Map names one of the strategies the WebDriver statements take, such
as `{xpath: "//h1"}` or `{link_text: "More"}`; see
[browser and device conventions](browser-conventions.md#selectors). A
statement acts on the first match. `botwork --check` and the language server
know these statements, so a misspelled one is an error before the run.

## Launching

`Launch Browser` takes a Map:

| Option | Kind | Default | Meaning |
| --- | --- | --- | --- |
| `browser` | String | `chromium` | `chromium`, `firefox`, or `webkit` |
| `headless` | Bool | true | Run without a window |
| `args` | Array | | Extra browser arguments |
| `executable_path` | String | | A browser binary to launch instead of Playwright's |
| `timeout_ms` | Int, 1–600000 | 30000 | How long each action and expectation in this browser waits |

## Handles

A browser, context, or page is a Map that names its kind and an ID, such as
`{playwright: "page", id: "8324d2ad-p4"}`; a browser also carries its `browser`
and `version`. Every ID starts with the run's own host's prefix, so a handle
from another run, of the wrong kind, or already closed fails with BW3003.

## Waits and expectations

Playwright waits for an element to be actionable before acting on it, up to
the browser's `timeout_ms`; if it is not, the action fails with BW4002.

`Wait For` waits up to `milliseconds` (0–600000) for a match to be visible,
and otherwise fails with BW9004, a condition not met, which `Try`/`Catch` can
catch.

The `Expect` statements are web-first assertions: they look again, every
100 ms, until the expectation holds or the browser's `timeout_ms` passes. A
failure is BW9001, an assertion, whose message has the expected and the actual
value:

```text
Assertion failed: `#status` should have the text "Saved", but it has "Saving"
```

For other checks, Botwork's own `Assert` compares what the statements return,
and [`Eventually`](polling.md) retries any of them.

## Artifacts

Screenshots, traces, and the videos of a context opened with
`recordVideo: {dir: "videos"}` become artifacts of the run, with `kind`
`screenshot`, `trace`, or `video`: in its record, the [JSON
report](json-report.md), the [HTML report](html-report.md), and a listener's
`artifact` events. A relative path is from the run's directory. A video is
recorded when its context closes with `Close Context`.

## Errors

| Code | When |
| --- | --- |
| BW3003 | A handle of the wrong kind, closed, or from another run, or an option or value that does not fit |
| BW4002 | Playwright failed or timed out (`Playwright: locator.click: Timeout 30000ms exceeded`, with its call log), is not installed, or Node is missing |
| BW9001 | An `Expect` did not hold in time |
| BW9004 | `Wait For` found no visible match in time |
| BW4001 | A screenshot or trace is missing after Playwright saved it |
| BW5001, BW5002 | The run was cancelled or reached its deadline; the statement in flight is dropped |
| BW5003 | The run is not asynchronous |

## Values

`Evaluate`'s arguments and result cross as JSON, with the rules of
[WebDriver scripts](webdriver.md#values): whole numbers that fit 32 bits are
Ints, other numbers 32-bit Floats unless a whole number would round, and
`null` and `undefined` are None.

## Trust

The Node process, Playwright, and the browsers run with the user's authority,
like [other trusted extensions](trust.md). `Evaluate` runs its script in the
page, in the browser's sandbox.

## Tests

`tests/playwright.rs` runs every statement through the real host against a fake
`playwright` package, wherever Node is (CI requires it on Linux, macOS, and
Windows): the commands sent, expectations that pass after retrying and fail
with their values, waits, handles of other kinds and runs, values, artifacts,
cleanup when a run fails with a browser open, a missing package or Node, and
`--check`. When `BOTWORK_PLAYWRIGHT` names a directory with Playwright and its
Chromium installed, a further test fills a form in Chromium, waits for the
result, takes a screenshot, and saves a trace. CI's Playwright job installs the
pinned version and sets it, with `BOTWORK_REQUIRE_PLAYWRIGHT`, so the test
cannot be skipped there. Firefox and WebKit take the same commands; CI tests
Chromium.
