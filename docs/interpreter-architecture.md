# Interpreter Architecture

## Parse Once, Execute Owned Syntax

`Program::parse(name, source)` in [ast.rs](../src/core/ast.rs) parses the complete input with Pest, converts its pairs into owned statements and expressions, then validates control placement. The returned program has no lifetime dependency on the caller's string. Syntax and placement errors prevent execution of the whole program.

The tree represents assignments, calls, definitions, branches, loops, error handlers, and control statements explicitly. Expressions retain their operator, operands, and grouping. The Pratt parser builds this structure; it no longer evaluates values. Map entries stay in source order until evaluation.

Strings and quoted map keys are decoded during construction. Numeric literals retain their source spelling and convert only when evaluated. Consequently an out-of-range number remains a catchable runtime error and does not fail inside an unselected branch or unused definition. `ExprKind::Access` stores a base expression and ordered `AccessSegment` values: literal names or computed expressions with bracket spans. Comments and whitespace do not become part of dot names. Array-index conversion remains deferred until lookup. Postfix access binds before power and unary operators; grouped bases preserve their parentheses in source spans.

Unary syntax retains its operator and operand spans. At runtime, a minus directly wrapping an integer atom converts the signed text together so the minimum `i32` literal is representable. Negation of compound expressions still evaluates the operand first and uses checked arithmetic; power grouping remains unchanged.

Expression evaluation takes an immutable context. Collection lookup borrows a variable's containers, or owns a computed base using `Cow`, then clones only the selected value. Computed keys evaluate once in order, immediately before receiver/key validation and lookup; a failed step skips later expressions. This preserves borrowing while allowing indexes to read the same context. Test-only expression visitation uses interior mutability and is absent from production builds.

Maps use exact string keys; arrays require ASCII digits for dot segments or nonnegative integer values for brackets. Bounds are checked without panicking on index overflow. Access errors include the path, failing segment, and reason; computed segments retain their bracket source text. Reads have no mutation operation. Indexed updates are reserved for future library statements returning replacement values.

## Source Ownership and Locations

Each program shares one `Arc<SourceFile>` containing its name and original UTF-8 text. Statement, expression, block, definition, identifier, and operator spans retain this source. Byte ranges use an exclusive end; `text()` returns the original range, including any whitespace consumed by that grammar node. Parenthesized operands retain their delimiters.

`line_column()` reports one-based lines and Unicode scalar columns. A tab occupies one column; CRLF counts as one line ending. Columns are not display widths or grapheme counts. Span ranges and source contents are private so callers cannot invalidate slicing boundaries.

`Statement::kind()` exposes an immutable view of typed syntax for inspection. `kind_name()` supplies the existing CLI trace labels. Control-placement errors use the offending statement's original file, line, and column. Runtime errors do not yet include these spans or statement call stacks; structured diagnostics remain a separate TODO.

## Validate Control Placement

`Program::validate()` walks all statements in source order, including unused definitions, skipped branches and handlers, and statements after an unconditional control transfer. It tracks whether a custom definition and a loop enclose each statement. Entering a definition resets loop permission; entering a loop preserves definition permission. Branches and handlers inherit both. `Return` requires a custom body; `Break` and `Continue` require a loop in that same body or at script level.

The first invalid placement returns `ControlFlowError` with the offending statement's original span. Validation does not evaluate expressions, convert numbers, resolve names, or catch errors. Full parsing/lowering finishes before validation, so syntax errors take precedence.

## Reuse Definitions

Executing a definition registers its `Arc<Definition>` in the context. Each invocation shares the same parsed parameter list and body. No invocation reparses the definition, reconstructs its expressions, or clones its whole syntax tree. The definition and its original source locations remain usable after the caller drops the defining program and input string.

Retained definitions keep their complete source file alive. Dropping the context releases them unless another program/context owns a reference. Local registration follows the defining invocation's lifetime; the immutable nested syntax can remain part of its outer definition's shared body.

## Invocation Frames

`Context` owns a stack of frames with separate variable and statement maps. The root frame persists across programs executed in that context. Each invocation adds a fresh frame whose parent index points to the frame containing its resolved definition. Lookup follows those lexical parents, skipping unrelated caller locals. Assignment and definition registration write only the current frame. Reads observe current bindings in that environment, not declaration-time snapshots.

Calls resolve their statement first and evaluate all arguments left to right in the caller before creating the new frame. On normal completion, Return, or an evaluation error, the frame is removed and execution resumes in the saved caller frame. Nested definitions cannot escape as first-class values, so their defining frames stay alive for every valid call. Parent indexes avoid reference cycles; cloning a context copies bindings and frame links while sharing immutable syntax.

If, While, and Try/Catch bodies share their enclosing frame. For evaluates its iterable first, saves only the iterator's binding in the current frame, and restores it on every completion path. If no local binding existed, it removes the iterator so a lexical ancestor's value becomes visible again. The body can still change other bindings in its enclosing frame.

## Execution API

```rust
use botwork::core::{ast::Program, eval::{evaluate_program, Context}};

let program = Program::parse("example.botwork", "|answer| = |2 ^ 3 ^ 2|")
    .expect("valid program");
let mut context = Context::default();
context.init_statements(); // Register native Log.
let result = evaluate_program(&program, &mut context).expect("successful execution");
```

`evaluate_program` validates its entire statement list before executing any statement. This also checks programs assembled by Rust callers from extracted syntax nodes. `execute_statement` validates its subtree at script scope. Neither entry point invokes the parser; internal loops and invocations do not repeat validation. The CLI finishes parsing/validation before any statement trace or output. An expression failure returns immediately, before later operands are visited. For `and` and `or`, the evaluator checks the left boolean and selects whether to visit the right expression. The value-level operator API remains strict when both values are supplied.

The existing `botwork(Pair<Rule>, &mut Context)` entry point lowers its supplied pair once, validates the resulting statement/block at script scope, and delegates to the same evaluator. It retains the pair's complete original input so nested offsets remain valid. Validation covers only the supplied subtree, so use the program API to reject a later invalid statement before earlier effects in a whole file. Separate compatibility calls allocate separate source owners.

## Completion Outcomes

Internal statement execution returns `Result<Completion, BWErr>`. `Completion` distinguishes a normal value from `Return(value)`, `Break`, and `Continue`. Blocks discard ordinary statement values and stop immediately on control transfer or error. Normally completed blocks, loops, branches, and handlers yield `None`; they retain no implicit result arrays.

Branches and Try/Catch pass control outcomes upward. A handler runs only for an evaluation error; a failed return expression is still an error until its value exists. For/While consume their own Break/Continue and propagate Return. A custom invocation consumes Return, preserving its exact value; fallthrough produces None. No control flags are stored in the context.

Runtime boundaries retain defensive checks for escaping controls, including a callee attempting to control its caller's loop. Public entry points reject invalid placement before execution, so a script cannot catch or bypass a placement error.

## Remaining Interpreter Work

Resource limits, the complete value/naming contracts, imports, structured runtime diagnostics, and adapter APIs retain their own roadmap items. Recursion is supported but not yet bounded. Scope and completion checks do not establish complete language conformance or the release quality gates.

[AST unit tests](../src/core/ast/tests.rs) check tree structure and spans. [Execution tests](../tests/ast_execution.rs) exercise ownership and compatibility, and evaluator tests verify shared definition identity and skipped operand evaluation. Both build profiles continue to run the full regression, contract, CLI, and example suites.
