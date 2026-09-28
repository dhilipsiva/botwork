# Suite and case fixtures

A suite can declare `SuiteSetup`, `SuiteTeardown`, `CaseSetup`, and
`CaseTeardown` blocks after its optional Library and before its cases. Each is
optional and can occur once, in any declaration order. Declaration words are
case-insensitive and remain available as ordinary custom names in scripts.
Discovery validates every block, including unselected suites, before effects.

<!-- botwork-test: fixture-suite -->
```botwork-suite
Suite |"sessions"| {
    SuiteSetup {
        |shared| = |42|
        Log |"suite opened"|
    }
    SuiteTeardown { Log |"suite closed"| }
    CaseSetup { |local| = |shared + 1| }
    CaseTeardown { Log |"case closed"| }
    Case |"first"| { Log |local| }
    Case |"second"| { Log |shared| }
}
```

With `--jobs 1`, this prints `suite opened`, `43`, `case closed`, `42`,
`case closed`, and `suite closed`. Run the expanded example with
`cargo run -- --suite examples/26-fixtures.suite.botwork --jobs 1`.

## Owners and inputs

A selected suite with either suite hook gets one retained context. Its Library
initializes once, followed by SuiteSetup. Successful setup exports an immutable
snapshot of root variables. Each selected case or dataset row receives an
independent copy, installs its own Library, runs CaseSetup, then its body, and
awaits CaseTeardown. SuiteTeardown runs in the original suite context only after
all admitted cases have finished their own cleanup. Suite variables, definitions,
and module state remain available to that teardown. Case mutations stay local.
SuiteSetup definitions and import aliases are not exported to cases: place
shared definitions/imports in Library. Library modules initialize once per owner.

Common CLI inputs load once for a suite owner; its snapshot supplies cases.
Without suite hooks, CLI inputs continue loading for each admitted case.
Dataset row bindings override matching snapshot variables before value copying.
CaseSetup can change its case's inputs. No hook introduces an extra lexical
scope. Top-level Return is invalid, as it is in Case and script bodies; custom
helpers can return normally, including through nested Try/Finally.

These owners sequence resource operations; they do not automatically close
literal values. Shared handles must support concurrent borrowers when jobs
exceed one. Initialize acquisition flags before acquiring resources, and release
only what that owner acquired. Native acquisition callbacks must undo partial
acquisition and failed handoff themselves. The [Finally contract](cleanup.md)
explains this boundary and protecting multiple independent releases.

## Completion, failure, and limits

Teardown is armed before Library/setup execution. After entry, it is attempted
on success, assertion callback failure, handled or unhandled errors, native panic
converted to a diagnostic, evaluation limits, cooperative cancellation, and
deadlines. Discovery, program validation, input admission, and other failures
before ownership entry do not run hooks. Setup failure stops the rest of setup.

| Failure | Remaining work | Report and rerun |
| --- | --- | --- |
| Case Library/CaseSetup | Skip body; attempt CaseTeardown | Case fails; siblings continue |
| Case body | Attempt CaseTeardown | Preserve case failure |
| CaseTeardown after successful body | Finish case | Case fails |
| Suite Library/SuiteSetup or snapshot export | Attempt SuiteTeardown; no cases enter | Pending cases skipped; all selected IDs saved as affected |
| Suite control stops | Drain admitted cases; skip queued cases; attempt SuiteTeardown | Suite fails; all selected IDs affected |
| SuiteTeardown | Finish owner | Suite fails, including when all case bodies passed; all selected IDs affected |

When setup/body and teardown both fail, keep the primary error and attach the
cleanup error as a cause, with original source locations and bounded diagnostic
omissions. A later parent cancellation/deadline retains the existing stop
priority. Successful cleanup cannot clear a failure. Suite fixture diagnostics
are separate from case outcomes. The summary includes skipped-case and failed
suite-fixture counts when either is nonzero; process status is unsuccessful if
any case, fixture, reporting, or failure-record publication fails. This is the
current console/failure-selection policy; collected assertions, expected failures,
and the common JSON/HTML report model remain separate work.

`--timeout-ms` covers each admitted case's preparation, setup, and body.
`--suite-timeout-ms` optionally covers a suite owner's setup and all borrowers,
starting at owner admission. Cases inherit that deadline as well as their own.
The suite flag requires a selected suite with a suite hook (an empty completed
failed-case rerun remains a successful no-op) and cannot be used while listing.
Each entered case and suite owner gets an independent cleanup allowance through
`--cleanup-timeout-ms` and `--max-cleanup-steps`. Nested Finally owners entered
*during* cleanup share that allowance. Cleanup retains the owner's other
cumulative quotas; exhausting snapshot/output/import capacity can also prevent
a release that needs that capacity. Teardown is attempted, not guaranteed to
complete successfully under arbitrary limits or host failures.

Suite snapshot export checks result-value bounds and charges snapshot-table work
before allocation. Case inheritance admits table work, names, and live values
before copying; rejection publishes no partial bindings. Table work conservatively
counts all snapshot entries, even keys overridden by row inputs. Suite AST
admission covers Library, all four hooks, and all cases together. Case program
admission also counts its generated cleanup wrapper. Suite and case counters are
separate; no idle suite owner's step/output budget is charged for case work.

## Scheduling and interruption

`--jobs` bounds active setup, case preparation/execution/cleanup, and suite
teardown phases together. A suite owner waiting for borrowers retains its state
without consuming a work slot. With one job, suite/case discovery order is
preserved. With more jobs, ready work is admitted in discovery order, allowing
other suites to progress past blocked setup; completion and output can interleave.
Ready teardown takes priority over acquiring another owner. A failed case does
not stop siblings. Reporter failure stops new admission and drains already entered
cases and suite owners before returning; requested failure history stays incomplete.

To cancel through the library, request cancellation and continue polling the
owned future until completion. Started blocking work must drain before teardown.
Dropping/aborting the future, runtime shutdown, SIGTERM/SIGKILL, process abort,
or host loss cannot execute awaited DSL cleanup. The CLI does not yet install
signal handlers. Cooperative deadlines cannot force an uncooperative callback
or stuck OS call to return. [Isolated workers](isolated-workers.md) supply a
separate process boundary; they do not make DSL cleanup or effect rollback durable.

## Embedding

`Suite::program`/`SelectedCase::program` include case hooks but do not execute
suite hooks. Use `evaluate_suite_fixture_async` to retain the suite owner, and
join/drain every borrower inside its body callback before returning. Inherit the
snapshot's control when constructing cases. The callback must not let borrowers
or snapshot clones escape the owner lifetime. `FixtureResult::result` describes
setup, owner control, and teardown; `body` contains independent host/case results
and is absent when setup failed. Hosts must combine both for overall success.

```rust
use botwork::core::{
    eval::{Context, evaluate_program_async, evaluate_suite_fixture_async},
    operation::OperationControl,
    run::RunLimits,
    suite::Suite,
};
# let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
# runtime.block_on(async {
let suite = Suite::parse("embedded", r#"
Suite |"s"| {
    SuiteSetup { |shared| = |42| }
    SuiteTeardown { |shared| = |0| }
    CaseTeardown { |local| = |0| }
    Case |"a"| { |local| = |shared| }
}
"#)?;
let program = suite.program(0).unwrap();
let owner = Context::with_control(RunLimits::default(), OperationControl::default())?;
let outcome = evaluate_suite_fixture_async(&suite, owner, |inputs| async move {
    let mut case = Context::with_control(RunLimits::default(), inputs.control().child(None))?;
    inputs.inherit_into(&mut case)?;
    evaluate_program_async(&program, case).await
}).await;
outcome.result?;
outcome.body.expect("setup succeeded")?;
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
# })?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Rule T3 is covered by ownership, admission, CLI, blocked-work, and reporter-failure
tests, the conformance corpus, and executed documentation/example 26.
[Validation evidence](fixtures-evidence.json) records the profile matrix and
focused mutation checks, including their limits.

The focused mutation campaign caught all 13 targeted changes and 46 of 47
compiled generated changes; 19 additional generated changes did not compile.
The remaining change affects unused numeric metadata in case reports and is
retained in the denominator with its review. Earlier mutation campaigns are not
remeasured by this fixture campaign.
