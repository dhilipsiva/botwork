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

See [editor support](editors.md) for the packages and how they are tested.

## Checking this page

`tests/reference_docs.rs` compares this page with its sources:

- the Tree-sitter, Helix, and Node.js versions with the CI workflow;
- the VS Code version with the extension's manifest;
- the toolchain policy with CI and `Cargo.toml`;
- the output formats in [reports and outputs](reporting.md) with the versions
  the code writes.
