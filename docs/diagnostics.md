# Structured Diagnostics

The CLI reports the failing source file, range, stable code, error, relevant source text, and repair guidance. A header such as `file.botwork:2:14-2:19: [BW2001]` uses one-based scalar positions with an exclusive end; an empty EOF range shows only its start. Runtime errors identify the innermost failing expression or access segment. Invalid conditions point to the condition, not the entire body. Entered calls appear innermost first, with call and definition locations. Native calls have no DSL definition.

## Stable Codes and Repairs

Use `BWErr::code()` or `Diagnostic::code()` to obtain `DiagnosticCode`; `as_str()` and Display return the stable identifier. Use `help()` for current guidance. Codes identify categories independently of wording, source locations, and call stacks. Existing identifiers will not be reassigned to a different meaning; new categories require new IDs. Rust's code enum is non-exhaustive so callers can handle future additions. Do not parse human-readable message wording as a protocol.

| Code | Category | Typical repair |
| --- | --- | --- |
| BW1001 | Syntax | Check the indicated token and matching delimiters/comment fences |
| BW1002 | Invalid control placement | Move Return into a custom body or loop control into its own invocation's loop |
| BW1003 | Duplicate parameter | Give parameters distinct case-sensitive names |
| BW1004 | Invalid signature metadata | Use declared parameter names and unique, nonempty error documentation |
| BW2001 | Undefined variable | Define the correctly spelled/cased name in its lexical scope before use |
| BW2002 | Undefined statement | Define it before use and match punctuation, argument positions, and count |
| BW2003 | Duplicate statement | Rename/remove the same-scope duplicate; the original remains registered |
| BW2004 | Parameter count | Match the registered signature's parameter count |
| BW3001 | Numeric conversion/range | Use an i32 integer or finite f32 decimal with ASCII digits |
| BW3002 | Arithmetic | Check divisors, intermediate overflow, and non-finite operands/results |
| BW3003 | Incompatible type | Use supported operand kinds, boolean conditions, and array For iterables |
| BW3004 | Collection access | Check exact map keys and nonnegative array indexes within bounds |
| BW4001 | Output failure | Fix the destination and account for any bytes already written before retrying |
| BW4002 | Native operation failure | Check the operation's requirements and completed effects before retrying |
| BW4003 | Native callback panic | Fix the callback and inspect captured host state before reuse |
| BW5001 | Operation cancellation | Inspect completed effects; use fresh control for an intentional retry |
| BW5002 | Operation deadline | Inspect completed effects and choose an appropriate new deadline |
| BW5003 | Async runtime failure | Keep a Tokio runtime with time enabled alive through operation completion |
| BW6001 | Module loading | Check the source-relative local .botwork path, permissions, and UTF-8 contents/path |
| BW6002 | Import cycle | Move shared definitions into a module outside the reported cycle |
| BW6003 | Namespace collision | Use a distinct alias or remove the same-scope qualified declaration |
| BW7001 | Input variables | Use exact variable names and JSON with checked i32/finite f32 values and at most 128 nested containers |
| BW7002 | Run configuration | Use an existing working directory, valid environment names/values, a representable timeout, and supported syntax/AST/evaluation/import ceilings |
| BW7003 | Entry source loading through Engine | Supply a readable UTF-8 file relative to the run directory |
| BW8001 | Source/runtime resource limit | Reduce the workload or adjust configurable budgets within documented ceilings |

Ordinary source calls with the wrong arity normally fail signature resolution as BW2002; BW2004 represents a resolved signature/count mismatch. Numeric conversion remains a runtime error despite the legacy `ParsingIntegerError` name. CLI argument parsing and entry-script file-loading errors are outside this language-error catalog. Variable-file loading/conversion uses BW7001 before execution, with origin/path/JSON position in `details.reason` and no DSL source span or call stack. Hints describe repairs without changing or automatically rerunning the script.

## Rust API

Engine configuration/source errors use `details.reason`. BW8001 exposes `details.resource` and a decimal-string `details.limit`; source ranges and call frames are included when execution has entered source syntax. Run cancellation/deadline/resource exhaustion bypasses DSL handlers and preserves interpreter binding/frame cleanup. See [embedded runs](embedded-runs.md) for classification and cooperative limits.

`core::diagnostic::Diagnostic` contains:

| Field | Meaning |
| --- | --- |
| `error` | Shared immutable `Arc<BWErr>`, retaining its category, details, and identity |
| `span` | Original UTF-8 source range, with source name, byte offsets, text, and scalar line/column |
| `label` | Whether the range is an expression or other source syntax |
| `call_stack` | Entered signatures, call sites, and optional definition sites |
| `related` | Associated locations, such as the first conflicting definition/parameter |
| `causes` | Errors being handled when this failure occurred, with their original spans/stacks |

Use `Program::parse_detailed`, `Program::validate_detailed`, `evaluate_program_detailed`, `execute_statement_detailed`, or `botwork_detailed`. Each returns `DiagnosticResult<T>`. The [execution API example](interpreter-architecture.md#execution-api) is checked by rustdoc.

Original methods without `_detailed` retain their `BWErr` results and discard diagnostic context at the public boundary. `Diagnostic::into_error()` makes that conversion explicit. The value-level operator API also retains `BWErr`, since it has no source tree.

Spans retain shared source ownership after the input, parsed program, or context is dropped. `line_column()` and `end_line_column()` give the start and exclusive end. Columns count Unicode scalars; tabs count as one column and CRLF as one line ending. Syntax errors retain the parser's byte location, including an empty range at EOF. Validation precedes all execution/debug traces. Parser-pair calls name their original input `<input>`.

## Calls and Recovery

Call stacks are snapshots captured before unwinding. Any call that fails during resolution or argument evaluation has not entered its body/callback and adds no frame; enclosing callers remain visible. Native callbacks receive validated argument values. A callback error, unwinding panic, or invalid returned value includes its native call site. Normal completion, returns, and errors remove active frames; later failures cannot inherit them.

Try/Catch still handles evaluation failures only. A successful handler consumes its error and emits no diagnostic. If the handler fails, its failure is primary and the handled error remains in `causes`. Nested handler failures retain all original spans/stacks in handling order, innermost first. A captured diagnostic includes the current caller even if Catch handles it before that caller unwinds. `Display` renders causes; `std::error::Error::source()` exposes the first handled cause, or the underlying category error when there is none.

Each handled cause retains its own code and guidance; a handler's undefined-variable failure does not recategorize the original arithmetic failure. Local imports retain these codes, ranges, causes, and entered callers while adding related import sites. Adapter-cause compatibility and bounded diagnostic resources remain separate roadmap work. Success output, failure status, argument order, and language scope/completion behavior retain their contracts.


## DSL Metadata

`Catch |name|` binds the same owned value returned by `Diagnostic::to_value()`. It is a map with these fields:

| Field | Value |
| --- | --- |
| `code`, `message`, `help` | Strings describing this error |
| `details` | Category-specific map described below |
| `source` | Source map, or None if no source exists |
| `call_stack` | Innermost-first array of `{signature, call_site, definition_site}`; sites are source maps and native definition sites are None |
| `related` | Array of `{message, source}` for declarations and rethrow sites |
| `causes` | Array of diagnostic maps preserving handled errors |

A source map contains string fields `file`, `text`, `start_byte`, `end_byte`, `line`, `column`, `end_line`, and `end_column`. Coordinates are decimal strings, not i32 values, so metadata never truncates a source offset. Byte and end-position semantics match `Span`. Missing source/definition sites are present with None values; they are not absent keys.

`details` contains `name` for BW2001; `call` for BW2002; `name`, `original`, and `duplicate` for BW1003; `signature`, `original`, and `duplicate` for BW2003; `path`, `segment`, and `reason` for BW3004; and `reason` for all other current codes. These values are strings. Check the code before reading category-specific keys. A file cannot catch its own pre-execution syntax/validation failure. An importer can inspect such a failure during runtime loading, retaining its original code.

Metadata copies have no mutable connection to the active error. Rethrow uses that error's shared identity, retains its original span/stack/causes, and adds a related rethrow location. The same error is not appended as its own cause. Fresh errors, even at identical source locations, remain distinct. The Rust diagnostic error field now uses `Arc<BWErr>` so cloning a diagnostic preserves identity; legacy `BWErr` APIs retain their return types and categories.

For BW6003, `details` contains string fields `namespace`, `original`, and `duplicate`. BW6001/BW6002 use `reason`. An importer can catch an imported module's parse/validation error during runtime loading; the module itself executes no statements before successful validation. Related import sites and original module locations remain available through inspection/rethrow.
