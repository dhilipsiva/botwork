# Reports and outputs

Every invocation reports what happened in three ways: its exit status, its
console output, and any machine-readable outputs it was asked for. This page
maps them, and says how the machine-readable ones are versioned.

## Choosing an output

| Output | Enable with | For | Details |
| --- | --- | --- | --- |
| Exit status | always | CI gates | [Exit statuses](cli.md#exit-statuses) |
| Console report | always, on stderr; script output stays on stdout | People watching a run | [Console reports](console-report.md) |
| JSON report | `--report-json PATH` | Tools and dashboards; the complete record of an invocation | [JSON reports](json-report.md) |
| HTML report | `--report-html PATH` | People reading results after the run; renders the JSON report | [HTML reports](html-report.md) |
| Event stream | `--listener PROGRAM` | Live integrations: an external program reads events as they happen | [Listeners](listeners.md) |
| Failed-case record | `--failures PATH` | Rerunning only what failed, with `--rerun-failed` | [Suites](suites.md) |
| Assertion artifacts | `--assertion-artifacts PATH` | Full expected and actual values that are too large for a diagnostic | [Assertion diagnostics](assertion-diagnostics.md) |
| Run records | The Rust API | Hosts that embed Botwork and keep structured results | [Run records](run-records.md) |

The JSON and HTML reports are written as incomplete markers before anything
runs, and completed at the end. If an invocation is forcibly terminated,
`--reconcile-report` finishes them from their journal. See
[terminal outcomes](terminal-outcomes.md) and [report limits](report-limits.md).

## Versioned formats

Each machine-readable output carries `format` and `version` fields; the event
stream carries them in its first event.

| Output | `format` | Current version | A new field | Details |
| --- | --- | --- | --- | --- |
| JSON report | `botwork-report` | 1 | Needs a new version: the schema is closed | [JSON report versioning](json-report.md#versioning) |
| Event stream | `botwork-events` | 1 | Keeps the version; consumers ignore unknown fields and event kinds | [Listener versioning](listeners.md#versioning) |
| Run record | `botwork-run` | 1 | Keeps the version; consumers ignore unknown fields | [Run records](run-records.md) |
| Failed-case record | `botwork-failed-cases` | 2, and version 1 is still read | Needs a new version: records have exactly the documented fields | [Suites](suites.md) |

In every format, removing a field, changing its type, or changing what a value
means increments the version. Consumers should check `format` and `version`
first, and reject a format or version they do not understand. Diagnostic codes
inside these outputs are [stable codes](diagnostics.md#stable-codes-and-repairs).
See [compatibility](compatibility.md) for what stays stable across releases.
