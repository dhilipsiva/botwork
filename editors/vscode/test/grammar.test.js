// Tokenizes the shared highlighting fixtures with VS Code's TextMate engine
// and checks each expected token's innermost scope.
const fs = require("fs");
const path = require("path");
const oniguruma = require("vscode-oniguruma");
const textmate = require("vscode-textmate");

const fixtures = path.join(__dirname, "..", "..", "test");
const syntaxes = path.join(__dirname, "..", "syntaxes");
const grammars = {
  "source.botwork": "botwork.tmLanguage.json",
  "source.botwork.suite": "botwork-suite.tmLanguage.json",
};
// Each category's scope prefixes.
const scopes = {
  comment: ["comment."],
  string: ["string.quoted."],
  escape: ["constant.character.escape."],
  keyword: ["keyword.control.", "keyword.other.suite."],
  number: ["constant.numeric."],
  boolean: ["constant.language.boolean."],
  function: ["entity.name.function."],
  namespace: ["entity.name.namespace."],
  variable: ["variable.other."],
  operator: ["keyword.operator."],
  continuation: ["punctuation.separator.continuation."],
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
      const grammar = path.join(syntaxes, file);
      return textmate.parseRawGrammar(fs.readFileSync(grammar, "utf8"), grammar);
    },
  });
  const expectations = JSON.parse(fs.readFileSync(path.join(fixtures, "highlighting.json"), "utf8"));
  const failures = [];
  let checked = 0;
  for (const [file, expected] of Object.entries(expectations)) {
    const scope = file.endsWith(".suite.botwork") ? "source.botwork.suite" : "source.botwork";
    const grammar = await registry.loadGrammar(scope);
    const lines = fs.readFileSync(path.join(fixtures, file), "utf8").split("\n");
    const tokens = [];
    let state = textmate.INITIAL;
    for (const line of lines) {
      const result = grammar.tokenizeLine(line, state);
      tokens.push(result.tokens);
      state = result.ruleStack;
    }
    for (const [number, prefix, token, category] of expected) {
      const line = lines[number - 1];
      const at = line.indexOf(prefix + token);
      if (at < 0) {
        failures.push(`${file}:${number}: ${JSON.stringify(prefix + token)} not found`);
        continue;
      }
      const start = at + prefix.length;
      for (let column = start; column < start + token.length; column++) {
        const found = tokens[number - 1].find((t) => t.startIndex <= column && column < t.endIndex);
        // The innermost scope, past the punctuation that opens a comment or string.
        const named = found ? found.scopes.filter((scope) => !scope.startsWith("punctuation.definition.")) : [];
        const innermost = named.length > 0 ? named[named.length - 1] : "";
        if (!scopes[category].some((prefix) => innermost.startsWith(prefix))) {
          failures.push(`${file}:${number}:${column + 1}: ${JSON.stringify(token)} is ${innermost}, expected ${category}`);
          break;
        }
      }
      checked += 1;
    }
  }
  if (failures.length > 0) {
    console.error(failures.join("\n"));
    process.exit(1);
  }
  console.log(`checked ${checked} tokens`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
