# Tree-sitter grammars

`editors/tree-sitter-botwork` holds two [Tree-sitter](https://tree-sitter.github.io)
grammars for editors and tools. Both are generated from one definition:

| Grammar | Scope | Files |
| --- | --- | --- |
| `botwork` | `source.botwork` | Scripts (`*.botwork`) |
| `botwork_suite` | `source.botwork.suite` | Suites (`*.suite.botwork`) and datasets (`*.dataset.botwork`) |

Scripts and suites are separate grammars because the interpreter parses them
separately: suite keywords such as `Suite` and `Case` are ordinary statement
names inside scripts.

## Layout

- `common/define-grammar.js` defines both grammars, following
  `src/core/grammar.pest` rule by rule. `botwork/grammar.js` and
  `suite/grammar.js` select the dialect.
- `common/scanner.h` is the external scanner. It recognizes what regular
  expressions cannot:
  - block comments, which must close;
  - line breaks, which end a statement where one may end and are whitespace
    inside expressions and headers;
  - sentence words;
  - the sixteen control keywords, matched case-insensitively as whole words.
- `*/src/` holds the generated parsers, which are checked in so tools can build
  them without the Tree-sitter CLI.
- `queries/highlights.scm` highlights both grammars, and
  `queries/suite-highlights.scm` adds suite and dataset keywords.
- `*/test/corpus/` holds Tree-sitter's own tests of the syntax trees.

[Editor support](editors.md) packages the grammars and queries for Helix.
[Keeping the tools aligned](alignment.md) compares their highlighting with Vim's
and VS Code's over the shared corpus.

## How the grammars follow the interpreter

- **Control keywords.** A control keyword is recognized only as a whole word,
  in any letter case, where the interpreter expects one. `If`, `Else`, `For`,
  `Break`, `Return`, `Continue`, `While`, `Try`, `Catch`, `Finally`, `Rethrow`,
  `Import`, `Eventually`, and `Retry` cannot start a statement name. `In` and
  `As` can, as in `In order |x|`.
- **Sentences.** A sentence takes every following word and parameter on its
  line, keywords included, as the interpreter does. `Log |1| If |2|` is one call.
- **Reserved words.** `true`, `false`, `and`, and `or` are never variable names.
- **Line breaks.**
  - A line break ends a statement where one may end.
  - Inside an expression or a statement header, a line break is whitespace.
  - Inside a call expression's sentence, a line break may only come right before
    the closing `}`.
- **Comments.** A block comment must close before the end of the file, and a
  comment cannot follow a `\` continuation.

## Verification

`tests/tree_sitter.rs` compares both grammars with the interpreter:

- **Agreement.** Every source in the repository goes to the matching grammar:
  every script in the conformance corpus, every example, module, dataset,
  conformance file, and executed documentation block, plus a list of invalid
  scripts and suites. When the interpreter parses a source, the grammar must
  parse it without an error node. When the interpreter reports a syntax error
  (BW1001), the grammar must produce an error node. Errors the interpreter finds
  after parsing, such as a misplaced `Break` or an exceeded limit, are
  grammatical, so the grammar must accept them.
- **Generated parsers.** The parsers are generated again from the grammar and
  must equal the checked-in sources.

The test needs the `tree-sitter` CLI (0.25) and a C compiler. Without the CLI it
is skipped, unless `BOTWORK_REQUIRE_TREE_SITTER` is set; continuous integration
installs the CLI and sets it. The same job runs `tree-sitter test` for both
grammars.

To change a grammar, edit `common/`, then run `tree-sitter generate` and
`tree-sitter test` in `botwork/` and `suite/`, and run
`cargo test --test tree_sitter`.

[Validation evidence](tree-sitter-evidence.json) records the measured profiles
and the sensitivity probes. Each probe injects a grammar fault and confirms that
the agreement test reports the case it breaks.

## Limits

- **Error recovery.** Error recovery follows Tree-sitter's defaults, so an
  invalid file's tree can differ from the interpreter's diagnostic.
- **Tree shape.** Syntax trees are shaped for editors: binary operators nest by
  precedence, while the interpreter's parse is flat.
