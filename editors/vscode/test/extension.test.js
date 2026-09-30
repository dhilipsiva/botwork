// Activates the extension against stand-ins for `vscode` and
// `vscode-languageclient`, and checks the language client it starts.
const Module = require("module");
const path = require("path");
const assert = require("assert");

const started = [];
const stubs = {
  vscode: {
    workspace: {
      getConfiguration: (section) => ({
        get: (key, fallback) =>
          section === "botwork" && key === "server.path" ? "/opt/botwork" : fallback,
      }),
      workspaceFolders: [{ uri: { fsPath: "/work" } }],
    },
  },
  "vscode-languageclient/node": {
    LanguageClient: class {
      constructor(id, name, server, client) {
        Object.assign(this, { id, name, server, client, running: false });
      }
      async start() {
        this.running = true;
        started.push(this);
      }
      async stop() {
        this.running = false;
      }
    },
  },
};
const load = Module._load;
Module._load = function (request, parent, main) {
  return request in stubs ? stubs[request] : load.call(this, request, parent, main);
};

const extension = require(path.join(__dirname, "..", "extension.js"));
const manifest = require(path.join(__dirname, "..", "package.json"));

(async () => {
  const context = { subscriptions: [] };
  await extension.activate(context);
  assert.strictEqual(started.length, 1);
  const client = started[0];
  assert.strictEqual(context.subscriptions[0], client);
  const server = { command: "/opt/botwork", args: ["--lsp"], options: { cwd: "/work" } };
  assert.deepStrictEqual(client.server, { run: server, debug: server });
  const languages = manifest.contributes.languages.map((language) => language.id);
  const selected = client.client.documentSelector.map((selector) => selector.language);
  for (const language of languages) {
    assert.ok(selected.includes(language), `${language} is not sent to the server`);
  }
  await extension.deactivate();
  assert.strictEqual(client.running, false);
  console.log("activation starts botwork --lsp for", languages.join(", "));
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
