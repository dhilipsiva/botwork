# Owned cleanup with Finally

`Try { … } Finally { … }` owns a cleanup block for one entry into the Try.
The block runs once when that entry finishes, including after a runtime error,
a handled error, `Return`, `Break`, `Continue`, cooperative cancellation, a
deadline, or an evaluation limit. An optional `Catch` runs before Finally:
`Try { … } Catch |error| { … } Finally { … }`.

`Finally` is now a reserved whole statement keyword, with the same
case-insensitive delimiter rules as `Catch`. Existing custom names that begin
with this whole word must be renamed; `Finally!` and `FinallyDone` remain valid
custom names. Variable names are unaffected.

Cleanup is available in ordinary scripts, custom statements, imported modules,
and suite Case bodies. [Suite and case fixtures](fixtures.md) use the same
awaited cleanup semantics with explicit shared and per-case owners.

## Ownership and order

Enter the Try before acquiring its resources. Initialize enough state to clean
up partial acquisition, and release only resources that this owner acquired.
Nested owners finish from innermost to outermost. Finally uses the same local
frame as its Try; resources and flags remain available until cleanup finishes.
It does not introduce a variable scope or automatically close arbitrary literal
values. Cleanup assignments persist in that frame. Called custom statements
retain the existing lexical lookup rules.

This small example uses named stand-ins for host resource operations:

<!-- botwork-test: cleanup-example -->
```botwork
Open |name| { Log |"open " + name|
    Return |name|
}
Close |resource| { Log |"close " + resource| }
Work {
    |opened| = |false|
    Try {
        |resource| = Open |"session"|
        |opened| = |true|
        Return |42|
    } Finally {
        If |opened| { Close |resource| }
    }
}
Log |@{ Work }|
```

The output is `open session`, `close session`, then `42`. The returned value is
evaluated before cleanup and cannot be changed by later assignments. See
[example 25](../examples/25-cleanup.botwork) for nested owners and recovery.

Host acquisition callbacks must own partially acquired resources until they
successfully transfer them, and undo acquisition if cancellation, return-value
validation, or handoff fails. A DSL flag cannot acknowledge an interrupted
native handoff atomically. Finally does not roll back external effects or make
an arbitrary acquisition API exception-safe.

## Completion and failure

| Protected execution | Cleanup | Result |
| --- | --- | --- |
| Normal completion, return, or loop control | Success | Original completion |
| Body failure recovered by Catch | Success | Handler completion |
| Body or handler failure | Success | Original failure |
| Success, return, or loop control | Failure | Cleanup failure |
| Body or handler failure | Failure | Original failure, with cleanup failure appended as a cause |

A Finally block cannot return from an enclosing custom statement, break or
continue an enclosing loop, or rethrow an enclosing Catch's error. Such placement
is rejected before any script effects, even in unreachable code. Loops,
definitions, and Catch blocks *inside* cleanup establish their own normal
control scopes. Cleanup never consumes the original failure. An explicit outer
Catch may handle ordinary failures; it cannot clear the original run's latched
stop. A stop observed on the original run while cleanup is executing still has
the existing cancellation/deadline priority when execution resumes.

An error in cleanup stops the remainder of that cleanup block. Already entered
outer owners still attempt their cleanup. Protect independent releases with
nested Try/Finally blocks when each must be attempted even if another fails.
Returned diagnostic causes keep their source locations and order. If combined
metadata exceeds diagnostic quotas, a bounded summary keeps the original
category and explicitly reports omitted cleanup evidence.

Discovery, syntax validation, and pre-entry failure do not arm cleanup. No
resources should be acquired by those phases. Catch bindings are restored before
the associated Finally runs.

## Independent cooperative allowance

Each owner entered during ordinary execution receives a fresh cleanup allowance
when it unwinds: **10,000 evaluation steps and five seconds** by default. It is
independent of the original run's cancellation token, deadline, and latched
failure. Thus a stopped body can still release resources. Both synchronous and
asynchronous entry points use this policy.

Owners entered *during cleanup* share that cleanup's allowance, including its
stop latch and deadline. A cleanup loop cannot keep extending its budget by
entering nested Try/Finally statements. Previously entered outer owners receive
their own allowance even if an inner cleanup exhausts its allowance. A cleanup
allowance failure is a cleanup error; an outer Catch can handle it when the
original run itself has not stopped.

Live value, temporary, definition, name, registry, and diagnostic quotas remain
shared, as do cumulative output, import, and snapshot charges. Cleanup does not
create free memory or output capacity. It retains the current call/evaluation
depth, so it cannot evade stack limits. Leave enough quota headroom for release
operations; exhausted shared quotas can make a cleanup attempt fail. Run step
statistics include cleanup work, so they can exceed the ordinary step limit.
With finite ordinary steps, the number of independent owners is also finite;
the time allowance is per owner, not a total run shutdown bound.

CLI flags `--max-cleanup-steps` and `--cleanup-timeout-ms` adjust the allowance.
Zero is permitted and stops nonempty cleanup before its first statement.
Embedding applications set `RunLimits::cleanup`:

```rust
use botwork::core::run::{CleanupLimits, Engine, RunLimits, RunOptions, RunOutcome};
use std::time::Duration;

let options = RunOptions {
    limits: RunLimits {
        steps: 10,
        cleanup: CleanupLimits { steps: 100, timeout: Duration::from_secs(1) },
        ..RunLimits::default()
    },
    ..RunOptions::default()
};
let run = Engine::default().run_source("cleanup", r#"
Try { While |true| {} } Finally { |released| = |true| }
"#, options);
assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
assert_eq!(run.variables["released"].to_string(), "true");
assert!(run.steps > 10);
```

## Cancellation and forced termination

To obtain awaited cleanup, request cooperative cancellation and continue polling
the run future to completion. Started blocking callbacks drain before Finally
begins. Async cleanup operations receive the cleanup control and are awaited.
Cancellation does not clear completed effects or turn an interrupted body into
success.

Dropping/aborting a future, dropping its runtime, terminating the host, or an
aborting panic cannot execute asynchronous DSL cleanup. Unwinding native callback
panics become ordinary diagnostics and do run cleanup; a process abort cannot.
An uncooperative native callback, stalled OS call, or unscheduled executor can
exceed these cooperative deadlines. Use the documented isolated-worker ownership
and supervision boundary for host operations requiring stronger termination.
Even process termination cannot guarantee rollback of external resources.

## Validation

`cargo test --locked --test cleanup` exercises completion paths, nested ownership,
native failure and panic, return preservation, control validation, independent
cancellation/deadlines, blocking drain, dropped futures, quotas, imported cleanup,
and CLI diagnostics. Budget unit tests cover shared accounting, clone isolation,
and zero/overflow limits. F10 conformance cases and the executed examples cover
language integration.

The [validation record](cleanup-evidence.json) records source hashes, commands,
profile results, and mutation outcomes. The mutation campaign is focused on
cleanup and its accounting; it does not replace the earlier full campaign.
Its initial baseline exposed a stack-frame regression, repaired by boxing the
cleanup future and verified by the existing and new depth tests.
