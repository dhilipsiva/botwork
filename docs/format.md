# Formatting

`botwork --format` rewrites files in one canonical layout, and
`botwork --format-check` reports files whose layout would change, for use in
continuous integration.

```sh
botwork --format --file checkout.botwork --suite checkout.suite.botwork
botwork --format-check --file checkout.botwork --suite checkout.suite.botwork
```

Names ending in `.suite.botwork` are formatted as suites, names ending in
`.dataset.botwork` as datasets, and everything else as scripts. Formatting never
runs a script.

- **`--format`.** Each file that changes is replaced by renaming a new copy over
  it, so a failure leaves the original intact; the file keeps its permissions.
- **`--format-check`.** Nothing is written, and each file that would change is
  listed.

Both exit with status 1 when a file cannot be read or parsed; such a file is
never written. `--format-check` also exits with 1 when any file would change.

## Layout

```text
# Totals for the checkout.
Total of |prices| {
    |total| = |0|
    For |price| In |prices| {
        |total| = |total + price| # running sum
    }
    Return |total|
}

|orders| = |[
    {items: [5], expected: 5},
    {items: [2, 3, 4], expected: 9},
]|
If |@{ Total of |[2, 3]| } == 5| {
    Log |"ok"|
} Else {
    Fail |"wrong total"|
}
```

- **Lines and indentation.** Each statement starts its own line. Blocks open at
  the end of their header line, their statements are indented by four spaces,
  and `}` returns to the header's indentation. `Else`, `Catch`, and `Finally`
  follow the closing brace on the same line, and an empty block is written `{}`.
- **Blank lines.** Blank lines between statements are kept, at most one in a
  row, and never at the start or end of a block or file.
- **Keywords.** Control keywords are written `If`, `Else`, `For`, `In`, `While`,
  `Try`, `Catch`, `Finally`, `Return`, `Break`, `Continue`, `Rethrow`, `Import`,
  `As`, `Eventually`, and `Retry`. Suite keywords are written `Suite`, `Named`,
  `Tags`, `Dataset`, `From`, `JSON`, `CSV`, `Row`, `Values`, `Library`,
  `SuiteSetup`, `SuiteTeardown`, `CaseSetup`, `CaseTeardown`, `Case`, and
  `Using`.
- **Statements.** Words and parameters of a statement are separated by single
  spaces. Statement names keep their letters and case; calls match them without
  regard to case or spacing.
- **Expressions.** Binary operators have one space on each side. Unary operators,
  access (`.name`, `[index]`), and parentheses have none. Commas are followed by
  one space, and map keys by `: `. A call inside an expression is written
  `@{ Name |argument| }`.
- **Collections.** An array or map written on one line stays on one line. One
  written across lines is laid out one item per line, indented, with a trailing
  comma.
- **Long lines.** A statement line longer than 100 characters is continued with
  `\` between words or parameters. It is left long when a single word or
  parameter would still exceed the width.
- **Line endings.** Line endings become LF, and the file ends with one newline.

## Comments and literals

Comment text and literal text are copied unchanged, including the contents of
strings, numbers as written, and line endings inside strings.

- **Between statements.** A comment on its own line keeps its place, including
  one inside a block.
- **At the end of a line.** A comment after a statement, or after a block's
  `{`, stays at the end of that line.
- **Inside a statement.** A comment within a statement, such as between an
  array's items, cannot keep its position once the statement is laid out. It
  moves to its own line directly above the statement, in its original order.

## Guarantees

Before writing, the formatter parses its own output and requires the same syntax
tree as the input: the same statements, names, expressions, values, and suite
declarations. Output that would differ is refused. A formatted file formats to
itself.

The test suite formats every valid script, suite, and dataset in the
conformance corpus, the examples, and the executed documentation. For each, it
checks that the syntax tree is unchanged, that every comment and string literal
is preserved, and that formatting again changes nothing. Invalid input is
refused.

## Limits

- **Moved comments.** Comments inside a statement move above it, so a comment
  that described one array item describes the whole statement.
- **Continuation lines.** Line breaks within statements are the formatter's
  choice; `\` continuations written by the author are not kept.
- **Trailing whitespace.** Trailing whitespace inside block comments and strings
  is content, so it is kept.

## Formatting from Rust

`core::format::format(name, source, kind)` returns the canonical text or the
diagnostic that makes the source invalid; `SourceKind::of_path` chooses the kind
from a file name.
