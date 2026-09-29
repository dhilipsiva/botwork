# Checking scripts

`botwork --check` finds mistakes without running anything. It parses each file,
validates control placement, and applies the lint rules below. No statement
runs: nothing is logged, no file is written, and no module, process, or request
is started.

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
would report.

## Rules

| Rule | Severity | Predicts | Finds |
| --- | --- | --- | --- |
| `undefined-statement` | error | BW2002 | A call that no built-in, definition, or import satisfies. When a statement with the same words but other parameter positions exists, it is suggested. A call through an alias that no `Import` defines is reported too |
| `statement-before-definition` | warning | BW2002 | A call that runs before the definition it names. Definitions register when they run, but a definition's body resolves names when it is called, so calling a later definition from a body is fine |
| `duplicate-statement` | error | BW2003 | A second definition with the same signature in one scope, or a root-level definition that redefines a built-in. Definitions inside another definition may shadow built-ins |
| `undefined-variable` | warning | BW2001 | A read of a variable that no reachable scope assigns, binds as a parameter, loop item, or caught error, or receives from a suite. It may still be an input variable, so this is a warning; each name is reported once |
| `unreachable-code` | warning | — | The first statement after one that always leaves its block: `Return`, `Break`, `Continue`, `Rethrow`, the built-in `Fail`, or an `If`/`Else` or `Try`/`Catch` whose every branch leaves |
| `argument-kind` | error | BW3003 | A literal argument of a kind the built-in's parameter never accepts, such as `Sleep |"10"|` |
| `condition-kind` | error | BW3003 | A literal `If` or `While` condition that is not a boolean, or a literal `For` input that is not an array |

Syntax errors (BW1001) and misplaced control statements (BW1002) are reported as
errors before the rules run.

Suites are checked as they run. The library, setup, teardown, and every case are
checked; a case may read the variables its suite setup assigns and its dataset
row. A finding in the library is reported once, not once per case.

## Limits

The check reports what it can establish without running code, and nothing more:

- **Imported modules.** Calls into an imported module, such as `helpers::Double`,
  are not checked, because the module is not loaded.
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
predicts.
