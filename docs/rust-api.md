# The Rust API

The `botwork` crate's embedding API follows SemVer from 1.0
([D15](decisions.md#d15-stable-contracts)). This page says what that API is,
how it may change in a minor release, and how a change to it is reviewed.

## What the API is

The API is exactly what [`public-api.txt`](public-api.txt) lists: every public
module, type, field, variant, function, constant, and trait implementation, by
each path that reaches it. `scripts/public_api.py` writes the listing from
rustdoc's JSON output, and CI fails when the crate's surface and the listing
differ, or when a public signature names something outside the listing.

Everything lives under `botwork::core`:

| Module | For |
| --- | --- |
| `run` | `Engine`, `RunOptions`, `RunResult`, and the limits a run enforces |
| `eval` | `Context` and the `evaluate_program` family, for hosts that manage contexts themselves |
| `ast` | `Program`, parsed and validated source, with `SourceFile` and `Span` |
| `suite` | Suites, cases, selections, and datasets |
| `grammar` | `Literal`, the value type, and `BWErr`, the error kinds |
| `diagnostic` | `Diagnostic`, `DiagnosticCode`, rendering, and diagnostic limits |
| `operation`, `signature` | Native statements: `NativeOperation`, `OperationControl`, `StatementSignature` |
| `worker` | `WorkerPool`, its worker protocol, and the worker journal |
| `report`, `listener`, `acceptance` | Run records, events, listeners, and outcomes |
| `analysis`, `format` | `--check` analysis and the formatter |
| `input`, `secret`, `paths` | Variable files, secret redaction, and path display |
| `ast_limits`, `syntax_limits`, `value_limits` | Source, syntax, and value limits |

### Internals

Items marked `#[doc(hidden)]` are not part of the API, and may change or go in
any release. They are public only because the `botwork` executable and the
tests, which are separate crates, use them:

- the syntax tree: `Program`'s statements and the node types in `ast`;
- the Pest parser: `grammar::BWParser`, `grammar::Rule`, and the `Operate`
  trait;
- statement-level evaluation: `eval::execute_statement` and `eval::botwork`,
  with their `_detailed` forms, and `Context::signature_for_call`;
- `language`, the editor analysis behind `--lsp`;
- the ceilings and defaults the CLI applies, such as `run::DEFAULT_STEPS`.

## How it can change

A minor release may add to the API. It may also:

- **Add variants to `#[non_exhaustive]` enums.** Every public enum except
  `format::SourceKind` is one, from `DiagnosticCode` and `BWErr` to `Literal`,
  `CaseStatus`, and `WorkerOutcome`. Match them with a wildcard arm.
- **Add fields to `#[non_exhaustive]` structs.** These are what the library
  produces, such as `RunResult`, `Diagnostic`, `WorkerReport`, and
  `analysis::Finding`. Read their fields, and leave constructing them to the
  library.
- **Add fields to option and limit structs.** These implement `Default`:
  `RunOptions`, `RunLimits` and every other `*Limits` type, `RecordOptions`,
  `ListenerOptions`, `Selection`, `RunIdentity`, and `WorkerProtocol`. Build
  them with struct update syntax, as Botwork's own examples do:

  ```rust
  use botwork::core::run::{RunLimits, RunOptions};
  let options = RunOptions {
      limits: RunLimits { steps: 1_000, ..RunLimits::default() },
      ..RunOptions::default()
  };
  # let _ = options;
  ```

  Cargo's SemVer rules count a new public field on such a struct as a breaking
  change, because a struct expression that names every field stops compiling.
  Botwork accepts that cost for these types only: marking them
  `#[non_exhaustive]` would forbid struct update syntax outside the crate, the
  way options are written throughout the documentation and in the hundreds of
  places the tests set them.

The remaining structs keep their fields until a major release:
`worker::WorkerCommand`, which callers construct; `diagnostic::CallFrame` and
`diagnostic::RelatedLocation`, which callers attach to diagnostics they build;
and the run records in `report`, which mirror the `botwork-run` records of the
versioned, closed [JSON report schema](json-report.md) and change only with
its version.

## Reviewing a change

A pull request that changes the API updates `docs/public-api.txt`, so the diff
shows the change. To regenerate it and check for leaks, with the toolchain CI
uses:

```sh
cargo +nightly-2026-08-14 rustdoc --lib --locked -- -Z unstable-options --output-format json
python3 scripts/public_api.py --leaks target/doc/botwork.json
python3 scripts/public_api.py target/doc/botwork.json > docs/public-api.txt
```

Before adding a public item, decide which of the groups above it belongs to.
A new enum or produced struct gets `#[non_exhaustive]`; a new options struct
implements `Default`; and something only the CLI or the tests need gets
`#[doc(hidden)]`.
