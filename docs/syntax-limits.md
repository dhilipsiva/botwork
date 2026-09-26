# Source and Syntax Limits

Program and public Pest parsing now reject excessive syntax before recursive parsing, AST lowering, or control validation. These guards also apply before CLI traces/output and before imported module initialization. This intentionally rejects oversized sources previously accepted without bounds.

| Budget | Default and fixed syntax ceiling | Rule |
| --- | --- | --- |
| Source bytes | 1 MiB default | Entire UTF-8 source; Program/Engine may explicitly change the byte budget |
| Syntax nesting | 32 | Open pipes, braces, brackets, parentheses, plus active Else-If links |
| Expression operators | 64 | Operator symbols and lowercase `and`/`or` words inside each pipe expression |
| Combined syntax complexity | 66, fixed | Twice current nesting plus the largest active expression operator count |

A pipe itself consumes one nesting level, so a normal pipe expression accepts 31 additional nested parentheses. Else-If links count while their chain is active, including nested branch bodies; separate chains start fresh. Each symbol in `+ - * / % ^ ! = < >` counts once, so `<=` consumes two operator units. Counters include nested expression arguments but exclude ordinary sentence text. Separate pipe expressions have separate counters; strings and comments do not consume these budgets. The combined ceiling prevents individually valid nesting and operator counts from exhausting the parser stack together; tighten it indirectly by lowering either configurable syntax limit.

The guard recognizes ASCII layout, line/block comments, escaped quotes, and the switch from an expression to sentence text inside `@{ ... }`. A quote in a sentence is ordinary text, including inside a call expression; it cannot hide a deeply nested parameter from the guard. Newlines/comments cannot reset a still-open expression's counter. The grammar still checks syntax validity; a preflight limit may take priority over a later syntax/control error.

## Rust Configuration and Errors

`Program::parse`/`parse_detailed` use defaults. `Program::parse_bounded(name, source, source_bytes, &SyntaxLimits)` can change the byte budget and tighten nesting/operator bounds. `RunLimits::syntax` supplies the same local options for Engine source/file/import parsing and checks reusable programs' original source before execution. Syntax values above the fixed ceilings return BW7002. Changing configuration never alters a global parser setting.

Program/Engine/CLI limit failures use BW8001 with `resource`/`limit` details. Syntax preflight diagnostics identify the first excessive token and retain only the source prefix through it, at valid UTF-8 boundaries. This avoids copying the whole rejected source into an error. CLI byte-limit errors identify the file without a source span. The public `BWParser::parse` keeps Pest's result type, returning a custom Pest error on preflight rejection; Program supplies stable diagnostic codes. Nonrecursive Pest rules, such as raw string/comment contents and sentence parts, check bytes only because their text is data rather than nested source. Native registration also checks source bounds before copying a header.

Entry-file and module-file reads are capped at the applicable byte budget plus one byte, checking oversize before UTF-8 decoding. Resource failures latch the shared runtime budget and bypass Catch in Engine, CLI, and legacy Context execution. Ordinary import syntax/read errors remain catchable. Direct entry-source failures always precede execution.

## Remaining Resource Work

These are conservative parser guards. They do not bound host-assembled AST structure, module counts/aggregate source, retained values/collections, or native callback duration. [Shared runtime budgets](embedded-runs.md#initial-budgets) now bound steps, call/evaluation depth, and active import initialization in Engine, CLI, and Context execution. Hard host termination and cleanup deadlines remain separate runtime work. The public owned AST is trusted host input until its structural validation work is complete.

`tests/syntax_limits.rs` exercises maximum/default/tightened and combined budgets, Unicode byte cuts, 10,000-level nesting, long unary/binary/power chains, multiline/comments, Else-If chains, quoted sentence bypasses, concurrent configuration, reused/imported programs, and CLI failures before effects. R2 corpus cases retain exact boundary output and an oversized-expression regression.
