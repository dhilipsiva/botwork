# Extending Botwork

Botwork grows in five ways. The first four work today; adapters for other
languages are planned.

| Way | For | Trust | Status |
| --- | --- | --- | --- |
| [Botwork modules](#botwork-modules) | Reusable statements written in Botwork | Runs with the script's own limits | Available |
| [Rust statements](#rust-statements) | New built-ins in a program that embeds Botwork | Trusted, in-process | Available |
| [Listeners](#listeners) | Live integrations fed by execution events | A separate process | Available |
| [Editor packages](#editor-packages) | Highlighting and editor features | Editor plugins | Available |
| [Language adapters](#language-adapters) | Statements written in Python, JavaScript, or WASM | Depends on the adapter | Available |

## Botwork modules

A module is a `.botwork` file whose definitions another script imports:

```text
Import |"lib/math.botwork"| As |math|
Log |@{ math::Double |2| }|
```

Imports resolve relative to the importing file. A module initializes once per
run, in its own scope, and exports its root definitions and the qualified
statements it imports. Modules shared between projects travel as
[packages](packages.md), which scripts import as `@name/path`. See
[local modules](language.md#local-modules) and [import limits](import-limits.md).
`--check` checks the modules a script imports, and the
[language server](lsp.md) navigates into them.

## Rust statements

A Rust program that embeds the interpreter can register statements of its own.
It registers a header, such as `Uppercase |text|`, with a callback. Calls to
that header then run the callback with the evaluated arguments.

- **Synchronous statements** register with `register_native` or, with
  documented parameter kinds and errors, `register_native_with_signature`. See
  [native statements](interpreter-architecture.md#register-native-statements)
  and [shared signature metadata](interpreter-architecture.md#shared-signature-metadata).
- **Asynchronous statements** register a `NativeOperation` with
  `register_operation`. The operation cooperates with cancellation and
  deadlines. See [async execution](async-execution.md) and
  [nonblocking I/O](nonblocking-io.md).
- **Runs** go through `core::run::Engine`, which keeps the registrations and
  gives each run fresh state. See [embedded runs](embedded-runs.md).

Rust statements are trusted: they run in the host process with its permissions.
An unwinding panic in a callback becomes diagnostic BW4003 instead of ending
the process.

## Listeners

`--listener PROGRAM` streams execution events, as JSON Lines, to a program
that Botwork starts. A listener can feed dashboards, notifications, or other
test tools without changing Botwork. A slow or failing listener is detached;
it never changes a run's outcome. See [listeners](listeners.md).

## Editor packages

The Helix, Vim, and VS Code packages highlight Botwork and connect to the
language server. See [editor support](editors.md).

## Language adapters

The [roadmap decisions](decisions.md) fix these adapters' design for 1.0.
[WebAssembly statements](wasm.md) are part of the binary,
[Python statements](python.md) are available in builds with the `python`
feature, and [JavaScript statements](javascript.md) wherever Node is
installed.

| Adapter | Hosting | Runtime needed | Trust |
| --- | --- | --- | --- |
| WASM | Wasmtime, in-process, with the component model and WASI Preview 2 ([D8](decisions.md#d8-wasm-runtime)); see [WebAssembly statements](wasm.md) | None: part of the binary | Sandboxed: clocks and randomness only, with fuel, memory, and epoch limits |
| Python | PyO3, behind the optional `python` build feature ([D9](decisions.md#d9-python-packaging)); see [Python statements](python.md) | libpython 3.10 or later | Trusted, in-process |
| JavaScript | A Node process per call over the worker protocol ([D7](decisions.md#d7-javascript-hosting)); see [JavaScript statements](javascript.md) | Node.js | A separate process, ended at a stop |

The default binary stays a single file, WebAssembly support included. Python
support needs a python-enabled build, and JavaScript support needs Node on the
machine. All three pass one [conformance suite](adapter-conformance.md).
Only WebAssembly runs sandboxed; [extension trust](trust.md) sets out what
each kind of extension can reach.
