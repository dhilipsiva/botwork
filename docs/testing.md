# Testing Botwork

Run `cargo test` from the repository root. Run `cargo test --release` to check the optimized build. Add `--offline` when dependencies are already cached.

- `src/core/grammar/tests.rs` checks program parsing and typed operators.
- `src/core/eval/tests.rs` checks evaluation, state, conditions, collections, and error handling.
- `tests/cli.rs` invokes Cargo's built CLI and checks exit status, stdout, and stderr independently. Inputs live under `tests/fixtures/`.
- `tests/examples.rs` checks the exact expected stdout of both bundled examples, plus successful status and empty stderr. Expected results are derived from each script's operations; update them only after reviewing an intentional behavior change.

Tests assert language behavior and error categories rather than Rust source line numbers or map iteration order. No external services or extra testing crates are required for the initial suite. Add a minimal regression before fixing a known defect; do not preserve defective behavior as an expected result.

Run `cargo test --test examples` to check the expression and syntax demonstrations alone. The syntax check includes every intended `While` iteration (`3`, `4`, `5`, `6` before the break), both `For` behaviors, and handled errors. The expression check preserves intentional whitespace inside strings and verifies deterministic map display without relying on internal hash-map iteration order.

The initial suite does not establish complete language conformance or the roadmap's final coverage targets. Track unfinished regression and validation work in [TODO.md](../TODO.md).

## Known Defects

`tests/regressions.rs` captures intended behavior for the confirmed DSL defects. Each unfixed case is explicitly ignored with a reason so the ordinary suite reports pending work. Run `cargo test --test regressions -- --ignored` to reproduce those failures, or pass a test name to isolate one. Enable each case in the commit that fixes it; an ignored test is never evidence of a passing requirement.

The collection-access regression temporarily permits a specific unsupported-access diagnostic until access is implemented; arbitrary errors do not satisfy it. A `Try` without `Catch` is specified as invalid syntax: write the statements directly when no error handling is intended. Other regression expectations follow the accepted roadmap. The parameter-scope reproducers deliberately isolate argument binding from the separate final-return defect.

## CLI Failure Contract

Successful scripts, including errors handled by `Try/Catch`, exit with status `0`. File-read failures, syntax errors, and uncaught evaluation errors exit with status `1` and write a diagnostic to stderr containing the input path. Invalid command-line arguments are rejected by clap. An uncaught runtime error stops execution before the following statement. Syntax diagnostics retain the parser's line and column information.
