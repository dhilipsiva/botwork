# Changelog

All notable changes to Botwork are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Before 1.0 a minor version may break
the language, the built-in statements, output formats, the CLI and the Rust API
([compatibility](docs/compatibility.md#before-10)).

## [Unreleased]

## [0.3.0] - Unreleased

The first release of the current implementation. The `botwork` 0.2.0 on
crates.io is a prototype published in April 2023; it shares the name and the
idea (sentence-style scripts, one binary) and little else. The version moves to
0.3.0, not 1.0, because the stable contracts in
[compatibility](docs/compatibility.md) begin at 1.0.

### Added

- The language: sentence statements with parameters, Unicode names, suites,
  cases and fixtures, `Eventually` and `Retry`, owned cleanup with `Finally`,
  datasets, input variables and secret inputs ([syntax](docs/syntax.md),
  [language](docs/language.md), [specification](docs/language-specification.md)).
- Built-in statements for collections, strings, dates and times, JSON and CSV,
  the operating system, processes and HTTP, with assertions that report their
  full operands ([statements](docs/statements.md)).
- Browser and device automation through WebDriver, Playwright and Appium
  ([WebDriver](docs/webdriver.md), [Playwright](docs/playwright.md),
  [Appium](docs/appium.md)).
- Extensions in Rust, Python (`--features python`), JavaScript through a Node
  worker, and WASI components ([extending](docs/extending.md)).
- `--check`, which finds mistakes without running; `--format`; and `--lsp`, a
  language server, with highlighting for Helix, Vim and VS Code
  ([editors](docs/editors.md)).
- Reports: a versioned JSON report, an event stream, run records and
  failed-case records ([reports and outputs](docs/reporting.md)), with stable
  diagnostic codes ([diagnostics](docs/diagnostics.md)).
- Parallel runs from the command line ([parallel runs](docs/parallel-cli.md)).
- Release packaging for Linux (static musl), macOS and Windows, a VS Code
  extension, and the channel manifests ([distribution](docs/distribution.md)).

### Changed

- Botwork is licensed under MIT OR Apache-2.0, at your option; 0.2.0 was MIT
  alone ([D20](docs/decisions.md#d20-license-and-minimum-rust-version)).
- `Cargo.toml` declares `rust-version = "1.99"`, the release series of the stable
  Rust this release is tested with; earlier, the policy was the latest stable,
  undeclared.

[Unreleased]: https://github.com/dhilipsiva/botwork/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/dhilipsiva/botwork/releases/tag/v0.3.0
