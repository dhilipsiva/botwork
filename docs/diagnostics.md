# Structured Diagnostics

The CLI reports the failing source file, range, stable code, error, relevant source text, and repair guidance. A header such as `file.botwork:2:14-2:19: [BW2001]` uses one-based scalar positions with an exclusive end; an empty EOF range shows only its start. Runtime errors identify the innermost failing expression or access segment. Invalid conditions point to the condition, not the entire body. Entered calls appear innermost first, with call and definition locations. Each shows its statement [as written](#statements-as-written). Native calls have no DSL definition. Files appear [relative to the working directory](#file-names) when they are inside it.

[Bounded rendering](diagnostic-rendering.md) preserves this layout for admitted output. Oversized output or source-position work produces an explicit summary with original codes, leading cause evidence, bounded filenames, and byte offsets. Hosts can inspect truncation and choose local limits through `render_with_limits`; Display and CLI diagnostics use the defaults. Standalone repair-guidance helpers also bound their returned strings.

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
| BW5003 | Async runtime failure | Keep a Tokio runtime with time enabled alive through operation completion; after a stop, unblock or isolate work that outlives the [stop grace](shutdown.md) |
| BW6001 | Module loading | Check the source-relative local .botwork path, permissions, and UTF-8 contents/path |
| BW6002 | Import cycle | Move shared definitions into a module outside the reported cycle |
| BW6003 | Namespace collision | Use a distinct alias or remove the same-scope qualified declaration |
| BW7001 | Input variables | Use exact variable names and JSON with checked i32/finite f32 values and at most 128 nested containers |
| BW7002 | Run configuration | Use an existing working directory, valid environment names/values, a representable timeout, and supported syntax/AST/value/evaluation/import ceilings |
| BW7003 | Entry source loading through Engine | Supply a readable UTF-8 file relative to the run directory |
| BW8001 | Source/runtime resource limit | Reduce the workload or adjust configurable budgets within documented ceilings |
| BW9001 | Assertion failure | Inspect the condition or compared values and repair the behavior or expectation |
| BW9002 | Explicit failure | Inspect the reason and the path that reached Fail |
| BW9004 | Eventually condition not met | Inspect the last attempt's failure (first cause) and `details.history`; fix the behavior or choose a deliberate deadline |
| BW9005 | Retry attempts exhausted | Inspect the last attempt's failure; each attempt may have repeated the action's effects |

BW9003 is not a public category: worker wire tag 9003 carries structured BW9001 assertion evidence. [Eventually and Retry](polling.md) failures expose `details.reason`, a decimal-string `details.attempts`, and `details.history`, a JSON array of up to 16 recent attempt records.

Ordinary source calls with the wrong arity normally fail signature resolution as BW2002; BW2004 represents a resolved signature/count mismatch. Numeric conversion remains a runtime error despite the legacy `ParsingIntegerError` name. CLI argument parsing and entry-script file-loading errors are outside this language-error catalog. Variable-file loading/conversion uses BW7001 before execution, with origin/path/JSON position in `details.reason` and no DSL source span or call stack. Hints describe repairs without changing or automatically rerunning the script.

## Rust API

`DiagnosticCode::ALL` lists every public category in catalogue order, and `DiagnosticCode::parse` maps an exact `BWnnnn` identifier back to its category. Engine configuration/source errors use `details.reason`. BW8001 exposes `details.resource` and a decimal-string `details.limit`; source ranges and call frames are included when execution has entered source syntax. Run cancellation/deadline/resource exhaustion bypasses DSL handlers and preserves interpreter binding/frame cleanup. See [embedded runs](embedded-runs.md) for classification and cooperative limits.

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
| `call_stack` | Innermost-first array of `{signature, call_site, definition_site}`; `signature` is the normalized form, sites are source maps, and native definition sites are None |
| `related` | Array of `{message, source}` for declarations and rethrow sites |
| `causes` | Array of diagnostic maps preserving handled errors |

A source map contains string fields `file`, `text`, `start_byte`, `end_byte`, `line`, `column`, `end_line`, and `end_column`. Coordinates are decimal strings, not i32 values, so metadata never truncates a source offset. Byte and end-position semantics match `Span`. Missing source/definition sites are present with None values; they are not absent keys.

`details` contains `name` for BW2001, and `suggestion` when a variable the read can reach has a [near name](#near-name-suggestions); `call` for BW2002; `name`, `original`, and `duplicate` for BW1003; `signature`, `original`, and `duplicate` for BW2003; `path`, `segment`, and `reason` for BW3004; and `reason` for all other current codes. These values are strings. Check the code before reading category-specific keys. A file cannot catch its own pre-execution syntax/validation failure. An importer can inspect such a failure during runtime loading, retaining its original code.

### File names

The CLI shows every file in its text output as it shows the files it was
given: relative to its working directory when the file is inside it, and in
full otherwise. An imported module, which Botwork names by its canonical path,
therefore appears as `lib/pricing.botwork` beside `main.botwork`, in error
headers, `imported here` sites, call frames, the failure recap, and `--check`
findings, including locations written into an error's message, such as a
duplicate parameter's. The location fields of reports, run records, and `Catch`
metadata, and the language server, keep full names. A host embedding Botwork can opt in with
`botwork::core::diagnostic::show_paths_relative_to`, which takes effect once
per process.

### Statements as written

Each entered call in the text output shows its statement as written, such as
``in `pricing::Line total of |quantity| at |unit_price| less |discount|` called at
main.botwork:3:18``. That is a definition's header, or a built-in or native
statement's registered signature, on one line, prefixed with the namespaces the
call used to reach it. Frames keep the header's source span, never a copy.
When the header is unknown, as for frames received through the
[worker protocol](worker-protocol.md), the frame shows the normalized
signature instead, such as `assert|param|equals|param|`. The `call_stack`
metadata keeps the normalized `signature`.

### Near-name suggestions

When a variable is undefined, Botwork looks for a variable the read could reach
whose name is at most a third of its length away in edits: inserted, deleted,
changed, or swapped adjacent characters, with a case change counting as one.
The nearest such name, with ties going to the one that sorts first, leads the
help as ``Did you mean `discount`?`` and is kept in `details.suggestion`.
Only names in the frames the read searches are candidates, so a caller's
variables are never offered to a statement it calls. Names longer than 64
characters are neither suggested nor given suggestions, and at most 4,096
candidates are compared, those that sort first. `--check` and the language server offer the same
suggestion for their `undefined-variable` warning. A suggestion does not cross
the [worker protocol](worker-protocol.md), which carries the name alone.

Metadata copies have no mutable connection to the active error. Rethrow uses that error's shared identity, retains its original span/stack/causes, and adds a related rethrow location. The same error is not appended as its own cause. Fresh errors, even at identical source locations, remain distinct. The Rust diagnostic error field now uses `Arc<BWErr>` so cloning a diagnostic preserves identity; legacy `BWErr` APIs retain their return types and categories.

For BW6003, `details` contains string fields `namespace`, `original`, and `duplicate`. BW6001/BW6002 use `reason`. An importer can catch an imported module's parse/validation error during runtime loading; the module itself executes no statements before successful validation. Related import sites and original module locations remain available through inspection/rethrow.

## Bounded Metadata Conversion

Catch bindings use [diagnostic conversion limits](diagnostic-value-limits.md) before copying metadata and reserve complete temporary storage before construction. Rejection returns BW8001 with the original error as a cause, skips the handler, and preserves its previous binding. Checked host conversion uses `Diagnostic::to_value_with_limits`; `value_size_with_limits` reports exact metadata size without copying its payload. Legacy `to_value` remains a full host-managed conversion. Original diagnostic construction/retention and text rendering have separate pending limits; this contract does not bound those allocations.

## Host Ownership Helpers

[Diagnostic ownership](diagnostic-ownership.md) provides borrowed size admission and `try_clone_with_limits` before copying mutable metadata. Full Clone is iterative and shares immutable error/source identity. Use `discard` or `into_error` to release unadmitted deep cause trees without recursive destruction. Runtime-owned blocking-worker errors use an iterative cleanup guard when abandoned; limits on runtime error construction and retained context remain pending.

## Explicit Omission Metadata

Owned [diagnostic admission](diagnostic-ownership.md#owned-admission-and-emergency-evidence) can return a fixed emergency summary after rejecting and freeing an oversized tree. The quota failure remains primary; its bounded cause preserves the original category and includes `omissions` with shortened detail counts, omitted context counts, prior-summary/custom-label flags, and optional filename/byte-offset evidence. Full diagnostics retain the original eight-key schema; only summaries add this ninth key. Rendering marks every summary explicitly. [Individual synchronous diagnostics](runtime-diagnostics.md) now apply admission before call-stack copies; aggregate retention and general output limits remain pending.
