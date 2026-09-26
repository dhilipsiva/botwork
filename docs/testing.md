# Testing Botwork

Run `cargo test` from the repository root. Run `cargo test --release` to check the optimized build. Add `--offline` when dependencies are already cached.

- `src/core/grammar/tests.rs` checks program parsing and typed operators.
- `src/core/eval/tests.rs` checks evaluation, state, conditions, collections, and error handling.
- `tests/cli.rs` invokes Cargo's built CLI and checks exit status, stdout, and stderr independently. Inputs live under `tests/fixtures/`.
- `tests/examples.rs` checks the exact expected stdout of both bundled examples, plus successful status and empty stderr. Expected results are derived from each script's operations; update them only after reviewing an intentional behavior change.

Tests assert language behavior and error categories rather than Rust source line numbers or map iteration order. No external services or extra testing crates are required for the initial suite. Add a minimal regression before fixing a known defect; do not preserve defective behavior as an expected result.

Run `cargo test --test examples` to check the expression and syntax demonstrations alone. The syntax check includes every intended `While` iteration (`3`, `4`, `5`, `6` before the break), both `For` behaviors, and handled errors. The expression check preserves intentional whitespace inside strings and verifies deterministic map display without relying on internal hash-map iteration order.

The initial suite does not establish complete language conformance or the roadmap's final coverage targets. Track unfinished regression and validation work in [TODO.md](../TODO.md).

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

Coverage includes executable lines in maintained Rust source files. It excludes test files and the isolated Pest-generated parser in `src/core/parser.rs`; handwritten operators in `grammar.rs` remain included. Other derives, such as clap and thiserror, can contribute mapped lines. Library-only coverage excludes `main.rs`; full-suite coverage includes it. Module-only files have no executable lines. Review the expected source-file lists in `scripts/coverage.py` whenever adding code; mismatches fail collection.

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

## Known Defects

`tests/regressions.rs` captures intended behavior for the confirmed DSL defects. Each unfixed case is explicitly ignored with a reason so the ordinary suite reports pending work. Run `cargo test --test regressions -- --ignored` to reproduce those failures, or pass a test name to isolate one. Enable each case in the commit that fixes it; an ignored test is never evidence of a passing requirement.

The active collection-access regression temporarily permits the typed `UnsupportedAccessError` until access is implemented; arbitrary errors do not satisfy it. Evaluator and CLI tests verify [catchable unsupported access](language.md#collection-access-status), preserved assignments, and failure status without a panic. A `Try` without `Catch` is rejected during parsing; the active regression, parser cases, and CLI fixture verify this [error-handling contract](language.md#trycatch). Other regression expectations follow the accepted roadmap. The parameter-scope reproducers deliberately isolate argument binding from the separate final-return defect.

## CLI Failure Contract

Successful scripts, including errors handled by `Try/Catch`, exit with status `0`. File-read failures, syntax errors, and uncaught evaluation errors exit with status `1` and write a diagnostic to stderr containing the input path. Invalid command-line arguments are rejected by clap. An uncaught runtime error stops execution before the following statement. Syntax diagnostics retain the parser's line and column information.

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
