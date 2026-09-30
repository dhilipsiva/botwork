# Checking scripts

`botwork --check` finds mistakes without running anything. It parses each file
and every local module it imports, validates control placement, and applies the
rules below. No statement runs: nothing is logged, no file is written, no module
initializes, and no process or request is started. The check also states what it
could not establish without running the script.

```sh
botwork --check --file checkout.botwork --file helpers.botwork
botwork --check --suite checkout.suite.botwork
```

Findings and a summary go to stderr. The command exits with status 1 when it
finds an error, including a syntax error or an unreadable file, and 0 when it
finds only warnings or nothing. Invalid command-line usage exits 2.

## An example

This script has three mistakes:

<!-- botwork-test: check-example -->
```botwork
Log |1| |2|
Sleep |"10"|
If |1| { Log |1| }
```

`botwork --check --file check-example.botwork` reports each of them without
running the script:

```text
check-example.botwork:1:1-1:12: error[undefined-statement]: [BW2002] Statement not defined: Log |1| |2|
  help: Did you mean `Log |value|`? Calls must match a definition's words and parameter positions.
check-example.botwork:2:8-2:12: error[argument-kind]: [BW3003] Parameter `milliseconds` (argument 1) of `sleep|param|` requires Int; got String
  help: Pass a value that `Sleep |milliseconds|` accepts; see --statement-help.
check-example.botwork:3:5-3:6: error[condition-kind]: [BW3003] If requires a boolean condition
  help: Conditions have no truthiness; compare the value to produce a boolean.
[check] 1 file: 3 errors, 0 warnings
```

Each finding gives:

- the file and the line and column range;
- the severity and the rule name;
- the diagnostic code of the runtime error it predicts, when there is one;
- a message and a suggested fix.

A predicted error has the same code and range as the error the run itself
would report. The [language server](lsp.md) shares this analysis and reports
the same problems in an editor.

## Rules

| Rule | Severity | Predicts | Finds |
| --- | --- | --- | --- |
| `undefined-statement` | error | BW2002 | A call that no built-in, definition, or imported module satisfies. When a statement with the same words but other parameter positions exists, it is suggested. Calls through an alias are checked against the module's statements, including the modules it imports in turn, and an alias that no `Import` defines is reported too |
| `statement-before-definition` | warning | BW2002 | A call that runs before the definition or import it needs. Definitions and imports register when they run, but a definition's body resolves names when it is called, so calling a later definition from a body is fine |
| `duplicate-statement` | error | BW2003 | A second definition with the same signature in one scope, or a root-level definition that redefines a built-in. Definitions inside another definition may shadow built-ins |
| `undefined-variable` | warning | BW2001 | A read of a variable that no reachable scope assigns, binds as a parameter, loop item, or caught error, or receives from a suite. It may still be an input variable, so this is a warning; each name is reported once. The help suggests a reachable variable with a [near name](diagnostics.md#near-name-suggestions) |
| `unreachable-code` | warning | — | The first statement after one that always leaves its block: `Return`, `Break`, `Continue`, `Rethrow`, the built-in `Fail`, or an `If`/`Else` or `Try`/`Catch` whose every branch leaves |
| `argument-kind` | error | BW3003 | A literal argument of a kind the built-in's parameter never accepts, such as `Sleep |"10"|` |
| `condition-kind` | error | BW3003 | A literal `If` or `While` condition that is not a boolean, or a literal `For` input that is not an array |
| `import-failure` | error | BW6001 | An import that cannot load: a URL, a path that is not a `.botwork` file, or a file that is missing or unreadable |
| `import-cycle` | error | BW6002 | An import that reaches a module already being imported; the message shows the chain |
| `duplicate-namespace` | error | BW6003 | A second import under an alias already used in the same scope |

Syntax errors (BW1001) and misplaced control statements (BW1002) are reported as
errors before the rules run, in the checked file and in every module.

## Modules

An import is resolved as the run would resolve it: relative to the importing
file's directory. Each module is read and checked once. Its findings and syntax
errors are reported against the module's own canonical path, as the run reports
them. The summary counts the modules it checked, as in
`[check] 1 file, 2 modules: 0 errors, 0 warnings`.

A module's statements are what its definitions declare, plus the statements of
the modules it imports under their aliases. Module code does not run, so a
definition inside a branch counts as declared.

Suites are checked as they run. The library, setup, teardown, and every case are
checked; a case may read the variables its suite setup assigns and its dataset
row. A finding in the library is reported once, not once per case.

## What a passing check does not establish

Before the summary, the check lists what only a run can tell:

```text
[check] not checked: input variables base_url, token
[check] not checked: results of calls that use files (2), the network (1)
[check] 1 file: 0 errors, 2 warnings
```

- **Input variables.** Variables the checked file reads but never assigns must
  arrive with `--var` or `--vars-file`; their values are unknown.
- **The outside world.** Calls whose results depend on files and the working
  directory, environment variables, processes, the network, or the clock are
  counted by kind. The check cannot know what they will return or whether they
  will fail. Lexical path statements, such as `Join Path`, are not counted.
- **Unreadable modules.** Calls into a module that could not be read or parsed
  are counted, since their statements are unknown; the import itself is an
  error.

## Limits

The check reports what it can establish without running code, and nothing more:

- **Datasets.** External suite datasets are read when the suite runs, not
  checked.
- **Host statements.** The CLI knows only the built-in statements. Rust hosts
  can check against their own registrations with `Analyzer::with_statements`.
- **Variable order.** A variable assigned anywhere in a scope counts as bound,
  so a read before its first assignment is not reported.
- **Values.** Only literal arguments and conditions are kind-checked. Values that
  depend on variables or calls, arithmetic errors, collection access, and I/O are
  checked when the script runs.
- **Handled errors.** A mistake inside `Try` is still reported, even if a
  `Catch` handles the error it raises.

## Checking from Rust

`core::analysis` exposes the same checks to hosts:

```rust
use botwork::core::{
    analysis::{Analyzer, Rule, Severity},
    ast::Program,
};

let program = Program::parse_detailed("demo.botwork", "Log |1| |2|\nLog |total|\n")?;
let findings = Analyzer::default().check_program(&program);
assert_eq!(findings[0].rule, Rule::UndefinedStatement);
assert_eq!(findings[0].severity(), Severity::Error);
assert_eq!(findings[1].rule, Rule::UndefinedVariable);
assert!(findings[1].to_string().starts_with("demo.botwork:2:6-2:11: warning[undefined-variable]"));
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

`Analyzer::check_suite` checks a parsed suite. Each `Finding` carries its rule,
source span, message, and help. `Rule::code` gives the runtime code a rule
predicts. `Analyzer::with_modules(directory)` also reads imported local
modules, and `report_program` and `report_suite` return a `Report` that adds
module diagnostics, input variables, and counts of calls that depend on the
outside world (`EXTERNAL_STATEMENTS` lists them).
