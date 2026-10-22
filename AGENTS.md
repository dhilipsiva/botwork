# Repository Guidelines

## Project Structure & Module Organization

Botwork is a Rust 2021 automation framework: a library and the `botwork` CLI in
one Cargo package, built with the latest stable Rust
([D16](docs/decisions.md#d16-minimum-supported-rust-version)).

The CLI (`src/`):

- `src/main.rs`: argument parsing and dispatch to the modes below.
- `src/batch.rs`: `--file` batches and `--jobs` admission.
- `src/suites.rs` and `src/suites/`: suite discovery, datasets, case execution,
  and the failure history behind `--failures` and `--rerun-failed`.
- `src/report_json.rs` and `src/report_json/`: the JSON report, the HTML report
  (`html.rs`), and the journal `--reconcile-report` finishes.
- `src/listener.rs`, `src/check.rs`, `src/formatting.rs`, `src/lsp.rs`,
  `src/secrets.rs`, `src/interrupt.rs`, `src/assertion_artifacts.rs`, and
  `src/atomic_file.rs`: `--listener`, `--check`, `--format`, `--lsp`, secret
  inputs, Ctrl-C and SIGTERM, assertion artifacts, and locked report outputs.

The library (`src/lib.rs` and `src/core/`):

- `src/lib.rs` and `src/core/mod.rs`: the public API; `docs/public-api.txt`
  lists it. `src/lib.rs` includes the Rust examples of the documentation pages
  as doctests.
- `src/core/grammar.pest`, `grammar.rs`, and `parser.rs`: the Pest grammar,
  operator precedence, and literals. `ast.rs` and `ast/` hold the owned syntax
  tree with original source locations.
- `src/core/eval.rs` and `eval/`: the evaluator and the built-in statements.
  `eval/imports/` holds the adapters: WebAssembly (`wasm.rs`), Python
  (`python.rs`), JavaScript (`javascript.rs` and `javascript_host.mjs`),
  Playwright, and WebDriver.
- `src/core/run.rs` and `run/`: fresh runs and their resource budgets
  (retained values, definitions, names, registry, snapshots, temporaries,
  results, output, imports, cleanup, attempts, and blocking I/O).
- `src/core/worker.rs` and `worker/`: supervised external workers, with the wire
  protocol, the supervisor, and the journal.
- `src/core/packages.rs` and `packages/`: `botwork.toml` manifests, the lock
  file, resolution, downloads, and the cache.
- `src/core/suite.rs`, `acceptance.rs`, `report.rs`, `listener.rs`,
  `diagnostic.rs`, `signature.rs`, `secret.rs`, `analysis.rs`, `language.rs`,
  `format.rs`, and the `*_limits.rs` files: suite identity, the acceptance
  verdict policy, run results and events, listener delivery, diagnostics,
  statement signatures, secret masking, static checks, editor analysis,
  canonical layout, and admission limits.

Everything else:

- `tests/`: integration tests, one file per feature, sharing
  `tests/cli_harness.rs` and `tests/support/`. `tests/conformance/` is the
  rule-indexed corpus, `tests/doc-examples/` holds the expected output of
  documented examples, and `tests/*_tools.py` test the helper scripts.
  Unit tests live in `tests.rs` modules beside the code.
- `examples/`: runnable scripts and suites with numbered names, such as
  `33-processes.botwork`; `tests/examples.rs` checks their exact output.
- `docs/`: one page per feature, indexed by `docs/README.md`, with
  `docs/decisions.md` recording the roadmap decisions. Each `*-evidence.json`
  beside a page is its validation record.
- `scripts/`: Python helpers for coverage (`coverage.py`), mutation testing
  (`mutation_core.py`), fuzzing (`fuzz_smoke.py`), performance budgets
  (`performance.py`), the public API listing (`public_api.py`), releases
  (`release.py`), and the WebAssembly test guest (`wasm_guest.py`).
- `fuzz/`: a separate cargo-fuzz package with its own lockfile, targets for
  parsing, expressions, and literals, and seed corpora.
- `benches/runtime/`: the performance workloads and `budgets.json`.
- `wit/botwork.wit`: the WIT interface for WebAssembly statements.
- `usability/`: the usability study kit for
  [D2](docs/decisions.md#d2-usability-sessions).
- `editors/`: the Tree-sitter grammars and the Helix, Vim, and VS Code
  packages, tested against `editors/test`.
- `flake.nix` and `flake.lock`: the dev shells, `default` (stable) and
  `nightly`; `tests/dev_shell.rs` keeps their versions equal to CI's and the
  scripts'. The `Justfile` holds the local recipes.
- `clippy.toml` sets `large-error-threshold = 129`, because `Diagnostic` is a
  public 128-byte value. `.cargo/config.toml` packs relative relocations on GNU
  Linux, and `.cargo/mutants.toml` configures cargo-mutants.
- `TODO.md`: the roadmap to 1.0, one checklist item per requirement.

## Build, Test, and Development Commands

Run commands from the repository root. `nix develop` (`flake.nix`) provides the
pinned toolchains and tools without rustup, and the `Justfile` runs CI's Linux
checks: `just check` before the tests, `just ci` with them, and `just coverage`,
`just mutants`, `just fuzz-smoke`, `just public-api`, and `just release-dry-run`
for the rest. These are the checks CI runs:

- `cargo fmt --all -- --check`: formatting.
- `cargo clippy --locked --all-targets -- -D warnings`: lints; add
  `--features python` for the Python adapter.
- `cargo test --locked --no-fail-fast`: the unit, integration, documentation,
  and doctests, 2,213 tests on 2026-10-22 (`cargo test -- --list` counts them).
  CI runs them in debug and release for GNU and static musl Linux, and on macOS
  arm64 and Windows x86_64.
- `python3 tests/<name>_tools.py`: each helper script's tests.
- `python3 scripts/wasm_guest.py check`: the committed WebAssembly component
  matches its sources.
- `cargo run -- --file examples/02-syntaxes.botwork`: run one example.

The worker, process, and namespace tests need Linux facilities and fixed tool
paths such as `/usr/bin/printf`; [testing](docs/testing.md) and the
[platform matrix](docs/worker-platforms.md) list them. Missing facilities fail
those tests rather than skip them.

## Coding Style & Naming Conventions

Use four-space indentation in Rust and let rustfmt handle layout. Use
`snake_case` for functions, modules, and variables; `PascalCase` for types; and
`SCREAMING_SNAKE_CASE` for constants. Match existing grammar rule names such as
`stmt_assign` and `param_invoke`. Name tests after the behavior they check,
such as `rejects_undefined_variables`.

Keep grammar, precedence, and evaluator changes coordinated, and update the
syntax reference, the Tree-sitter grammars, and the examples with them. Statement
names stay case-insensitive and variables case-sensitive.

## Testing Guidelines

Every behavior change needs a test that fails without it. Cover parsing
failures, precedence, value kinds, control flow, limits, and cancellation where
they apply, and run the affected suites before the full one.

Documentation is tested too:

- A `botwork` or `botwork-suite` fence needs a `<!-- botwork-test: id -->`
  marker and its expected output registered in `tests/documentation.rs`.
- `docs/README.md` must link every page, and the reference pages' links must
  resolve (`tests/reference_docs.rs`).
- Changing the public API means regenerating `docs/public-api.txt`
  ([Rust API](docs/rust-api.md)).

A finished `TODO.md` item records its evidence. Its page gets a
`docs/<page>-evidence.json` record: the base revision, the environment, the
validation runs (GNU and musl, debug and release), the helper checks, the
mutation results, observations, and the boundaries it does not cover. The
`TODO.md` entry links it. `tests/traceability.rs` builds `docs/traceability.md`
from these records; regenerate it with
`BOTWORK_UPDATE_TRACEABILITY=1 cargo test --test traceability`. Evidence
records stay out of the published crate.

## Commit & Pull Request Guidelines

Follow the history's short, lowercase, action-oriented subjects, such as
`record browser matrix evidence` or `verify every browser integration's failure
paths`, and keep each commit focused.

For pull requests, describe the behavior change, link related issues when
available, and list the validation performed. Language changes include a small
`.botwork` input with its expected output. See [CONTRIBUTING.md](CONTRIBUTING.md).
