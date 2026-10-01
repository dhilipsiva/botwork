# Botwork documentation

Start with [getting started](getting-started.md). Every page in `docs/` is
listed below; `tests/reference_docs.rs` fails when a page is missing from this
index.

## Using Botwork

- [Getting started](getting-started.md): install, write a first script, read a
  failure, and build an acceptance-test workflow.
- [Installing and distributing Botwork](distribution.md): builds, release
  channels, and sharing Botwork code.
- [Command-line reference](cli.md): every option and exit status.
- [Configuration](configuration.md): what configures a run, with defaults and
  precedence.
- [Compatibility](compatibility.md): stable contracts, platforms, and tool
  versions.
- [The Rust API](rust-api.md): what the crate's stable API is, and how it can
  change in a minor release.
- [Complete automation examples](automation-examples.md)
- [Running scripts in parallel](parallel-cli.md)
- [Input variables](input-variables.md)
- [Secret inputs](secrets.md)

## The language

- [Syntax reference](syntax.md)
- [Language behavior](language.md)
- [Core language specification](language-specification.md)
- [Statement reference](statements.md)
- [Standard built-in statements](builtins.md)
- [Collection statements](collections.md)
- [String statements](strings.md)
- [Date and time statements](datetime.md)
- [Structured data: JSON and CSV](structured-data.md)
- [Operating-system statements](operating-system.md)
- [Process statements](processes.md)
- [HTTP statements](http.md)
- [Assertions and acceptance verdicts](acceptance-policy.md)
- [Assertion diagnostics and full operands](assertion-diagnostics.md)
- [Eventually and Retry](polling.md)
- [Owned cleanup with Finally](cleanup.md)
- [Structured diagnostics](diagnostics.md)

## Suites and datasets

- [Named suites and cases](suites.md)
- [Suite and case fixtures](fixtures.md)
- [Parameterized cases and reusable datasets](parameterized-cases.md)
- [Setup failures and skipped cases](setup-failure.md)

## Reports and outputs

- [Reports and outputs](reporting.md): choosing an output, and how formats are
  versioned.
- [Console report](console-report.md)
- [JSON report](json-report.md)
- [HTML report](html-report.md)
- [Report listeners](listeners.md)
- [Run records](run-records.md)
- [Terminal outcomes](terminal-outcomes.md)
- [Report limits and recovery](report-limits.md)

## Tools

- [Checking scripts](check.md)
- [Formatting](format.md)
- [Language server](lsp.md)
- [Editor support](editors.md)
- [Tree-sitter grammars](tree-sitter.md)
- [Keeping the tools aligned](alignment.md): one corpus for parsing,
  formatting, analysis, and highlighting, and how error recovery differs.

## Extending and embedding

- [Extending Botwork](extending.md): modules, Rust statements, listeners, and
  language adapters.
- [Python statements](python.md): statements written in Python, in builds with
  the `python` feature.
- [JavaScript statements](javascript.md): statements written in JavaScript,
  run with Node.js.
- [WebAssembly statements](wasm.md): statements in WebAssembly components,
  run in a sandbox with Wasmtime.
- [Embedded Rust runs](embedded-runs.md)
- [Interpreter architecture](interpreter-architecture.md)
- [Asynchronous execution](async-execution.md)
- [I/O and blocking callbacks in async runs](nonblocking-io.md)
- [Isolated worker supervision](isolated-workers.md)
- [Typed worker protocol](worker-protocol.md)
- [Worker platform and facility evidence](worker-platforms.md)

## Runtime guarantees

- [Run isolation](run-isolation.md)
- [Cancellation and resource release](cancellation.md)
- [Shutdown bounds](shutdown.md)
- [Stress repetition](stress.md)
- [Runtime performance measurements](performance.md)

## Resource limits

- [Source and syntax limits](syntax-limits.md)
- [Owned syntax admission](ast-limits.md)
- [Import resource budgets](import-limits.md)
- [Value admission and cleanup](value-limits.md)
- [Live evaluation temporaries](temporary-limits.md)
- [Owned run results](result-limits.md)
- [Output admission and completion](output-limits.md)
- [Frame and module-cache snapshots](snapshot-limits.md)
- [Retained variable values](retained-values.md)
- [Retained variable names](retained-names.md)
- [Retained DSL definitions](retained-definitions.md)
- [Retained statement and namespace metadata](retained-registry.md)
- [Retained calls and handler diagnostics](retained-diagnostics.md)
- [Aggregate operation ownership](operation-ownership.md)

## Diagnostics internals

- [Diagnostic detail construction](diagnostic-construction.md)
- [Host diagnostic ownership](diagnostic-ownership.md)
- [Bounded diagnostic rendering](diagnostic-rendering.md)
- [Diagnostic metadata conversion limits](diagnostic-value-limits.md)
- [Native operation diagnostic limits](operation-diagnostics.md)
- [Synchronous runtime diagnostic limits](runtime-diagnostics.md)

## Quality and project records

- [Testing Botwork](testing.md)
- [Traceability](traceability.md): every rule, statement, and guarantee, with
  its specification, examples, tests, and review status.
- [Core conformance corpus](conformance-corpus.md)
- [Execution order and cleanup evidence](execution-conformance.md)
- [Generated core validation](generated-validation.md)
- [Core mutation testing](mutation-testing.md)
- [Reproducing regressions](regression-reproduction.md)
- [DSL quality assessment protocol](quality-assessment.md)
- [Running the usability study](usability-study.md)
- [Usability pilot](usability-pilot.md)
- [Roadmap decisions](decisions.md)
- [Development log](development-log.md)
