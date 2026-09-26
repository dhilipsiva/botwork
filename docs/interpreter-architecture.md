# Interpreter Architecture

## Parse Once, Execute Owned Syntax

`Program::parse(name, source)` in [ast.rs](../src/core/ast.rs) parses the complete input with Pest, converts its pairs into owned statements and expressions, then validates control placement and parameter names. The returned program has no lifetime dependency on the caller's string. Syntax and validation errors prevent execution of the whole program.

The tree represents assignments, calls, definitions, branches, loops, error handlers, and control statements explicitly. Expressions retain their operator, operands, and grouping. The Pratt parser builds this structure; it no longer evaluates values. Map entries stay in source order until evaluation.

The grammar keeps horizontal whitespace separate from explicit LF/CRLF rules. Delimited expressions and fixed syntax can span lines; custom sentences require explicit continuation outside expressions. Atomic sentence parts and comments preserve token boundaries. Continuation pairs remain in source spans but are excluded from signatures and Return operands. Triple-hash comments cannot fall back to single-line comments if their closing fence is missing.

Identifiers use Unicode `XID_START` plus underscore, then `XID_CONTINUE`, including combining marks. Keyword boundaries use the same continuation rule. Numeric literals use ASCII digits, while a separate `path_index` rule retains existing Unicode numeric map-key segments. No Unicode normalization occurs during parsing, matching, or lookup.

Strings and quoted map keys are decoded during construction. Numeric literals retain their source spelling and convert only when evaluated. Consequently an out-of-range number remains a catchable runtime error and does not fail inside an unselected branch or unused definition. `ExprKind::Access` stores a base expression and ordered `AccessSegment` values: literal names or computed expressions with bracket spans. Comments and whitespace do not become part of dot names. Array-index conversion remains deferred until lookup. Postfix access binds before power and unary operators; grouped bases preserve their parentheses in source spans.

Unary syntax retains its operator and operand spans. At runtime, a minus directly wrapping an integer atom converts the signed text together so the minimum `i32` literal is representable. Negation of compound expressions still evaluates the operand first and uses checked arithmetic; power grouping remains unchanged.

Expression evaluation takes a mutable context because `ExprKind::Call` invokes native/custom statements through the common invocation path. The `@{ ... }` grammar encloses exactly one call and retains both the expression's delimiters and the inner call's source span. Argument calls, ordinary operands, and collection values preserve source order; short-circuit operands remain unevaluated.

Frame bindings hold immutable `Arc<Literal>` values. Collection lookup retains a variable's Arc, or owns a computed base, across effectful key evaluation, then clones only the selected result. Each computed key evaluates once immediately before receiver/key validation and lookup; a failed step skips later expressions. The base snapshot cannot change if a later operation replaces the binding. Context clones share immutable value storage, with independent binding maps and owned exported values. Test-only expression visitation is absent from production builds.

Maps use exact string keys; arrays require ASCII digits for dot segments or nonnegative integer values for brackets. Bounds are checked without panicking on index overflow. Access errors include the path, failing segment, and reason; computed segments retain their bracket source text. Reads have no mutation operation. Indexed updates are reserved for future library statements returning replacement values.

## Value Comparison

`numeric_pair` widens integers and stored binary32 floats exactly to binary64 for all numeric comparisons. Arithmetic retains its separate binary32 conversion policy. `values_equal` first validates both complete value trees for non-finite host floats, then compares structural values using an explicit work list. Arrays compare positions; maps compare key sets and associated values independently of iteration order. Distinct nonnumeric kinds are unequal. Neither pass adds recursive comparison frames; source syntax has preflight bounds; runtime value nesting and broader limits remain separate work.

Equality runs before constructing generic incompatible-operator diagnostics, avoiding unnecessary formatting of entire collections on successful comparisons. Both operands have already evaluated before the value operator runs, so structural mismatches do not suppress expression errors or effects.

`QuotedString` formats collection strings and map keys with visible combining marks; quotes, backslashes, controls, and other nonprinting characters retain escapes. Top-level string display emits its raw contents. Display is human-readable and is not a source or serialization round trip.

## Source Ownership and Locations

Each program shares one `Arc<SourceFile>` containing its name and original UTF-8 text. Statement, expression, block, definition, identifier, and operator spans retain this source. Byte ranges use an exclusive end; `text()` returns the original range, including any whitespace consumed by that grammar node. Parenthesized operands retain their delimiters.

`line_column()` reports one-based lines and Unicode scalar columns. A tab occupies one column; CRLF counts as one line ending. Columns are not display widths or grapheme counts. Span ranges and source contents are private so callers cannot invalidate slicing boundaries.

`Statement::kind()` exposes an immutable view of typed syntax for inspection. `kind_name()` supplies the existing CLI trace labels. Detailed syntax, validation, and runtime diagnostics retain the innermost relevant span. Runtime errors snapshot active calls before unwinding; related declaration locations and handler causes retain their own source owners. See [structured diagnostics](diagnostics.md).

## Validate Controls and Parameter Names

`Program::validate()` walks all statements in source order, including unused definitions, skipped branches and handlers, and statements after an unconditional control transfer. It tracks whether a custom definition and a loop enclose each statement. Entering a definition resets loop permission; entering a loop preserves definition permission. Branches and handlers inherit both. `Return` requires a custom body; `Break` and `Continue` require a loop in that same body or at script level. Rethrow requires a lexical Catch in the same invocation; entering a definition resets Catch permission as well as loop permission.

The first invalid placement returns `ControlFlowError` with the offending statement's original span. Validation does not evaluate expressions, convert numbers, resolve names, or catch errors. Full parsing/lowering finishes before validation, so syntax errors take precedence.

Each definition also validates exact parameter-name uniqueness before visiting its body. `DuplicateParameter` records both original locations and is rejected before any execution, including for unused definitions. `Span::location()` formats original file/line/column for declaration diagnostics.

## Reuse Definitions

Executing a definition registers its `Arc<Definition>` in the context. Each invocation shares the same parsed parameter list and body. No invocation reparses the definition, reconstructs its expressions, or clones its whole syntax tree. The definition and its original source locations remain usable after the caller drops the defining program and input string.

Signatures strip ASCII spaces/tabs and lowercase Unicode characters individually; parameters contribute positional placeholders. Registration checks only the current frame and returns `DuplicateStatement` instead of replacing an existing entry. Both definition locations survive through retained source owners. Native entries retain a parsed header and named source origin; initialization fills vacant slots only. Lexical parent shadowing remains valid, and invocation cleanup applies to collision errors like other runtime failures.

Retained definitions keep their complete source file alive. Dropping the context releases them unless another program/context owns a reference. Local registration follows the defining invocation's lifetime; the immutable nested syntax can remain part of its outer definition's shared body.

## Invocation Frames

`Context` owns a stack of frames with separate variable and statement maps. The root frame persists across programs executed in that context. Each invocation adds a fresh frame whose parent index points to the frame containing its resolved definition. Lookup follows those lexical parents, skipping unrelated caller locals. Assignment and definition registration write only the current frame. Reads observe current bindings in that environment, not declaration-time snapshots.

Calls resolve their statement first and evaluate all arguments left to right in the caller before creating the new frame. On normal completion, Return, or an evaluation error, the frame is removed and execution resumes in the saved caller frame. Nested definitions cannot escape as first-class values, so their defining frames stay alive for every valid call. Parent indexes avoid reference cycles; cloning a context copies bindings and frame links while sharing immutable syntax.

If, While, and Try/Catch bodies share their enclosing frame. For evaluates its iterable first, saves only the iterator's binding in the current frame, and restores it on every completion path. If no local binding existed, it removes the iterator so a lexical ancestor's value becomes visible again. The body can still change other bindings in its enclosing frame.

## Execution API

```rust
use botwork::core::{ast::Program, eval::{evaluate_program, Context}, grammar::Literal};

let program = Program::parse("example.botwork", "|answer| = |2 ^ 3 ^ 2|")
    .expect("valid program");
let mut context = Context::default();
context.init_statements(); // Register native Log.
let result = evaluate_program(&program, &mut context).expect("successful execution");
assert!(matches!(result, Literal::Int(512)));
```

Use the detailed entry points to inspect the original failure location:

```rust
use botwork::core::{ast::Program, eval::{evaluate_program_detailed, Context}};

let program = Program::parse_detailed("failure.botwork", "|answer| = |1 + missing|")?;
let error = evaluate_program_detailed(&program, &mut Context::default()).unwrap_err();
assert_eq!(error.span.as_ref().unwrap().text(), "missing");
assert_eq!(error.span.as_ref().unwrap().line_column(), (1, 17));
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

The original entry points convert detailed failures back into `BWErr` at their public boundary. Detailed methods retain spans, entered calls, related declarations, and handler causes. Parser-pair lowering retains locations for both construction and execution failures.

`evaluate_program` validates its entire statement list before executing any statement. This also checks programs assembled by Rust callers from extracted syntax nodes. `execute_statement` validates its subtree at script scope. Neither entry point invokes the parser; internal loops and invocations do not repeat validation. The CLI finishes parsing/validation before any statement trace or output. An expression failure returns immediately, before later operands are visited. For `and` and `or`, the evaluator checks the left boolean and selects whether to visit the right expression. The value-level operator API remains strict when both values are supplied.

The existing `botwork(Pair<Rule>, &mut Context)` entry point lowers its supplied pair once, validates the resulting statement/block at script scope, and delegates to the same evaluator. It retains the pair's complete original input so nested offsets remain valid. Validation covers only the supplied subtree, so use the program API to reject a later invalid statement before earlier effects in a whole file. Separate compatibility calls allocate separate source owners.

## Completion Outcomes

Internal statement execution returns `Result<Completion, Diagnostic>`. `Completion` distinguishes a normal value from `Return(value)`, `Break`, and `Continue`. Blocks discard ordinary statement values and stop immediately on control transfer or error. Normally completed blocks, loops, branches, and handlers yield `None`; they retain no implicit result arrays.

Branches and Try/Catch pass control outcomes upward. A handler runs only for an evaluation error; a failed return expression is still an error until its value exists. For/While consume their own Break/Continue and propagate Return. A custom invocation consumes Return, preserving its exact value; fallthrough produces None. No control flags are stored in the context.

Runtime boundaries retain defensive checks for escaping controls, including a callee attempting to control its caller's loop. Public entry points reject invalid placement before execution, so a file cannot execute or catch its own placement error. An importer can handle the failed load without executing the invalid file.

## Catch State and Metadata

Try creates a handler record only after a runtime error. Optional binding metadata is an owned Literal map; a saved current-frame value or absence is restored on every handler outcome before invocation cleanup. Active handler records carry their owning invocation index, so defensive Rethrow checks cannot use an unrelated caller's error. Nested handlers push/pop independently.

Diagnostics share immutable error identity through `Arc<BWErr>`. Rethrow clones the original context and adds its own related location; handler error propagation avoids attaching that same identity as a self-cause. Fresh errors with matching text/spans remain distinct. A nested Try can catch a rethrow normally. Metadata changes cannot change the retained error, and successful handling leaves no pending state. Position metadata uses decimal strings to preserve usize coordinates within the current i32-only DSL.

## Remaining Interpreter Work

Broader resource limits, asynchronous DSL execution, and adapter integrations retain their own roadmap items. Engine, Context, and CLI execution share source/syntax guards and step/call/evaluation/import-initialization depth budgets. [Owned syntax admission](ast-limits.md) bounds tree structure and source ownership; aggregate retained memory and hard host termination remain open. Core value, naming, Unicode, scope, and completion checks do not establish exhaustive language conformance or the release quality gates.

[AST unit tests](../src/core/ast/tests.rs) check tree structure and spans. [Execution tests](../tests/ast_execution.rs) exercise ownership and compatibility, and evaluator tests verify shared definition identity and skipped operand evaluation. Both build profiles continue to run the full regression, contract, CLI, and example suites.

## Register Native Statements

`Context::register_native(header, callback)` registers a synchronous Rust closure. The header is a complete DSL sentence header without a body; parameter labels must be valid, distinct identifiers. Registration uses the same grammar, normalization, collision checks, and lexical lookup as custom statements. Invalid headers return source-bearing diagnostics without registering anything or calling the closure. Header diagnostics use `<native>`; built-in Log uses `<builtin Log>`. Repeated `init_statements()` calls fill only vacant slots.

```rust
use botwork::core::{
    ast::Program,
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, Literal},
};

let mut context = Context::default();
context.register_native("Uppercase |text|", |arguments| {
    let Literal::String(text) = &arguments[0] else {
        return Err(BWErr::OperationIncompatibleError("Uppercase requires a string".into()));
    };
    Ok(Literal::String(text.to_uppercase()))
})?;
let program = Program::parse_detailed("native.botwork", "|result| = Uppercase |\"hello\"|")?;
assert_eq!(evaluate_program_detailed(&program, &mut context)?.to_string(), "HELLO");
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

Callbacks implement `Fn(&[Literal]) -> LiteralResult + Send + Sync + 'static`. Resolution and arity checks precede argument evaluation; every argument evaluates once, left to right, in the caller. All values must be finite, including nested floats in arrays/maps. The callback receives immutable values in parameter order and cannot access the interpreter context. Declare accepted kinds with the shared metadata below; callbacks still check operation-specific shapes/ranges before performing effects.

Return an owned `Literal` for assignment or further calls; return `Literal::None` explicitly when there is no result. Every value kind is supported, with no coercion or host-reference aliasing. Returned non-finite values fail with BW3002 before assignment. Log uses this same registration/invocation path and returns its input after writing it.

Return a suitable `BWErr` for expected failures, including `NativeError(reason)` (BW4002) for operation failures. Existing error categories keep their codes. The interpreter attaches the call span, native frame, enclosing callers, and any handled cause; Catch inspection and Rethrow work normally. Callback errors do not undo completed effects.

Unwinding callback panics become BW4003 and unwind language frames/bindings through normal error handling. The Rust panic hook still runs; Botwork does not alter process-global hooks. Process aborts, fatal signals, blocking callbacks, and corrupted or poisoned captured state are outside this recovery guarantee. Callbacks must restore their own host resources/state and return errors for expected failures. These are trusted in-process extensions with the host process's capabilities.

Cloning a context copies DSL bindings/registries and shares callback closures through Arc. Synchronize intentionally shared captured state, or register separate closures in fresh contexts for independent host state. `Send + Sync` bounds permit that sharing; execution remains synchronous. Async cancellation, operation deadlines, and adapter conversion/cause contracts retain separate roadmap tasks.

## Shared Signature Metadata

`core::signature::StatementSignature` is the contract used by both native and DSL registrations. It retains the normalized key, original header span, origin, ordered parameter names/kinds, return kinds, description, and documented errors. `Definition::signature_metadata()` exposes a parsed DSL declaration without executing it. DSL parameters and results are `Any`; their bodies are dynamic, so return types and possible failures are not inferred. Empty error documentation never promises error-free execution.

Native hosts can constrain kinds before registration:

```rust
use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode,
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    signature::{StatementSignature, ValueKind},
};

let signature = StatementSignature::native("Uppercase |text|")?
    .parameter("text", ValueKind::String)?
    .returns(ValueKind::String)
    .description("Return the Unicode uppercase text.");
let mut context = Context::default();
context.register_native_with_signature(signature, |arguments| {
    let Literal::String(text) = &arguments[0] else { unreachable!("checked before entry") };
    Ok(Literal::String(text.to_uppercase()))
})?;
let program = Program::parse_detailed("typed.botwork", "Uppercase |1|")?;
let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
assert_eq!(error.code(), DiagnosticCode::IncompatibleType);
assert!(error.call_stack.is_empty()); // The callback never ran.
let metadata = context.statement_signature("u p p e r c a s e |value|")?.unwrap();
assert!(metadata.help().contains("text: String"));
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

`ValueKind` names None, Int, Float, Bool, String, Array, and Map. `ValueKinds::one(kind).union(other.into())` accepts several kinds; `ValueKinds::ANY` accepts all seven. These are top-level kinds, with no implicit conversion or structural schema for collections. Finite checks still traverse every nested value. Sets cannot be empty; displays and iteration have a fixed order. Metadata builders preserve normalized names; unknown case-sensitive parameter labels or duplicate/empty error descriptions return BW1004. `documents_error(code, description)` adds sorted operation documentation without restricting which errors can propagate.

For both entry kinds, the interpreter resolves/checks arity first, then evaluates and validates each argument in order. A kind mismatch is BW3003 at that argument, skips later arguments, and adds no unentered callee frame. Return validation happens after completion but before assignment; a mismatch is BW3003 with the entered frame and preserves the destination. Completed callback effects remain completed. Ordinary `register_native` defaults to Any; operation authors should declare narrower kinds when known. Log documents Any input/result and output failure BW4001 through the same schema.

`statement_signatures()` lists visible signatures in normalized order, resolving lexical shadowing. `statement_signature(header)` looks up a complete header; `signature_for_call(call)` gives a hover consumer the same metadata in the current lexical environment. `complete_statements(prefix)` matches normalized initial sentence text before the first parameter and returns the same records; it does not parse incomplete expressions. `help()` renders those records. These queries never invoke callbacks or execute definitions. Metadata clones own their retained source and are independent of registry mutations.

CLI `--list-statements` and `--statement-help 'Log |value|'` use the initialized built-in registry and require no script. They cannot be combined with file execution/debug flags. Unknown headers return BW2002 and malformed headers return syntax diagnostics. Static analysis of unexecuted modules, editor protocol wiring, inferred DSL types, and incomplete-edit recovery remain tooling work; registry queries alone do not resolve arbitrary nested source scopes.


## Async Operation Contract

`core::operation::NativeOperation` establishes the host interface for future I/O statements and adapters. It uses the shared native signature schema and owned finite values. `asynchronous(signature, callback)` accepts a future-producing callback; `blocking(signature, max_in_flight, callback)` dispatches synchronous work to a bounded Tokio worker pool. Clones share the callback and its blocking capacity. Hosts supply a live Tokio runtime with time enabled. The synchronous DSL evaluator does not dispatch these operations yet; asynchronous program/CLI integration remains a separate runtime task.

```rust
use botwork::core::{
    operation::{NativeOperation, OperationControl},
    signature::{StatementSignature, ValueKind},
    grammar::Literal,
};

let signature = StatementSignature::native("Echo |value|")?
    .parameter("value", ValueKind::String)?
    .returns(ValueKind::String);
let operation = NativeOperation::asynchronous(signature, |mut values, control| async move {
    control.checkpoint()?;
    tokio::task::yield_now().await;
    Ok(values.remove(0))
})?;
let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
let value = runtime.block_on(operation.invoke(
    vec![Literal::String("hello".into())], OperationControl::default(),
))?;
assert_eq!(value.to_string(), "hello");
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

`OperationControl` clones share a cancellation request. `child(optional_deadline)` inherits parent cancellation and the earlier monotonic deadline; cancelling a child does not stop its parent/siblings. Each invocation creates its own child control. Callbacks use `checkpoint()` before effects and periodically during computation, or await `stopped()` alongside I/O. Cancellation is BW5001, deadline expiry BW5002, and runtime/configuration failure BW5003. When both stop conditions are observed together, explicit cancellation takes priority. A cancelled/expired control rejects entry, including callback construction; there are no implicit retries.

Arguments are owned, checked for arity/kinds/finiteness before entry, and delivered once in order. Results are checked before publication. Callbacks return `DiagnosticResult<Literal>` to preserve original sources, codes, call context, and nested causes. Missing locations use the registered header; direct host calls do not invent DSL call frames. Factory/poll/worker unwinding panics become BW4003; process aborts and panicking destructors are outside this guarantee. Runtime panic hooks remain active.

Async factories must return promptly and defer effects into the future. Future polls must yield; cancellation/deadlines cannot preempt blocking code inside a poll. On a stop request, the invocation drops the async future and its synchronous resources before returning. Hosts dropping the invocation also drop that future and signal its child. Async cleanup that must itself be awaited needs the planned resource lifecycle contract. Do not detach unowned tasks from a callback. These limits follow Tokio's [timeout contract](https://docs.rs/tokio/1.53.1/tokio/time/fn.timeout.html).

Blocking callbacks run through `spawn_blocking`, with an explicit nonzero maximum shared by operation clones. Capacity waiting is cancellable, and a permit remains held until its worker exits. A stop request signals the worker, aborts it if still queued, then awaits completion before returning the primary cancellation/timeout. A worker failure during that drain remains a structured cause. Blocking callbacks must use checkpoints and bounded waits so they can finish cooperatively; no hard termination deadline is promised for uncooperative in-process work. Tokio [cannot abort started blocking callbacks](https://docs.rs/tokio/1.53.1/tokio/task/fn.spawn_blocking.html). Integrations needing hard termination require isolated workers under the later shutdown contract.

If a host drops a blocking invocation instead of requesting cancellation and awaiting it, its worker is signalled but cannot be joined synchronously by Drop. The worker can continue until it cooperates; its permit remains held meanwhile. Keep the runtime alive until owned work finishes. Cancellation does not undo completed external effects or guarantee an effect never happened; adapters must expose appropriate retry/idempotency semantics. Whole-run cancellation, teardown, reporting outcomes, bounded parallel runs, and async DSL dispatch remain explicit roadmap work.


## Local Module Loading

`StatementKind::Import` stores a decoded literal path, its original span, and an alias Name. The evaluator's imports module resolves source-relative paths using the Context's captured working directory, canonicalizes module identity, and tracks an active load chain separately from completed cache entries. Whole-module parsing/validation precedes initialization. Errors keep their category/span and gain related import sites; importer handlers can catch these runtime loading failures.

Each loaded module retains an immutable root Frame snapshot, containing initialized values, definitions, native callbacks, and any imported namespaces. Host operations are inherited from the visible native registry; caller variables/custom definitions are not copied. Imported entries retain a module Arc, export key, qualified shared metadata, and import span. Calls evaluate/validate arguments in the caller, then execute the resolved export using an isolated module context and fresh invocation frame. Caller call stacks are retained for diagnostics; handlers and locals remain separate. Nested imports still use the defining source file.

Contexts own cache maps and requested-to-canonical path mappings. Isolated initialization/call contexts copy those maps and merge successful dependency additions back even after a parent fails. Completed module Frames reference only dependency modules and source owners, never a Context/cache, avoiding ownership cycles. Context clones copy cache maps and share immutable completed modules. A Weak-source test checks that dropping the final context releases cached sources. Cache lifetime and native captures do not establish future parallel-run isolation guarantees.

Namespace registration checks the whole prefix before module initialization and publishes exports/alias only on success. Same-frame aliases protect their prefix from later declaration writes. Lookup stops at the nearest namespace owner if an export is absent, and listing/completion apply the same whole-namespace shadowing. Qualified metadata uses `display_header()` for labels while retaining the actual definition's header span for source navigation. See [module semantics](language.md#local-modules) for cache/retry and capability rules.

## Input Variables

`core::input` parses JSON files/settings without evaluating expressions. Borrowed raw JSON tokens retain their original numeric spelling for direct checked i32/f32 conversion; object keys remain ordinary strings. A preliminary scan caps container nesting at 128, including discarded duplicate values, before recursive conversion. JSON payload size and broader runtime budgets remain separate work.

```rust
use botwork::core::{
    ast::Program,
    eval::{evaluate_program_detailed, Context},
    input::parse_variables,
};

let mut context = Context::default();
context.set_input_variables(parse_variables("settings.json", r#"{"count":3,"name":"Ada"}"#)?)?;
let program = Program::parse_detailed("inputs.botwork", "|result| = |[name, count + 1]|")?;
let result = evaluate_program_detailed(&program, &mut context)?;
assert_eq!(result.to_string(), "[\"Ada\", 4]");
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

`set_input_variables` validates the entire batch before updating root bindings. Its input map is ordered, making name-validation error priority deterministic. Host-created values receive recursive finite checks; imported module roots remain independent. Input errors use BW7001 without echoing whole payloads or inventing DSL spans. The [input contract](input-variables.md) defines file/flag precedence, number ranges, duplicate keys, and scope.

## Embedded Runs

`core::run::Engine` owns native registrations; each call creates independent run state. Its configuration does not change the host environment or working directory. Native closures receive immutable run environment data and a cooperative control handle; captured host state keeps its explicitly shared lifetime.

```rust
use std::collections::BTreeMap;
use botwork::core::{
    grammar::Literal,
    run::{Engine, RunOptions, RunOutcome},
};

let mut engine = Engine::default();
engine.register_native("Greeting |name|", |values, environment| {
    let prefix = environment.get("GREETING").unwrap().to_string_lossy();
    Ok(Literal::String(format!("{prefix}, {}", values[0])))
})?;
let report = engine.run_source("greeting.botwork", "Greeting |name|", RunOptions {
    variables: BTreeMap::from([("name".into(), Literal::String("Ada".into()))]),
    inherit_environment: false,
    environment: BTreeMap::from([("GREETING".into(), Some("Hello".into()))]),
    ..RunOptions::default()
});
assert_eq!(report.outcome(), RunOutcome::Succeeded);
assert_eq!(report.result?.to_string(), "Hello, Ada");
assert_eq!(report.steps, 2);
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

`run_program` accepts reusable owned syntax, and `run_file` performs a bounded source read relative to the run directory. Run snapshots include completed root variables, a detailed terminal result, steps, and elapsed duration. Module contexts share the run's budget/control/environment while retaining isolated globals. Public Context clones copy counters; module isolation explicitly shares them. Stop errors latch, bypass Catch, and preserve frame/iterator cleanup. See [embedded-run contracts](embedded-runs.md) for exact defaults, count boundaries, clocks, compatibility, and cooperative execution limits.

## Source Preflight

`syntax_limits` scans source before entering the generated Pest parser. The public Pest-compatible wrapper lives in grammar.rs, keeping maintained guard code in coverage scope while excluding generated code. Program parsing uses the private generated parser only after a successful preflight. Guard failures retain a bounded source prefix and typed resource diagnostics. Engine options tighten syntax limits locally; CLI and legacy module reads use the default byte cap. [Source-limit rules](syntax-limits.md) specify counting, lexical contexts, fixed ceilings, and compatibility.


## Configure Owned Syntax Admission

Set tree budgets independently of execution steps and parser syntax limits. Reassembled programs are checked before effects; shared source owners count once.

```rust
use botwork::core::{ast::Program, ast_limits::AstLimits, run::{Engine, RunLimits, RunOptions, RunOutcome}};
let mut program = Program::parse("host", "|x| = |1|")?;
program.statements.push(program.statements[0].clone());
let run = Engine::default().run_program(&program, RunOptions {
    limits: RunLimits {
        ast: AstLimits { nodes: 5, ..AstLimits::default() },
        ..RunLimits::default()
    },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
assert!(run.variables.is_empty());
assert_eq!(run.steps, 0);
# Ok::<(), Box<dyn std::error::Error>>(())
```


## Configure Value Admission

Value limits apply to root input before any binding is installed. Owned rejected values are released iteratively.

```rust
use std::collections::BTreeMap;
use botwork::core::{grammar::Literal, run::{Engine, RunLimits, RunOptions, RunOutcome}, value_limits::ValueLimits};
let run = Engine::default().run_source("input", "", RunOptions {
    variables: BTreeMap::from([("value".into(), Literal::Array(vec![Literal::Int(1), Literal::Int(2)]))]),
    limits: RunLimits {
        values: ValueLimits { entries: 1, ..ValueLimits::default() },
        ..RunLimits::default()
    },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
assert!(run.variables.is_empty());
assert_eq!(run.steps, 0);
```
