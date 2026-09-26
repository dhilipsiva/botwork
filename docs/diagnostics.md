# Structured Diagnostics

The CLI reports the failing source file, range, stable code, error, relevant source text, and repair guidance. A header such as `file.botwork:2:14-2:19: [BW2001]` uses one-based scalar positions with an exclusive end; an empty EOF range shows only its start. Runtime errors identify the innermost failing expression or access segment. Invalid conditions point to the condition, not the entire body. Entered calls appear innermost first, with call and definition locations. Native calls have no DSL definition.

## Stable Codes and Repairs

Use `BWErr::code()` or `Diagnostic::code()` to obtain `DiagnosticCode`; `as_str()` and Display return the stable identifier. Use `help()` for current guidance. Codes identify categories independently of wording, source locations, and call stacks. Existing identifiers will not be reassigned to a different meaning; new categories require new IDs. Rust's code enum is non-exhaustive so callers can handle future additions. Do not parse human-readable message wording as a protocol.

| Code | Category | Typical repair |
| --- | --- | --- |
| BW1001 | Syntax | Check the indicated token and matching delimiters/comment fences |
| BW1002 | Invalid control placement | Move Return into a custom body or loop control into its own invocation's loop |
| BW1003 | Duplicate parameter | Give parameters distinct case-sensitive names |
| BW2001 | Undefined variable | Define the correctly spelled/cased name in its lexical scope before use |
| BW2002 | Undefined statement | Define it before use and match punctuation, argument positions, and count |
| BW2003 | Duplicate statement | Rename/remove the same-scope duplicate; the original remains registered |
| BW2004 | Parameter count | Match the registered signature's parameter count |
| BW3001 | Numeric conversion/range | Use an i32 integer or finite f32 decimal with ASCII digits |
| BW3002 | Arithmetic | Check divisors, intermediate overflow, and non-finite operands/results |
| BW3003 | Incompatible type | Use supported operand kinds, boolean conditions, and array For iterables |
| BW3004 | Collection access | Check exact map keys and nonnegative array indexes within bounds |
| BW4001 | Output failure | Fix the destination and account for any bytes already written before retrying |

Ordinary source calls with the wrong arity normally fail signature resolution as BW2002; BW2004 represents a resolved signature/count mismatch. Numeric conversion remains a runtime error despite the legacy `ParsingIntegerError` name. CLI argument parsing and file-loading errors are outside this language-error catalog. Hints describe repairs without changing or automatically rerunning the script.

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

Spans retain shared source ownership after the input, parsed program, or context is dropped. `line_column()` and `end_line_column()` give the start and exclusive end. Columns count Unicode scalars; tabs count as one column and CRLF as one line ending. Syntax errors retain the parser's byte location, including an empty range at EOF. Validation precedes all execution/debug traces. Parser-pair calls name their original input `<input>`.

## Calls and Recovery

Call stacks are snapshots captured before unwinding. A custom call that fails during resolution or argument evaluation has not entered its body and adds no frame; enclosing callers remain visible. Native Log evaluates arguments inside its callback, so a failing Log argument includes that native frame. Normal completion, returns, and errors remove active frames; later failures cannot inherit them.

Try/Catch still handles evaluation failures only. A successful handler consumes its error and emits no diagnostic. If the handler fails, its failure is primary and the handled error remains in `causes`. Nested handler failures retain all original spans/stacks in handling order, innermost first. A captured diagnostic includes the current caller even if Catch handles it before that caller unwinds. `Display` renders causes; `std::error::Error::source()` exposes the first handled cause, or the underlying category error when there is none.

Each handled cause retains its own code and guidance; a handler's undefined-variable failure does not recategorize the original arithmetic failure. DSL inspection/rethrow, imports, adapter-cause compatibility, and bounded diagnostic resources remain separate roadmap work. Success output, failure status, argument order, and language scope/completion behavior retain their contracts.
