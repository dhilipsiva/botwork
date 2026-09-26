# Interpreter Architecture

## Parse Once, Execute Owned Syntax

`Program::parse(name, source)` in [ast.rs](../src/core/ast.rs) parses the complete input with Pest and converts its pairs into owned statements and expressions. The returned program has no lifetime dependency on the caller's string. Syntax errors prevent execution of the whole program.

The tree represents assignments, calls, definitions, branches, loops, error handlers, and control statements explicitly. Expressions retain their operator, operands, and grouping. The Pratt parser builds this structure; it no longer evaluates values. Map entries stay in source order until evaluation.

Strings are decoded during construction. Numeric literals retain their source spelling and convert only when evaluated. Consequently an out-of-range number remains a catchable runtime error and does not fail inside an unselected branch or unused definition. Dot access retains its source path and continues to report the temporary unsupported-access error.

## Source Ownership and Locations

Each program shares one `Arc<SourceFile>` containing its name and original UTF-8 text. Statement, expression, block, definition, identifier, and operator spans retain this source. Byte ranges use an exclusive end; `text()` returns the original range, including any whitespace consumed by that grammar node. Parenthesized operands retain their delimiters.

`line_column()` reports one-based lines and Unicode scalar columns. A tab occupies one column; CRLF counts as one line ending. Columns are not display widths or grapheme counts. Span ranges and source contents are private so callers cannot invalidate slicing boundaries.

`Statement::kind()` exposes an immutable view of typed syntax for inspection. `kind_name()` supplies the existing CLI trace labels. Runtime errors do not yet include these spans or statement call stacks; structured diagnostics remain a separate TODO.

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

`evaluate_program` runs owned statements in order. `execute_statement` permits the CLI to emit its existing trace before each statement. Neither entry point invokes the parser. An expression failure returns immediately, before later operands are visited. Boolean short-circuiting remains pending: a successfully evaluated left boolean still proceeds to the right operand at this stage.

The existing `botwork(Pair<Rule>, &mut Context)` entry point lowers its supplied pair once and delegates to the same evaluator. It retains the pair's complete original input so nested offsets remain valid. Prefer the program API when executing a whole file; separate compatibility calls otherwise allocate separate source owners.

## Remaining Interpreter Work

Explicit completion outcomes, invocation scopes, lazy booleans, control-placement validation, and resource limits retain their own roadmap items. Parser-only `Else`/`Catch` wrappers are flattened in the tree; implicit block-result arrays remain transitional behavior, not the specified custom-return contract. The AST refactor does not establish complete language conformance.

[AST unit tests](../src/core/ast/tests.rs) check tree structure and spans. [Execution tests](../tests/ast_execution.rs) exercise ownership and compatibility, and evaluator tests verify shared definition identity and skipped operand evaluation. Both build profiles continue to run the full regression, contract, CLI, and example suites.
