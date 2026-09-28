# Testing Botwork

Run `cargo test` from the repository root. Linux typed-worker interoperability tests also require `python3` on PATH (the same interpreter used by the coverage-helper checks). Worker tests require the enabled facilities and test tools in the [platform matrix](worker-platforms.md); missing facilities fail the tests rather than skip coverage. Run `cargo test --release` to check the optimized build. Add `--offline` when dependencies are already cached.

[Generated core validation](generated-validation.md) adds seeded expression/literal properties, parser mutations, and a separate fixed libFuzzer smoke campaign. The ordinary test suite runs the seeded checks; `python3 scripts/fuzz_smoke.py` records the sanitizer campaign with its pinned nightly and tools.

[Core mutation testing](mutation-testing.md) freezes generated and targeted mutations for precedence, scope, control flow, errors, and isolation. Run `python3 scripts/mutation_core.py generated` and `python3 scripts/mutation_core.py targeted`; the guide records tool versions, survivor repairs, scoring, and replay instructions.

[Async execution](async-execution.md) adds suspended DSL/operation integration cases while keeping existing synchronous contract tests on the shared evaluator. Its evidence records debug/release checks, native and isolated operation cleanup, and the updated mutation scope.

[Named suites](suites.md) add model tests in `src/core/ast/suite/tests.rs` and CLI
tests in `tests/suite_cli.rs`. These cover bounded discovery/metadata, stable IDs,
selection, independent case state, failed-case persistence, concurrent admission,
interruption, locking, and reporter failure. T1 registers three conformance cases;
example 23 and the `botwork-suite` documentation fence run with exact output and
case status checks.

- `src/core/worker/protocol/tests.rs` checks the typed wire codec and worker SDK; `tests/typed_workers.rs` exercises an independent Python subprocess, signatures, budgets, cleanup ownership, and stop priority.
- `src/core/grammar/tests.rs` checks program parsing and typed operators.
- `src/core/ast/tests.rs` checks owned syntax, expression grouping, shared sources, original byte/line/column spans, and control-placement validation.
- `src/core/eval/tests.rs` checks evaluation, state, conditions, collections, and error handling.
- `src/core/eval/execution_contract.rs` checks ordered argument/collection visits, branch selection, 56 nested-loop completion/restoration combinations, While condition timing, and recursive frame traces. [Execution evidence](execution-conformance.md) explains the test-only recorder and companion CLI fixtures.
- `tests/ast_execution.rs` checks execution after source/program ownership ends, deferred numeric errors, parser-pair compatibility, and validation before effects for extracted/assembled syntax.
- `tests/cli.rs` invokes Cargo's built CLI and checks exit status, stdout, and stderr independently. Inputs live under `tests/fixtures/`.
- `tests/examples.rs` checks the exact expected stdout and successful status of bundled examples. Scripts require empty stderr; suite examples check case progress records. Expected results are derived from the examples' operations; update them only after reviewing an intentional behavior change.
- `tests/documentation.rs` inventories README/docs fences and runs every registered Botwork snippet through the CLI, checking exact stdout and status, empty script stderr, and suite progress records. `src/lib.rs` includes Rust API examples for doctesting. The [reproduction guide](regression-reproduction.md) explains registration, timeouts, and failure metadata.
- `tests/conformance.rs` checks the [rule-indexed corpus](conformance-corpus.md): 36 CLI scripts (including two module projects) plus seven host registration/value/signature cases, with positive/invalid/boundary evidence for every current rule. `tests/cli_harness.rs` verifies isolated workspaces, stream/status capture, timeout termination, and subsequent execution; documentation and conformance share this runner.
- `tests/diagnostics.rs` checks detailed error categories, innermost spans, syntax/validation locations, related declarations, full call snapshots, source ownership, recursive cleanup, handler causes, native/Pair behavior, and legacy API compatibility. CLI fixtures check visible stacks and original causes; Rust API examples run as doctests.
- `tests/diagnostic_codes.rs` pins all 21 core codes, uniqueness/catalog coverage, common-error repairs, range endpoints, and distinct cause codes/guidance. CLI and corpus cases check rendered codes and successful recovery remains silent.
- `tests/catch_contract.rs` checks temporary metadata bindings, nested handlers/rethrows, immutable original identity, fresh same-source failures, schema/positions, lexical validation, Unicode syntax, and Pair boundaries. Unit checks cover 32 binding/completion combinations before frame disposal plus the defensive invocation guard. Example `17`, CLI fixtures, and a documentation snippet verify real output.
- `tests/language_contract.rs` checks named expectations from the [core specification](language-specification.md), including lexical scope and control propagation. All its current cases are active; further conformance work remains in the roadmap.
- `tests/value_contract.rs` checks all 686 binary and 14 unary operator/value-kind combinations, control/iterable kinds, None/absence, collection copies, duplicate map keys, iteration/display order, comparison chains, and implicit/explicit result rules.
- `tests/layout_contract.rs` checks LF/CRLF, tabs, multiline syntax, statement boundaries/continuations, comments, reserved delimiters, string preservation, invalid layouts, and original source locations. Example `14` and CLI fixtures cover the same grammar through process execution.
- `tests/naming_contract.rs` checks space/tab/case normalization, Unicode distinctions, unique signatures, duplicate parameter/definition errors, retained source locations, lexical shadowing, cleanup, native initialization, and preserved original registrations. Example `15`, Pair compatibility, and CLI cases verify public execution boundaries.
- `tests/unicode_contract.rs` checks identifiers across scripts, combining marks, exact spellings, keyword boundaries, fixed syntax tokens, invalid names/numbers, numeric map paths, original UTF-8 locations, readable collection display, and duplicate declarations. Example `16` and a CLI fixture cover multilingual execution and rejection before output.

Tests assert language behavior and error categories rather than Rust source line numbers or map iteration order. No external services or extra testing crates are required for the initial suite. Add a minimal regression before fixing a known defect; do not preserve defective behavior as an expected result.

Run `cargo test --test examples` to check the bundled demonstrations alone. The syntax check includes every intended `While` iteration (`3`, `4`, `5`, `6` before the break), both `For` behaviors, and handled errors. The expression check preserves intentional whitespace inside strings and verifies deterministic map display without relying on internal hash-map iteration order. The precedence example checks mixed arithmetic/comparisons, boolean grouping, parentheses, and subtraction with unary minus. The arithmetic example checks caught numeric failures, reciprocal powers, and a valid integer boundary.

The powers example checks right association, unary grouping, negative exponents, repeated prefixes, and caught intermediate overflow. The keyword example checks identifiers and custom statement names containing keyword prefixes, including punctuation and mixed-case invocation. The short-circuit example checks skipped errors, guarded loop termination, and recovery from a required operand's error; evaluator instrumentation verifies which operands are visited and in what order. The control-flow example checks exact return values, nested returns, repeated calls, bare return/fallthrough, handler returns, and loop controls through Try/Catch. The suite does not establish complete language conformance or the roadmap's final coverage targets. Track unfinished regression and validation work in [TODO.md](../TODO.md).

The scopes example checks argument binding, caller preservation, lexical lookup, native Log inside a call, recursion, loop-variable restoration after failure, and nested definition lifetime. Unit cases also inspect frame cleanup, cloned-context isolation, and iterator restoration before an invocation is discarded.

The signed-integer example checks both range boundaries, leading zeroes, power grouping, and caught literal/negation errors. The CLI overflow fixture verifies that the valid minimum prints before a later overflow fails the process and skips subsequent output.

The collection-access example checks nested map/array paths, Unicode keys, call/loop composition, catchable failures, and skipped access. Unit tests cover exact map keys, index validation, oversized bounds, None versus absence, first-failure priority, comments between segments, and copy semantics.

The computed-access example adds variable indexes, quoted and empty map keys, temporary expression bases, and mixed bracket/dot paths. Unit instrumentation verifies that the base and each required index run once in order, with later keys skipped after failure. AST cases preserve bracket/decoded-key spans and reject malformed accesses and indexed assignments. CLI fixtures check bounds diagnostics and syntax rejection before any output, including malformed skipped operands; both owned and parser-pair execution APIs are covered.

The value-comparison example checks exact mixed numeric comparisons, arithmetic rounding, structural array/map equality, None, and numeric-only ordering. Unit matrices cover all value-kind pairs, equality laws, all six numeric comparisons in both directions near precision/range boundaries, invalid nested host floats, decimal rounding, and operand evaluation order. A CLI fixture checks that valid collection equality prints before an uncaught ordering error stops execution.

## Coverage Measurement

Use Python 3.9 or newer and the active Rust toolchain's LLVM tools:

```sh
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --version 0.9.1 --locked
python3 tests/coverage_tools.py
python3 scripts/coverage.py --offline
```

Omit `--offline` when Cargo dependencies need downloading. If installing the tool with `--root`, add that directory's `bin` to `PATH`. The helper requires the recorded tool version; review upgrades explicitly. Use the default Cargo compiler/profile configuration and unset custom compiler and LLVM-reporting overrides before capture. The initial configuration uses the compiler's matching LLVM tools.

The command collects **library unit tests** and **all active Rust tests** separately. Use `--scope unit` or `--scope all` for a single capture. Each scope uses its own target directory and cleans previous profiles. CLI subprocesses inherit profiling settings with process/module identifiers, so their execution contributes to full-suite coverage. Do not run concurrent captures of the same scope.

Reports and test logs are written under ignored `target/coverage/`: `unit.json`, `all.json`, their `.log` files, and `both-summary.json` (or the selected scope's summary). Summaries record commands, versions, platform, source/test hashes, test counts, and per-file covered/total lines. The Git revision is the base revision; when the worktree is dirty, input hashes identify the measured files. Stop editing source/tests during collection.

Coverage includes executable lines in maintained Rust source files, including owned syntax construction in `ast.rs`, evaluation in `eval.rs`, and operators in `grammar.rs`. It excludes test files and the isolated Pest-generated parser in `src/core/parser.rs`. Other derives, such as clap and thiserror, can contribute mapped lines. Library-only coverage excludes `main.rs`; full-suite coverage includes it. Module-only files have no executable lines. Review the expected source-file lists in `scripts/coverage.py` whenever adding code; mismatches fail collection.

Line coverage does **not** measure grammar-rule coverage, branch coverage, ignored regressions, doctests, assertions' quality, or correctness of every exercised path. README's 50% unit-coverage goal is an intermediate target, and TODO milestone 10 retains the stronger release gates. The helper's eight tests run in CI; instrumented coverage collection is currently a local command.

Tool references: [cargo-llvm-cov usage](https://github.com/taiki-e/cargo-llvm-cov) and [Rust coverage instrumentation](https://doc.rust-lang.org/rustc/instrument-coverage.html).

### Initial Baseline — 2026-09-26

[Recorded JSON](coverage-baseline.json) preserves the first capture's inputs and results. The debug capture used Rust/Cargo 1.97.1, cargo-llvm-cov 0.9.1, and matching LLVM 22.1.6 tools on x86_64 Linux under WSL2. It measured the parser-module extraction and coverage helper on top of revision `e04fbbd`; the recorded input hashes identify the measured worktree precisely.

| Maintained source | Library unit tests | Full active suite |
| --- | ---: | ---: |
| `src/core/eval.rs` | 334 / 415 | 378 / 415 |
| `src/core/grammar.rs` | 79 / 133 | 79 / 133 |
| `src/main.rs` | Outside library scope | 25 / 36 |
| **Total covered / executable lines** | **413 / 548 (75.36%)** | **482 / 584 (82.53%)** |
| Rust tests | 35 passed | 54 passed; 13 ignored |

The library unit result exceeds the intermediate 50% numerical target within its stated scope. CLI coverage comes from integration tests. A second clean unit capture after the full suite returned identical counts, confirming profile isolation for this run. Full-suite execution covered CLI source lines, confirming that subprocess profiles contributed. Separate uninstrumented debug and release suites also passed.

These are baseline measurements, not release-gate results: 13 known regressions remain unresolved, grammar-rule and branch coverage are unmeasured, and the measured operator code still has substantial gaps. Keep this initial record; later measurements should be recorded separately with their own source hashes and tool versions.

### Owned AST Measurement — 2026-09-26

[The AST capture](coverage-after-ast.json) records the expanded source scope: `ast.rs`, `eval.rs`, and `grammar.rs`, plus `main.rs` in the full suite. Library unit coverage is **686/774 lines (88.63%)** from 82 tests; the full suite covers **723/799 lines (90.49%)** from 132 active tests, with 16 pending tests ignored. All recorded input hashes matched the measured worktree. Source scope and implementation changed, so these percentages are not a like-for-like comparison with the initial baseline. Branch and grammar-rule coverage, pending behavior, and final release gates remain unmeasured or unfinished.

## Regression Status

`tests/regressions.rs` captures the originally confirmed DSL defects. All current cases are active and pass; run `cargo test --test regressions` or pass a test name to isolate one. Preserve these minimal reproductions. If a future unfixed case is temporarily ignored, document its reason and enable it with its fix; an ignored test is never evidence of a passing requirement.

All current `tests/language_contract.rs` expectations are active, including lexical lookup, invocation-local bindings and definitions, loop-variable restoration, return values, and control propagation. Run both suites with `--release` as well. The specification and roadmap still require broader conformance coverage; passing these cases alone does not satisfy every release gate.

The collection-access regression requires the selected value; its former temporary unsupported-error allowance has been removed. Evaluator and CLI tests verify [lookup errors](language.md#collection-access), preserved assignments, and failure status without a panic. A `Try` without `Catch` is rejected during parsing; the active regression, parser cases, and CLI fixture verify this [error-handling contract](language.md#trycatch). Whole-program control validation has AST, library, compatibility, and CLI checks; runtime guard tests deliberately bypass validation internally to exercise the defensive paths. Other regression expectations follow the accepted roadmap.

## CLI Failure Contract

Successful scripts, including errors handled by `Try/Catch`, exit with status `0`. File-read failures, syntax/control-placement errors, and uncaught evaluation errors exit with status `1` and write a diagnostic to stderr containing the input path. Invalid command-line arguments are rejected by clap. An uncaught runtime error stops execution before the following statement. Syntax and placement failures prevent all execution and debug traces; their diagnostics include line and column information.

The `invalid-control-*` fixtures cover unused and nested definitions, skipped branches, handlers, and exact source locations. The Unicode fixture intentionally uses CRLF; `.gitattributes` preserves those bytes and recognizes CRLF during whitespace checks.

## Continuous Integration

[CI configuration](../.github/workflows/ci.yml) runs on pushes to `main`, pull requests, and manual dispatch. Ubuntu jobs use the stable Rust toolchain and build/test both debug and release profiles. Formatting and Clippy run in the debug job. The checked-in `Cargo.lock` fixes dependency resolution for all jobs.

Run the same checks locally:

```sh
cargo +stable fmt --all -- --check
cargo +stable clippy --locked --all-targets -- -D warnings
cargo +stable build --locked --all-targets
cargo +stable test --locked
cargo +stable build --locked --all-targets --release
cargo +stable test --locked --release
```

Use `--offline` with build, test, and Clippy commands when dependencies are cached (before `--` for Clippy). Formatting needs no offline flag. Ignored regressions remain visible pending work; CI does not claim they pass. New stable-toolchain lints may require narrow maintenance fixes, which must retain behavioral tests.

The checkout action is pinned to the upstream v7.0.1 commit, verified against its tag. Configuration references: [checkout action](https://github.com/actions/checkout/tree/3d3c42e5aac5ba805825da76410c181273ba90b1) and [GitHub workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax). Local command and YAML checks establish configuration readiness; the first hosted run must be observed after publication before claiming hosted CI success.

Native registration is covered in `tests/native_registration.rs`: valid/invalid headers, collision preservation, argument order and failure boundaries, every return kind, recursive finite checks, typed errors, panic recovery, captured-state sharing, and lexical shadowing. Evaluator tests additionally instrument visits and defensive malformed-call/host-value boundaries. The native API example executes as a doctest.

Shared metadata is covered in `tests/signature_metadata.rs`, including 127 kind sets and both 49-pair argument/return matrices. Evaluator checks cover metadata visibility/restoration, CLI tests compare help output to the actual registry, and host corpus cases enforce the invocation boundary.

Call composition is covered in `tests/call_composition.rs`: nested effects, argument/type validation order, operator precedence, short-circuiting, collection/key snapshots, condition/loop/Return timing, recursion and cleanup, errors/rethrow, Unicode/layout, original spans, and Pair execution. Example `18`, CLI syntax/effect failures, a documentation snippet, and three corpus cases check observable behavior.

`tests/async_operations.rs` uses Tokio controlled time and explicit worker handshakes for cancellation/deadline and resource-lifetime checks. It covers async and blocking paths, shared capacity, pre-entry failures, dropped invocation behavior, typed diagnostics, panic conversion, and missing runtime configuration. Blocking handshakes have bounded waits; no external service is required.

`tests/local_imports.rs` builds isolated temporary module projects for relative/canonical paths, initialized-cache lifetime, failures/retries, preserved dependency caches, namespaces/collisions, lexical module isolation, imported-function paths, qualified metadata/call stacks, source release, and Unix symlink cases. Its CLI checks run from an unrelated directory; example `19` and multi-file corpus cases preserve exact output.

`tests/input_variables.rs` checks JSON/host/CLI input conversion, numeric boundaries and rounding, exact Unicode names, nesting, duplicate keys, ordered file/flag replacement, atomic installation, script/module/cloned scope, file failures, and pre-effect diagnostics. It also runs example `20` with its JSON defaults and README overrides; the conformance corpus records the I1 rule and rustdoc executes host installation.

`tests/embedded_runs.rs` checks fresh/reused/concurrent Engine execution, owned outcomes, per-run directory/environment overlays, source loading/imports, initial source/step/call-depth budgets, binding cleanup, cancellation/deadlines, callback causes, and clock consistency. Handshakes have five-second timeouts; controlled Tokio time checks relative deadline construction and expiry during a native callback without wall-clock timing thresholds.

`tests/syntax_limits.rs` exercises source/parser guard boundaries, high-depth and long-chain inputs, quotes/comments/Else-If context, direct Pest and Program APIs, local configuration isolation, imported source and reused programs, and CLI rejection before output/debug traces. CLI cases run with five-second process deadlines and check normal error exit rather than abort/panic.

`tests/output_limits.rs`, the output writer/budget unit tests, allocation observations, and R24 corpus cases check byte admission, complete flush semantics, partial writes, cancellation, and CLI/module accounting; see [output limits](output-limits.md).

`tests/isolated_workers.rs` exercises real Linux process supervision, pipe backpressure, forced stop, reaping, cancellation/abandonment, bounded history, and pool shutdown. Other platforms reject this worker API before process entry; see [isolated workers](isolated-workers.md).

[Nonblocking I/O validation](nonblocking-io.md) adds worker admission/draining tests,
real stalled-file and output-pipe probes, native thread/control checks, and F9 corpus cases.

[Parallel CLI validation](parallel-cli.md) covers bounded batch admission, fresh
variables/module caches/quotas, queued deadlines, stable run IDs, sibling failures,
whole output records, and reporter backpressure/failure. Linux tests use controlled
FIFO peers and pipe capacity; child watchdogs always terminate/reap on failure.
R32 adds two CLI corpus cases, and examples 21–22 run in the example suite.
The [concurrency policy checks](concurrency-policy-evidence.json) additionally
hold siblings active across a CLI failure and an embedded cancellation, verifying
continued admission, aggregate failure, independent control, and shared-operation
reservation release. These tests use explicit entry/release handshakes.

The [performance protocol](performance.md) separates correctness smoke checks
from full measurements. CI runs reduced workloads and native memory calibration
in each GNU/musl profile; the Python helper checks prevent incomplete or incorrect
samples from becoming successful measurement evidence.
