# JSON report

`--report-json PATH` writes one versioned `botwork-report` document for the whole
invocation. It works with a single `--file`, a batch of files, and `--suite`
runs. The document holds the same verdict and exit status as the
[console report](console-report.md), plus the full [run record](run-records.md)
of each run. CI systems and other tools can read it without parsing console
text. Consumers can validate a report against
[`json-report.schema.json`](json-report.schema.json), a JSON Schema (draft
2020-12) for this version. `--report-html PATH` renders the same document as a
self-contained [HTML report](html-report.md).

## Example

<!-- botwork-test: json-report-suite -->
```botwork-suite
Suite |"checkout"| {
    Dataset |"carts"| {
        Row |"single"| Values |{items: 1, total: 5}|
        Row |"pair"| Values |{items: 2, total: 9}|
    }
    Case |"total"| Using |"carts"| As |cart| {
        Log |cart.items|
        Assert |cart.items * 5| Equals |cart.total|
    }
}
```

Running `botwork --suite checkout.suite.botwork --report-json report.json`
exits 1 and writes the following. Timestamps and durations vary between runs:

```json
{
  "format": "botwork-report",
  "version": 1,
  "complete": true,
  "mode": "suites",
  "started_at": "2026-09-29T06:28:31.473145Z",
  "finished_at": "2026-09-29T06:28:31.479572Z",
  "duration_us": 6399,
  "exit_code": 1,
  "verdict": {
    "status": "failed",
    "complete": true,
    "cases": {
      "total": 2,
      "succeeded": 1,
      "expected_failure": 0,
      "failed": 1,
      "unexpected_pass": 0,
      "skipped": 0,
      "cancelled": 0,
      "timed_out": 0,
      "limit_exceeded": 0,
      "interrupted": 0
    },
    "fixture_failures": 0,
    "delivery": "complete"
  },
  "runs": [
    {
      "number": 1,
      "format": "botwork-run",
      "version": 1,
      "identity": {"id": "checkout/total/single", "name": "total / single", "dataset": "carts", "row": "single"},
      "status": "succeeded",
      "complete": true,
      "skip_reason": null,
      "started_at": "2026-09-29T06:28:31.474679Z",
      "finished_at": "2026-09-29T06:28:31.478112Z",
      "duration_us": 3431,
      "statements": [
        {
          "index": 0,
          "kind": "call",
          "location": {
            "file": "checkout.suite.botwork",
            "start_byte": 210,
            "end_byte": 226,
            "line": 7,
            "column": 9,
            "end_line": 7,
            "end_column": 25
          },
          "offset_us": 3086,
          "duration_us": 241,
          "status": "succeeded",
          "code": null
        },
        {
          "index": 1,
          "kind": "call",
          "location": {
            "file": "checkout.suite.botwork",
            "start_byte": 235,
            "end_byte": 278,
            "line": 8,
            "column": 9,
            "end_line": 8,
            "end_column": 52
          },
          "offset_us": 3338,
          "duration_us": 53,
          "status": "succeeded",
          "code": null
        }
      ],
      "omitted_statements": 0,
      "logs": [{"statement": 0, "offset_us": 3239, "bytes": 1, "text": "1", "truncated": false}],
      "omitted_logs": 0,
      "logged_bytes": 1,
      "error": null,
      "artifacts": [],
      "omitted_artifacts": 0,
      "events": 7
    },
    {
      "number": 2,
      "format": "botwork-run",
      "version": 1,
      "identity": {"id": "checkout/total/pair", "name": "total / pair", "dataset": "carts", "row": "pair"},
      "status": "failed",
      "complete": true,
      "skip_reason": null,
      "started_at": "2026-09-29T06:28:31.478264Z",
      "finished_at": "2026-09-29T06:28:31.479124Z",
      "duration_us": 859,
      "statements": [
        {
          "index": 0,
          "kind": "call",
          "location": {
            "file": "checkout.suite.botwork",
            "start_byte": 210,
            "end_byte": 226,
            "line": 7,
            "column": 9,
            "end_line": 7,
            "end_column": 25
          },
          "offset_us": 513,
          "duration_us": 180,
          "status": "succeeded",
          "code": null
        },
        {
          "index": 1,
          "kind": "call",
          "location": {
            "file": "checkout.suite.botwork",
            "start_byte": 235,
            "end_byte": 278,
            "line": 8,
            "column": 9,
            "end_line": 8,
            "end_column": 52
          },
          "offset_us": 696,
          "duration_us": 119,
          "status": "failed",
          "code": "BW9001"
        }
      ],
      "omitted_statements": 0,
      "logs": [{"statement": 0, "offset_us": 615, "bytes": 1, "text": "2", "truncated": false}],
      "omitted_logs": 0,
      "logged_bytes": 1,
      "error": {
        "code": "BW9001",
        "status": "failed",
        "message": "Assertion failed: Expected 9 (Int), got 10 (Int)\n  difference at $: expected 9 (Int), got 10 (Int)\n  full operands: details.expected / details.actual (typed JSON)",
        "truncated": false,
        "location": {
          "file": "checkout.suite.botwork",
          "start_byte": 235,
          "end_byte": 278,
          "line": 8,
          "column": 9,
          "end_line": 8,
          "end_column": 52
        },
        "causes": [],
        "omitted_causes": 0
      },
      "artifacts": [],
      "omitted_artifacts": 0,
      "events": 7
    }
  ],
  "omitted_run_details": 0,
  "fixtures": []
}
```

## Lifecycle

1. **Before any run.** The report path is checked and locked, and an incomplete
   marker is published. This happens before suite discovery, input loading, or any
   script effect. The marker has `complete: false`; `finished_at`, `duration_us`,
   `exit_code`, and `verdict` are `null`; `runs` and `fixtures` are empty.
2. **During the invocation.** Bounded records are kept in memory as runs finish,
   and a [journal](terminal-outcomes.md#forced-termination-and-reconciliation)
   beside the report records each run as it starts and finishes. The report file
   itself is not rewritten per run.
3. **After the verdict.** The final report replaces the marker, and the journal
   is removed. The new file is written beside the target, `fsync`ed, and renamed
   over it, so readers see either the marker or the final report, never a
   partial file.

The final report is also published when admission stopped early:

- after an [interruption](terminal-outcomes.md#interruption), with
  `verdict.delivery: "interrupted"`;
- after a console reporting failure or a failed `--failures` record, with
  `verdict.delivery: "failed"`.

Every run that started then has its one record. The report is `complete` only
when every selected run has one.

The marker stays in these cases:

- a discovery or configuration error before any run;
- a lost run task, whose missing record the command reports;
- a forced termination. Its journal remains, and
  `botwork --reconcile-report PATH` finishes the report with the started runs
  `interrupted`; until then, new invocations refuse the path.

**Consumers must check `complete` and `verdict.delivery` before trusting that
the report covers every selected run.**

Command-line usage errors exit 2 before the report path is touched.

### Protecting existing files

- An existing file is replaced only when it is an ordinary file of at most 64 MiB
  holding JSON whose `format` is `"botwork-report"`. Anything else is refused
  before any run starts and left unchanged.
- Directories, symbolic links, and other special files are refused.
- A persistent `PATH.lock` sidecar holds an exclusive advisory lock for the whole
  invocation. A concurrent invocation writing the same report fails instead of
  interleaving.
- The same lock rejects using one path for both `--report-json` and `--failures`.
- Files are created with mode `0600` on Unix.

## Document

| Field | Contents |
| --- | --- |
| `format`, `version` | `"botwork-report"`, `1` |
| `complete` | `true` once the document is final and every selected run has a record |
| `mode` | `"file"` (one `--file`), `"batch"` (several), or `"suites"` |
| `started_at`, `finished_at` | RFC 3339 UTC times with microseconds; `finished_at` is `null` in the marker |
| `duration_us` | Monotonic invocation duration in microseconds, or `null` |
| `exit_code` | `0` or `1`, the process exit status the verdict decides, or `null` |
| `verdict` | `status`, `complete`, per-status `cases` counts, `fixture_failures`, and `delivery` |
| `runs` | One entry per selected run, ordered by `number` |
| `omitted_run_details` | Number of runs reduced to summaries by the size budget |
| `fixtures` | One entry per shared suite fixture that ran |

The verdict is the `RunVerdict` that also decides the console summary and the
exit status; see [acceptance policy](acceptance-policy.md). In a complete report,
`exit_code` always equals the process exit status. `verdict.cases.total` always
equals the number of `runs`.

`verdict.delivery` is one of these values:

- `complete`;
- `failed`, when the console report, a `--failures` record, a
  [listener](listeners.md), or the journal could not be delivered;
- `interrupted`, after a signal or a reconciliation.

In the last two cases the verdict fails with `exit_code` 1. The counts still
describe every run that has a record.

## Runs

Each entry is a `botwork-run` [run record](run-records.md) with its `number`
added:

- `number` matches the console's `[run N]` numbering: files and cases in selection
  order, whatever order they finish in.
- **Files.** Both `id` and `name` are the path as given on the command line.
- **Cases.** `id` is the stable suite/case ID, `name` is the display name, and
  dataset rows add `dataset` and `row`.
- **Statements** are the run's top-level statements. A case with `CaseSetup` or
  `CaseTeardown` records its body as one `finally` statement.
- **Skipped cases** have one `RunSkipped` record: `status: "skipped"`, a
  `skip_reason` of `suite_setup_failed` or `suite_stopped`, and no times or
  statements.
- **Failures** carry the unhandled diagnostic's code, status, bounded message,
  location, and cause codes. An entry file that cannot be read has no language
  diagnostic and is recorded as BW7003, as embedded runs record it.
- **Artifacts.** Assertion evidence files exported by
  [`--assertion-artifacts`](assertion-diagnostics.md) are attached to the run that
  produced them, as `{"kind": "assertion", "path": ...}`. Each path is the one
  printed on stderr.

### Retention

Each run keeps at most:

- 256 statements;
- 64 logs, each up to 1 KiB of text, and 16 KiB of log text in all;
- a 2 KiB error message;
- 16 cause codes;
- 64 artifacts.

The `omitted_*` fields count what was not kept. `logged_bytes` still counts every
logged byte.

The serialized runs share a 32 MiB budget. Each run is measured once, when it is
recorded, and keeps its full details while they fit. A run that finishes after the
budget is spent becomes a summary, so memory stays bounded however many runs are
selected. With `--jobs 1`, runs finish in `number` order. A summary has these
fields:

- `number`, `identity`, `status`, `complete`;
- `error`, the primary code only;
- `details_omitted: true`.

`omitted_run_details` counts these summaries. Counts, statuses, and the verdict
are never reduced. [Report limits](report-limits.md) lists every bound in one
place, including the 4,096-run selection limit and the recovery of interrupted
writes.

## Shared fixtures

Each suite whose `SuiteSetup`/`SuiteTeardown` ran adds one `fixtures` entry
containing:

- the suite ID;
- the fixture's status;
- its error, in the same form as a run error, or `null`.

A failed fixture counts in `verdict.fixture_failures` and fails the invocation,
even when every case succeeded.

## Versioning

`format` and `version` identify the document. Consumers should reject other
formats and any version they do not understand.

Version 1 is closed: the schema rejects any field it does not document. The
following changes increment `version`, with a new schema:

- adding, removing, or renaming a field;
- changing a field's type;
- changing what a value means.

Two vocabularies stay open within a version: statement `kind` (one name per
language statement kind) and artifact `kind`. Treat unknown values as opaque
labels. Diagnostic codes are the [stable codes](diagnostics.md#stable-codes-and-repairs).

## Verification

`cargo test --locked --test json_report` covers the following, and validates
every report it reads against the schema:

- single-file, batch, and suite reports;
- selection order and every stop status;
- dataset rows, skipped cases, and shared fixtures;
- artifact attachment;
- the incomplete marker, both during a run and after a discovery failure;
- refusal of unrelated and shared output paths;
- agreement between the report and the exit status;
- a check that the schema rejects undocumented, missing, and unknown values;
- the example above.

Unit tests in `src/report_json.rs` cover the run budget, missing records, the
artifact limit, and replacement rules.
[Validation evidence](json-report-evidence.json) records the measured profiles
and mutations.
