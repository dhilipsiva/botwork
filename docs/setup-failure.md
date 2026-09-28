# Setup failures and skipped cases

These rules define the current fixture outcomes before assertions and shared
report formats build on them. They apply to ordinary cases and expanded dataset
rows, identified by their stable qualified IDs. The [fixture lifecycle](fixtures.md)
defines ownership, limits, and teardown entry.

## Admission determines the case outcome

A case is **skipped** when it was selected but never admitted because its suite
could not finish preparation/setup or because the suite control stopped before
case admission. It runs no Library, CaseSetup, body, or CaseTeardown. The console
emits one skipped record with its ID and reason, with no started/succeeded/failed
record for that case. This guarantee requires successful report delivery.

A started record means the case was admitted to preparation. If preparation,
Library, or CaseSetup fails, that case **fails** (or is cancelled, timed out, or
limit-exceeded under the existing diagnostic categories). Its body never runs.
CaseTeardown is attempted only if the case reached ownership entry before the
failure. Failure during input admission or complete-program validation does not
arm teardown. Failing CaseSetup is not a skipped case, even though its body did
not run. A case failure leaves selected siblings runnable.

Suite preparation/setup failure blocks only that suite's selected cases. Other
selected suites continue under the global job bound. An entered suite still
attempts SuiteTeardown after partial setup failure. It cannot turn setup failure
into success or admit previously skipped cases. Repair and rerun are separate
executions with fresh owners.

Filtered-out cases are unselected, have no lifecycle records, and are absent
from the selected/skipped totals and failure selection. Excluding everything is
a selection error before effects. A completed empty failed-case selection is
the existing successful no-op exception. Listing never enters fixtures. There
is currently no explicit runtime Skip statement or user skip declaration.

Reporter failure stops admission and drains entered owners; undelivered outcomes
and unfinished work are not inferred to be skipped or successful. Requested
failure history remains incomplete. Forced termination likewise cannot establish
a completed skip/pass from partial output.

## Preserve failures across cleanup

Within one owner, the first unhandled setup/body failure stays primary when
teardown also fails. Append the teardown diagnostic as a cause, retaining both
categories, source locations, and available call frames. A successful teardown,
including one that handles its own error with Catch, cannot consume the earlier
failure. Handling an error *inside setup* before setup completes can let the
body run normally. A teardown failure after successful setup/body fails that
owner. Return from a helper is ordinary completion; it cannot dismiss an earlier
unhandled failure. Escaping control from a teardown block is invalid before effects.

Case and suite owners have independent results. For example, a case setup error
with a case cleanup error is reported on that case; a later SuiteTeardown error
is a separate suite fixture diagnostic. Both remain visible. A failed shared
teardown does not rewrite successful case records as failures, but the overall
run fails. Embedders must inspect both `FixtureResult::result` and its independent
`body` result(s), as shown in the fixture API documentation.

Cancellation/deadline priority remains the runtime's existing rule: a later
parent stop can become the outer diagnostic, with the earlier primary and its
cleanup cause retained beneath it. Independent cleanup control allows teardown
to run after that stop; successful cleanup does not clear it. When diagnostic
quotas prevent retaining full evidence, preserve the failure category and emit
an explicit omission summary. Bounded rendering likewise identifies truncation;
missing details never imply that the failure disappeared.

## Totals, exit status, and reruns

For a completed report, selected cases equal succeeded plus failed plus skipped.
The failed count includes admitted cases that are cancelled, time out, or exceed
limits. Failed shared fixtures are counted separately; they can coexist with
successful case records. Any case failure, blocked/skipped case, failed fixture,
reporting failure, or requested history-publication failure makes the CLI exit
unsuccessfully. Successful cleanup cannot change that decision.

The version 2 failed-case file is a **rerun selection**, not an outcome report.
Its `failed` IDs include failed cases, cases skipped by a failed suite, and all
selected cases affected by a shared fixture failure—even cases whose bodies
passed. IDs remain unique and in discovery order. Excluded cases and unaffected
successful suites are absent. Records become complete only after all admitted
owners, outcomes, and summary delivery finish. Use both `--rerun-failed PATH`
and `--failures PATH` to refresh the same file after repair.

## An intentionally failing example

This example uses a string as a stand-in for a partially acquired resource.
Setup fails on `missing`; teardown logs its release and then fails independently.
Both cases are skipped and neither case hook runs.

<!-- botwork-test: setup-failure-suite -->
```botwork-suite
Suite |"environment"| {
    SuiteSetup {
        |session| = |"demo"|
        Log |"open " + session|
        |ready| = |missing|
    }
    SuiteTeardown {
        Log |"close " + session|
        CleanupFailed
    }
    CaseSetup { Log |"case setup must not run"| }
    CaseTeardown { Log |"case cleanup must not run"| }
    Case |"first"| { Log |"body must not run"| }
    Case |"second"| { Log |"body must not run"| }
}
```

Stdout contains `open demo` then `close demo`. Stderr preserves `BW2001` for the
undefined setup variable, followed by the `BW2002` cleanup cause, two skipped
case records, and this summary:

```text
[cases] 2 selected: 0 succeeded, 0 failed, 2 skipped; 1 suite fixtures failed
```

Run [example 27](../examples/27-setup-failure.suite.botwork) with
`cargo run -- --suite examples/27-setup-failure.suite.botwork --jobs 1`.
It intentionally exits with status 1. The example, documentation output, T4 corpus
cases, ownership matrix, and CLI selection/rerun tests are executable checks of
this policy. [Validation evidence](setup-failure-evidence.json) records the checks
and targeted failure mutations. Built-in assertions, expected failures, collected
failures, and JSON/HTML report delivery remain separate tasks; they must preserve
these owner and admission distinctions.
