# Terminal outcomes

Every run or case that starts ends with exactly one terminal outcome, and that
outcome reaches every output once:

- one terminal console record;
- one record in the [JSON](json-report.md) and [HTML](html-report.md) reports;
- one `run_finished` event for [listeners](listeners.md).

This holds after failures, after console reporting errors, and after an
interruption. When a forced termination prevents it, the reports can be
reconciled afterwards. Runs that had started but have no outcome then become
`interrupted`; a pass is never inferred from partial output.

## Ownership of events and artifacts

- **Numbering.** Each run is numbered in selection order before it starts. Its
  events carry that number and their own gapless `sequence`, including events
  from workers, imported modules, cleanup, and polling attempts.
- **Artifacts.** Assertion artifacts are written after the run finishes and
  attached to that run's record by number. Concurrent failures never share or
  swap artifacts.
- **Skipped cases.** A case that never starts, such as one behind a failed
  shared fixture, has exactly one `run_skipped` outcome and no start.

## Interruption

The first interrupt stops the invocation cooperatively. On Linux and macOS that
is SIGINT (Ctrl-C) or SIGTERM; on Windows, Ctrl-C or Ctrl-Break:

- No new run, case, or shared fixture starts.
- Every started run and fixture is cancelled through the invocation's root
  control. Each run ends with one `cancelled` outcome (BW5001); `Finally`
  cleanup and `SuiteTeardown` of fixtures already set up still run under their
  own [cleanup limits](cleanup.md).
- The console prints
  `[interrupted] stopping runs; interrupt again to exit at once`. Once the started
  work drains, the normal summary follows, then a closing line:
  `Interrupted: 3 of 4 selected runs have outcomes; the others never started`.
- The reports are published with `verdict.delivery: "interrupted"`. The verdict
  status is `interrupted` and the exit status is 1. The listener's
  `stream_finished` carries the same status.
- A report is `complete` only when every selected run has a record, so runs that
  never started leave it `complete: false`.
- A `--failures` record stays incomplete, so no rerun selection comes from
  partial results.

A second interrupt exits at once with status 130 and leaves the reports as
incomplete markers with their journal for reconciliation. On Linux and macOS it
first ends the process group of every process and JavaScript worker the runs
started, so none outlives the invocation; on Windows each worker's Job Object
ends it as the CLI exits.

Every platform behaves the same way ([D12](decisions.md#d12-platform-parity)).
On Windows, other console events, such as closing the console window or logging
off, keep their default and end the process at once; reconcile its reports as
after any forced termination. `tests/terminal_outcomes.rs` covers Linux and
macOS, and `tests/console_interrupts.rs` sends Ctrl-Break on Windows.

## Reporter failures

When the console report cannot be written, admission stops and started runs
drain, as before. Each started run still gets its one record and its listener
events.

The reports are then published with `verdict.delivery: "failed"`, instead of
staying incomplete markers. They are `complete: false` when some selected runs
never started, and the original reporting error decides the exit status.

A `--failures` record that cannot be written is also a delivery failure. The
reports are still published, with `delivery: "failed"`.

## Forced termination and reconciliation

Whenever `--report-json` or `--report-html` is given, Botwork keeps a journal
beside the first report: `report.json.journal`, or `report.html.journal` for an
HTML-only invocation. It is an append-only JSON Lines file (mode `0600`), and
each line is written with a single write as the event happens:

| `entry` | Written when |
| --- | --- |
| `header` | The report starts: journal format and version, mode, start time, and report paths |
| `selected` | Discovery knows how many runs are selected |
| `started` | A run starts: its number, identity, and start time |
| `run` / `summary` | A run's record is kept (full, or a summary beyond the detail budget) |
| `fixture` | A shared suite fixture finishes |

The journal is removed once the final report is published. An invocation that
stops before any run started removes it too.

The journal survives an operating-system crash as well as a killed process. Its
header is synced to disk as it is created, with, on Unix, its name in its
directory, and each `started` line is synced before its run goes on, so every
run that began is still known to have. Other lines are not synced: a crash can
lose a run's record, and reconciliation then reports that run as interrupted,
never as passing. A journal that cannot be synced is a write failure, below.

If the process is killed or crashes, the reports stay incomplete markers and the
journal remains. Botwork then refuses to start another invocation with the same
report, so the evidence is never overwritten, and suggests reconciliation:

```text
botwork --reconcile-report report.json
```

Pass the report the journal sits beside: the JSON report when both reports were
written.

Reconciliation proceeds in these steps:

1. It locks the reports named in the journal header. The locks prove the
   original invocation has ended; while it still runs, reconciliation fails and
   changes nothing.
2. It folds the journal. Finished runs keep their records. A run that started
   but has no record becomes an `interrupted` record, with its identity and start
   time and `complete: false`.
3. It publishes the reports with an `interrupted` verdict and
   `delivery: "interrupted"`, then removes the journal:
   - `finished_at` is the reconciliation time and `duration_us` is `null`.
   - `complete` is true only when every selected run has a record.
4. It prints a summary, such as
   `Reconciled report.json: 2 of 2 selected runs have records, 1 of them interrupted`,
   and exits 1. An interrupted verdict never passes.

A termination can cut the journal's final line short. That torn line has no
closing newline, so reconciliation ignores it and reports its size. A malformed
complete line, a journal without its header, or a journal over 256 MiB is
refused, and the reports and journal are left unchanged.

Write failures never stop runs, so a journal that cannot be written loses the
forced-termination guarantee. The final report is still published, but with
`delivery: "failed"`, and the error is printed with exit status 1.

## Stored records

Reconciliation reads records back, so run records now deserialize as well as
serialize (see [run records](run-records.md#serialization-and-compatibility)).
Loading validates:

- the record format and version;
- every diagnostic code and statement kind;
- the status vocabulary.

Fields added later within a version are ignored.

## Verification

`cargo test --locked --test terminal_outcomes` covers:

- an interrupted batch, where admission stops, started runs are cancelled once,
  and every output agrees;
- an interrupted suite, where ready fixtures are torn down and the failed-case
  record stays incomplete;
- a second interrupt, which exits at once and is followed by reconciliation of
  both reports;
- a forced termination, which blocks new reports until reconciled, and
  reconciliation, which waits for the invocation to end;
- a console that disappears mid-run while every started run keeps one record;
- sixteen concurrent failures, each with exactly one outcome and its own
  artifact.

Unit tests cover:

- stopped deliveries;
- reconciliation;
- torn and corrupt journals;
- blocked reports;
- journals without started runs;
- stored-record round trips;
- statement kind names.

[Validation evidence](terminal-outcomes-evidence.json) records the measured
profiles and mutations.
