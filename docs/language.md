# Language Behavior

This reference records implemented behavior as language TODOs are completed. The [core language specification](language-specification.md) defines the intended semantics and identifies pending implementation work.

## Values and Supported Operations

The seven value kinds are None, boolean, `i32` integer, finite `f32` float, Unicode string, ordered array, and string-keyed map. A variable or map entry bound to None is present; an absent variable or key raises its lookup error. None has no source literal: a bare `Return` or custom-statement fallthrough produces it. Its displayed spelling `none` is an ordinary variable name in source, not a reserved literal.

Only booleans are accepted by `If`, `While`, `!`, `and`, and `or`; no value has implicit truthiness. `For` accepts arrays only, including empty arrays. Empty strings/maps/arrays and numeric zero do not turn into false. Conversion is limited to the [numeric rules](#numeric-precision-and-comparison); strings are not parsed as numbers and booleans are not integers.

| Operation | Accepted operands | Result |
| --- | --- | --- |
| `+` | Two numbers; two strings; two arrays | Numeric sum; concatenated string; concatenated array |
| Binary `-`, `*`, `/`, `%` | Two numbers | Number under the numeric contract |
| `^` | Numeric base and integer exponent | Number under the power contract |
| Unary `-` | Number | Same numeric kind, with overflow checks |
| `<`, `<=`, `>`, `>=` | Two numbers | Boolean |
| `==`, `!=` | Any two finite values | Boolean under structural equality |
| `!` | Boolean | Boolean |
| `and`, `or` | Boolean left and, when evaluated, boolean right | Boolean with short-circuiting |
| Dot/bracket access | Map/string key or array/nonnegative integer index | Selected value under the access contract |

All other operator/value-kind combinations produce catchable `OperationIncompatibleError`, except access failures use `CollectionAccessError`. There is no map merge, string/array repetition, implicit string concatenation with numbers, collection ordering, or unary plus. Invalid arithmetic values have their specified range/divisor errors. Operator precedence and comparison chains follow ordinary binary grouping: `1 == 1 == 1` is false because `(1 == 1)` is a boolean, unequal to integer `1`.

## Collection Ownership and Map Order

Arrays and maps are owned values. Variable reads, assignment copies, argument binding, explicit returns, and exported Rust results do not share mutable collection storage. Reassigning a copy or a callee's parameter does not change the original; concatenation creates a replacement array. Nested elements follow the same copy rules. Indexed assignment is unsupported, and future collection-update statements will return replacement values. A For iterable is evaluated once, so reassigning its source during the loop does not replace the iteration sequence.

Map literals evaluate all values in source order, including duplicate entries. If multiple keys decode to the same exact string, the last value wins: `{a: 1, "a": 2}` contains `a: 2`. An error in any earlier or overwritten value still stops evaluation; a failed assignment preserves its old destination. Quoted escapes are decoded before key matching, with no Unicode normalization or case folding.

Maps have no insertion-order contract. Direct `For` iteration over a map is a type error; use an explicit ordered key array, such as `For |key| In |["z", "a"]| { Log |map[key]| }`, when order matters. Rust `Literal::Map` iteration inherits `HashMap`'s unspecified order. Map equality ignores order, while displayed map keys are sorted lexicographically.

## Whitespace, Lines, and Comments

ASCII spaces and tabs separate syntax tokens; indentation has no semantic meaning. LF and CRLF are supported line endings. Bare CR outside strings/comments is invalid. Other Unicode whitespace is not syntax whitespace; Unicode text inside strings or sentence names is preserved under the naming rules. Empty files, blank lines, and comment-only files are valid.

Expressions inside `|...|` may span lines, including around binary/unary operators, parentheses, map colons, and dot/bracket access. Arrays and maps allow line breaks, comments, and an optional trailing comma. Commas remain mandatory between entries. A complete operator token cannot be split: `<` followed by a newline and `=` is invalid.

Assignment tokens, required control-header tokens, and opening braces may be on separate lines. `Else` and `Catch` may follow their preceding closing brace on the same line or after blank/comment lines; they attach to that preceding construct. An orphaned handler/branch remains a syntax error. One-line blocks with self-delimiting statements, such as `{ |x| = |7| Return |x| }`, remain valid. Write each statement on its own line for readability.

Outside open expressions, a newline ends a custom sentence. `First Second` is one call name; `First` and `Second` on separate lines are two calls. Continue a call or definition header explicitly with `\`, followed only by optional spaces/tabs and LF/CRLF, then the next sentence part or parameter:

<!-- botwork-test: multiline-call -->
```botwork
Pair |first| with \
    |second| { Return |[first, second]| }
|answer| = Pair |1| with \
    |2|
Log |answer|
```

Continuation markers do not enter the statement signature. A trailing marker without another part, a comment after the marker, or an intervening blank/comment-only line is invalid. Backslash outside strings is reserved for this continuation syntax. Bare `Return` ends at its line boundary; open its parameter pipe before breaking the line, or use explicit `Return \` followed by `|value|`. It never silently consumes the next line's assignment.

`#` starts a line comment, including `##`; `###` exclusively opens a block comment ending at the next `###`. Block comments do not nest and must close, including at EOF. Comments separate tokens but cannot split identifiers, keywords, or multi-character operators. Newlines inside block comments belong to that whitespace token and do not end a sentence. Comment markers and delimiters inside quoted strings remain literal contents.

Outside strings, `|` delimits parameters/expressions, braces delimit blocks or maps by context, brackets delimit arrays/access, and parentheses group expressions. `#` and `\` have the comment/continuation meanings above. There is no semicolon statement separator; punctuation otherwise belongs to sentence text or the expression grammar. String escapes are limited to the [three documented forms](#strings). Literal strings preserve their exact line endings and Unicode text. See [the multiline example](../examples/14-multiline-layout.botwork).

## Keywords and Names

Expression keywords are lowercase: `true`, `false`, `and`, and `or`. They are reserved as complete identifiers, so `|or| = |7|` is invalid, while `order`, `trueValue`, `falsehood`, and `android` are valid names. An expression keyword cannot be immediately followed by an identifier continuation, including a combining mark. This prevents `true andfalse` from being read as `true and false`. Variables remain case-sensitive; `True` is an identifier, not a boolean literal.

Control keywords (`If`, `Else`, `For`, `Break`, `Return`, `Continue`, `While`, `Try`, and `Catch`) are case-insensitive and reserved at the start of a statement. They must be contiguous and followed by a space, tab, line ending, parameter pipe, brace, comment marker, or end of input. `If|true|{}` is valid. `Format report`, `Elsewhere`, `Break!`, and `Return-value` are whole custom statement names. Spaces or comments between letters do not form a control keyword; a name such as `I f` can be defined as a custom statement.

`In` follows the same keyword rules within `For |item| In |items| { ... }`, but remains available in custom names such as `In order`. Comments may separate complete tokens. Parentheses can delimit boolean operators: `(true)and(false)` is valid. See [the keyword example](../examples/06-keywords.botwork).

Identifier characters follow the Unicode rules below. Statement matching and collision behavior follow the signature rules; layout follows the whitespace rules above.

### Statement Signatures and Definition Collisions

Statement matching removes ASCII spaces/tabs and applies Unicode lowercase mapping to each remaining character. Parameters contribute positional placeholders; their labels do not affect call matching. Thus `Read value |x|`, `READ\tVALUE |other|` (with a literal tab), and `readvalue|x|` share one signature. Parameter count, placement, and punctuation remain significant: `Read!`, `Read?`, `Read`, and `Read |x|` are different signatures. Comments and explicit continuations do not contribute to the name.

This is per-character lowercase matching, with no locale-specific rules, Unicode normalization, or full case folding. Accented uppercase/lowercase pairs such as `É`/`é` match; composed/decomposed spellings, `ß`/`SS`, and final/ordinary Greek sigma remain distinct. Other Unicode whitespace remains literal sentence text. Variable and parameter names retain exact, case-sensitive spelling: `x` and `X` are different bindings.

A definition becomes available when executed. Registering the same normalized signature again in the **same frame** raises catchable `DuplicateStatement`, reporting the original and conflicting source file/line/column. The existing definition is preserved. Skipped branches register nothing; repeating a declaration in a loop collides on its second execution. A new invocation frame may shadow a parent definition, and separate invocations may create their own local helpers. Calls resolve the nearest lexical definition uniquely.

The CLI registers native `Log` first; redefining its signature in that frame reports the native origin as `<builtin Log>`. Library `init_statements()` is idempotent and fills only unoccupied native slots, preserving custom definitions registered beforehand. Initialize natives first when they should own their signatures. Future imports must use the same collision rules rather than replacing existing definitions.

Repeated parameter labels within one definition raise `DuplicateParameter` during whole-program validation, even in unused/unreachable definitions. Both parameter locations are reported before any statement executes; this validation error cannot be caught by the script. Distinct case-sensitive labels such as `|x|` and `|X|` remain valid. See [the naming example](../examples/15-statement-names.botwork).

## Multilingual Authoring

Source files are UTF-8. Variable names, parameter labels, unquoted map keys, and named dot segments start with a Unicode `XID_START` character or `_`, followed by zero or more `XID_CONTINUE` characters. These properties come from the locked Pest dependency. Combining marks are accepted after a valid start, supporting Tamil, Hindi, and decomposed accented names. Digits cannot start identifiers; emoji, spaces, and punctuation are not identifier characters. Quote arbitrary map keys and read them with brackets.

Custom sentence names accept broader Unicode text and punctuation, subject to the reserved delimiters and line rules above. For example:

<!-- botwork-test: multilingual-call -->
```botwork
கூட்டு |முதல்| உடன் |இரண்டாம்| { Return |முதல் + இரண்டாம்| }
|விடை| = கூட்டு |2| உடன் |3|
Log |விடை|
```

Variables, parameters, strings, and map keys preserve exact code points and case: composed `café`, decomposed `café`, and `Café` are distinct names. No NFC/NFKC normalization or transliteration occurs. Statement names use the per-character lowercase rule above, so `Écho` and `écho` collide in the same frame, but composed/decomposed spellings remain distinct. Re-run Unicode conformance cases when upgrading Pest or Rust, whose Unicode tables govern identifiers and lowercase matching.

Syntax tokens remain fixed: English control keywords, lowercase `true`/`false`/`and`/`or`, ASCII numeric digits `0`–`9`, decimal `.`, and the documented operators/delimiters. A translated boolean word is an ordinary identifier until bound. ASCII spaces/tabs and LF/CRLF delimit syntax; other Unicode spacing remains literal sentence/string text. Existing Unicode numeric dot segments preserve their spelling as map keys (`map.٣`); arrays still require ASCII indexes. This does not make Unicode digits numeric literals.

Collection display preserves combining marks in quoted text while escaping quotes, backslashes, controls, and other nonprinting characters. It is readable output, not a serialization format. Source spans retain UTF-8 byte ranges; reported columns count Unicode scalars, not visual glyphs. See [the multilingual example](../examples/16-multilingual.botwork) for Tamil variables/parameters/maps/loops and accented-name collisions.

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

`Log |value|` writes the value followed by a newline to stdout and returns that value. Top-level strings are printed as their contents, including literal newlines. Numbers and booleans use plain text; the None value displays as `none`.

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

`Try` requires exactly one `Catch` block. `Catch` can start on the same line as the try block's closing brace or after blank/comment lines. Both blocks may be empty; keywords are case-insensitive, and complete `Try/Catch` statements may nest.

<!-- botwork-test: catch-recovery -->
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
