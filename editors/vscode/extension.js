// Starts `botwork --lsp` for Botwork documents.
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

let client;

async function activate(context) {
  const command = vscode.workspace
    .getConfiguration("botwork")
    .get("server.path", "botwork");
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  const server = { command, args: ["--lsp"], options: { cwd: folder } };
  client = new LanguageClient(
    "botwork",
    "Botwork",
    { run: server, debug: server },
    {
      documentSelector: ["botwork", "botwork-suite"].flatMap((language) => [
        { scheme: "file", language },
        { scheme: "untitled", language },
      ]),
    },
  );
  context.subscriptions.push(client);
  await client.start();
}

function deactivate() {
  return client?.stop();
}

module.exports = { activate, deactivate };
