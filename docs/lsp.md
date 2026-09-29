# Language server

`botwork --lsp` runs a [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
server on stdin and stdout. Editors start it the way they start any other
language server:

```sh
botwork --lsp
```

The server works on scripts (`*.botwork`), suites (`*.suite.botwork`), and
datasets (`*.dataset.botwork`). It chooses the kind from the file name.

## Features

| Feature | What it gives |
| --- | --- |
| Diagnostics | Syntax errors, plus every finding that [`--check`](check.md) reports, published on open and on every change. Each diagnostic has the runtime code (such as `BW2002`) or, for a rule without one, the rule name. The message names the rule and ends with the same `help:` suggestion that `--check` prints. Errors have severity Error and warnings have severity Warning. |
| Completion | Inside a `\|...\|` parameter: the variables the file assigns or binds, plus `true` and `false`. Elsewhere: control keywords, built-in statements, and the file's own definitions. |
| Hover | For a built-in statement, the same help as `--statement-help`. For a definition, or a call to one, its header and where it is defined. For a call through an import alias, the header in the module file. For a variable, whether the file assigns it or it can only be an input variable. |
| Go to definition | From a call to its definition, using the same scope rules as a run: the innermost enclosing definition first, then outer scopes. From an aliased call such as `m::Double \|2\|` to the definition in the module file, following re-exports. From an `Import` path to the module file. From a variable to its first binding in its scope. |
| References | Every call that reaches a definition, plus the definition's header when the editor asks for declarations. For a built-in statement, every call to it. For a variable, every read and binding in its scope. |

The diagnostics come from the same analysis as `botwork --check`: the parser, the
control-placement validation, and the lint rules. The two do not disagree about
what is wrong with a file.

## Protocol details

- **Synchronization.** Editors send the full text on each change.
- **Positions.** Lines and characters count UTF-16 code units, the protocol
  default. The server announces `positionEncoding: "utf-16"`.
- **Incomplete edits.** While a document does not parse, its diagnostics show
  the syntax error. Completion, hover, definitions, and references keep working
  from the last version that parsed.
- **Closing.** Closing a document publishes an empty diagnostic list for it.
- **Imports.** A `file:` document resolves `Import` paths relative to its own
  directory, as a run does. A document without a file path, such as an unsaved
  buffer, resolves them from the server's working directory. Module files are
  read from disk, so unsaved changes to a module are not seen until it is saved.
- **Shutdown.** `shutdown` then `exit` ends the server with status 0. An `exit`
  without a `shutdown` ends it with status 1, and requests after `shutdown` fail
  with `InvalidRequest`.
- **Errors.** An unknown request fails with `MethodNotFound`, and a message whose
  body is not JSON gets a `ParseError` response. Unknown notifications are
  ignored.

## Editor configuration

Any editor with a generic language-client setting can use the server. For
example, in Helix (`languages.toml`):

```toml
[language-server.botwork]
command = "botwork"
args = ["--lsp"]

[[language]]
name = "botwork"
scope = "source.botwork"
file-types = ["botwork"]
comment-token = "#"
language-servers = ["botwork"]
```

In Neovim 0.11 or later:

```lua
vim.filetype.add({ extension = { botwork = "botwork" } })
vim.lsp.config("botwork", { cmd = { "botwork", "--lsp" }, filetypes = { "botwork" } })
vim.lsp.enable("botwork")
```

Syntax highlighting comes from the [Tree-sitter grammars](tree-sitter.md).

## Verification

- `tests/lsp.rs` starts `botwork --lsp` and talks to it over its pipes. It covers
  diagnostics on open and change, including an incomplete edit and the removal
  of a fixed error, as well as clearing on close. It also covers:
  - completion inside and outside parameters, and navigation while the text does
    not parse;
  - definitions, references, and hover, with positions after a character that
    takes two UTF-16 code units;
  - definitions in an imported module file, and a missing module;
  - protocol errors, and the exit status with and without `shutdown`.
- The unit tests in `src/core/language/tests.rs` check the shared analysis:
  - problems from the parser and the checks;
  - scope-aware definitions and references;
  - hover text and completions;
  - module resolution through re-exports;
  - suites, whose case programs are indexed once.
- The unit tests in `src/lsp.rs` check message framing, UTF-16 position
  conversion, and file URIs.

[Validation evidence](lsp-evidence.json) records the measured profiles and the
focused mutation campaign.

## Limits

- **Renaming and signature help** are not implemented yet.
- **Workspace-wide references.** References cover the open document. Calls to
  a module's definitions from other files are not listed.
- **Datasets** get diagnostics only; they define no statements or variables.
- **Recovery.** A syntax error hides the lint findings until the file parses
  again, because the checks need a parsed file.
