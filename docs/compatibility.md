# Compatibility

This page says what stays stable across Botwork releases, which platforms and
tools are supported, and how to check what an upgrade changes. The
[roadmap decisions](decisions.md) D5 and D15–D17 set these policies.

## Before 1.0

Botwork is at version 0.x. Until 1.0, a release may change the language, the
built-in statements, output formats, CLI options, and the Rust API. The version
fields in [output formats](reporting.md#versioned-formats) still change
whenever a format does, so tools can detect it. Record the Botwork revision you
test with, and run `botwork --check` over your scripts after upgrading. It
reports calls that no longer resolve and misplaced controls without running
anything.

## Stable at 1.0

From 1.0, these contracts are stable ([D15](decisions.md#d15-stable-contracts)).
Breaking one needs a new major version and migration notes.

| Contract | What stays stable | How changes are marked |
| --- | --- | --- |
| Language | Syntax, semantics, and the [conformance corpus](conformance-corpus.md) | Additions in minor versions; removals only in a major version |
| Built-in statements | Each signature, its argument and return kinds, and its documented errors ([statement reference](statements.md)) | New statements in minor versions. A new built-in conflicts with a script's root-level definition of the same signature (BW2003), so release notes name every new statement. |
| Diagnostic codes | Each `BW` code keeps its meaning ([diagnostics](diagnostics.md#stable-codes-and-repairs)) | New codes may be added; retired codes are never reused |
| Output formats | The JSON report, event stream, run records, and failed-case records ([reports and outputs](reporting.md)) | Each format's `version`, under the rules in its table |
| CLI | The [options](cli.md) and [exit statuses](cli.md#exit-statuses) | New options in minor versions |
| Rust API | The `botwork` crate's public API, as [`public-api.txt`](public-api.txt) lists it; hidden items are not part of it ([Rust API](rust-api.md)) | SemVer, with additions to `#[non_exhaustive]` types and to option structs allowed in minor versions |

## Versioned contracts

Everything Botwork writes for another release to read, and everything an
extension or package is built against, carries a version. Botwork reads the
versions in the table. It refuses others before acting on them, with a
message that names the version found, the versions it reads, and what to do.

| Contract | Current | Also reads | Another version | Fixtures |
| --- | --- | --- | --- | --- |
| Botwork, as `botwork --version` prints | 0.2.0 | | A project's or package's `botwork` requirement that excludes it fails `--fetch`, and any run that imports from the project, with a version incompatibility: upgrade Botwork, or depend on a version of the package that supports this one | `packages/project-needs-newer`, `packages/dependency-needs-newer` |
| [Package manifest](packages.md), `botwork.toml` | | | A key this Botwork does not know is an error, unless the manifest's `botwork` requirement excludes this Botwork: then the error is that requirement | `packages/newer-keys` |
| [Lockfile](packages.md#fetching), `botwork.lock` | 1 | | A newer lockfile: upgrade Botwork, or delete it and run `botwork --fetch` to lock the project again. Any other: delete it and fetch again | `packages/locked`, `packages/newer.lock` |
| [JSON report](json-report.md), `botwork-report` | 1 | | Botwork only writes reports. It replaces an existing report of any version | `outputs/report-v1.json` |
| [HTML report](html-report.md), `botwork-report-html` | 1 | | As for the JSON report | |
| [Event stream](listeners.md), `botwork-events` | 1 | | Botwork only writes the stream | `outputs/events-v1.jsonl` |
| [Run record](run-records.md), `botwork-run` | 1 | | A newer record: upgrade Botwork | `run-records/v1.json`, `run-records/v2.json` |
| Report journal, `botwork-report-journal` | 1 | | A newer journal: reconcile it with the Botwork that wrote it | `journals/v1`, `journals/v2` |
| [Failed-case record](suites.md#rerun-failed-cases), `botwork-failed-cases` | 2 | 1, [deprecated](#deprecations) | A newer record is neither read nor replaced: upgrade Botwork | `failed-cases/v1.json`, `failed-cases/v2.json`, `failed-cases/v3.json` |
| [Worker protocol](worker-protocol.md), `BWIP` | 1 | | BW5003, naming both versions: the worker and Botwork need the same protocol | `worker/response-v1.bin`, `worker/response-v2.bin` |
| [WebAssembly interface](wasm.md), `botwork:statements` | 0.1.0 | 0.1.x | BW6001, naming the component's version and the range this Botwork supports: rebuild the component against this Botwork's `wit/botwork.wit` | `wasm/statements-0.2.0.wasm` |

The other adapters have no versions of their own. The [JavaScript
host](javascript.md) ships inside Botwork and speaks the worker protocol.
[Python modules](python.md) read the running Botwork's version as
`botwork.__version__`.

### Deprecations

Botwork deprecates a version it still reads before it stops reading it.
Reading a deprecated version prints a `[deprecated]` line on stderr that says
how to replace it; the run is otherwise unchanged, exit status included.

| Deprecated | Replacement | Notice |
| --- | --- | --- |
| Failed-case record version 1 | Version 2, which `--failures` writes | `[deprecated] failed-case record PATH is version 1, which a later Botwork will stop reading; rewrite it as version 2 by also passing --failures PATH` |

### Compatibility fixtures

`tests/compatibility/` keeps a sample of each version in the table, and of
newer versions Botwork must refuse; its README says how each was made.
`tests/compatibility.rs` checks each:

- Botwork reads every sample of a version it reads, and refuses each other one
  with the message above.
- The JSON report has exactly its fixture's fields, and the event stream at
  least its fixture's fields, with the same types. A field change without a new
  version fails, and so does a new version without a new fixture.
- The rows of this table match the fixtures and the versions in the code.

## Rust toolchain

Botwork builds with the latest stable Rust release, and CI tests the newest
stable. No older toolchain is supported
([D16](decisions.md#d16-minimum-supported-rust-version)). `Cargo.toml` declares
no `rust-version`.

## Platforms

| Platform | Status |
| --- | --- |
| Linux x86_64, GNU and static musl | Supported and tested in CI: every test for both, in debug and release |
| macOS arm64 | Built and tested in CI, in debug and release. The process statements and the default worker pool run with Linux's process-group guarantees; the process-tree and PID-namespace worker modes are unavailable, since macOS cannot follow detached descendants ([D12](decisions.md#d12-platform-parity)) |
| Windows x86_64 | Built and tested in CI, in debug and release. The process statements and the default worker pool run each worker in a Job Object, which ends every descendant with it, the process-tree mode reports a tree reaped once its job is empty, and `with_recovery` keeps the worker journal ([D12](decisions.md#d12-platform-parity)) |

Process statements and the default worker pool run on every platform above,
and the process-tree mode on Linux and Windows; macOS refuses it at
construction. The PID-namespace mode is Linux-only, and the worker journal runs on
Linux and Windows.
Cooperative interruption works on all three: SIGINT and SIGTERM on Linux and
macOS, and Ctrl-C and Ctrl-Break on Windows. See
[worker platforms](worker-platforms.md) and
[terminal outcomes](terminal-outcomes.md#interruption).

## Tools and editors

| Tool | Supported | Tested |
| --- | --- | --- |
| Tree-sitter CLI, for the grammars | 0.25 | 0.25.9 in CI |
| VS Code | 1.91 or later | The grammars run through `vscode-textmate` in CI |
| Helix | 25.07 | 25.07.1 in CI |
| Vim | 9 | The version Ubuntu ships, in CI |
| Neovim | 0.11 or later | Configuration documented, not tested |
| Node.js, to build the VS Code extension | 22 | 22 in CI |
| Playwright, for [Playwright statements](playwright.md) | 1.63 | 1.63.0 with Chromium in CI |

See [editor support](editors.md) for the packages and how they are tested.

## Checking this page

`tests/reference_docs.rs` compares this page with its sources:

- the Tree-sitter, Helix, and Node.js versions with the CI workflow;
- the VS Code version with the extension's manifest;
- the toolchain policy with CI and `Cargo.toml`;
- the output formats in [reports and outputs](reporting.md) with the versions
  the code writes.
