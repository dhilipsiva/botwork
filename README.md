# botwork

botwork is a single-binary, generic and open-source automation framework written in Rust for acceptance testing, acceptance test driven development (ATDD), and robotic process automation (RPA). The syntax uses sentence names with parameters. Custom names, identifiers, and text support Unicode; control keywords, operators, and numeric syntax remain fixed. See [multilingual authoring](docs/language.md#multilingual-authoring) and the [Tamil/accented-text example](examples/16-multilingual.botwork). Easily extendible with Rust, Python & JavaScript. An efficient, fast alternative to Robot Framework.

# Why botwork?

I have been using RobotFramework for a couple of years now. While it is a super-awesome framework, there are a couple of things that I am not very fond of:

1. It basically requires Python (and virtualenv) to run. This means, it needs more space (when building container images, for instance) and consumes a lot of memory (Python is the love of my life, but it is slow and resource-heavy).
2. The syntax could have been even more simpler. For instance, two (or more) space token seperator, `${}`, `@{}`, etc. variable usage confuses people who are new to the framework.
3. It is mostly extendible only with Python.

I wanted:

1. An efficient, fast, single-binary tool.
1. An even more simpler syntax than RobotFramework.
1. Extendible with Rust, Python (via PyO3), JavaScript (via a Node worker), and WASM (via WASI)
1. Proper language defnition with PEG parser.
1. LSP & TreeSitter Support. 
1. Most of all, to have fun building something that I can introduce to my kids.


# Getting Started

The [getting-started guide](docs/getting-started.md) covers installation, a first
script, reading failures, and a complete acceptance-test workflow with reports.
The [syntax reference](docs/syntax.md) lists every form a script or suite can use,
and the [statement reference](docs/statements.md) lists every built-in statement.
`--check` [finds mistakes without running](docs/check.md) a file or suite, and
`--format` rewrites files in [canonical layout](docs/format.md), and `--lsp`
runs a [language server](docs/lsp.md), packaged with highlighting for
[Helix, Vim, and VS Code](docs/editors.md).
To look around quickly:

1. Clone the repo
2. run `cargo run -- --file examples/02-syntaxes.botwork`

`Log` writes readable values to stdout. Diagnostics go to stderr, and uncaught
script errors return a nonzero exit status. Add `--debug` after `cargo run --`
to trace top-level statement locations on stderr. See [language behavior](docs/language.md)
and [testing instructions](docs/testing.md) for the implemented contracts.

Repeat `--file` and set `--jobs` to run scripts concurrently with independent
state, for example `cargo run -- --file examples/21-parallel-first.botwork --file
examples/22-parallel-second.botwork --jobs 2`. See [parallel CLI execution](docs/parallel-cli.md)
for run IDs, ordering, per-run limits, and failure behavior.

Use `--suite examples/23-named-cases.suite.botwork` for named acceptance cases.
Add `--list-cases`, select with `--case` or `--tag`, and save `--failures failed.json`
for a later `--rerun-failed failed.json`. See [parameterized cases and reusable datasets](docs/parameterized-cases.md)
for independent row execution, and [named suites](docs/suites.md) for
stable IDs, library declarations, discovery order, and case isolation.

For complete tasks, [build a verified catalogue or check an HTTP response contract](docs/automation-examples.md).
Both examples combine imported helpers, JSON input variables, assertions, and
standard statements, with runnable local fixtures and failure demonstrations.

Batch and suite runs end with a [console report](docs/console-report.md): a recap of
failed, timed-out, and stopped work, then a summary whose counts match the exit status.
Add `--report-json report.json` for a versioned [JSON report](docs/json-report.md)
that CI can consume. It records the same verdict and exit status, every run's
statements, logs, errors, and artifacts, and a [schema](docs/json-report.schema.json)
to validate against. `--report-html PATH` renders the same results as a
self-contained [HTML report](docs/html-report.md), with source excerpts, timings,
logs, and artifact links. `--listener PROGRAM` streams the same execution events live
to a [report listener](docs/listeners.md), as JSON Lines on its stdin. The stream
has defined ordering, and slow or failing listeners are detached without slowing
the runs.
Every started run ends with exactly one [terminal outcome](docs/terminal-outcomes.md).
Ctrl-C cancels started runs cooperatively and still publishes the reports.
`--reconcile-report PATH` finishes reports after a forced termination, marking
unfinished runs as interrupted. Every report output has a documented
[limit](docs/report-limits.md), so large suites and long loops keep reports
bounded.
Runs are [isolated](docs/run-isolation.md): they share only the parsed trees of
unchanged imported modules, and each builds its own module state.
Stops reach nested statements, I/O, listeners, and cleanup, and runs release
their resources afterwards; see [cancellation](docs/cancellation.md).
A stopped run returns within its [shutdown bound](docs/shutdown.md), even when a
system call cannot be interrupted: blocked work is abandoned after
`--stop-grace-ms` (2 seconds by default).
[Stress repetition](docs/stress.md) runs deterministic isolation and cancellation
scenarios 1,000 times each and checks every outcome and released resource.
Mark inputs as [secrets](docs/secrets.md) with `--secret NAME` or
`--secret-env NAME=VARIABLE`. Their values are then masked as `***` in logs,
diagnostics, traces, reports, listener events, and artifacts.

Embedded hosts can request a versioned [run record](docs/run-records.md) of each run's
statements, logs, timing, and outcome through `RunOptions::record`.

Convert JSON and CSV test data with `Parse JSON`, `Format JSON`, and `Parse CSV`,
or read suite rows directly with `Dataset |"id"| From JSON |"rows.json"|`. See
[structured data](docs/structured-data.md) for the conversion rules.

Wait for eventual conditions with `Eventually` and repeat side-effecting actions with
`Retry`. Both use bounded [deadlines, attempts, and backoff](docs/polling.md) and
report the last failure together with the recent attempt history.

[Assertion diagnostics](docs/assertion-diagnostics.md) show nested value differences
and case/dataset identity. Add `--assertion-artifacts PATH` to retain full permitted
operands from unhandled assertions in separate JSON files.

Default [source and syntax limits](docs/syntax-limits.md) reject oversized inputs before parsing or execution.
[Runtime budgets](docs/embedded-runs.md#cli-and-low-level-contexts) stop excessive steps and recursion;
configure `--max-steps`, `--max-call-depth`, `--max-evaluation-depth`, and cooperative `--timeout-ms`.
[Output budgets](docs/output-limits.md) admit each complete record before writing;
configure `--max-output-record-bytes` and `--max-output-bytes` for logs, traces, and statement help.

Use `cargo run -- --list-statements` to list built-ins, or
`cargo run -- --statement-help 'Log |value|'` for parameter/return kinds and errors.
Rust hosts can [register statements with checked signatures](docs/interpreter-architecture.md#shared-signature-metadata).
[Typed isolated operations](docs/worker-protocol.md) run external workers on Linux with bounded requests, results, diagnostics, and supervised cancellation. Optional [process-tree guardians](docs/isolated-workers.md#process-tree-guardians) reap detached descendants and clean up after host termination.
Use the [embedded Engine](docs/embedded-runs.md) for fresh runs with local variables,
directory/environment configuration, cooperative cancellation, budgets, and structured results.
Use [call expressions](docs/language.md#calls-inside-expressions), such as `@{Double |3|}`,
to compose statement results inside other expressions.
[Local modules](docs/language.md#local-modules) use `Import |"helpers.botwork"| As |helpers|`
and qualified calls such as `helpers::Double |3|`.

Supply [input variables](docs/input-variables.md) from JSON files and repeatable overrides.
[Input budgets](docs/input-variables.md#input-resource-budgets) bound reads and decoded values before execution:

```sh
cargo run -- --file examples/20-input-variables.botwork \
  --vars-file examples/inputs/defaults.json --var 'name="Ada"' --var 'attempts=3'
```

[Retained-value budgets](docs/retained-values.md) bound variable storage across calls,
modules, and cloned contexts, releasing capacity when stored values are dropped.
[Definition budgets](docs/retained-definitions.md) also account for installed DSL
definitions and shared source text across repeated evaluations.
[Variable-name budgets](docs/retained-names.md) bound key storage and share it
across Context and module snapshots.
[Registry budgets](docs/retained-registry.md) cover statement/namespace metadata,
including native templates and imported wrappers. [Snapshot budgets](docs/snapshot-limits.md)
admit frame/cache copies before table allocation and define host Clone ownership.
[Result budgets](docs/result-limits.md) admit owned terminal/root exports, transfer
unique payloads, and report omitted snapshots explicitly.
[Temporary budgets](docs/temporary-limits.md) cover live expressions, arguments,
collection construction, and operand/output overlap across calls and modules.

Or you can create a file from below sample and pass the path to cargo run


# Sample Code

Here is what a botwork script might look like right now

<!-- botwork-test: readme-sample -->
```botwork
# Declaration
What is the square of |number| divided by |divisor| equals, eh?!... {
	|square| = |number ^ 2|
	Return |square / divisor|
}

# Invocation (case-insensitive, with flexible spacing)
|answer| = WHAT is  the   sQuAre of |6| divided by|2|equals, EH?!...

Log |"Here is your answer:"|
Log |answer|
```

# Roadmap to version 1.0

botwork is just taking its baby steps. There are so many things that are still missing and it goes without saying the the syntax & apis will change any time. Not to mention the hacy code that I managed to get working over the weekend. The Idea is to let it out in the wild and see if people are interested in a tool like this. 

If there is interest out there for a tool like botwork, I plan to dedicate more time to make v1.0 happen. So here is a bunch of things that needs to be done before botwork can be tagged v1.0. [TODO.md](TODO.md) has the detailed plan, and [roadmap decisions](docs/decisions.md) records the choices that shape it:

- [ ] Basic syntax
  - [x] Statements
  - [x] If condition
  - [x] For & While loop
  - [x] Basic arithmatic and logical operations
  - [x] Datatypes: int, float, string, bool, array, map
  - [x] Try/Catch and [awaited Finally cleanup](docs/cleanup.md)
  - [x] [Suite and case setup/teardown](docs/fixtures.md)
  - [x] Custom statements
  - [x] [Map and Array access](docs/language.md#collection-access)
  - [x] [Imports of local botwork files](docs/language.md#local-modules)
  - [ ] Imports of wasm files and packages, locally or from URL
- [ ] Docs
  - [x] README
  - [x] [Getting started docs](docs/getting-started.md)
  - [x] [Syntax docs](docs/syntax.md)
  - [x] [Statement docs](docs/statements.md)
- [ ] More statements out-of-box (like the ones RobotFramework Offers)
  - [x] [Built-ins](docs/builtins.md)
  - [x] [Collections](docs/collections.md)
  - [x] [Datetime](docs/datetime.md)
  - [x] [Operating system](docs/operating-system.md)
  - [x] [Process](docs/processes.md)
  - [x] [Strings](docs/strings.md)
  - [x] [Making HTTP Requests](docs/http.md)
  - [x] [JSON and CSV data](docs/structured-data.md)
- [ ] integrations
  - [ ] Selenium/Webdriver
  - [ ] Appium
  - [ ] Playwirght
- [x] Reports
  - [x] [Console Reports](docs/console-report.md)
  - [x] [JSON Reports](docs/json-report.md)
  - [x] [HTML Reports](docs/html-report.md)
  - [x] [Report listeners](docs/listeners.md)
- [ ] Tooling
  - [x] [LSP support](docs/lsp.md)
  - [x] [Editor support](docs/editors.md) (Helix, Vim, Neovim, VS Code)
  - [x] [TreeSitter grammar](docs/tree-sitter.md)
  - [x] [Linting](docs/check.md)
  - [ ] Trusted & Verified registry (for botwork packages), after 1.0 ([D10](docs/decisions.md#d10-packages-and-registry))
  - [ ] Python extension support (via PyO3, [D9](docs/decisions.md#d9-python-packaging))
  - [ ] JavaScript extension support (via an out-of-process Node worker, [D7](docs/decisions.md#d7-javascript-hosting))
  - [ ] WASM extension support (via Wasmtime and WASI, [D8](docs/decisions.md#d8-wasm-runtime))
- [x] CLI
  - [x] Ability to pass variables from CLI / Files
  - [x] [Run in parallel](docs/parallel-cli.md)
- [x] Fully async, non-blocking operations ([async execution](docs/async-execution.md), [nonblocking I/O](docs/nonblocking-io.md))
- [ ] Refactor my crappy code :P 
- [x] [Unit tests with at least 50% coverage](docs/testing.md)
