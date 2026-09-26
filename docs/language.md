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

Float literal evaluation and supported numeric operations reject non-finite values/results. Unsupported operand-type errors take priority over checks on host-supplied non-finite values. Finite underflow to zero is allowed. Ordinary mixed arithmetic converts integer operands to `f32`; rounding can lose integer precision (`16777217 + 0.0` becomes `16777216`) or retain a finite maximum after a small addition. Floating powers instead convert their base exactly to `f64` for intermediate calculations. Equality accepts every value kind and validates nested floating values as described below. Host-value serialization remains separate roadmap work.

Arithmetic errors can be handled by `Try/Catch`. An uncaught error stops execution and returns CLI status `1`; direct failed numeric assignments preserve their previous value. See [the arithmetic example](../examples/04-arithmetic-errors.botwork).

## Signed Integer Literals

Integer literals support the full range `-2147483648` through `2147483647`. When unary minus directly wraps an integer atom, the sign and digits convert together at runtime. Whitespace, leading zeroes, and parentheses around that atom are allowed: `- 2147483648`, `-0002147483648`, and `-(2147483648)` all produce the minimum integer. Positive `2147483648` and negative `-2147483649` produce catchable `ParsingIntegerError` values.

Compound operands still evaluate before negation with the usual precedence and intermediate bounds. `--2147483648` overflows when the outer minus negates the minimum value. `-(2147483648 + 0)` fails while converting its positive operand. `-2147483648 ^ 0` likewise attempts the power first and fails on its positive base; `(-2147483648) ^ 0` is `1`. See [the signed-integer example](../examples/10-signed-integers.botwork).

Parsing keeps numeric text unevaluated. Unused definitions, unselected branches, and skipped boolean operands do not trigger literal-conversion errors.

## Numeric Precision and Comparison

Integers are exact `i32` values. Decimal float literals round to IEEE binary32 (`f32`) using nearest-value rounding with ties to even; for example, `16777217.0` becomes `16777216.0`, while `16777219.0` becomes `16777220.0`. Binary floats cannot represent every decimal fraction. Signed zero and finite subnormal values are supported; overflow to infinity is an error and underflow to zero is allowed.

| Operation | Conversion and result |
| --- | --- |
| Integer `+`, `-`, `*`, `%`; nonnegative integer powers | Checked exact `i32` result |
| Mixed/float `+`, `-`, `*`, `%` | Convert integer operands to `f32` first, perform the binary32 operation, return finite `f32` |
| `/`, including integer division | Convert integer operands to `f32` first, return finite `f32` |
| Floating-base or negative-exponent power | Widen base exactly to `f64`, use integer repeated squaring, round the final result to finite `f32` |
| Numeric equality and ordering | Compare stored values exactly by widening both to `f64`; return boolean |

Every `i32` and finite `f32` value is exactly representable in `f64`, so comparisons do not round integers to floats first. `16777217 > 16777216.0` and `2147483647 < 2147483648.0` are true. Float-literal rounding has already happened: `16777217.0 == 16777216` is true. Arithmetic retains its documented rounding: `16777217 + 0.0 == 16777216.0` is also true. Comparison does not change either operand's value or type.

There is no implicit string/boolean conversion, float-to-integer conversion, or approximate equality tolerance. Both signs of zero compare equal. Ordering (`<`, `<=`, `>`, `>=`) accepts numbers only; strings, booleans, None, arrays, and maps produce type errors. Ordering chains remain ordinary binary expressions.

## Value Equality

`==` and `!=` compare every pair of finite language values and return booleans. `!=` is the exact complement of `==`:

| Values | Equal when |
| --- | --- |
| Numbers, including mixed integer/float | Their stored numeric values are exactly equal |
| Booleans | They are both true or both false |
| Strings | Their Unicode contents match exactly, without normalization or case folding |
| None | Both values are None; an undefined variable still raises an error |
| Arrays | Lengths match and values at every corresponding position are equal |
| Maps | Exact string key sets match and every corresponding value is equal; insertion order is irrelevant |
| Different nonnumeric kinds | Never: `1 == true`, `1 == "1"`, and `[] == {}` are false |

Collection equality applies the same rules at every depth. Thus `[1, 2] == [1.0, 2.0]` is true, while `[1, 2] == [2, 1]` is false. Reads/copies compare by value, without identity or alias checks. Both operands and all their collection values evaluate in source order before comparison; `[1] == [2, missing]` still fails when reading `missing`.

Host-supplied NaN and infinities are invalid language values. Equality checks both complete operands, including nested collections, before returning either boolean; an invalid float raises catchable `ArithmeticError` even when shapes or other values differ. This avoids results depending on map iteration order. Resource limits remain separate work.

Compatibility: earlier prototypes returned type errors for collection/None equality and incompatible scalar kinds. They also rounded integer operands during mixed numeric comparisons. The rules above intentionally replace those behaviors. See [the comparison example](../examples/13-value-comparisons.botwork).

## Strings

Double quotes delimit a string; they are not part of its value. `"a" + "b"` produces the same value as `"ab"`. Strings preserve Unicode text without normalization and may contain literal newlines, pipes, comment markers, and braces.

Three escape sequences are supported:

| Source text | Value |
| --- | --- |
| `\n` | A newline |
| `\"` | A double quote |
| `\\` | A backslash |

Escapes are decoded once. For example, `"\\n"` contains a backslash followed by `n`; it does not contain a newline. Other escapes, including `\t`, are syntax errors. Map keys can be identifiers such as `label` in `{label: "hello"}` or quoted strings such as `"display name"`; quoted keys and values use the same decoding rules.

## Log Output

`Log |value|` writes the value followed by a newline to stdout and returns that value. Top-level strings are printed as their contents, including literal newlines. Numbers and booleans use plain text; the internal absent value displays as `none`.

Arrays use brackets and maps use braces. Nested strings and map keys are quoted with escaped newlines, quotes, backslashes, and other control characters. Map keys are sorted lexicographically for stable output: `Log |{z: 2, a: 1}|` prints `{"a": 1, "z": 2}`. This is a human-readable display format, not a serialization contract or a promise that every displayed value can be parsed as DSL source.

Output failures become evaluation errors. Errors are reported on stderr with a nonzero process status when uncaught. Pass `--debug` to the CLI to add top-level statement locations and kinds on stderr; it does not copy statement contents or trace nested execution. Normal logging remains on stdout.

## Variables and Invocation Scope

Every custom call gets fresh parameter and local bindings. Its arguments evaluate once, left to right, in the caller before parameters are installed. If an argument fails, no parameters or invocation locals are installed. With caller `x = 10`, `Pair |1| with |x|` therefore receives `1, 10` even when its parameters are named `x` and `y`.

Variables and statement names resolve from the current invocation through the environment where its definition was registered. Reads see the latest values there. An unrelated caller's private variables and definitions are invisible. Assignments create or update a binding in the current frame, shadowing outer bindings without changing them. Each recursive call has its own parameters and locals; nested definitions disappear when their defining invocation finishes. See [the scopes example](../examples/09-scopes.botwork).

If, While, and Try/Catch bodies share their enclosing invocation or script frame. A For iterator is temporary in that frame: after normal completion, Break, Return, or an error, its previous local value is restored, or the temporary binding is removed if none existed. An inherited value then becomes visible again. Continue retains the iterator for the next iteration, and empty iteration leaves existing bindings unchanged. Other loop-body assignments persist in the enclosing frame.

## While Loops

`While |condition| { ... }` evaluates its boolean condition before each iteration, including the first. A false condition skips the body. Normal completion and `Continue` reevaluate the condition; `Continue` skips the rest of the current body. `Break` exits the loop immediately. A condition that is not boolean raises an evaluation error, including when its type changes during execution.

`Return` inside a loop exits its containing custom statement. Loops consume only their own `Break` and `Continue`; a custom call cannot transfer those controls to its caller's loop.

## Returns and Control Flow

`Return |value|` evaluates its expression once and returns that exact value from the containing custom statement. It crosses nested `If`, `For`, `While`, `Try`, and `Catch` blocks, skipping every remaining statement in the invocation. A return at the end of a body behaves identically to one followed by unreachable statements. The caller resumes after its call.

Bare `Return` and custom statements that finish without returning a value produce `None` (displayed as `none`). Normally completed definitions and control constructs also produce `None`. Blocks and loops do not collect their statements' results into arrays; an explicit array or map return preserves that value. Assignment and native `Log` retain their value results. See [the control-flow example](../examples/08-control-flow.botwork).

No pending control state survives an invocation, and its local variables and definitions are discarded on completion, return, or error.

## Control-Placement Validation

The complete program is checked before any execution. `Return` requires a custom-statement body; top-level Return is invalid, including inside a script-level loop. `Break` and `Continue` require an enclosing For/While in the same invocation. A custom definition nested inside a loop starts its own control scope and cannot break or continue that outer loop.

Validation checks unused definitions, unselected branches, handlers that never run, and unreachable statements. For example, `Unused { Break }` fails even without a call to `Unused`. A valid `Break` inside `While |false| { Break }` remains allowed because the enclosing loop is present.

Invalid placement produces `ControlFlowError` with the offending source file and one-based line/column; columns count Unicode scalars and each tab as one column. The CLI exits with status `1` before any Log output or debug trace. Try/Catch cannot recover these validation errors. Syntax errors are reported first; numeric conversions, name resolution, and expression evaluation still happen at runtime.

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

This contract covers evaluation errors, including the arithmetic failures described above. Valid `Return`, `Break`, and `Continue` pass through Try/Catch without running its handler. An error evaluating a return expression remains catchable, and a handler can return a fallback value or raise another error. Preserving structured error causes remains a separate roadmap item.

## Collection Access

Collection reads visit values left to right: `m.a`, `items.0`, `items[index]`, and `m.items[index]["display name"]`. The base can be a variable, literal, or parenthesized expression: `[7, 8][1]`, `{items: [7]}.items[0]`, and `([7] + [8])[1]` work. Access binds before powers and unary operators; `-items[0] ^ 2` negates the square of the selected value. Variables and computed keys use ordinary lexical lookup.

Dot segments are literal names/digits: `items.index` does not evaluate `index`. Map keys preserve exact, case-sensitive spelling; array indexes require ASCII decimal digits. Leading zeroes are accepted for arrays (`items.01` selects index 1), while map keys retain their spelling (`m.01` selects `"01"`). Whitespace and comments between path tokens do not become part of names. Negative dot indexes such as `items.-1` are syntax errors.

Bracket expressions produce **nonnegative integers for arrays** or **strings for maps**, without coercion. `items[1.0]`, `items["1"]`, `items[-1]`, and `m[1]` fail; `items[0]` and `m["1"]` use different key types. Indexes are zero-based and checked against length. Nested reads such as `items[positions[0]]` work. Integer conversion and arithmetic inside a key retain their ordinary runtime failures.

Map literals accept identifier or quoted string keys: `{"Content-Type": "application/json", "": 7, café: 8}`. Quoted keys use the existing string escapes, including escaped quotes, backslashes, and newlines. Punctuation, Unicode, delimiters, numeric text, and reserved words can appear in quoted keys. Keys are decoded once; map values still evaluate in source order. Computed map-literal keys such as `{[key]: value}` are unsupported.

Evaluate the base once, then each bracket expression once immediately before its lookup. Check the receiver and key types after evaluating that key. Thus `7[missing]` reports the undefined key variable; `missing[1 / 0]` reports the undefined base. Stop at the first failed evaluation or lookup, skipping all later segments. Short-circuiting can skip an entire access, but malformed skipped syntax still prevents execution.

A missing variable yields `VariableNotDefined`. Missing map keys, invalid indexes, bounds failures, and traversal through scalars or None yield catchable `CollectionAccessError`. It identifies the path, failing segment (brackets included for computed keys), and reason. Dot names are canonicalized; bracket contents retain their source spelling. A present None-valued entry succeeds. Errors preserve a failed assignment's destination.

Reads return independent values without changing their source container. Both dot and bracket indexed assignment are syntax errors. Updates are reserved for the planned collection library: operations will return replacement collections for ordinary variable assignment; no dedicated indexed-update syntax is introduced. These update statements are not implemented yet. Access works in expressions, conditions, arguments, returns, and loop iterables. See examples [11](../examples/11-collection-access.botwork) and [12](../examples/12-computed-access.botwork).
