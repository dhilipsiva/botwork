# Command-line reference

The `botwork` executable runs scripts and suites, and provides the tools that
check, format, and edit them. This page lists every option; `botwork --help`
prints the same list. `tests/cli_reference.rs` keeps the two in step.

## Modes

One invocation does one of these things:

| Mode | Invocation | Details |
| --- | --- | --- |
| Run scripts | `botwork --file a.botwork [--file b.botwork ...]` | [Getting started](getting-started.md), [parallel runs](parallel-cli.md) |
| Run suites | `botwork --suite checkout.suite.botwork` | [Suites](suites.md) |
| List cases | `botwork --suite checkout.suite.botwork --list-cases` | [Suites](suites.md) |
| Check without running | `botwork --check --file a.botwork` | [Checking scripts](check.md) |
| Format | `botwork --format --file a.botwork`, or `--format-check` | [Formatting](format.md) |
| Language server | `botwork --lsp` | [Language server](lsp.md) |
| Statement help | `botwork --list-statements`, `botwork --statement-help "Log \|value\|"` | [Built-ins](builtins.md), [statement reference](statements.md) |
| Finish a report | `botwork --reconcile-report report.json` | [Report limits](report-limits.md) |

## Options

### Inputs and selection

| Option | Default | Effect | Details |
| --- | --- | --- | --- |
| `-f`, `--file <FILE>` | | A script to run. Repeat it to run several; each occurrence starts a fresh run. | [Parallel runs](parallel-cli.md) |
| `--suite <SUITE>` | | A suite file to discover cases from. Repeat it for several; paths keep their order. | [Suites](suites.md) |
| `--case <CASE>` | | Select a case or dataset row by its stable ID, `suite/case` or `suite/case/row`. Repeatable. | [Suites](suites.md), [parameterized cases](parameterized-cases.md) |
| `--tag <TAG>` | | Include cases with any of these tags, inherited or local. Repeatable. | [Suites](suites.md) |
| `--exclude-tag <EXCLUDE_TAG>` | | Exclude cases with any of these tags. Repeatable. | [Suites](suites.md) |
| `--list-cases` | | List the selected cases as JSON lines, without running libraries or cases. | [Suites](suites.md) |
| `--rerun-failed <PATH>` | | Select only the IDs in a completed failed-case record. | [Setup failures](setup-failure.md), [suites](suites.md) |

### Tools

| Option | Default | Effect | Details |
| --- | --- | --- | --- |
| `--lsp` | | Serve the Language Server Protocol on stdin and stdout. Takes no other option. | [Language server](lsp.md) |
| `--check` | | Check files or suites without running them: syntax, control placement, and lint rules. | [Checking scripts](check.md) |
| `--format` | | Rewrite files or suites in canonical layout. `*.dataset.botwork` files are datasets. | [Formatting](format.md) |
| `--format-check` | | Report files or suites whose layout is not canonical, without changing them. | [Formatting](format.md) |
| `--list-statements` | | List the built-in statement headers. | [Built-ins](builtins.md) |
| `--statement-help <HEADER>` | | Show a built-in's parameters, return kinds, and documented errors. | [Statement reference](statements.md) |

### Reports and records

| Option | Default | Effect | Details |
| --- | --- | --- | --- |
| `--report-json <PATH>` | | Write a versioned `botwork-report` JSON document, marked incomplete until the invocation finishes. | [JSON reports](json-report.md) |
| `--report-html <PATH>` | | Write a self-contained HTML report, marked incomplete until the invocation finishes. | [HTML reports](html-report.md) |
| `--reconcile-report <PATH>` | | Finish a report whose invocation was forcibly terminated: started runs without an outcome become interrupted. | [Report limits](report-limits.md) |
| `--failures <PATH>` | | Write a failed-case record. It is invalidated before discovery and completed at the end. | [Suites](suites.md) |
| `--assertion-artifacts <PATH>` | | Save full assertion operands in a fresh directory beneath `PATH`. | [Assertion diagnostics](assertion-diagnostics.md) |
| `--listener <PROGRAM>` | | Stream execution events as JSON Lines to `PROGRAM`'s stdin. The program runs without a shell. | [Listeners](listeners.md) |
| `--listener-arg <ARG>` | | An argument for the listener program. Repeatable. | [Listeners](listeners.md) |
| `--listener-queue <EVENTS>` | 8192 | Events queued for a slow listener before it is detached, from 1 to 1,048,576. | [Listeners](listeners.md) |
| `--listener-timeout-ms <MS>` | 10000 | Milliseconds to wait for the listener after the last event, from 1 to 3,600,000. | [Listeners](listeners.md) |

### Variables and secrets

| Option | Default | Effect | Details |
| --- | --- | --- | --- |
| `--var <NAME=JSON>` | | Set a root variable from JSON. Repeatable; overrides every variable file. | [Input variables](input-variables.md) |
| `--vars-file <PATH>` | | Read root variables from a JSON object. Repeatable; later files override earlier ones. | [Input variables](input-variables.md) |
| `--secret <NAME>` | | Mark an input variable secret, so its values are masked in all output. Repeatable. | [Secrets](secrets.md) |
| `--secret-env <NAME=VARIABLE>` | | Define a secret string input from an environment variable. Repeatable. | [Secrets](secrets.md) |

### Execution

| Option | Default | Effect | Details |
| --- | --- | --- | --- |
| `-j`, `--jobs <JOBS>` | 4 | Maximum simultaneous runs, including file and input preparation, from 1 to 64. | [Parallel runs](parallel-cli.md) |
| `--debug` | | Trace top-level statement locations on stderr. | [Language behavior](language.md) |
| `--timeout-ms <TIMEOUT_MS>` | none | A cooperative timeout for each run, including loading and parsing after admission. | [Cancellation](cancellation.md) |
| `--suite-timeout-ms <SUITE_TIMEOUT_MS>` | none | A timeout for the whole lifetime of each suite that has `SuiteSetup` or `SuiteTeardown`. | [Fixtures](fixtures.md) |
| `--stop-grace-ms <STOP_GRACE_MS>` | 2000 | After a stop, milliseconds to wait for started blocking work before abandoning it. | [Shutdown](shutdown.md) |

### Limits

| Option | Default | Effect | Details |
| --- | --- | --- | --- |
| `--max-steps <MAX_STEPS>` | 1000000 | Evaluation steps before a run is terminated. | [Embedded runs](embedded-runs.md#initial-budgets) |
| `--max-call-depth <MAX_CALL_DEPTH>` | 32 | Nested native or custom calls. | [Embedded runs](embedded-runs.md#initial-budgets) |
| `--max-evaluation-depth <MAX_EVALUATION_DEPTH>` | 96 | Combined evaluation depth; 96 is also the ceiling. | [Embedded runs](embedded-runs.md#initial-budgets) |
| `--max-output-record-bytes <MAX_OUTPUT_RECORD_BYTES>` | 8388608 | Bytes in one `Log`, debug trace, or statement-help record. | [Output limits](output-limits.md) |
| `--max-output-bytes <MAX_OUTPUT_BYTES>` | 33554432 | Output bytes per run or help request; failed writes count too. | [Output limits](output-limits.md) |
| `--max-cleanup-steps <MAX_CLEANUP_STEPS>` | 10000 | Evaluation steps for each independent `Finally` cleanup. | [Cleanup](cleanup.md) |
| `--cleanup-timeout-ms <CLEANUP_TIMEOUT_MS>` | 5000 | Cooperative timeout for each independent `Finally` cleanup, in milliseconds. | [Cleanup](cleanup.md) |

Runs execute on the CLI's own 8 MiB thread, so programs reach the same depth
limits on every platform; see [stack headroom](embedded-runs.md#stack-headroom).

### Information

| Option | Default | Effect | Details |
| --- | --- | --- | --- |
| `-h`, `--help` | | Print the options. | |
| `-V`, `--version` | | Print the version. | [Compatibility](compatibility.md) |

## Exit statuses

| Status | Meaning |
| --- | --- |
| 0 | Every selected run or case, and every shared setup, succeeded. `--check` found no errors, `--format-check` found nothing to change, or the language server received `shutdown` before `exit`. |
| 1 | Something failed, timed out, was skipped, or was interrupted and then drained. `--check` found an error, `--format` or `--format-check` could not read or parse a file, `--format-check` found a file to change, or the language server received `exit` without `shutdown`. |
| 2 | Invalid command-line usage; nothing ran. |
| 130 | A second interrupt exited at once, leaving reports for [reconciliation](terminal-outcomes.md#interruption). |

Diagnostics go to stderr and program output to stdout. See
[console reports](console-report.md) and [terminal outcomes](terminal-outcomes.md).
