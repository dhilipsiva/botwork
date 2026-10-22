# Contributing to botwork

Thank you for helping. Bug reports, documentation fixes, examples, and code are
all welcome.

## Reporting a problem

Open an issue with the botwork version (`botwork --version`), your platform, the
smallest script or suite that shows the problem, the command you ran, and what
you expected. Diagnostics carry a stable code such as `BW9001`; include the
whole diagnostic.

## Building and testing

botwork builds with the Rust that `Cargo.toml`'s `rust-version` names, or
newer ([D20](docs/decisions.md#d20-license-and-minimum-rust-version)). From a
checkout:

```sh
cargo build
cargo test --locked --no-fail-fast
```

Before sending a change, run the checks CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --no-fail-fast
```

With Nix, `nix develop --command just ci` runs the same checks with CI's pinned
toolchain; `just --list` shows the other recipes.

Some tests need Linux facilities (user namespaces, `/proc`) and fixed tool
paths such as `/usr/bin/printf`; [testing](docs/testing.md) explains them and
the optional campaigns: coverage, mutation testing, fuzzing, and performance
budgets. [AGENTS.md](AGENTS.md) describes the repository layout.

## Making a change

- Add a test that fails without your change. Unit tests live in a `tests.rs`
  module beside the code, and integration tests under `tests/`.
- Update the documentation the change affects. Every `botwork` and
  `botwork-suite` example in the README and `docs/` runs as a test: give a new
  one a `<!-- botwork-test: id -->` marker and register its expected output in
  `tests/documentation.rs`. A new page goes into the
  [documentation index](docs/README.md).
- A language change updates the [syntax reference](docs/syntax.md), the
  Tree-sitter grammars under `editors/`, and the examples.
- A change to the public Rust API updates `docs/public-api.txt`; see
  [Rust API](docs/rust-api.md).
- Larger changes follow [TODO.md](TODO.md) and the
  [roadmap decisions](docs/decisions.md). Open an issue first if the change
  departs from them.

## Commits and pull requests

Write short, lowercase, action-oriented commit subjects, such as
`record browser matrix evidence`, and keep each commit focused. A pull request
describes the behavior change, links related issues, and lists the validation
you ran. For a language change, include a small `.botwork` input and its
expected output. CI runs on every pushed branch and every pull request.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in botwork by you, as defined in the Apache-2.0 license, shall be
dual licensed as in the [README](README.md#license), without any additional
terms or conditions.
