# Testing Botwork

Run `cargo test` from the repository root. Run `cargo test --release` to check the optimized build. Add `--offline` when dependencies are already cached.

- `src/core/grammar/tests.rs` checks program parsing and typed operators.
- `src/core/eval/tests.rs` checks evaluation, state, conditions, collections, and error handling.
- `tests/cli.rs` invokes Cargo's built CLI and checks exit status, stdout, and stderr independently. Inputs live under `tests/fixtures/`.

Tests assert language behavior and error categories rather than Rust source line numbers or map iteration order. No external services or extra testing crates are required for the initial suite. Add a minimal regression before fixing a known defect; do not preserve defective behavior as an expected result.

The initial suite does not establish complete language conformance or the roadmap's final coverage targets. Track unfinished regression and validation work in [TODO.md](../TODO.md).
