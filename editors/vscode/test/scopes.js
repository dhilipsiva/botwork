// Tokenizes each file named on the command line with VS Code's TextMate engine
// and prints one JSON line per file: the tokens of each line, with their scopes.
// tests/alignment.rs compares these with Tree-sitter and Vim.
const fs = require("fs");
const path = require("path");
const oniguruma = require("vscode-oniguruma");
const textmate = require("vscode-textmate");

const grammars = {
  "source.botwork": "botwork.tmLanguage.json",
  "source.botwork.suite": "botwork-suite.tmLanguage.json",
};

async function main() {
  const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"));
  await oniguruma.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
      createOnigString: (text) => new oniguruma.OnigString(text),
    }),
    loadGrammar: async (scope) => {
      const file = grammars[scope];
      if (!file) return null;
      const grammar = path.join(__dirname, "..", "syntaxes", file);
      return textmate.parseRawGrammar(fs.readFileSync(grammar, "utf8"), grammar);
    },
  });
  for (const file of process.argv.slice(2)) {
    const scope = /\.(suite|dataset)\.botwork$/.test(file) ? "source.botwork.suite" : "source.botwork";
    const grammar = await registry.loadGrammar(scope);
    let state = textmate.INITIAL;
    const lines = [];
    for (const line of fs.readFileSync(file, "utf8").split("\n")) {
      const result = grammar.tokenizeLine(line, state);
      state = result.ruleStack;
      lines.push(result.tokens.map((token) => [token.startIndex, token.endIndex, token.scopes]));
    }
    process.stdout.write(JSON.stringify({ file, lines }) + "\n");
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
