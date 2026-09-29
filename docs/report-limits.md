# Report limits and recovery

Every report output has a fixed bound. Work beyond a bound is counted, truncated
with a marker, or reduced to a summary; it is never silently dropped. A long run
keeps no value from its loop iterations: records hold counts, bounded text, and
top-level statements only. This page lists the bounds and how interrupted writes
recover.

## Selection

| Bound | Limit | Beyond it |
| --- | --- | --- |
| Runs selected by `--file` | 4,096 | BW8001 before any output or run |
| Cases selected by `--suite`, rows included | 4,096 | BW8001 during discovery |
| Suites per invocation | 64 | BW8001 during discovery |

Every per-run structure an invocation keeps grows with the selection, so these
bounds also bound the reports. Examples include report records, summaries,
console totals, and the journal. The scheduler holds live state only for the at
most 64 runs it has admitted.

## Per-run retention

CLI reports record each run with these limits (embedded hosts choose their own;
see [run records](run-records.md#limits)):

| Bound | Limit | Beyond it |
| --- | --- | --- |
| Top-level statements | 256 | `omitted_statements` counts the rest |
| Logs | 64 | `omitted_logs` counts the rest; `logged_bytes` stays exact |
| Text of one log | 1 KiB | Cut at a character boundary and marked `…[truncated]` |
| Captured log text | 16 KiB | Later logs are counted, not kept |
| Error message | 2 KiB | Cut and marked; `truncated` is true |
| Cause codes | 16 | `omitted_causes` counts the rest |
| Artifacts | 64 | `omitted_artifacts` counts the rest |

A loop is one statement however often it repeats. A 25,000-iteration loop that
logs every value produces a report under 64 KiB. The report keeps 64 logs, and
its byte count covers all 25,000.

## Report sizes

| Output | Bound | Beyond it |
| --- | --- | --- |
| JSON report run details | 32 MiB serialized, measured as each run is recorded | Later runs keep a summary: number, identity, status, completeness, and error code |
| HTML report run details | 64 MiB of rendered page | Later runs keep a summary section, while the run table still lists every run |
| HTML source excerpts | 64 files of at most 4 MiB, 64 KiB line prefixes, 220 characters per excerpt | "source unavailable" |
| Report journal | One line per selection, start, kept record, and fixture | Reconciliation refuses journals over 256 MiB |
| Listener queue | 1–1,048,576 events (default 8,192) | The listener is detached and delivery fails |
| Console recap | 50 entries | `…and N more`, with exact counts |
| Assertion artifacts | 256 files and 64 MiB per invocation | The artifact write fails and the run's failure is kept |
| Failed-case record | 4,096 IDs, 2 MiB | Rejected as invalid |

- **JSON report.** It stays a little over 32 MiB at most: full details within
  the budget, plus at most 4,096 bounded summaries and 64 fixture entries.
- **HTML report.** Escaping can enlarge recorded text several times, so the page
  has its own budget rather than inheriting the JSON one.
- **Listener memory.** Each event carries at most one bounded log text or error
  message, so the queue's memory is bounded by its size. Log events past the
  retention limits carry an empty `text` and an exact `bytes`.

## Recovering from interrupted writes

- **Atomic publication.** Reports, markers, and failed-case records are written
  to a temporary file beside the output, `.{name}.{process}.{n}.tmp` (mode
  `0600`). Each is then `fsync`ed, renamed over the output, and followed by an
  `fsync` of the directory. A termination at any point leaves the previous
  content or the new content, never a partial file.
- **Stale temporary files.** An interrupted publication can leave its temporary
  file behind. The next invocation that locks the same output removes that
  output's temporaries. Only the lock holder writes them, so any found under the
  lock are stale. Other outputs' files, symbolic links, directories, and
  non-matching names are left alone.
- **Incomplete markers.** A report is marked incomplete before any run starts.
  After a forced termination, its [journal](terminal-outcomes.md#forced-termination-and-reconciliation)
  and `--reconcile-report` produce the final report. A torn final journal line,
  cut short by the termination, is ignored and reported.
- **Assertion artifacts.** Each artifact is written into the invocation's own
  fresh directory and persisted without replacing any existing file, so an
  interrupted write stays inside that directory.
- **Listener streams.** A stream that ends without `stream_finished` is
  incomplete.

## Verification

`cargo test --locked --test report_limits` covers:

- a 25,000-iteration logging loop that keeps exact counts, a report under 64 KiB,
  a page under 256 KiB, and a complete event stream whose text stays within the
  retention limits;
- the batch selection bound, rejected before any report or artifact directory
  exists;
- stale temporary files of the JSON report, HTML report, and failed-case record,
  removed while an unrelated one is kept.

Unit tests cover:

- the sweep's naming, symbolic link, and directory rules;
- the HTML page budget at its exact boundary;
- the existing run-detail budget, retention, and listener overflow tests.

[Validation evidence](report-limits-evidence.json) records the measured profiles
and mutations.
