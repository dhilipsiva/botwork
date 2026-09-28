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

Broader resource limits, nonblocking I/O libraries, and adapter integrations retain their own roadmap items. [Asynchronous DSL execution](async-execution.md) now shares the interpreter with synchronous entry points. Engine, Context, and CLI execution share source/syntax guards and step/call/evaluation/import-initialization depth budgets. [Owned syntax admission](ast-limits.md) bounds tree structure and source ownership; aggregate retained memory and hard host termination remain open. Core value, naming, Unicode, scope, and completion checks do not establish exhaustive language conformance or the release quality gates.

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

Cloning a context copies DSL bindings/registries and shares callback closures through Arc. Synchronize intentionally shared captured state, or register separate closures in fresh contexts for independent host state. `Send + Sync` bounds permit that sharing. Synchronous entry invokes callbacks inline; [async execution](async-execution.md) awaits [bounded workers](nonblocking-io.md), including for Log/debug output. NativeOperation provides explicit async/blocking/isolated adapter contracts. Adapter conversion/cause contracts retain separate roadmap tasks.

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

These builder errors pass default diagnostic text/source admission before message construction and source retention. If those quotas fail, BW8001 retains bounded BW1004 evidence and original header byte locations. Builder checks are independent of Context quotas and leave other metadata owners usable. See [builder construction rules](diagnostic-construction.md#signature-builder-errors).

For both entry kinds, the interpreter resolves/checks arity first, then evaluates and validates each argument in order. A kind mismatch is BW3003 at that argument, skips later arguments, and adds no unentered callee frame. Return validation happens after completion but before assignment; a mismatch is BW3003 with the entered frame and preserves the destination. Completed callback effects remain completed. Ordinary `register_native` defaults to Any; operation authors should declare narrower kinds when known. Log documents Any input/result and output failure BW4001 through the same schema.

`statement_signatures()` lists visible signatures in normalized order, resolving lexical shadowing. `statement_signature(header)` looks up a complete header; `signature_for_call(call)` gives a hover consumer the same metadata in the current lexical environment. `complete_statements(prefix)` matches normalized initial sentence text before the first parameter and returns the same records; it does not parse incomplete expressions. `help()` renders those records. These queries never invoke callbacks or execute definitions. Metadata clones own their retained source and are independent of registry mutations.

CLI `--list-statements` and `--statement-help 'Log |value|'` use the initialized built-in registry and require no script. They cannot be combined with file execution/debug flags. Unknown headers return BW2002 and malformed headers return syntax diagnostics. Static analysis of unexecuted modules, editor protocol wiring, inferred DSL types, and incomplete-edit recovery remain tooling work; registry queries alone do not resolve arbitrary nested source scopes.


## Async Operation Contract

`core::operation::NativeOperation` establishes the host interface for future I/O statements and adapters. It uses the shared native signature schema and owned finite values. `asynchronous(signature, callback)` accepts a future-producing callback; `blocking(signature, max_in_flight, callback)` dispatches synchronous work to a bounded Tokio worker pool. Clones share the callback and its blocking capacity. Hosts supply a live Tokio runtime with time enabled. `Engine::register_operation` and `Context::register_operation` now connect these operations to [asynchronous DSL execution](async-execution.md). Synchronous entry points reject contexts containing operation registrations before statement effects; the CLI uses the shared async driver.

Clones also share an [operation ownership budget](operation-ownership.md). Arguments reserve capacity when the invocation future is constructed; results require overlap headroom and remain charged through worker handoff. Budgets can be shared across distinct operations, and reservations follow abandoned workers until completion or disposal. Public results transfer to host ownership.

```rust
use botwork::core::{grammar::Literal, operation::{NativeOperation, OperationBudget, OperationControl, OperationOwnershipLimits, OperationUsage}, signature::StatementSignature};
let budget = OperationBudget::new(OperationOwnershipLimits {
    invocations: 1, values: 2, nodes: 2, payload_bytes: 8,
    ..OperationOwnershipLimits::default()
});
let operation = NativeOperation::asynchronous(StatementSignature::native("Echo |value|")?,
    |mut values, _| async move { Ok(values.pop().unwrap()) })?
    .with_ownership_budget(budget.clone());
let invocation = operation.invoke(vec![Literal::Int(7)], OperationControl::default());
assert_eq!(budget.usage().invocations, 1);
assert_eq!(budget.usage().payload_bytes, 4);
let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
let value = runtime.block_on(invocation)?;
assert_eq!(value.to_string(), "7");
assert_eq!(budget.usage(), OperationUsage::default());
# Ok::<(), Box<dyn std::error::Error>>(())
```

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

Blocking callbacks run through `spawn_blocking`, with an explicit nonzero maximum shared by operation clones. Capacity waiting is cancellable, and a permit remains held until its worker exits. A stop request signals the worker, aborts it if still queued, then awaits completion before returning the primary cancellation/timeout. A worker failure during that drain remains a structured cause. Blocking callbacks must use checkpoints and bounded waits so they can finish cooperatively; no hard termination deadline is promised for uncooperative in-process work. Tokio [cannot abort started blocking callbacks](https://docs.rs/tokio/1.53.1/tokio/task/fn.spawn_blocking.html). The [Linux worker supervisor](isolated-workers.md) supplies a byte-oriented process boundary for noncooperative external executables. The [typed worker protocol](worker-protocol.md) connects it to `NativeOperation::isolated`, with pre-copy value/diagnostic admission and reservations retained through cleanup. Complete process-tree/shutdown integration remains tracked work.

If a host drops a blocking invocation instead of requesting cancellation and awaiting it, its worker is signalled but cannot be joined synchronously by Drop. The worker can continue until it cooperates; its permit remains held meanwhile. Keep the runtime alive until owned work finishes. Cancellation does not undo completed external effects or guarantee an effect never happened; adapters must expose appropriate retry/idempotency semantics. Async DSL dispatch now propagates run cancellation and drops pending operation futures with the run. Awaited teardown, coordinated shutdown/reporting outcomes, and bounded parallel CLI runs remain explicit roadmap work.


## Local Module Loading

`StatementKind::Import` stores a decoded literal path, its original span, and an alias Name. The evaluator's imports module resolves source-relative paths using the Context's captured working directory, canonicalizes module identity, and tracks an active load chain separately from completed cache entries. Whole-module parsing/validation precedes initialization. Errors keep their category/span and gain related import sites; importer handlers can catch these runtime loading failures.

Each loaded module retains an immutable root Frame snapshot, containing initialized values, definitions, native callbacks, and any imported namespaces. Host operations are inherited from the visible native registry; caller variables/custom definitions are not copied. Imported entries retain a module Arc, export key, qualified shared metadata, and import span. Calls evaluate/validate arguments in the caller, then execute the resolved export using an isolated module context and fresh invocation frame. Caller call stacks are retained for diagnostics; handlers and locals remain separate. Nested imports still use the defining source file.

Contexts own cache maps and requested-to-canonical path mappings. Isolated initialization/call contexts copy those maps and merge successful dependency additions back even after a parent fails. Completed module Frames reference only dependency modules and source owners, never a Context/cache, avoiding ownership cycles. Context clones copy cache maps and share immutable completed modules. A Weak-source test checks that dropping the final context releases cached sources. Cache lifetime and native captures do not establish future parallel-run isolation guarantees.

Namespace registration checks the whole prefix before module initialization and publishes exports/alias only on success. Same-frame aliases protect their prefix from later declaration writes. Lookup stops at the nearest namespace owner if an export is absent, and listing/completion apply the same whole-namespace shadowing. Qualified metadata uses `display_header()` for labels while retaining the actual definition's header span for source navigation. See [module semantics](language.md#local-modules) for cache/retry and capability rules.

## Input Variables

`core::input` parses JSON files/settings without evaluating expressions. Borrowed raw JSON tokens retain their original numeric spelling for direct checked i32/f32 conversion; object keys remain ordinary strings. A preliminary scan caps container nesting at 128, including discarded duplicate values, before recursive conversion. [Input budgets](input-variables.md#input-resource-budgets) now bound encoded size/tokens, raw container/string admission, decoded values, and merged names. Aggregate runtime state remains separate work.

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


## Configure Input Decoding

Input budgets apply before root installation and share counters across an ordered file/flag load. Individual parse calls start fresh counters.

```rust
use botwork::core::{input::{InputLimits, parse_variable_with_limits}, value_limits::ValueLimits};
let limits = InputLimits {
    source_bytes: 7, total_bytes: 7, sources: 1, raw_nodes: 3, variables: 1,
    values: ValueLimits { nodes: 3, depth: 2, entries: 2, payload_bytes: 8, ..ValueLimits::default() }
};
let (name, value) = parse_variable_with_limits("flag", "x=[1,2]", &limits)?;
assert_eq!(name, "x");
assert_eq!(value.to_string(), "[1, 2]");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Configure Retained Values

Stored values reserve aggregate capacity before publication. Replacing a binding needs room for both old and new values.

```rust
use botwork::core::run::{Engine, RetainedValueLimits, RunLimits, RunOptions, RunOutcome};
let run = Engine::default().run_source("retention", "|x| = |1|\n|x| = |2|", RunOptions {
    limits: RunLimits {
        retained_values: RetainedValueLimits { values: 1, ..RetainedValueLimits::default() },
        ..RunLimits::default()
    },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
assert_eq!(run.variables["x"].to_string(), "1");
```

## Configure Retained Definitions

Multiple declarations in one source share its retained text/name allowance.

```rust
use botwork::core::run::{Engine, RetainedDefinitionLimits, RunLimits, RunOptions, RunOutcome};
let source = "First {}\nSecond {}";
let run = Engine::default().run_source("source", source, RunOptions {
    limits: RunLimits {
        retained_definitions: RetainedDefinitionLimits {
            definitions: 2, nodes: 6, source_bytes: source.len() + "source".len()
        },
        ..RunLimits::default()
    },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::Succeeded);
```

## Configure Variable Names

Replacing a binding reuses its immutable name allocation. Value storage retains its separate overlap rule.

```rust
use botwork::core::run::{Engine, RetainedNameLimits, RunLimits, RunOptions, RunOutcome};
let run = Engine::default().run_source("names", "|é| = |1|\n|é| = |2|", RunOptions {
    limits: RunLimits {
        retained_names: RetainedNameLimits { names: 1, name_bytes: 2, total_bytes: 2 },
        ..RunLimits::default()
    },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::Succeeded);
assert_eq!(run.variables["é"].to_string(), "2");
```

## Configure Registry Metadata

Native documentation and source ownership are admitted alongside lookup keys and parameter descriptors.

```rust
use botwork::core::{
    diagnostic::DiagnosticCode, eval::Context, grammar::Literal,
    run::{RetainedRegistryLimits, RunLimits}, signature::StatementSignature,
};
let mut context = Context::with_limits(RunLimits {
    retained_registry: RetainedRegistryLimits {
        entries: 1, nodes: 3, name_bytes: 11, text_bytes: 32, source_bytes: 20
    },
    ..RunLimits::default()
})?;
let signature = StatementSignature::native("Read |value|")?
    .description("é").documents_error(DiagnosticCode::Native, "bad")?;
context.register_native_with_signature(signature, |_| Ok(Literal::None))?;
assert_eq!(context.statement_signatures().len(), 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Make a Budgeted Host Snapshot

`try_clone()` charges the source before copying tables. Ordinary Clone retains its host-owned contract.

```rust
use botwork::core::{eval::Context, run::{RunLimits, SnapshotLimits}};
let mut context = Context::with_limits(RunLimits {
    snapshots: SnapshotLimits { entries: 41, path_bytes: usize::MAX },
    ..RunLimits::default()
})?;
context.init_statements();
let copy = context.try_clone()?;
assert_eq!(copy.statement_signatures().len(), 41);
assert!(context.try_clone().is_err());
assert!(copy.checkpoint().is_ok());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Bound Owned Result Exports

An export failure omits the entire root map and exposes a separate snapshot diagnostic.

```rust
use botwork::core::run::{Engine, ResultLimits, RunLimits, RunOptions, RunOutcome};
let run = Engine::default().run_source("result", "|x| = |7|", RunOptions {
    limits: RunLimits {
        results: ResultLimits { values: 1, ..ResultLimits::default() },
        ..RunLimits::default()
    },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
assert!(run.variables.is_empty());
assert!(run.snapshot_error.is_some());
```

## Bound Live Expression Values

Concatenation admits the output while both operands remain charged.

```rust
use botwork::core::run::{Engine, RunLimits, RunOptions, RunOutcome, TemporaryLimits};
let run = Engine::default().run_source("temporary", "|x| = |\"ab\" + \"cd\"|", RunOptions {
    limits: RunLimits {
        temporaries: TemporaryLimits { values: 3, nodes: 3, payload_bytes: 8 },
        ..RunLimits::default()
    },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::Succeeded);
assert_eq!(run.variables["x"].to_string(), "abcd");
```

## Checked Diagnostic Metadata

Measure or convert a borrowed diagnostic with explicit limits. Checked conversion preserves the original when it rejects; the host can retain its category and choose how to report it. Engine Catch bindings additionally intersect ordinary value limits and reserve live temporary storage before constructing metadata.

```rust
use botwork::core::{
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticValueLimits},
    grammar::{BWErr, Literal},
    value_limits::ValueLimits,
};
let original = Diagnostic::new(BWErr::NativeError("offline".into()));
let limits = DiagnosticValueLimits::default();
let size = original.value_size_with_limits(&limits)?;
let value = original.to_value_with_limits(&limits)?;
assert_eq!(limits.values.check(&value)?, size);
assert!(matches!(value, Literal::Map(_)));
let tight = DiagnosticValueLimits {
    values: ValueLimits { nodes: size.nodes - 1, ..ValueLimits::default() },
    ..limits
};
assert_eq!(original.to_value_with_limits(&tight).unwrap_err().code(), DiagnosticCode::ResourceLimit);
assert_eq!(original.code(), DiagnosticCode::Native);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Host Diagnostic Ownership

Checked cloning admits logical tree and source ownership before copying mutable context. Errors and sources retain shared identity. Use explicit disposal for unadmitted host trees; ordinary field access and ownership remain compatible.

```rust
use botwork::core::{diagnostic::{Diagnostic, DiagnosticLimits}, grammar::BWErr};
use std::sync::Arc;
let original = Diagnostic::new(BWErr::NativeError("offline".into()));
let limits = DiagnosticLimits { text_bytes: 13, source_bytes: 0, ..DiagnosticLimits::default() };
let size = limits.check(&original)?;
assert_eq!(size.diagnostics, 1);
assert_eq!(size.text_bytes, 13); // label "source" plus error detail "offline"
let copy = original.try_clone_with_limits(&limits)?;
assert!(Arc::ptr_eq(&original.error, &copy.error));
original.discard();
assert_eq!(copy.into_error().code().as_str(), "BW4002");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Owned Diagnostic Rejection

Owned admission retains a complete diagnostic only when it fits. Rejection frees the original tree and returns bounded emergency evidence, including its category and explicit omission metadata. The emergency representation has its own fixed caps and remains available with zero input quotas.

```rust
use botwork::core::{diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLimits}, grammar::BWErr};
let original = Diagnostic::new(BWErr::NativeError("x".repeat(4096)));
let limits = DiagnosticLimits { text_bytes: 32, ..DiagnosticLimits::default() };
let failure = limits.admit(original).unwrap_err();
assert_eq!(failure.code(), DiagnosticCode::ResourceLimit);
let summary = &failure.causes[0];
assert_eq!(summary.code(), DiagnosticCode::Native);
assert_eq!(summary.omissions.as_ref().unwrap().detail_fields, 1);
assert!(summary.span.is_none());
assert!(summary.causes.is_empty());
```

## Synchronous Runtime Diagnostic Limits

Configure per-error tree/context/source quotas independently of Catch metadata conversion. Exceeding a diagnostic quota latches the run, skips handlers, and preserves bounded original-category evidence. Aggregate retained diagnostic accounting remains a separate contract.

```rust
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, run::{Engine, RunLimits, RunOptions, RunOutcome}};
let run = Engine::default().run_source("diagnostic", "Try { Missing } Catch { |handled| = |true| }", RunOptions {
    limits: RunLimits { diagnostics: DiagnosticLimits { diagnostics: 0, ..DiagnosticLimits::default() }, ..RunLimits::default() },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
assert!(!run.variables.contains_key("handled"));
let failure = run.result.unwrap_err();
assert_eq!(failure.causes[0].code(), DiagnosticCode::UndefinedStatement);
assert!(failure.causes[0].omissions.is_some());
```

## Retained Calls and Handler Errors

Configure aggregate live records separately from individual diagnostic quotas. Context/module snapshots share immutable records; checked snapshots count copied handles. Rejection releases the original handler tree and preserves bounded category evidence.

```rust
use botwork::core::{diagnostic::DiagnosticCode, run::{Engine, RetainedDiagnosticLimits, RunLimits, RunOptions, RunOutcome}};
let run = Engine::default().run_source("retention", "Try { Missing } Catch { |handled| = |true| }", RunOptions {
    limits: RunLimits { retained_diagnostics: RetainedDiagnosticLimits { records: 0, ..RetainedDiagnosticLimits::default() }, ..RunLimits::default() },
    ..RunOptions::default()
});
assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
assert!(!run.variables.contains_key("handled"));
let failure = run.result.unwrap_err();
assert_eq!(failure.causes[0].code(), DiagnosticCode::UndefinedStatement);
assert!(failure.causes[0].omissions.is_some());
```

## Operation Diagnostic Admission

Operations admit callback errors before worker handoff and final publication. Configure individual quotas independently of value limits; rejected errors preserve bounded original-category evidence after iterative disposal. See [operation diagnostic rules](operation-diagnostics.md).

```rust
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, grammar::BWErr, operation::{NativeOperation, OperationControl}, signature::StatementSignature};
let operation = NativeOperation::asynchronous(StatementSignature::native("Fail")?, |_, _| async {
    Err(BWErr::NativeError("reason".into()).into())
})?.with_diagnostic_limits(DiagnosticLimits { text_bytes: 11, ..DiagnosticLimits::default() })?;
let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
let error = runtime.block_on(operation.invoke(vec![], OperationControl::default())).unwrap_err();
assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
assert!(error.causes[0].omissions.is_some());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Borrowed Diagnostic Details

Missing names and synchronous native-panic details pass [construction admission](diagnostic-construction.md) before their first string copy. Count complete context plus raw detail bytes; accepted errors retain their original category and source.

```rust
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, run::{Engine, RunLimits, RunOptions}};
let run = Engine::default().run_source("detail", "Missing", RunOptions {
    limits: RunLimits { diagnostics: DiagnosticLimits { text_bytes: 12, ..DiagnosticLimits::default() }, ..RunLimits::default() },
    ..RunOptions::default()
});
// Label "source" plus "Missing" needs 13 UTF-8 bytes.
let error = run.result.unwrap_err();
assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
assert!(error.causes[0].omissions.as_ref().unwrap().source.is_some());
```

## Formatted Signature Errors

Signature argument/return details are counted before their initial message allocation. Quota failures preserve the original incompatible-type category and skip rejected callback entry.

```rust
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, grammar::Literal, operation::{NativeOperation, OperationControl}, signature::{StatementSignature, ValueKind}};
let signature = StatementSignature::native("Read |value|")?.parameter("value", ValueKind::Int)?;
let operation = NativeOperation::asynchronous(signature, |_, _| async { panic!("rejected callback entered") })?
    .with_diagnostic_limits(DiagnosticLimits { text_bytes: 0, ..DiagnosticLimits::default() })?;
let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
let error = runtime.block_on(operation.invoke(vec![Literal::Bool(true)], OperationControl::default())).unwrap_err();
assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
assert_eq!(error.causes[0].code(), DiagnosticCode::IncompatibleType);
assert!(error.causes[0].omissions.is_some());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Collection Access Diagnostic Construction

Path, failing segment, and reason are admitted as a group before copying any field. A rejected group retains bounded collection-error evidence and the original byte location.

```rust
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, run::{Engine, RunLimits, RunOptions}};
let run = Engine::default().run_source("access", "|data| = |{}|\n|out| = |data.missing|", RunOptions {
    limits: RunLimits { diagnostics: DiagnosticLimits { text_bytes: 0, ..DiagnosticLimits::default() }, ..RunLimits::default() },
    ..RunOptions::default()
});
let error = run.result.unwrap_err();
assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
assert_eq!(error.causes[0].code(), DiagnosticCode::CollectionAccess);
assert!(error.causes[0].omissions.as_ref().unwrap().source.is_some());
assert!(!run.variables.contains_key("out"));
```

## Incompatible Operator Diagnostic Construction

Operand descriptions are measured before formatting their owned error message. Runtime rejection preserves the original incompatible-type category and source-byte evidence.

```rust
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, run::{Engine, RunLimits, RunOptions}};
let run = Engine::default().run_source("operator", "|out| = |true + 1|", RunOptions {
    limits: RunLimits { diagnostics: DiagnosticLimits { text_bytes: 0, ..DiagnosticLimits::default() }, ..RunLimits::default() },
    ..RunOptions::default()
});
let error = run.result.unwrap_err();
assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
assert_eq!(error.causes[0].code(), DiagnosticCode::IncompatibleType);
assert!(error.causes[0].omissions.as_ref().unwrap().source.is_some());
```

## Host Input Diagnostic Construction

Input names are checked without parser allocations. Invalid-name and host non-finite-value messages use local diagnostic quotas before formatting owned details; the complete batch is validated before installing any binding.

```rust
use std::collections::BTreeMap;
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, eval::Context, grammar::Literal, run::RunLimits};
let mut context = Context::with_limits(RunLimits {
    diagnostics: DiagnosticLimits { text_bytes: 0, ..DiagnosticLimits::default() },
    ..RunLimits::default()
})?;
let error = context.set_input_variables(BTreeMap::from([("invalid name".into(), Literal::None)])).unwrap_err();
assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
assert_eq!(error.causes[0].code(), DiagnosticCode::Input);
assert!(error.causes[0].span.is_none());
assert!(context.checkpoint().is_err());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Validation Diagnostic Construction

Control-placement and duplicate-parameter diagnostics use local construction limits before formatting their owned locations. Even unreachable invalid controls are rejected before effects; bounded evidence keeps their original category and byte offset.

```rust
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, run::{Engine, RunLimits, RunOptions}};
let run = Engine::default().run_source("validation", "If |false| { Break }", RunOptions {
    limits: RunLimits { diagnostics: DiagnosticLimits { text_bytes: 0, ..DiagnosticLimits::default() }, ..RunLimits::default() },
    ..RunOptions::default()
});
assert_eq!(run.steps, 0);
let error = run.result.unwrap_err();
assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
assert_eq!(error.causes[0].code(), DiagnosticCode::InvalidControl);
assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
```

## Syntax Diagnostic Construction

Syntax details are measured before message construction, including the parser excerpt and underline. A rejected message retains the syntax category and bounded byte evidence without running any statements from the malformed file.

```rust
use botwork::core::{diagnostic::{DiagnosticCode, DiagnosticLimits}, run::{Engine, RunLimits, RunOptions}};
let run = Engine::default().run_source("syntax", "Log |1|\n|x| = |1 +|", RunOptions {
    limits: RunLimits { diagnostics: DiagnosticLimits { text_bytes: 0, ..DiagnosticLimits::default() }, ..RunLimits::default() },
    ..RunOptions::default()
});
assert_eq!(run.steps, 0);
let error = run.result.unwrap_err();
assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
assert_eq!(error.causes[0].code(), DiagnosticCode::Syntax);
assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
```

## Diagnostic Rendering

[Rendering limits](diagnostic-rendering.md) bound complete output and source-position work while leaving structured errors unchanged. A truncated rendering preserves the original primary/cause codes and identifies what was omitted; the returned text owns no source references.

```rust
use botwork::core::{diagnostic::{Diagnostic, DiagnosticCode, DiagnosticRenderLimits, RENDER_SUMMARY_BYTES}, grammar::BWErr};
let error = Diagnostic::new(BWErr::NativeError("destination unavailable".into()));
let rendered = error.render_with_limits(&DiagnosticRenderLimits {
    output_bytes: 0,
    ..DiagnosticRenderLimits::default()
});
assert!(rendered.truncation.is_some());
assert!(rendered.text.starts_with("[BW4002]"));
assert!(rendered.text.contains("diagnostic rendering truncated"));
assert!(rendered.text.len() <= RENDER_SUMMARY_BYTES);
assert_eq!(error.code(), DiagnosticCode::Native);
```

Rethrow copies need [retained diagnostic headroom](retained-diagnostics.md) while the original handler record is still live. A quota failure bypasses outer Catch; successful copying retains the original catchable error.

```rust
use botwork::core::{diagnostic::DiagnosticCode, run::{Engine, RunLimits, RunOptions, RunOutcome, RetainedDiagnosticLimits}};
let source = "Try { Try { Missing } Catch { Rethrow } } Catch {}";
for records in [1, 2] {
    let run = Engine::default().run_source("copy", source, RunOptions {
        limits: RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits { records, ..Default::default() },
            ..Default::default()
        },
        ..Default::default()
    });
    if records == 1 {
        assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
        assert_eq!(run.result.unwrap_err().causes[0].code(), DiagnosticCode::UndefinedStatement);
    } else {
        assert_eq!(run.outcome(), RunOutcome::Succeeded);
    }
}
```
