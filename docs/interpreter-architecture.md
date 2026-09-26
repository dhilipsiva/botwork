# Interpreter Architecture

## Parse Once, Execute Owned Syntax

`Program::parse(name, source)` in [ast.rs](../src/core/ast.rs) parses the complete input with Pest, converts its pairs into owned statements and expressions, then validates control placement. The returned program has no lifetime dependency on the caller's string. Syntax and placement errors prevent execution of the whole program.

The tree represents assignments, calls, definitions, branches, loops, error handlers, and control statements explicitly. Expressions retain their operator, operands, and grouping. The Pratt parser builds this structure; it no longer evaluates values. Map entries stay in source order until evaluation.

Strings are decoded during construction. Numeric literals retain their source spelling and convert only when evaluated. Consequently an out-of-range number remains a catchable runtime error and does not fail inside an unselected branch or unused definition. Dot access retains its source path and continues to report the temporary unsupported-access error.

## Source Ownership and Locations

Each program shares one `Arc<SourceFile>` containing its name and original UTF-8 text. Statement, expression, block, definition, identifier, and operator spans retain this source. Byte ranges use an exclusive end; `text()` returns the original range, including any whitespace consumed by that grammar node. Parenthesized operands retain their delimiters.

`line_column()` reports one-based lines and Unicode scalar columns. A tab occupies one column; CRLF counts as one line ending. Columns are not display widths or grapheme counts. Span ranges and source contents are private so callers cannot invalidate slicing boundaries.

`Statement::kind()` exposes an immutable view of typed syntax for inspection. `kind_name()` supplies the existing CLI trace labels. Control-placement errors use the offending statement's original file, line, and column. Runtime errors do not yet include these spans or statement call stacks; structured diagnostics remain a separate TODO.

## Validate Control Placement

`Program::validate()` walks all statements in source order, including unused definitions, skipped branches and handlers, and statements after an unconditional control transfer. It tracks whether a custom definition and a loop enclose each statement. Entering a definition resets loop permission; entering a loop preserves definition permission. Branches and handlers inherit both. `Return` requires a custom body; `Break` and `Continue` require a loop in that same body or at script level.

The first invalid placement returns `ControlFlowError` with the offending statement's original span. Validation does not evaluate expressions, convert numbers, resolve names, or catch errors. Full parsing/lowering finishes before validation, so syntax errors take precedence.

## Reuse Definitions

Executing a definition registers its `Arc<Definition>` in the context. Each invocation shares the same parsed parameter list and body. No invocation reparses the definition, reconstructs its expressions, or clones its whole syntax tree. The definition and its original source locations remain usable after the caller drops the defining program and input string.

Retained definitions keep their complete source file alive. Dropping the context releases them unless another program/context owns a reference. Invocation-local binding and definition visibility still await the scope implementation specified in the [core contract](language-specification.md).

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

Invocation scopes and resource limits retain their own roadmap items. Parameter binding still uses shared context variables, and For bindings are not yet restored. The completion and placement changes do not establish those scope guarantees or complete language conformance.

[AST unit tests](../src/core/ast/tests.rs) check tree structure and spans. [Execution tests](../tests/ast_execution.rs) exercise ownership and compatibility, and evaluator tests verify shared definition identity and skipped operand evaluation. Both build profiles continue to run the full regression, contract, CLI, and example suites.
