# Repository Guidelines

## Project Structure & Module Organization

Botwork is a Rust 2021 automation framework with a library and CLI in one Cargo package.

- `src/main.rs`: CLI argument parsing, file loading, and execution.
- `src/lib.rs` and `src/core/mod.rs`: public module exports.
- `src/core/grammar.pest`: Pest language grammar.
- `src/core/grammar.rs`: parser setup, operator precedence, literals, and errors.
- `src/core/eval.rs`: execution context, statements, and expression evaluation.
- `examples/*.botwork`: runnable language examples; use numbered names such as `03-feature.botwork` for additions.
- `editors/tree-sitter-botwork/`: Tree-sitter grammars for scripts and suites; see `docs/tree-sitter.md`.
- `src/core/language.rs` and `src/lsp.rs`: editor analysis and the `--lsp` language server; see `docs/lsp.md`.

There is currently no separate test or asset directory.

## Build, Test, and Development Commands

Run commands from the repository root:

- `cargo build`: compile the library and CLI; add `--release` for an optimized binary.
- `cargo check`: check compilation without producing an executable.
- `cargo run -- --file examples/02-syntaxes.botwork`: execute the syntax demonstration.
- `cargo run -- --file examples/01-expressions.botwork`: exercise expressions and literals.
- `cargo test`: run Rust unit, integration, and documentation tests as they are added.
- `cargo fmt --all`: format Rust code with rustfmt.
- `cargo clippy --all-targets`: run standard Rust lint checks.

No custom rustfmt or Clippy configuration is checked in.

## Coding Style & Naming Conventions

Use four-space indentation in Rust and let rustfmt handle layout. Use `snake_case` for functions, modules, and variables; `PascalCase` for types; and `SCREAMING_SNAKE_CASE` for constants. Match existing grammar rule names such as `stmt_assign` and `param_invoke`.

Keep grammar, precedence, and evaluator changes coordinated. Update examples when syntax changes, preserving case-insensitive statement names and case-sensitive variables.

## Testing Guidelines

No automated tests are currently checked in. Use Rust's built-in `#[test]` framework for new tests, with unit tests in `#[cfg(test)] mod tests` beside the implementation and integration tests under `tests/`. Name tests after behavior, such as `rejects_undefined_variables`.

Cover parsing failures, operator precedence, type combinations, and control flow when relevant. Run `cargo test` and affected examples. The README's goal of at least 50% unit-test coverage is a roadmap item, not an enforced threshold.

## Commit & Pull Request Guidelines

Follow the history's short, action-oriented commit subjects, such as `validate keywords` or `update docs`. Keep each commit focused.

For pull requests, describe the behavior change, link related issues when available, and list validation performed. Include a small `.botwork` input and expected output for language changes; update the README or examples when user-facing behavior changes.
