# Roadmap to Botwork 1.0 and a 9.5+ DSL

This expands the [README roadmap](README.md#roadmap-to-version-10) with the DSL review findings. Numbered milestones give the default dependency order; independent tasks can proceed once their stated prerequisites are available. Correct execution comes first, followed by reusable automation, integrations, and tooling. Start validation alongside each feature and complete the final evidence gates in milestone 10.

Unchecked items are planned work. A task is complete when its behavior is implemented, documented, and supported by the relevant validation evidence. Design decisions must lead to tested behavior. Add regression tests and update examples with each change; documentation and testing continue throughout the roadmap.

## 9.5+ Quality Target

The target is a DSL rated at least **9.5/10 for acceptance testing and automation**. Completion requires evidence that people can read, compose, debug, and reliably execute real workflows. The score applies to the finished implementation under the rubric below; checking boxes alone does not establish it or guarantee other reviewers' ratings.

| Dimension | Weight | Required evidence |
| --- | ---: | --- |
| Semantic correctness and consistency | 25% | Executable specification, conformance corpus, regression and boundary tests |
| Readability, learnability, and composition | 20% | Usability sessions, readable examples, reusable statement workflows |
| Automation expressiveness | 15% | Suites, datasets, assertions, fixtures, imports, and complete automation tasks |
| Diagnostics and developer tools | 15% | Failure-repair tasks, formatter, CLI checks, consistent editor diagnostics |
| Runtime reliability and performance | 15% | Isolation, cleanup, limits, stress results, and reproducible benchmarks |
| Documentation, extensibility, and compatibility | 10% | Executed documentation, adapter conformance, reproducible packages, migration evidence |

- [x] Publish this rubric and representative evaluation tasks before implementation and usability studies. Define scoring anchors: `10` means all criteria are evidenced with no material friction found; `9` permits minor documented friction; `8` indicates recurring workarounds. Explain intermediate scores with specific evidence. See the [quality assessment protocol](docs/quality-assessment.md).
- [ ] Maintain a traceability index from each language rule, public statement, and runtime guarantee to its specification, examples, tests, and review results. Register representative workloads and performance budgets during milestone 1.
- [ ] Have at least two reviewers who did not author the assessed implementation independently score every dimension. Each review must reach a weighted score of at least `9.5`; no dimension may score below `9.0`, and semantics and runtime reliability must each reach `9.5`.
- [ ] Treat unresolved serious correctness defects, user-input crashes, cross-run state leaks, silent data loss, and broken compatibility promises as blockers regardless of the weighted score.
- [ ] Reopen tasks for every material gap found in review, fix the behavior, and repeat the affected assessment until all thresholds and mandatory gates pass. Record remaining minor limitations explicitly.

The numerical validation thresholds below are project acceptance targets. Freeze tasks, budgets, and scoring before measuring; any subsequent revision needs a written rationale and renewed assessment. Begin usability checks during syntax design and repeat them before release.

## Existing Prototype

These README items already have implementations. Their checked status records the starting point; the defects below still need fixing before release.

- [x] Statements and assignment.
- [x] `If` conditions.
- [x] `For` and `While` loops.
- [x] Arithmetic and logical operations.
- [x] Integer, float, string, boolean, array, and map literals.
- [x] `Try/Catch`.
- [x] Custom statement definitions and invocation.
- [x] Initial README and runnable examples.

## 1. Establish Regression Tests and Reliable Failure Reporting

Failures must be observable before expanding the interpreter. Start here so subsequent changes have a reliable baseline.

- [x] Add Rust unit tests for parsing, operators, and evaluation, plus CLI integration tests that assert exit status, stdout, and stderr. See [testing instructions](docs/testing.md).
- [x] Turn both `examples/*.botwork` scripts into behavior checks with expected results. [Example tests](tests/examples.rs) verify full stdout, empty stderr, and successful status in debug and release, including the complete loop behavior.
- [x] Capture the confirmed regressions in milestone 2 before fixing each one, including minimal scripts that fail independently. See [regression tests](tests/regressions.rs) and [how to reproduce them](docs/testing.md#known-defects).
- [x] **Fix successful exit on failure.** Return a nonzero status for syntax errors and uncaught evaluation errors; keep successfully handled `Try/Catch` errors distinct. [CLI failure contract](docs/testing.md#cli-failure-contract) and active regression tests verify status `1`, stderr diagnostics, and fail-fast execution in [src/main.rs](src/main.rs).
- [x] Replace `dbg!` output with a defined user interface: normal `Log` output on stdout, diagnostics on stderr, and optional `--debug` tracing. See [log output](docs/language.md#log-output); CLI and writer tests verify the output contract.
- [x] Set up CI for `cargo test`, compilation, formatting, and agreed Clippy checks. The [workflow](.github/workflows/ci.yml) builds/tests debug and release with locked dependencies; all declared checks pass locally. Hosted execution remains to be observed after publication.
- [x] Add coverage measurement and record the initial result. The [baseline](docs/testing.md#initial-baseline--2026-09-26) records **75.36% library unit** and **82.53% full-suite** line coverage with explicit source scope, exclusions, tool versions, and 13 pending regressions. Repeat with `python3 scripts/coverage.py`; README's 50% target remains an intermediate milestone and the stronger release gates remain in milestone 10.
- [ ] Build an executable conformance corpus: every specified rule needs positive, invalid-input, and applicable boundary cases. Run CLI cases against debug and release binaries, checking output, status, and generated results.
- [ ] Execute documentation examples in CI and preserve minimal reproductions for every regression. Record randomized-test seeds, platform, adapter versions, and resource limits so failures can be reproduced.
- [ ] Define supported platforms and adapter configurations, plus benchmark hardware, datasets, and absolute latency/memory budgets for representative tasks. Agree these budgets before optimization and track them through milestone 6.
- [ ] Introduce property and fuzz harnesses alongside the parser/evaluator work in milestones 2–3, then extend them as imports and adapters arrive. Begin mutation testing once regression tests exist; milestone 10 verifies the final campaigns and thresholds.

## 2. Make DSL Execution Predictable

The following defects were reproduced during the DSL review. Relevant implementation files are [grammar.pest](src/core/grammar.pest), [grammar.rs](src/core/grammar.rs), and [eval.rs](src/core/eval.rs).

### Language Contract and Interpreter Structure

- [ ] Write a short language specification covering evaluation order, precedence, value types, scope, statement return values, and catchable errors. Use it to define expected regression results.
- [ ] Convert parsed input into an owned syntax tree with source spans. Store custom statement bodies once, preserve unevaluated expressions for short-circuiting, and stop reparsing source on each invocation.
- [ ] Represent normal completion, `Return`, `Break`, and `Continue` explicitly. Propagate control flow through nested blocks; consume returns at invocation boundaries and loop controls at the appropriate loop.
- [ ] Introduce invocation scopes. Specify variable lookup, assignments, recursion, loop-variable lifetime, and whether callers' variables are visible or mutable.
- [ ] Complete the value contract: distinguish absent values from `None`, specify truthiness, coercion, equality for every type, comparison chains, collection aliasing/mutation, map iteration order, and implicit return values. Specify and test unsupported combinations as errors.
- [ ] Test evaluation order and nested control flow systematically: arguments with observable effects, recursion, shadowing, loops, returns, catches, and scope restoration on every completion path.
- [ ] Define whitespace, line termination, multiline calls/collections, comments, escaping, and reserved delimiters without ambiguous parsing. Cover LF/CRLF, tabs, empty input, Unicode text, and `|`, `#`, and braces inside strings.

### Confirmed Control-Flow and Scope Defects

- [x] **Fix `While` stopping after one ordinary iteration.** A counter starting at `0` and incrementing while below `3` finishes at `3`. The active regression and unit tests cover repeated conditions, zero iterations, `Continue`, `Break`, and nonboolean conditions. See [loop semantics](docs/language.md#while-loops).
- [ ] **Fix position-dependent `Return`.** A custom statement ending with `Return |7|` must return `7`, not `[7]`. Adding unreachable statements must not change the result, and no pending return may leak into another invocation.
- [ ] **Fix nested return propagation.** A return inside an `If`, loop, or `Try/Catch` must exit the containing custom statement. Outer statements after that return must not execute.
- [ ] **Fix parameter binding corrupting arguments and caller state.** With caller `x = 10`, calling `Pair |1| with |x|` against parameters `x` and `y` must pass `1, 10`. Evaluate all arguments in caller scope before binding parameters; preserve the caller's `x`.

### Confirmed Expression and Parser Defects

- [x] **Correct operator precedence.** Arithmetic, ordering comparisons, equality, `and`, and `or` have distinct [documented levels](docs/language.md#binary-operator-precedence). The active regression verifies `1 + 2 == 3`; evaluator and example tests cover mixed levels, parentheses, left association, and binary/unary minus without the former operator-registration panic.
- [x] **Define exponentiation and unary precedence.** Powers associate right (`2 ^ 3 ^ 2` is `512`); unary minus binds after power, negative exponents and repeated unary operators are supported, and parentheses override grouping. [The contract](docs/language.md#powers-and-unary-operators), active regression, evaluator/parser cases, and executable example cover results, intermediate overflow, type errors, and Catch recovery.
- [ ] **Implement boolean short-circuiting.** `false and undefined` must return `false`; `true or undefined` must return `true`, without evaluating the unused operand.
- [x] **Decode strings into values.** Remove delimiters and process supported escapes. `("a" + "b") == "ab"` evaluates to `true`; escaped newlines, quotes, and backslashes have consistent runtime meanings. See [string semantics](docs/language.md#strings) and active regression tests.
- [ ] **Match complete keywords.** Accept identifiers such as `order` and statement names such as `Format report`; reserved-word prefixes must not reject valid names or split them into control statements.
- [x] **Handle accepted collection-access syntax.** Parsed dot paths now return a typed, catchable `UnsupportedAccessError` containing the path instead of reaching `unreachable!()`. Unit and CLI tests cover propagation, recovery, skipped branches, and failure status. See [temporary access contract](docs/language.md#collection-access-status); actual lookup remains in milestone 3.
- [x] **Make arithmetic failures catchable.** Numeric operations return `ArithmeticError` for zero divisors, integer overflow, and non-finite results; incompatible operand types remain type errors. [The arithmetic contract](docs/language.md#arithmetic-boundaries-and-errors) defines exponent bounds, reciprocal powers, rounding, and edge cases. Unit, CLI, and example tests verify matching debug/release behavior and Catch recovery.
- [x] **Align `Catch` syntax and execution.** `Try` requires exactly one `Catch` block. The grammar rejects missing or malformed handlers before any script execution; parser, evaluator, and CLI tests verify valid nesting, skipped handlers on success, and propagation of handler failures. See [Try/Catch](docs/language.md#trycatch).

## 3. Complete Core Language Features and Runtime Contracts

Complete these foundations before building a large statement library or external adapters.

- [ ] Implement map and array access, including nested access, integer indexes, and errors for missing keys, invalid indexes, and incompatible values. Specify whether and how indexed assignment works.
- [ ] Define numeric precision and conversion rules for the current `i32`/`f32` values; decide how collection equality and mixed numeric comparisons behave. Support the full signed literal range: the recorded `minimum_signed_integer_literal_is_representable` regression currently rejects `-2147483648` before unary negation.
- [ ] Validate control-flow placement. Reject `Break`/`Continue` outside loops and define top-level `Return`; user scripts must receive diagnostics rather than interpreter panics.
- [ ] Define statement-name normalization and duplicate-definition behavior. Cover spaces, tabs, capitalization, punctuation, and collisions; document case-sensitive variables alongside case-insensitive statement names. Calls must resolve to exactly one signature; reject collisions at definition/import time with both source locations.
- [ ] Resolve custom-call composition before freezing syntax. Implement one unambiguous form for using custom results inside expressions, or demonstrate that explicit intermediate assignments meet all registered composition tasks and usability thresholds. Document the chosen model and errors for invalid nesting.
- [ ] Validate multilingual authoring promised by the README. Specify Unicode identifiers, sentence text, normalization, case matching, and whitespace; include Tamil and accented-text examples, collision cases, and clear documentation of syntax tokens that remain fixed.
- [ ] Introduce structured errors with source file, line, column, offending expression, and statement call stack. Preserve useful details through catch handlers and reporting.
- [ ] Assign stable diagnostic codes and preserve causes across imports and adapters. For common mistakes, show the relevant source range and actionable correction; verify offsets with Unicode, tabs, and CRLF.
- [ ] Let catch handlers inspect structured error codes and details and rethrow with the original cause. Define how a failure in a handler is reported without erasing the original failure.
- [ ] Add a public Rust statement registration API with parameter validation, documented return values, and a stable error contract. Use it for native built-ins and later adapters.
- [ ] Give native and DSL-defined statements shared signature metadata: parameter names, accepted value kinds, return kinds, and documented errors. Diagnose invalid calls before the affected operation begins; reuse this metadata in help, completion, and hover.
- [ ] Establish an async statement interface with cancellation, timeouts, and explicit handling of blocking work. Resolve this contract before writing I/O libraries and language bridges.
- [ ] Support local `.botwork` imports with paths relative to the importing file, module caching, namespaces, duplicate-name rules, and cycle diagnostics.
- [ ] Accept variables from CLI arguments and files. Document supported value types, file format, precedence, invalid-input behavior, and which scope receives supplied variables.
- [ ] Expose an embeddable Rust execution API with per-run variables, working directory, environment overlay, cancellation, limits, and structured results. Per-run configuration must not mutate process-global environment or working directory.
- [ ] Bound source size, nesting, recursion, execution steps, collections, and host-operation duration. Report exceeded limits as structured failures with bounded cleanup; test adversarial inputs without allowing unbounded evaluation.

### Acceptance-Test Model

Define this model before standard assertions and reports so they share case identity, lifecycle, and failure semantics.

- [ ] Support named suites/cases with stable identifiers, tags, selection, and rerunning failed cases. Separate library definitions from runnable cases and specify discovery and execution order.
- [ ] Add parameterized cases and reusable datasets. Each row needs a stable identity and its own result; one failed row must not silently prevent unrelated rows from running.
- [ ] Define suite/case setup and teardown with explicit resource ownership. Cleanup must run after success, assertion failure, returns, handled/unhandled errors, and cooperative cancellation; document forced-termination limits.
- [ ] Provide a resource/cleanup construct for ordinary scripts and custom statements as well as test fixtures. Test nested resource ownership and release on every supported completion path.
- [ ] Specify setup failure and skipped-case behavior. Preserve the primary failure when cleanup also fails, report both causes, and ensure teardown cannot silently turn a failed case into a pass.
- [ ] Decide immediate versus collected assertion failures, expected-failure handling, and final suite status. Keep console, JSON, HTML, and process exit status consistent with those decisions.

## 4. Provide Useful Standard Statements

Build these groups on the stable registration and async contracts. Give each statement documentation, examples, parameter checks, and success/failure tests.

- [ ] **Built-ins:** assertions, explicit failure, logging, variable inspection, and reusable control helpers needed for acceptance testing.
- [ ] **Collections:** array/map creation, lookup, updates, membership, length, iteration helpers, and comparisons.
- [ ] **Strings:** formatting, joining, splitting, replacement, matching, and documented Unicode behavior.
- [ ] **Date/time:** parsing, formatting, durations, comparisons, and explicit timezone handling.
- [ ] **Operating system:** files, directories, paths, environment access, and cleanup, with platform-specific behavior documented.
- [ ] **Processes:** execution, argument lists, environment, working directory, stdout/stderr capture, exit codes, timeouts, and cancellation.
- [ ] **HTTP:** methods, headers, request bodies, response status and content, timeout handling, and explicit redirect/retry behavior.
- [ ] Add complete examples combining assertions, imports, variables, and standard statements into useful automation tasks.
- [ ] Make assertions diagnostic: report expected and actual values, useful string/collection differences, source location, and case/dataset identity. Truncate large values clearly while keeping full permitted artifacts available.
- [ ] Provide bounded polling for eventual conditions with deadlines, intervals/backoff, cancellation, and last-observed diagnostics. Distinguish observing a condition from retrying an action with side effects; retain attempt history.
- [ ] Support structured JSON and tabular test data with documented conversion rules and precise errors. Exercise nested payloads, Unicode, missing fields, numeric boundaries, and dataset-driven HTTP assertions.
- [ ] Maintain complete reference workflows for HTTP contract tests, filesystem/process automation, imported custom statements, and dataset-driven suites. Add browser acceptance and failure diagnosis as their integrations arrive; use these workflows in usability and release assessment.

## 5. Define Execution Results and Reports

A common result/event model must precede report formats and listeners. Preserve ordering within each run so parallel execution can reuse the same model.

- [ ] Define run and statement results: identifiers, source locations, start/end times, outcomes, logs, errors, and associated artifacts. Define success, failure, cancellation, and skipped work consistently.
- [ ] Add a console report with readable progress, failures, and a final summary consistent with the process exit status.
- [ ] Add a documented, versioned JSON report suitable for CI and other consumers.
- [ ] Add report listeners using the same execution events. Define ordering, listener failures, and how slow listeners are handled.
- [ ] Build HTML reports from the shared result model, including source references, timings, logs, and artifacts.
- [ ] Produce exactly one terminal outcome per started run/case during normal or cooperatively cancelled execution, with stable ownership of events and artifacts. Reconcile incomplete records after forced termination as interrupted runs; never infer a pass from partial output. Test reporter errors and concurrent failures.
- [ ] Bound event queues and report sizes; specify retention, truncation, and recovery from interrupted writes. Reports must handle large suites without retaining every loop iteration's value in memory.
- [ ] Support explicitly secret-marked inputs and redact them from logs, diagnostics, traces, reports, and adapter exceptions, including nested values. Render script-provided HTML report content as text and verify artifact links stay attached to the correct run.

## 6. Finish Async Execution and Add Parallel Runs

Async interfaces begin in milestone 3. Complete their implementation before relying on concurrency in adapters and integrations.

- [ ] Make I/O operations nonblocking where supported; isolate blocking native or foreign-runtime operations so they do not stall other runs.
- [ ] Propagate cancellation and timeouts through nested statements, I/O, listeners, and cleanup. Test resource release after failures.
- [ ] Add CLI support for running multiple scripts in parallel with bounded concurrency and independent variables, scopes, and run identifiers.
- [ ] Specify resource sharing, output ordering, fail-fast behavior, and aggregate exit status. Test one run failing or being cancelled while others continue.
- [ ] Measure parsing, repeated calls, I/O, and concurrent execution; use results to guide performance work.
- [ ] Prove run isolation covers variables, module state, sessions, environment overlays, working directories, reports, and artifacts. Cache immutable compiled modules independently from mutable invocation state.
- [ ] Establish core shutdown bounds and test blocked I/O and cleanup failures. Enforce and test this contract for each adapter/integration as it arrives in milestones 7–8; use isolated workers when an advertised cancellation guarantee cannot be met in process.
- [ ] Run reproducible workloads for CLI startup, a 10,000-statement parse, 100,000 custom calls, a 1,000,000-iteration loop, and 100 concurrent waiting runs. Publish p50/p95 latency, peak memory, and environment; meet the budgets registered in milestone 1.
- [ ] Check scaling when input or concurrency doubles, verify bounded memory for long-running loops with bounded live data, and investigate unexpected superlinear behavior. Block unexplained p95 or memory regressions exceeding 10% against the accepted baseline on the same environment.
- [ ] Repeat deterministic isolation/cancellation stress scenarios at least 1,000 times with zero unexplained flakes, leaked resources, or incorrect outcomes. Use controlled clocks and services so environmental failures can be distinguished from runtime defects.

## 7. Add Extensions, Remote Imports, and Packages

Build adapters on the invocation, async, error, and reporting contracts. Local module resolution must work before remote package resolution.

- [ ] **Python via PyO3:** define value conversion, exception mapping, interpreter lifecycle, dependency packaging, and interaction with async execution.
- [ ] **JavaScript:** evaluate the README's Neon proposal, choose the hosting model, and implement value conversion, exceptions, asynchronous calls, and runtime distribution.
- [ ] **WASM via WASI:** define callable exports, host capabilities, value/error conversion, execution limits, and module lifecycle.
- [ ] Define package metadata, dependency resolution, version compatibility, and reproducible dependency records.
- [ ] Import local WASM modules and packages using the same namespace and diagnostic rules as `.botwork` modules.
- [ ] Support URL imports of `.botwork` files, WASM modules, and packages with explicit fetch/cache behavior, version or content pinning, integrity checks, and offline reuse.
- [ ] Build the trusted and verified package registry from the README roadmap. Specify what verification guarantees, publisher identity, artifact integrity, installation policy, and revocation behavior before publishing packages through it.
- [ ] Document which optional adapters require additional runtimes and how they affect the single-binary distribution goal.
- [ ] Run one adapter conformance suite for all advertised configurations: Unicode, numeric boundaries, nested values, `None`, async results, errors, cancellation, ownership, and cleanup. Unsupported conversions must fail explicitly without truncation or silent coercion.
- [ ] Verify packages from clean installation through cached offline execution, integrity rejection, dependency conflicts, version incompatibility, and namespace collisions. Distinguish trusted in-process extensions from capability-restricted WASM modules in documentation and execution policy.
- [ ] Version public runtime, adapter, package, and report contracts. Add compatibility fixtures and test supported upgrades, deprecation diagnostics, and actionable rejection of unsupported versions.

## 8. Add Browser and Mobile Automation Integrations

Use the common statement API, cancellation, and report artifacts. Integrations can be implemented independently once those contracts are available.

- [ ] **Selenium/WebDriver:** sessions, navigation, locating elements, actions, assertions, waits, screenshots, and cleanup.
- [ ] **Playwright:** select an integration approach and expose browser/context/page lifecycle, actions, assertions, waits, and artifacts.
- [ ] **Appium:** device/session configuration, element operations, waits, screenshots, and cleanup.
- [ ] Define shared conventions for selectors, timeout handling, retries, and session ownership. Avoid uncontrolled implicit retries that hide failed assertions.
- [ ] Provide runnable end-to-end examples and integration tests with explicit external prerequisites and teardown.
- [ ] Verify every integration on its advertised platform/version matrix, including session creation failure, element timeout, assertion failure, cancellation, and parallel session isolation. Require diagnostic artifacts and cleanup for each supported failure path.

## 9. Complete Documentation and Developer Tooling

Keep docs current during earlier milestones; this milestone completes coverage and adds tools on the stabilized grammar, syntax tree, and diagnostics.

- [ ] Write getting-started documentation covering installation, the first script, execution, failures, and a complete acceptance-test workflow.
- [ ] Publish the syntax reference: literals, operators, precedence, variables, scope, statements, control flow, imports, errors, and naming rules.
- [ ] Publish the statement reference with signatures, argument/return types, examples, failures, and adapter prerequisites.
- [ ] Update README examples and roadmap status. Correct the example named “square-root” that currently calculates a square, and remove examples that depend on defective return behavior.
- [ ] Add DSL linting for undefined names, duplicate statements, invalid control flow, unreachable code, and other statically detectable mistakes.
- [ ] Add a side-effect-free `check` command that parses and analyzes local modules, names, signatures, and control flow without executing statements, adapter initialization, or network operations. Identify unresolved dynamic behavior honestly instead of declaring it validated.
- [ ] Add a canonical formatter with a CI check mode and readable multiline formatting. Across the conformance corpus, require idempotence, preserved comments/literal contents, and unchanged program meaning.
- [ ] Build a Tree-sitter grammar and verify it against the interpreter's syntax examples and invalid-input corpus.
- [ ] Implement LSP diagnostics, completion, hover documentation, go-to-definition, and symbol references using shared language analysis.
- [ ] Share semantic analysis between CLI checks, linting, and LSP; require matching diagnostic codes and source ranges for identical input. Test incomplete edits, recovery, imported symbols, Unicode positions, and removal of stale diagnostics.
- [ ] Add safe symbol rename and signature help across modules. Reject ambiguous edits and verify that applying a rename preserves resolution and behavior in the language corpus.
- [ ] Document and package editor support for Helix, Vim, and VS Code, including highlighting and LSP configuration.
- [ ] Publish extension, package, CLI, configuration, and reporting documentation with compatibility guidance.
- [ ] Keep syntax highlighting, parsing, formatting, and editor analysis aligned through a shared corpus; document any intentional differences in error recovery.
- [ ] Run usability sessions with at least ten participants, including at least five automation newcomers and five experienced practitioners. Using published docs only, measure reading an unfamiliar script, writing a first passing script, defining a reusable statement, composing results, adding a dataset case, and fixing a reported failure.
- [ ] Require at least 90% unaided completion for each registered task, median time to a first passing script of at most 10 minutes, and at least 80% of participants fixing the planted diagnostic task within 5 minutes. Report newcomer and experienced cohorts separately and retain original results alongside retests. Revise syntax/docs for recurring misunderstandings and repeat failed tasks with fresh participants before freezing the language.

## 10. Verify 1.0 Readiness

- [ ] Resolve all confirmed DSL regressions and cover each with focused tests, including failure exit codes and errors inside `Try/Catch`.
- [ ] Require complete rule-to-test traceability and at least 95% line coverage of parser/evaluator/runtime correctness code, plus 90% branch coverage measured with a documented suitable tool. Specify the measured source set and review exclusions; generated code must not inflate coverage. Retain integration coverage for CLI, imports, reports, cancellation, adapters, and parallel isolation.
- [ ] Use mutation testing on precedence, scope, control flow, errors, and isolation. Kill at least 90% of non-equivalent mutations in that critical subset; individually review survivors and repair meaningful test gaps.
- [ ] Add property tests for string decoding, formatter round trips, statement normalization, adapter conversions, and evaluation against an independently calculated subset of expressions. Check successful and rejected programs against the specification in debug and release builds.
- [ ] Fuzz parsing, syntax-tree construction, bounded evaluation, import resolution, and advertised adapter conversion boundaries. Run a fixed smoke budget in CI and at least 24 CPU-hours per target before release; archive seeds and minimized failures. Resolve all discovered crashes, hangs, and semantic mismatches before acceptance.
- [ ] Pass the conformance corpus on every supported OS and the adapter suite on every advertised configuration. Verify clean-machine installation, offline cached use, reproducible dependency resolution, and migration from the previous supported release when one exists.
- [ ] Meet the registered performance/resource budgets, stress criteria, usability thresholds, and editor/CLI consistency checks. A failed gate requires remediation and rerun, not a coverage-only exception.
- [ ] Complete the interpreter refactoring started in milestone 2, documenting parser, syntax-tree, evaluator, runtime, and adapter responsibilities.
- [ ] Verify all roadmap acceptance criteria and examples; document supported platforms and optional runtime dependencies.
- [ ] Stabilize language, statement, package, and report contracts; record compatibility and migration policies before tagging 1.0.
- [ ] Publish a release evidence report covering specification traceability, regression/conformance results, coverage, mutation/fuzz results, platform/adapter compatibility, performance, usability, and remaining limitations.
- [ ] Complete the independent scored reviews defined above. Require every designated review's weighted score to reach 9.5+, all category floors to pass, and all hard blockers to be resolved before claiming the 9.5+ quality target is achieved.
