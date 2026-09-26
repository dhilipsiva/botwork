# Language Behavior

This reference records implemented behavior as language TODOs are completed. It is not yet the complete language specification required by the roadmap.

## Binary Operator Precedence

These levels run from weakest to strongest binding:

| Level | Operators |
| --- | --- |
| 1 | `or` |
| 2 | `and` |
| 3 | `==`, `!=` |
| 4 | `<`, `<=`, `>`, `>=` |
| 5 | `+`, binary `-` |
| 6 | `*`, `/`, `%` |
| 7 | `^` |

Thus `1 + 2 == 3` means `(1 + 2) == 3`, and `1 < 2 == 3 < 4` compares two boolean comparison results. `true or false and false` evaluates to `true`; `(true or false) and false` evaluates to `false`. Parentheses select grouping explicitly.

Addition/subtraction and multiplication/division/remainder associate left within their respective levels: `20 - 5 - 2` gives `13`, and `12 / 3 / 2` gives `2.0`. Unary minus and logical negation remain supported, including `3 - -2` and `!(1 > 2)`. Invalid operand combinations produce type errors rather than implicit boolean/numeric coercion.

Run `cargo run -- --file examples/03-precedence.botwork` for an executable example. Exponent-chain associativity, the complete unary/power precedence contract, comparison-chain semantics, and boolean short-circuiting remain separate roadmap work. In particular, `and` and `or` still evaluate both operands at this stage.

## Strings

Double quotes delimit a string; they are not part of its value. `"a" + "b"` produces the same value as `"ab"`. Strings preserve Unicode text without normalization and may contain literal newlines, pipes, comment markers, and braces.

Three escape sequences are supported:

| Source text | Value |
| --- | --- |
| `\n` | A newline |
| `\"` | A double quote |
| `\\` | A backslash |

Escapes are decoded once. For example, `"\\n"` contains a backslash followed by `n`; it does not contain a newline. Other escapes, including `\t`, are syntax errors. Map keys such as `label` in `{label: "hello"}` are identifiers; string decoding applies to the quoted value.

## Log Output

`Log |value|` writes the value followed by a newline to stdout and returns that value. Top-level strings are printed as their contents, including literal newlines. Numbers and booleans use plain text; the internal absent value displays as `none`.

Arrays use brackets and maps use braces. Nested strings and map keys are quoted with escaped newlines, quotes, backslashes, and other control characters. Map keys are sorted lexicographically for stable output: `Log |{z: 2, a: 1}|` prints `{"a": 1, "z": 2}`. This is a human-readable display format, not a serialization contract or a promise that every displayed value can be parsed as DSL source.

Output failures become evaluation errors. Errors are reported on stderr with a nonzero process status when uncaught. Pass `--debug` to the CLI to add top-level statement locations and kinds on stderr; it does not copy statement contents or trace nested execution. Normal logging remains on stdout.

## While Loops

`While |condition| { ... }` evaluates its boolean condition before each iteration, including the first. A false condition skips the body. Normal completion and `Continue` reevaluate the condition; `Continue` skips the rest of the current body. `Break` exits the loop immediately. A condition that is not boolean raises an evaluation error, including when its type changes during execution.

Nested `Return` propagation remains a tracked defect; the completed loop-iteration fix does not establish correct function-return behavior.

## Try/Catch

`Try` requires exactly one `Catch` block. Use the existing `} Catch {` layout: `Catch` starts on the same line as the try block's closing brace. Both blocks may be empty; keywords are case-insensitive, and complete `Try/Catch` statements may nest.

```botwork
Try {
    |value| = |missing|
} Catch {
    Log |"recovered"|
}
```

The try body runs once. If it succeeds, the handler is skipped. On an evaluation error, the remaining try-body statements are skipped and the handler runs once. A successful handler resumes execution after the whole construct. An error in the handler propagates to an enclosing try or becomes an uncaught error; it does not rerun the same handler. Work completed before an error is preserved.

A missing, orphaned, or malformed `Catch` is a syntax error. The CLI parses the entire file before execution, so this prevents even earlier `Log` statements from running. Syntax errors cannot be caught by a script. Write ordinary statements directly when no handler is intended.

This contract covers returned evaluation errors. Converting arithmetic panics into catchable errors, preserving structured error causes, and correcting nested `Return` remain separate roadmap items.

## Collection Access Status

Dot access such as `m.a`, `items.0`, or `m.items.0` is accepted syntax but is not implemented yet. Evaluating it returns `UnsupportedAccessError` with a diagnostic such as `Collection access is unsupported: m.items.0`. No base-variable lookup or index conversion is attempted, so an undefined base receives the same unsupported-feature error.

The error propagates through expressions, collections, conditions, and call arguments. A direct assignment such as `|answer| = |m.a|` preserves the destination's previous value when access fails; caller-state preservation during custom calls remains a separate defect. `Try/Catch` can handle the error; an uncaught error stops execution with CLI status `1`. Access in an unselected `If` branch is not evaluated. Malformed paths remain syntax errors.

This temporary contract prevents interpreter panics. Actual key/index lookup and its missing-key, bounds, and type errors remain planned work; its implementation must replace the temporary unsupported-access expectations in the tests.
