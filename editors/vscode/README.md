# Botwork for VS Code

Highlighting for Botwork scripts (`*.botwork`), suites (`*.suite.botwork`),
and datasets (`*.dataset.botwork`). The extension also connects to the botwork
language server, which provides:

- diagnostics, the same as `botwork --check`;
- completion and hover;
- go to definition and references;
- rename and signature help.

## Requirements

- VS Code 1.91 or later.
- The `botwork` executable, on `PATH` or named by the `botwork.server.path`
  setting.

## Build and install

```sh
npm ci
npm run package
code --install-extension botwork.vsix
```

See [editor support](https://github.com/dhilipsiva/botwork/blob/main/docs/editors.md)
for the other editors and for how the package is tested.
