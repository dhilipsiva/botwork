# Language Behavior

This reference records implemented behavior as language TODOs are completed. The [core language specification](language-specification.md) defines the intended semantics and identifies pending implementation work.

## Keywords and Names

Expression keywords are lowercase: `true`, `false`, `and`, and `or`. They are reserved as complete identifiers, so `|or| = |7|` is invalid, while `order`, `trueValue`, `falsehood`, and `android` are valid names. An expression keyword cannot be immediately followed by an identifier continuation: a Unicode letter, Unicode number, or underscore. This also prevents `true andfalse` from being read as `true and false`. Variables remain case-sensitive; `True` is an identifier, not a boolean literal.

Control keywords (`If`, `Else`, `For`, `Break`, `Return`, `Continue`, `While`, `Try`, and `Catch`) are case-insensitive and reserved at the start of a statement. They must be contiguous and followed by a space, tab, line ending, parameter pipe, brace, comment marker, or end of input. `If|true|{}` is valid. `Format report`, `Elsewhere`, `Break!`, and `Return-value` are whole custom statement names. Spaces or comments between letters do not form a control keyword; a name such as `I f` can be defined as a custom statement.

`In` follows the same keyword rules within `For |item| In |items| { ... }`, but remains available in custom names such as `In order`. Comments may separate complete tokens. Parentheses can delimit boolean operators: `(true)and(false)` is valid. See [the keyword example](../examples/06-keywords.botwork).

Identifier characters, multilingual normalization, statement-name collisions, and full whitespace/line-termination rules remain separate specification work. This change preserves the existing identifier alphabet and fixes prefix matching.

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

Comparisons at the same level associate left as ordinary binary operators: `1 < 2 < 3` fails when comparing a boolean with an integer, while `1 == 2 == false` evaluates to `true`. Run `cargo run -- --file examples/03-precedence.botwork` for an executable precedence example.

## Boolean Short-Circuiting

`and` and `or` evaluate their left operand once and require a boolean. They evaluate the right operand only when it can affect the result:

| Expression | Evaluate `rhs`? | Result |
| --- | --- | --- |
| `false and rhs` | No | `false` |
| `true and rhs` | Yes | The boolean value of `rhs` |
| `true or rhs` | No | `true` |
| `false or rhs` | Yes | The boolean value of `rhs` |

A skipped operand is neither evaluated nor type-checked: `false and missing`, `false and 1`, and `true or (1 / 0)` all succeed. A required operand still raises its usual evaluation or type error. The left type is checked first, so `1 and missing` reports an incompatible left type without looking up `missing`. There is no truthiness conversion.

Grouping controls selection: `true or false and missing` is `true`, but `(true or false) and missing` fails. These rules apply inside collections, calls, conditions, and other expressions. A loop guarded by `i < 3 and 6 / (3 - i) > 0` can stop at `i = 3` without dividing by zero. A skipped error never triggers `Catch`; a required operand's error remains catchable. See [the short-circuit example](../examples/07-short-circuit.botwork).

The entire file is still parsed before execution. Invalid syntax such as `true or (1 +)` prevents all execution, including earlier statements; short-circuiting skips runtime evaluation only.

## Powers and Unary Operators

Powers associate right: `2 ^ 3 ^ 2` means `2 ^ (3 ^ 2)` and produces `512`. Parentheses override grouping: `(2 ^ 3) ^ 2` produces `64`.

Unary minus and logical negation (`!`) bind more tightly than multiplication but less tightly than a power to their right. Thus `-2 ^ 2` is `-(2 ^ 2)`, producing `-4`, while `(-2) ^ 2` produces `4`. Prefixes may repeat and apply from right to left: `--2` is `2`, `!!true` is `true`, and `- - -2` is `-2`. Unary minus requires a number; `!` requires a boolean. Unary plus is unsupported.

The exponent may start with a unary operator. `2 ^ -2` produces `0.25`; `2 ^ -2 ^ 2` means `2 ^ (-(2 ^ 2))`, producing `0.0625`. Multiplication stays outside that exponent: `2 ^ -2 * 4` produces `1.0`.

Every intermediate operation must satisfy the arithmetic contract below. `-2 ^ 31` fails because the positive intermediate `2 ^ 31` exceeds the integer range; `(-2) ^ 31` produces `-2147483648`. Likewise, `2 ^ 2 ^ -1` fails the integer-exponent requirement after its inner power produces `0.5`. Missing operands and unmatched parentheses are syntax errors; evaluation errors in operands propagate and remain catchable. See [the powers example](../examples/05-powers.botwork).

## Arithmetic Boundaries and Errors

Integers currently use signed 32-bit values; floats use 32-bit binary floating point. Integer `+`, `-`, `*`, unary negation, and nonnegative integer powers return `ArithmeticError` when the result exceeds the integer range. Results are checked in debug and release builds. Invalid operand types retain `OperationIncompatibleError`.

Division always returns a float. Division and remainder reject a zero divisor, including either sign of floating zero. Integer remainder follows the dividend's sign; `(-2147483647 - 1) % -1` returns integer `0`, while division of the same operands returns floating `2147483648.0`.

Powers require an integer exponent. A nonnegative exponent with an integer base returns an integer; a negative exponent or floating base returns a float. `0 ^ 0` is `1`, while zero to a negative power is an arithmetic error. Floating powers preserve integer exponent parity and use at most 32 repeated-squaring steps, wider intermediates, and one final rounding to `f32`. For example, `2 ^ -3` is `0.125`, and `2 ^ -149` remains a positive subnormal value.

Float literal evaluation and supported numeric operations reject non-finite values/results. Operand-type errors take priority over checks on host-supplied non-finite values. Finite underflow to zero is allowed. Ordinary mixed arithmetic converts integer operands to `f32`; rounding can lose integer precision (`16777217 + 0.0` becomes `16777216`) or retain a finite maximum after a small addition. Floating powers instead convert their base exactly to `f64` for intermediate calculations. These checks do not redesign numeric precision, comparison, or host-value serialization contracts, which remain roadmap work.

Arithmetic errors can be handled by `Try/Catch`. An uncaught error stops execution and returns CLI status `1`; direct failed numeric assignments preserve their previous value. See [the arithmetic example](../examples/04-arithmetic-errors.botwork). The current positive-integer literal conversion cannot represent the magnitude in `-2147483648` directly; use `(-2147483647 - 1)` pending the full literal/value contract.

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

This contract covers returned evaluation errors, including the arithmetic failures described above. Preserving structured error causes and correcting nested `Return` remain separate roadmap items.

## Collection Access Status

Dot access such as `m.a`, `items.0`, or `m.items.0` is accepted syntax but is not implemented yet. Evaluating it returns `UnsupportedAccessError` with a diagnostic such as `Collection access is unsupported: m.items.0`. No base-variable lookup or index conversion is attempted, so an undefined base receives the same unsupported-feature error.

The error propagates through expressions, collections, conditions, and call arguments. A direct assignment such as `|answer| = |m.a|` preserves the destination's previous value when access fails; caller-state preservation during custom calls remains a separate defect. `Try/Catch` can handle the error; an uncaught error stops execution with CLI status `1`. Access in an unselected `If` branch is not evaluated. Malformed paths remain syntax errors.

This temporary contract prevents interpreter panics. Actual key/index lookup and its missing-key, bounds, and type errors remain planned work; its implementation must replace the temporary unsupported-access expectations in the tests.
