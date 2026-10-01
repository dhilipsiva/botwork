# Browser and device conventions

[WebDriver](webdriver.md), [Playwright](playwright.md), and
[Appium](appium.md) share these rules, so a script reads the same whichever
drives the browser or device, and none hides a failure. `tests/browser_conventions.rs`
checks them through both modules.

## Selectors

A String selector is CSS in both modules; Playwright also reads its own
engines in a String, such as `text=Sign in` or `role=button`. A Map names one
strategy, with the same keys in both:

| Map | WebDriver sends | Playwright uses |
| --- | --- | --- |
| `{css: "main h1"}` | `css selector` | `css=main h1` |
| `{xpath: "//h1"}` | `xpath` | `xpath=//h1` |
| `{link_text: "More"}` | `link text` | `a:text-is("More")` |
| `{partial_link_text: "Mor"}` | `partial link text` | `a:has-text("Mor")` |
| `{tag_name: "h1"}` | `tag name` | `css=h1` |

Appium's strategies (`accessibility_id`, `id`, `class_name`,
`android_uiautomator`, `ios_predicate`, `ios_class_chain`) are WebDriver's
alone; Playwright refuses them with BW3003. A statement acts on the first
match; `Find Elements` and `Count` see every match.

## Timeouts

- **Each command is bounded.** `timeout_ms`, an Int from 1 to 600000 with
  30000 by default, bounds every WebDriver command of a session (an option of
  `Open Browser`), and every Playwright action and expectation in a browser (an
  option of `Launch Browser`).
- **Runs bound everything.** A run's deadline or stop drops the command in
  flight at once, as BW5002 or BW5001, and the run then closes its sessions.
- **Waits name their own bound.** `Wait For Element … Within |milliseconds|`
  and `Wait For … Within |milliseconds|` take 0 to 600000.

## Waits

A wait looks again until its element appears, and otherwise fails with BW9004,
a condition not met, with the selector and the bound in its reason. `Try`/`Catch`
catches it, as any failure.

## Retries

No statement retries on its own, so a failure is never hidden behind a later
success:

- **WebDriver** sends each command once. It does not set the session's implicit
  wait, so a missing element fails at once, unless the session's capabilities
  set `timeouts` themselves.
- **Playwright** acts on an element once it is actionable, waiting for it up to
  `timeout_ms`, as Playwright does; it does not repeat an action that failed.
  Its `Expect` statements look again until they hold or `timeout_ms` passes,
  then fail with BW9001 and the last value they saw.
- **An assertion** on a returned value, such as
  `Assert |@{ web::Text Of |heading| }| Equals |"Saved"|`, reads the value
  once and fails at once.

To retry, say so: wrap the statements in [`Eventually` or `Retry`](polling.md).
Their failure, BW9004 or BW9005, counts the attempts and keeps the recent
attempts' own failures, each assertion included.

## Session ownership

- A browser, session, context, or page belongs to the run that opened it.
  Another run's handle, a closed one, or one from the other module fails with
  BW3003 and never reaches the driver.
- Runs in parallel open their own sessions; nothing is shared between them.
- However a run ends, it closes what it left open: it deletes its WebDriver
  sessions and ends the drivers it started, and it closes its Playwright
  browsers and ends its Node host. On Linux and macOS each started process
  runs in a process group of its own, which ends with it.

## Errors

| Code | Means, in every module |
| --- | --- |
| BW3003 | A handle, selector, option, or value that does not fit |
| BW4002 | The driver, Playwright, or the device failed the command, timed out, or could not be reached or started |
| BW9001 | An assertion failed: Botwork's `Assert`, or a Playwright `Expect` |
| BW9004 | A wait found nothing in time, or an `Eventually` ran out |
| BW4001 | A screenshot or trace could not be written |
| BW5001, BW5002 | The run was cancelled or reached its deadline |
