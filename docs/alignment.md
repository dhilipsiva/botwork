# Keeping the tools aligned

Five tools read Botwork source:

- the interpreter, which parses and runs it;
- the formatter;
- the language analysis behind `--check` and the language server;
- the Tree-sitter grammars;
- the editors' own highlighting in Vim and VS Code.

They stay aligned because the tests hold them all to one shared corpus.

## The shared corpus

`tests/support/sources.rs` collects every Botwork source in the repository:

- the conformance corpus scripts;
- the examples, example modules, and example datasets;
- the conformance and fixture files;
- the editor fixtures in `editors/test`;
- every executed Botwork block in the documentation.

Some tests also add the formatted form of each source.

| Aligned | Test | What must hold for every source |
| --- | --- | --- |
| Parsing | `tests/tree_sitter.rs` | Tree-sitter accepts exactly the sources the interpreter parses, and marks a syntax error wherever the interpreter reports one |
| Formatting | `tests/format.rs` | Formatting is idempotent and keeps comments, literals, and meaning |
| Formatting and parsing | `tests/alignment.rs` | Formatted sources still parse in Tree-sitter |
| Formatting and analysis | `tests/alignment.rs` | The analysis reports the same problem codes for a source and its formatted form |
| Analysis | `tests/shared_analysis.rs` | `--check` and the language server report the same problems, codes, and ranges |
| Highlighting | `tests/alignment.rs` | The Tree-sitter queries, Vim, and VS Code's TextMate grammars agree on every character of every valid source, and of its formatted form |
| Editor fixtures | `tests/editors.rs` | Vim and VS Code highlight each expected token of `editors/test` as `highlighting.json` says |

## What the highlighters must agree on

Each highlighter sorts every character that isn't whitespace into one of five
categories: comment, string, number, keyword, or anything else. All three must
put each character in the same category. In the recorded run, that covered 292
sources and about 75,000 characters.

The finer distinctions are left to each editor:

- Operators, variables, statement words, and punctuation all fall in the last
  category.
- Escapes count as part of their string, although Vim and VS Code color them
  separately.
- Suite IDs count as strings, although the Tree-sitter queries capture them as
  labels.

The highlighters agree because Vim and VS Code follow the grammar's rules:

- **Where a keyword ends.** Keywords end where the grammar's `control_keyword`
  does: before a space, tab, `|`, `{`, `}`, `#`, `\`, or the end of the line.
  So `Break! { ... }` defines a statement named `Break!`, and none of it is a
  keyword.
- **Where keywords appear.** Control keywords count at the start of a
  statement, after a brace, and as `Else If`. `In` and `As` count in `For` and
  `Import`.
- **Suite headers.** A suite, case, or dataset header runs from its keyword to
  its block, so `Named`, `Tags`, `Using`, `As`, and `From` are keywords
  anywhere in it, even on a continued line. A dataset row is a header that ends
  with its line.

## Error recovery

The tools differ on invalid source, by design. `tests/alignment.rs` checks each
behavior below on a script with two syntax errors around valid lines.

| Tool | On a syntax error |
| --- | --- |
| Interpreter and `--check` | Stops at the first error and reports one BW1001 diagnostic at its position. Nothing runs. |
| Language server | Reports the same single diagnostic. Navigation and completion use the last version that parsed. |
| Formatter | Refuses the file with the interpreter's diagnostic and leaves it unchanged. |
| Tree-sitter | Recovers, marks every error with an error node, and keeps parsing, so the rest of the file still highlights. |
| Vim and VS Code | Highlight line by line with regular expressions, whatever the errors. |

The highlighting comparison therefore covers valid sources only. On an invalid
file, the highlighters may disagree near the error. For example, Tree-sitter
does not highlight a block comment that never closes, while Vim and VS Code
highlight it as a comment to the end of the file. Each still highlights the
valid lines around the error.

## Limits

- A dataset row that continues onto the next line highlights only its first
  line's keywords in Vim and VS Code, which treat a row as ending with its line.
- Vim and VS Code recognize suite keywords by position, so a custom statement
  in a suite's library whose name starts with `Case` or `Row` highlights as a
  suite keyword.
