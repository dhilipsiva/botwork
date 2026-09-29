# HTML report

`--report-html PATH` writes one self-contained HTML page for the invocation. It
works with a single `--file`, a batch of files, and `--suite` runs. The page is
rendered from the same document as the [JSON report](json-report.md): the same
verdict, exit status, run records, and shared fixtures. Both options can be
given together; the two files then describe the same invocation.

Open the page in a browser; it needs no network access or scripts. Archive it
with the directory given to `--assertion-artifacts` when that directory lies
beside or beneath the report, so artifact links keep working.

## Example

<!-- botwork-test: html-report-example -->
```botwork
|items| = |[3, 4]|
|total| = |0|
For |item| In |items| {
    |total| = |total + item|
}
Log |"total"|
Log |total|
Assert |total| Equals |7|
```

`botwork --file checkout.botwork --report-html report.html` prints `total` and
`7`. The page then shows:

- the verdict (`succeeded`), mode, start and finish times, duration, exit code,
  and delivery;
- a summary of runs by status;
- one run with six statements, each with its source excerpt, start offset,
  duration, and a timeline bar;
- the two logs.

## Page contents

| Section | Contents |
| --- | --- |
| Header | Overall status, mode, start and finish times, duration, exit code, and delivery |
| Summary | The run total, each nonzero status count, and shared fixture failures |
| Shared fixtures | Each suite's `SuiteSetup`/`SuiteTeardown` outcome, with its error |
| Runs | A table of every run: number, name and ID, dataset row, status, duration, and error code |
| Run details | One collapsible section per run, open when the run failed |

Each run's details contain:

- **Identity.** Dataset and row for dataset cases, and the skip reason for
  skipped cases.
- **Times.** The start and finish times and the duration.
- **Error.** The code, status, message, location with a source excerpt, and cause
  codes.
- **Statements.** For each top-level statement: its kind and `file:line:column`,
  a source excerpt with the statement marked, and its start offset and duration.
  A timeline bar places the statement within the run, and its status and code
  follow.
- **Logs.** The captured text of each log, under the report's
  [retention limits](json-report.md#retention), and the total bytes logged.
- **Artifacts.** Links to assertion evidence files. A link is relative when the
  file lies beneath the report's directory, and a `file://` URL otherwise. Each
  link sits in the run that produced it.

Runs beyond the report's 32 MiB [detail budget](json-report.md#retention) show
only their summary row and a note that details were omitted. Rendered details
also stop at a 64 MiB page budget, because escaping can enlarge recorded text;
later runs keep a summary section. See [report limits](report-limits.md).

### Source excerpts

Records store locations, not source text. When the page is written, each
location's line is read from its file and shown only when the file still places
the location at the recorded line and column. A file changed during the run, an
unreadable or synthetic source such as `<builtin>`, or a file beyond 4 MiB shows
"source unavailable" instead of text that may no longer match. At most 64 files
are read, and each excerpt keeps up to 60 characters before the statement, 120
of it, and 40 after. A line that contains a [secret input](secrets.md) is shown
whole and masked, without the statement highlight.

## Rendering recorded content as text

Every recorded value is escaped before it reaches the page: names, IDs, dataset
rows, log text, error messages, source excerpts, and artifact paths. Script
output such as `<script>` therefore appears as text, never as markup. Link
targets are percent-encoded, and only in-page anchors, relative paths beneath the
report, and `file://` URLs are produced.

As a second barrier, the page's Content-Security-Policy
(`default-src 'none'; style-src 'unsafe-inline'`) forbids scripts, frames, images,
and every remote request. Only the page's own inline styles apply.

## Lifecycle and file protection

The HTML report follows the JSON report's [lifecycle](json-report.md#lifecycle):

- **Incomplete marker.** Before discovery or any script effect, the path is
  locked and a page saying the report is incomplete is published.
- **Replacement.** The complete page replaces it atomically once every selected
  run has a record, and the marker stays when the invocation stops early.
- **Existing files.** An existing file is replaced only when its first 4 KiB
  contain the report's generator marker,
  `<meta name="generator" content="botwork-report-html 1">`. Other files,
  directories, and special files are refused before any run starts.
- **Shared paths.** The same lock rejects using one path for two outputs.
- **Stopped invocations.** After an interruption or a reconciliation, a banner
  says the invocation was interrupted. A second banner notes that runs without a
  record are missing when not every selected run started. See
  [terminal outcomes](terminal-outcomes.md).

The page grows with the retained records. The JSON report's per-run retention
and run detail budget bound it.

## Verification

`cargo test --locked --test html_report` covers:

- runs, timings, source excerpts, logs, and agreement with the JSON verdict;
- hostile names, logs, and messages rendered as text, with only the renderer's own
  tags on the page;
- artifact links attached to the runs that produced them;
- suite rows, skips, and shared fixtures;
- the incomplete marker;
- changed sources;
- refusal of unrelated and shared output paths;
- the example above.

Every page the tests read is checked for its generator marker, its
Content-Security-Policy, a tag allowlist, and safe link targets.
Unit tests in `src/report_json/html.rs` cover escaping, link encoding, excerpt
validation, and duration formatting.
[Validation evidence](html-report-evidence.json) records the measured profiles
and mutations.
