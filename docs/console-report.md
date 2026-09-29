# Console report

Batch (`--file` given more than once) and suite (`--suite`) invocations write a
console report to stderr: one progress record per run, a recap of unsuccessful
work, and a final summary. Script output (`Log`) stays on stdout. A single
`--file` run writes no progress or summary; its exit status and any diagnostic
stand alone.

## Progress

Each admitted run writes a `started` record and exactly one terminal record, in
completion order. Diagnostics follow their terminal record:

```text
[run 2] started: "checks.botwork"
[run 2] timed out: "checks.botwork"
checks.botwork:4:1-4:14: [BW5002] Timeout: …
[case users/profile/bob] failed: "profile / bob" [dataset "expected", row "bob"]
[case s/blocked] skipped: "blocked": suite setup did not complete
[suite s] fixture failed:
```

The terminal label is the run's [acceptance status](acceptance-policy.md), taken
from the primary diagnostic code: `succeeded`, `failed`, `cancelled`,
`timed out`, or `limit exceeded`. Skipped cases have no `started` record.

## Failure recap

When any run, case, or shared suite fixture is unsuccessful, a recap precedes the
summary. Failures in a long run scroll away with their full diagnostics, so the
last lines of stderr always explain the exit status:

```text
[failures] 3:
  [run 3] timed out "slow.botwork" (BW5002 at slow.botwork:1:1)
  [case good/fail] failed (BW9001 at mixed.suite.botwork:5:9)
  [suite broken] fixture failed (BW2001 at broken.suite.botwork:2:25)
```

- Entries appear in completion order: the run or case ID, its status, and the
  primary code with its location.
- A failure raised inside a built-in, such as a `Sleep` deadline, is located at
  the innermost script call.
- Entries deliberately omit the colon of progress records, so each progress
  record stays unique.
- Skipped cases are counted in the summary and not repeated in the recap.
- The recap keeps 50 entries and then reports `…and N more`. The count in the
  header covers every unsuccessful entry.

## Summary and exit status

The final line summarizes every terminal status:

```text
[batch] 5 runs: 1 succeeded, 2 failed, 1 timed out, 1 limit exceeded
[cases] 3 selected: 1 succeeded, 1 failed, 1 skipped; 1 suite fixtures failed
```

- The count of `succeeded` and `failed` is always shown.
- `expected failure`, `unexpected pass`, `cancelled`, `timed out`,
  `limit exceeded`, and `interrupted` counts appear only when nonzero.
- Stops are no longer folded into `failed`.
- Suite summaries add `N skipped; M suite fixtures failed` when either count is
  nonzero.

The summary and the exit status come from one `RunVerdict` built from the same
per-status counts. The command exits 0 exactly when every run succeeded or met a
strict expected failure, and every shared fixture succeeded. Everything else,
including skipped work, exits 1, and command-line usage errors exit 2. When
reporting itself fails, admission stops and started work drains. The command
then exits 1 without claiming a complete summary.

## Verification

Console tests cover:

- a clean batch without a recap;
- mixed assertion, timeout, output-limit, and syntax failures, with an exact recap
  and summary, and progress records that stay unique;
- suites with a failed case, a failed shared fixture, and a skipped case;
- recap truncation at 50 entries with exact counts;
- exit status agreeing with the summary across outcome mixes.

Unit tests pin the summary text for every status and the shared exit decision.
Existing batch, suite, dataset, and fixture tests pin the progress records. Run
them with `cargo test --locked --test console_report`.
[Validation evidence](console-report-evidence.json) records the measured
profiles and mutations.
