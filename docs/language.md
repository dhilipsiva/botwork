# Language Behavior

This reference records implemented behavior as language TODOs are completed. It is not yet the complete language specification required by the roadmap.

## Strings

Double quotes delimit a string; they are not part of its value. `"a" + "b"` produces the same value as `"ab"`. Strings preserve Unicode text without normalization and may contain literal newlines, pipes, comment markers, and braces.

Three escape sequences are supported:

| Source text | Value |
| --- | --- |
| `\n` | A newline |
| `\"` | A double quote |
| `\\` | A backslash |

Escapes are decoded once. For example, `"\\n"` contains a backslash followed by `n`; it does not contain a newline. Other escapes, including `\t`, are syntax errors. Map keys such as `label` in `{label: "hello"}` are identifiers; string decoding applies to the quoted value.
