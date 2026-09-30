# Editor support

`editors/` holds packages for Helix, Vim, and VS Code. Each one highlights
scripts, suites, and datasets, and starts the [language server](lsp.md),
`botwork --lsp`, for diagnostics, completion, hover, navigation, rename, and
signature help.

| Editor | Highlighting | Language server | Package |
| --- | --- | --- | --- |
| Helix | [Tree-sitter grammars](tree-sitter.md) and their queries | `languages.toml` | `editors/helix` |
| Vim | A Vim syntax file | Any LSP plugin | `editors/vim` |
| Neovim | The Vim syntax file | Built-in client | `editors/vim` |
| VS Code | TextMate grammars | The extension | `editors/vscode` |

Every package runs the `botwork` executable, so it must be on `PATH`. In VS
Code, you can set its path instead.

Suites (`*.suite.botwork`) and datasets (`*.dataset.botwork`) have keywords
that scripts do not, such as `Suite`, `Case`, and `Row`. Each package
therefore treats them as a second language: `botwork-suite` in Helix and VS
Code, and extra keywords in Vim.

## Helix

1. Add `editors/helix/languages.toml` to `~/.config/helix/languages.toml`.
2. Copy the queries into Helix's runtime:

   ```sh
   mkdir -p ~/.config/helix/runtime/queries
   cp -r editors/helix/queries/. ~/.config/helix/runtime/queries/
   ```

3. Fetch and build the grammars, then check the setup:

   ```sh
   hx --grammar fetch
   hx --grammar build
   hx --health botwork
   ```

`languages.toml` fetches the grammars from this repository at a pinned
revision. To build from a local checkout instead, replace each grammar's
`source` with its directory:

```toml
source = { path = "/path/to/botwork/editors/tree-sitter-botwork/botwork" }
```

The queries are copies of `editors/tree-sitter-botwork/queries`; the suite
queries inherit the script queries and add the suite keywords.

## Vim and Neovim

Add `editors/vim` to the runtime path, for example in `~/.vimrc` or
`init.vim`:

```vim
set runtimepath+=/path/to/botwork/editors/vim
filetype plugin on
syntax on
```

This detects `*.botwork` files and highlights them. It also sets `#` as the
comment leader and four-space indentation. In suite and dataset files, suite
keywords are highlighted at the start of a declaration and after a
parameter, as in `Case |"c"| Using |"rows"| As |row|`.

Vim has no built-in language client. With
[yegappan/lsp](https://github.com/yegappan/lsp):

```vim
call LspAddServer([#{name: 'botwork', filetype: ['botwork'], path: 'botwork', args: ['--lsp']}])
```

With [vim-lsp](https://github.com/prabirshrestha/vim-lsp):

```vim
autocmd User lsp_setup call lsp#register_server(#{
      \ name: 'botwork',
      \ cmd: {server_info -> ['botwork', '--lsp']},
      \ allowlist: ['botwork'],
      \ })
```

In Neovim 0.11 or later, the built-in client needs no plugin:

```lua
vim.lsp.config("botwork", {
  cmd = { "botwork", "--lsp" },
  filetypes = { "botwork" },
  root_markers = { ".git" },
})
vim.lsp.enable("botwork")
```

## VS Code

Build the extension and install it:

```sh
cd editors/vscode
npm ci
npm run package
code --install-extension botwork.vsix
```

The extension needs VS Code 1.91 or later. It highlights `.botwork` files as
`botwork`, and suite and dataset files as `botwork-suite`. For both, it starts
`botwork --lsp` in the first workspace folder, so rename can reach the
folder's other files. The `botwork.server.path` setting names the executable
when it is not on `PATH`.

## Verification

`tests/editors.rs` checks each package:

- **Highlighting.** Vim and VS Code highlight the shared fixtures in
  `editors/test` as `editors/test/highlighting.json` expects, token by token,
  in a script and a suite. Vim is probed headless for the syntax group at every
  character. VS Code's own TextMate engine, `vscode-textmate` with Oniguruma,
  tokenizes the fixtures.
- **VS Code.** The manifest declares both languages, each grammar's scope, the
  comment tokens, and the server setting. The lockfile pins exactly the
  manifest's dependencies, and the language client supports the extension's
  VS Code versions. Activation, with VS Code stood in for, starts
  `botwork --lsp` for both languages and stops it on deactivation.
- **Helix.** The queries equal the Tree-sitter queries. The pinned revision
  holds the grammars that are checked in. Helix builds both grammars from the
  checkout, and `hx --health` finds the parser, the highlight queries, and the
  language server for both languages.

A check skips when its editor, Node.js, or the Git history is missing, unless
`BOTWORK_REQUIRE_EDITORS` names it (`vim`, `vscode`, `helix`, `git`, or `all`).
The `Editor packages` job in continuous integration does the following:

- installs Vim, Helix 25.07.1 (checked against its SHA-256), and the
  extension's locked dependencies;
- packages the extension;
- runs `tests/editors.rs` with every check required.

[Validation evidence](editors-evidence.json) records the measured profiles and
the sensitivity probes. Each probe injects a fault into one package and
confirms the check that reports it.

## Limits

- **Regular-expression highlighting.** Vim and VS Code highlight with regular
  expressions, not the grammar, so they recognize keywords by position. Over
  the shared corpus they agree with the Tree-sitter queries on every
  character. [Keeping the tools aligned](alignment.md) gives the rules they
  follow and the cases where they still differ.
- **Neovim** configuration follows its documented API but is not exercised by
  the tests.
- **The VS Code extension** is not published to the Marketplace. Install it
  from the packaged `.vsix`.
