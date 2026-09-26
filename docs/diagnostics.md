# Structured Diagnostics

The CLI reports the failing source file, one-based line/column, error, and relevant source text. Runtime expression errors identify the innermost failing expression; access errors identify the failed segment. Invalid conditions point to the condition, not the entire body. Entered calls appear innermost first, with call and definition locations. Native calls have no DSL definition.

## Rust API

`core::diagnostic::Diagnostic` contains:

| Field | Meaning |
| --- | --- |
| `error` | Boxed original `BWErr`, retaining its category and details |
| `span` | Original UTF-8 source range, with source name, byte offsets, text, and scalar line/column |
| `label` | Whether the range is an expression or other source syntax |
| `call_stack` | Entered signatures, call sites, and optional definition sites |
| `related` | Associated locations, such as the first conflicting definition/parameter |
| `causes` | Errors being handled when this failure occurred, with their original spans/stacks |

Use `Program::parse_detailed`, `Program::validate_detailed`, `evaluate_program_detailed`, `execute_statement_detailed`, or `botwork_detailed`. Each returns `DiagnosticResult<T>`. The [execution API example](interpreter-architecture.md#execution-api) is checked by rustdoc.

Original methods without `_detailed` retain their `BWErr` results and discard diagnostic context at the public boundary. `Diagnostic::into_error()` makes that conversion explicit. The value-level operator API also retains `BWErr`, since it has no source tree.

Spans retain shared source ownership after the input, parsed program, or context is dropped. Columns count Unicode scalars; tabs count as one column and CRLF as one line ending. Syntax errors retain the parser's byte location, including an empty range at EOF. Validation precedes all execution/debug traces. Parser-pair calls name their original input `<input>`.

## Calls and Recovery

Call stacks are snapshots captured before unwinding. A custom call that fails during resolution or argument evaluation has not entered its body and adds no frame; enclosing callers remain visible. Native Log evaluates arguments inside its callback, so a failing Log argument includes that native frame. Normal completion, returns, and errors remove active frames; later failures cannot inherit them.

Try/Catch still handles evaluation failures only. A successful handler consumes its error and emits no diagnostic. If the handler fails, its failure is primary and the handled error remains in `causes`. Nested handler failures retain all original spans/stacks in handling order, innermost first. A captured diagnostic includes the current caller even if Catch handles it before that caller unwinds. `Display` renders causes; `std::error::Error::source()` exposes the first handled cause, or the underlying category error when there is none.

Stable diagnostic codes, repair suggestions, DSL inspection/rethrow, imports, adapter causes, and bounded diagnostic resources remain separate roadmap work. Success output, failure status, argument order, and language scope/completion behavior retain their contracts.
