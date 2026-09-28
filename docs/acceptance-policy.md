# Assertions and acceptance verdicts

The acceptance model uses **immediate assertion failure** and **strict expected
failure**. `core::acceptance` implements this policy once for console labels,
JSON status projections, HTML status fragments, and execution exit decisions.
It builds on the [setup failure policy](setup-failure.md).

## Immediate failure and explicit recovery

An assertion failure transfers control immediately, like other execution errors.
Statements after it do not run unless an enclosing Catch explicitly handles the
error. Entered owners still await Finally and teardown. Other selected cases
continue under the existing finish-all scheduling policy. There is no implicit
assertion collection, retry, or continue-on-failure mode; `AssertionMode` accepts
only `immediate`. A future collected mode would need its own evidence, limits,
and explicit syntax before it could be supported.

Handling an assertion with Catch consumes that failure. If the case subsequently
completes successfully, an expected-failure annotation produces **unexpected
pass**, not expected failure. An error handled inside teardown cannot consume an
earlier unhandled body failure. A helper Return likewise cannot erase it.

## Strict expected failure

`CaseExpectation::default()` requires success. `CaseExpectation::failure(reason)`
requires a nonblank reason of at most 512 UTF-8 bytes with no control characters.
It applies to one selected case or expanded dataset row, never silently to its
siblings or shared fixtures. The reason belongs with that case's eventual report
record; it is not part of the status fragment.

Only one unhandled **body assertion**, with successful preparation/setup and
teardown, qualifies as expected failure. Assertion identity must come from a
typed adapter classification. Native error text, statement names, tags, or a
matching substring do not establish it. Mixed errors, secondary failures,
omitted diagnostic evidence, panics, and operational errors cannot qualify.
Adapters must classify uncertain evidence as `FailureKind::Other` (or the
observed stop category) and retain the detailed diagnostics separately.

| Observation | Ordinary expectation | Expected failure |
| --- | --- | --- |
| All phases succeed | `succeeded` | `unexpected_pass` |
| Only body assertion fails | `failed` | `expected_failure` |
| Setup assertion fails | `failed` | `failed` |
| Only teardown assertion fails | `failed` | `failed` |
| Body assertion plus any teardown failure | `failed` | `failed` |
| Operational error or mixed/omitted evidence | `failed` | `failed` |
| Primary cancellation, timeout, or resource limit | corresponding stop | corresponding stop |
| Selected case blocked before admission | `skipped` | `skipped` |
| Missing terminal observation | `interrupted` | `interrupted` |

The first unhandled failure within an owner remains primary. A cleanup timeout
after a body assertion therefore fails the case, preserving the assertion as
primary and the timeout as secondary. A later **parent** stop can override the
verdict; `ParentStop` represents that distinct observation. Keep all available
primary and cleanup diagnostics in either situation. The verdict reducer neither
owns nor replaces the diagnostic tree.

`CaseCompletion::executed` rejects a missing body outcome after successful setup
and a body outcome following failed setup. An absent/unarmed teardown is a
successful no-op observation, not proof that cleanup ran. Skipped observations
retain their suite-setup/suite-stop reason. Excluded cases produce no observation.

## Aggregate and delivery policy

`CaseTotals` keeps distinct bounded-memory counts for every case status; its
checked increments reject overflow without changing either counter. Expected
failures are visible separately from succeeded cases. A completed aggregate
succeeds only when all cases succeeded or met strict expectations, every shared
fixture succeeded, and outcome/report/requested-history delivery completed.
Unexpected passes, skipped selected cases, ordinary failures, and stops all fail
the aggregate. A shared teardown failure fails it without rewriting successful
case records. A completed empty failed-case rerun remains a successful no-op;
this does not change the CLI's rejection of an ordinary empty selection.

The caller must account for every selected case before using `Delivery::Complete`.
Missing terminal observations or an interrupted delivery yield `interrupted`,
`complete: false`, exit 1. Failed delivery yields `failed`, `complete: false`,
exit 1. Never infer success or complete failure history from partial output.
An individual terminal failure can still have `complete: true`; completion and
success are separate facts. Final case/fixture outcomes must be drained before
publishing the summary, and requested history must publish before exit success.

Console labels use spaces (`expected failure`); machine names use snake case
(`expected_failure`). `StatusView` serializes the machine name, console label,
failure decision, and completion decision. HTML derives its label, class, and
data attributes from the same closed vocabulary and streams formatter errors
back to its writer. It interpolates no user strings. `RunVerdict` additionally
serializes category counts, fixture failures, and delivery state. All successful
or expected-failure case verdicts exit 0; other case verdicts and unsuccessful
aggregates exit 1. CLI argument usage errors continue to exit 2.

## Executable host example and current boundary

This example classifies a real built-in assertion from a body-only script, then
uses one verdict for the three presentations and aggregate exit decision.

```rust
use botwork::core::acceptance::{
    CaseCompletion, CaseExpectation, CaseStatus, CaseTotals, Delivery,
    FailureKind, PhaseOutcome,
};

let expectation = CaseExpectation::failure("BUG-42: upstream value mismatch")?;
let body = botwork::core::run::Engine::default().run_source(
    "body.botwork", "Assert |1| Equals |2|", Default::default(),
);
let error = body.result.unwrap_err();
let observed = CaseCompletion::executed(
    PhaseOutcome::Succeeded,
    Some(PhaseOutcome::Failed(FailureKind::from_diagnostic(&error))),
    PhaseOutcome::Succeeded,
    None, // no later parent stop
)?;
let status = observed.decide(&expectation);
assert_eq!(status, CaseStatus::ExpectedFailure);
assert_eq!(status.view().label(), "expected failure");
assert_eq!(serde_json::to_value(status.view())?["status"], "expected_failure");
assert!(status.view().html().to_string().contains("data-failed=\"false\""));

let mut cases = CaseTotals::default();
cases.record(status)?;
let verdict = cases.finish(0, Delivery::Complete);
assert_eq!(verdict.cases().count(CaseStatus::ExpectedFailure), 1);
assert_eq!(verdict.exit_code(), 0);
assert!(verdict.complete());

// A successful body under that same expectation is a strict unexpected pass.
let repaired = CaseCompletion::executed(
    PhaseOutcome::Succeeded, Some(PhaseOutcome::Succeeded),
    PhaseOutcome::Succeeded, None,
)?;
assert_eq!(repaired.decide(&expectation), CaseStatus::UnexpectedPass);
assert_eq!(repaired.decide(&expectation).exit_code(), 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

The existing CLI console error labels and completed batch/suite exit decisions
use this policy; embedded `RunResult::outcome` shares diagnostic classification.
Current generic runtime errors do **not** identify assertions and never become
expected failures. The suite grammar has no expected-failure declaration yet.
[Standard assertions](builtins.md) now provide BW9001 and the conservative
`FailureKind::from_diagnostic` classifier. Full JSON/HTML report files, statement events, artifact
links, and expected-failure rerun selection remain the separate reporting work.
These JSON projections and HTML fragments establish verdict consistency, not a
complete report schema or report-delivery CLI option.

The phase matrices, format/exit checks, runtime propagation tests, existing
fixture/CLI regressions, and this executed example verify the policy. See
[validation evidence](acceptance-policy-evidence.json) for the measured scope.
