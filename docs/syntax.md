# Syntax reference

This page lists every syntactic form in botwork scripts and suite files, with a
short example of each. The [language guide](language.md) defines how each form
behaves; the links below point to the relevant section. The
[grammar summary](#grammar-summary) at the end follows `src/core/grammar.pest`.
A test checks that every keyword and operator in that grammar appears on this
page.

## Files, lines, and comments

- **Encoding.** A script is UTF-8 text. Scripts use the `.botwork` extension,
  and suite files use `.suite.botwork`.
- **Layout.** Each statement ends at the end of its line; LF and CRLF are both
  accepted. Spaces and tabs separate tokens, and indentation has no meaning.
- **Continuation.** A backslash (`\`) at the end of a line continues a statement
  call or definition header on the next line. Expressions between pipes, arrays,
  and maps may span lines without it.
- **Comments.** `#` starts a comment that runs to the end of the line.
  `###` opens a block comment that ends at the next `###`.
- **Canonical layout.** `botwork --format` rewrites a file in one layout; see
  [formatting](format.md).

See [whitespace, lines, and comments](language.md#whitespace-lines-and-comments).

## Literals

| Kind | Examples | Notes |
| --- | --- | --- |
| Integer | `0`, `42`, `-7` | 32-bit signed; a leading `-` is unary minus |
| Float | `3.25`, `0.5` | Digits on both sides of the point; 32-bit |
| String | `"text"`, `"line\nbreak"` | Escapes: `\n`, `\"`, and `\\` |
| Boolean | `true`, `false` | Lowercase only |
| Array | `[1, "two", [3]]` | Any values; an optional trailing comma |
| Map | `{name: "Ada", "display name": "A"}` | Keys are names or strings |

There is no literal for None: a bare `Return`, or a custom statement that
finishes without returning, produces it. See [values](language.md#values-and-supported-operations),
[strings](language.md#strings), and
[numeric precision](language.md#numeric-precision-and-comparison).

## Names

- **Variables** appear between pipes, such as `|total|`. A name starts with a
  Unicode letter or `_` and continues with letters, digits, marks, or `_`. Names
  are case-sensitive.
- **Reserved words.** `true`, `false`, `and`, and `or` cannot be variable names.
- **Statement names** are sentences, such as `Total of |prices|`. Calls match
  them without regard to case or spacing between words.
- **Control keywords** are reserved at the start of a statement. They are listed
  under [control flow](#control-flow).
- **Namespaces.** Statements from an imported module are called as
  `alias::Statement name`.

See [keywords and names](language.md#keywords-and-names) and
[multilingual authoring](language.md#multilingual-authoring).

## Expressions

An expression appears between pipes. It is built from:

- a literal or variable;
- a parenthesized expression, such as `(a + b)`;
- access: `.name` or `.0` for a map key or array index, and `[expression]` for a
  computed key or index;
- a call expression, `@{ Statement |argument| }`, which uses the result of a
  statement call;
- unary `-` for numbers and `!` for booleans;
- binary operators.

Binary operators, from weakest to strongest binding:

| Level | Operators | Grouping |
| --- | --- | --- |
| 1 | `or` | Left, short-circuiting |
| 2 | `and` | Left, short-circuiting |
| 3 | `==`, `!=` | Left |
| 4 | `<`, `<=`, `>`, `>=` | Left |
| 5 | `+`, `-` | Left |
| 6 | `*`, `/`, `%` | Left |
| 7 | `^` | Right |

Unary operators bind more tightly than `*` but less tightly than `^` on their
right, so `-2 ^ 2` is `-4`.

<!-- botwork-test: syntax-values -->
```botwork
### Literals and expressions.
Each value below is one expression between pipes. ###
|order| = |{id: "A-7", "line items": [{sku: "pen", price: 2.5, count: 4}], paid: false}|
Log |order.id|                        # named access
Log |order["line items"][0].sku|      # computed access, then named access
Log |order["line items"].0.count|     # a numeric index in a dotted path
Log |2 + 3 * 4 ^ 2|                   # precedence: ^ before *, * before +
Log |-(2 ^ 2) == -4 and !order.paid|  # unary minus and !
Log |"a" + "b" + "\n" + "c"|          # string concatenation and an escape
Log |[1, 2] + [3]|                    # array concatenation
```

Output:

```text
A-7
pen
4
50
true
ab
c
[1, 2, 3]
```

See [precedence](language.md#binary-operator-precedence),
[short-circuiting](language.md#boolean-short-circuiting),
[powers and unary operators](language.md#powers-and-unary-operators),
[collection access](language.md#collection-access), and
[calls inside expressions](language.md#calls-inside-expressions).

## Statements

| Form | Syntax |
| --- | --- |
| Call | `Log |"hello"|`, or `alias::Statement |value|` for an imported statement; see the [statement reference](statements.md) |
| Assignment | `|name| = |expression|`, or `|name| = Statement |argument|` |
| Definition | `Sentence with |parameter| { ... }` |
| Return | `Return |value|` or a bare `Return`, inside a definition |
| Import | `Import |"helpers.botwork"| As |helpers|` |

A definition's header is a sentence whose parameters are variable names between
pipes; its block is the body. Definitions may be nested, and a nested one is
visible only inside its enclosing body. An import runs a local `.botwork` module
once and publishes its statements under the alias. See
[variables and invocation scope](language.md#variables-and-invocation-scope),
[returns](language.md#returns-and-control-flow), and
[local modules](language.md#local-modules).

## Control flow

| Form | Syntax |
| --- | --- |
| Condition | `If |condition| { ... }`, then optionally `Else { ... }` or `Else If |condition| { ... }` |
| Array loop | `For |item| In |array| { ... }` |
| Condition loop | `While |condition| { ... }` |
| Loop control | `Break` and `Continue` |
| Errors | `Try { ... }` followed by `Catch { ... }`, `Catch |error| { ... }`, and/or `Finally { ... }` |
| Rethrow | `Rethrow`, inside `Catch` |
| Polling | `Eventually |{timeout_ms: 5000}| { ... }` |
| Retries | `Retry |{attempts: 3}| { ... }` |

Keywords are case-insensitive. Conditions must be booleans, and `For` requires
an array.

<!-- botwork-test: syntax-statements -->
```botwork
# A definition: sentence parts with parameters, then a block.
Classify |number| {
    If |number < 0| {
        Return |"negative"|
    } Else If |number == 0| {
        Return |"zero"|
    }
    Return |"positive"|
}

# A long header or call continues on the next line after a backslash.
Describe |value| as \
    |label| {
    Return |label + ": " + @{ classify |value| }|
}

|results| = |[]|
For |number| In |[-2, 0, 5, 7]| {
    If |number == 7| {
        Break
    }
    |results| = |results + [@{ Describe |number| as \
        |"n"| }]|
}
Log |results|

|attempt| = |0|
While |attempt < 5| {
    |attempt| = |attempt + 1|
    If |attempt % 2 == 1| {
        Continue
    }
    Log |attempt|
}

Try {
    Fail |"out of stock"|
} Catch |error| {
    Log |error.code|
} Finally {
    Log |"cleanup runs either way"|
}
```

Output:

```text
["n: negative", "n: zero", "n: positive"]
2
4
BW9002
cleanup runs either way
```

See [While loops](language.md#while-loops), [Try/Catch](language.md#trycatch),
[catch details and Rethrow](language.md#catch-details-and-rethrow),
[cleanup](cleanup.md), and [polling](polling.md).

## Errors

- **Before running.** A syntax error (BW1001) or a misplaced control statement
  (BW1002), such as `Break` outside a loop, stops the whole file before any
  statement runs. See
  [control-placement validation](language.md#control-placement-validation).
- **While running.** Runtime errors, such as an undefined variable or a failed
  assertion, can be handled with `Try`/`Catch`. Cancellation, deadlines, and
  resource limits cannot be caught.
- **Raising errors.** `Fail |"reason"|` raises BW9002, and `Assert |condition|`
  raises BW9001 when the condition is false.

Every diagnostic carries a stable `BWnnnn` code; see
[diagnostic codes](diagnostics.md). `botwork --check` reports many of these
errors without running the script; see [checking scripts](check.md).

## Suite files

A suite file holds one suite. Its parts must appear in this order: datasets, an
optional library, fixtures, and then one or more cases.

| Form | Syntax |
| --- | --- |
| Suite | `Suite |"id"| Named |"name"| Tags |["tag"]| { ... }`; `Named` and `Tags` are optional |
| Inline dataset | `Dataset |"id"| { Row |"id"| Values |value| ... }`; rows may have `Named` and `Tags` |
| External dataset | `Dataset |"id"| From |"rows.dataset.botwork"|`, or `From JSON |"rows.json"|` or `From CSV |"rows.csv"|` |
| Library | `Library { ... }`, definitions and imports that every case can use |
| Fixtures | `SuiteSetup`, `SuiteTeardown`, `CaseSetup`, and `CaseTeardown`, each followed by a block |
| Case | `Case |"id"| Named |"name"| Tags |["tag"]| Using |"dataset"| As |row| { ... }`; `Named`, `Tags`, and `Using` are optional |

Dataset values are written like literals and may also use `none`. The
[getting-started guide](getting-started.md#an-acceptance-test-workflow) has a
complete suite. See [suites](suites.md), [fixtures](fixtures.md), and
[parameterized cases](parameterized-cases.md).

## Grammar summary

This summary uses `[ ]` for an optional part, `{ }` for repetition, and `|` for
alternatives. Quoted text is literal and keywords are case-insensitive. A pipe
that is part of the syntax is quoted as `"|"`.

```text
script      = { statement }
statement   = import | assignment | definition | call | if | for | while | try
            | eventually | retry | "Return" [ param ] | "Break" | "Continue" | "Rethrow"
import      = "Import" "|" string "|" "As" "|" name "|"
assignment  = "|" name "|" "=" ( param | call )
definition  = sentence-with-names block
call        = sentence-with-params
if          = "If" param block [ "Else" ( block | if ) ]
for         = "For" "|" name "|" "In" param block
while       = "While" param block
try         = "Try" block ( catch [ "Finally" block ] | "Finally" block )
catch       = "Catch" [ "|" name "|" ] block
eventually  = "Eventually" param block
retry       = "Retry" param block
block       = "{" { statement } "}"
param       = "|" expression "|"

expression  = operand { binary-op operand }
operand     = ( "-" | "!" ) operand | primary [ "^" operand ]
primary     = ( "@{" call "}" | literal | name | "(" expression ")" )
              { "." ( name | digits ) | "[" expression "]" }
binary-op   = "or" | "and" | "==" | "!=" | "<" | "<=" | ">" | ">="
            | "+" | "-" | "*" | "/" | "%"
literal     = integer | float | string | "true" | "false" | array | map
array       = "[" [ expression { "," expression } [ "," ] ] "]"
map         = "{" [ key ":" expression { "," key ":" expression } [ "," ] ] "}"
key         = name | string

suite       = "Suite" id [ "Named" id ] [ "Tags" tags ] "{" { dataset } [ library ]
              { fixture } case { case } "}"
dataset     = "Dataset" id [ "Named" id ] [ "Tags" tags ] "{" row { row } "}"
            | "Dataset" id "From" [ "JSON" | "CSV" ] id
row         = "Row" id [ "Named" id ] [ "Tags" tags ] "Values" "|" value "|"
library     = "Library" block
fixture     = ( "SuiteSetup" | "SuiteTeardown" | "CaseSetup" | "CaseTeardown" ) block
case        = "Case" id [ "Named" id ] [ "Tags" tags ] [ "Using" id "As" "|" name "|" ] block
id          = "|" string "|"
tags        = "|" "[" [ string { "," string } [ "," ] ] "]" "|"
value       = literal-value | "none"
```

A `sentence-with-params` is sentence text interleaved with `|expression|`
parameters. A `sentence-with-names` is the same with `|name|` parameters.
Sentence text is any run of characters other than `|`, braces, `#`, `\`, and
line endings, and it cannot start with a control keyword. In dataset values,
`literal-value` is a literal whose entries are also literal values rather than
expressions.
