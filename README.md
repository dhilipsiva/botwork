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
1. Extendible with Rust, Python (via PyO3), JavaScript (via neon), etc
1. Proper language defnition with PEG parser.
1. LSP & TreeSitter Support. 
1. Most of all, to have fun building something that I can introduce to my kids.


# Getting Started

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

Here is what a botwork script might looke like right now

<!-- botwork-test: readme-sample -->
```botwork
# Declaration
What is square-root of |number| divided by |divisor| equals, eh?!... {
	|square| = |number ^ 2|
	Return |square/divisor| 
	Log |"This statement will never execute"|
}

# Invocation (case-insensitive)
|answer| = WHAT is    sQuAre-RoOt of |6| divided by|2|equals, EH?!...

Log |"Here is your answer:"|
Log |answer|
```

# Roadmap to version 1.0

botwork is just taking its baby steps. There are so many things that are still missing and it goes without saying the the syntax & apis will change any time. Not to mention the hacy code that I managed to get working over the weekend. The Idea is to let it out in the wild and see if people are interested in a tool like this. 

If there is interest out there for a tool like botwork, I plan to dedicate more time to make v1.0 happen. So here is a bunch of things that needs to be done before botwork can be tagged v1.0:

- [ ] Basic syntax
  - [x] Statements
  - [x] If condition
  - [x] For & While loop
  - [x] Basic arithmatic and logical operations
  - [x] Datatypes: int, float, string, bool, array, map
  - [x] Try/Catch and [awaited Finally cleanup](docs/cleanup.md)
  - [x] [Suite and case setup/teardown](docs/fixtures.md)
  - [x] Custom statements
  - [ ] Map and Array access 
  - [ ] Imports (other botwork files, wasm files, packages; locally or from URL)
- [ ] Docs
  - [x] README
  - [ ] Getting started docs
  - [ ] Syntax docs
  - [ ] Statement docs
- [ ] More statements out-of-box (like the ones RobotFramework Offers)
  - [x] [Built-ins](docs/builtins.md)
  - [x] [Collections](docs/collections.md)
  - [x] [Datetime](docs/datetime.md)
  - [x] [Operating system](docs/operating-system.md)
  - [x] [Process](docs/processes.md)
  - [x] [Strings](docs/strings.md)
  - [x] [Making HTTP Requests](docs/http.md)
- [ ] integrations
  - [ ] Selenium/Webdriver
  - [ ] Appium
  - [ ] Playwirght
- [ ] Reports
  - [ ] Console Reports
  - [ ] JSON Reports
  - [ ] HTML Reports
  - [ ] Report listeners
- [ ] Tooling
  - [ ] LSP support
  - [ ] Editor support (Mainly Helix/Vim/VS Code)
  - [ ] TreeSitter grammar
  - [ ] Linting  
  - [ ] Trusted & Verified registry (for botwork packages)
  - [ ] Python extention support (via pyo3)
  - [ ] Javascript extention support (via neon)
  - [ ] WASM extention support (via WASI)
- [ ] CLI
  - [x] Ability to pass variables from CLI / Files
  - [ ] Run in parallel
- [ ] Fully async, non-blocking operations
- [ ] Refactor my crappy code :P 
- [ ] Unit-tests with atleast 50% coverage 
