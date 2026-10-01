# Shutdown bounds

A stopped run returns within a known bound, even when work it started cannot
stop. Stops come from cancellation, deadlines, and CLI interrupts; see
[cancellation](cancellation.md). This page states the bound, what happens to
work that outlives it, and the tests that prove it.

## The stop grace

Most work ends at the stop itself:

- statements, loops, and calls stop at their next checkpoint;
- sleeps, asynchronous operations, and HTTP requests race the stop;
- process statements kill and reap their child within the cleanup allowance.

Started blocking work cannot be interrupted: neither Rust nor Tokio can stop a
running system call or callback. That work includes:

- file and environment operations;
- `Log` and `--debug` output;
- blocking native callbacks (`NativeOperation::blocking`);
- CLI preparation, which loads the entry file, variable files, and inputs.

After a stop, such work gets the control's **stop grace** to return. It defaults
to 2 seconds (`DEFAULT_STOP_GRACE`). Work still running when the grace ends is
**abandoned**:

- The stop is returned at once, with a BW5003 cause: "A started blocking
  operation did not stop within N ms of the stop and was abandoned".
- The worker thread stays blocked until its call returns. Until then it keeps its
  pool permit and the values it owns.
- Its result, success or failure, is discarded. An effect it completes later
  still happens, after the run has returned.

A job that finishes within the grace is handled as before: its error, if any,
becomes a cause of the stop.

## The bound

After a stop, a run returns within:

- the stop grace, for the blocking job the run was waiting on; plus
- for each `Finally` that runs after the stop, its cleanup timeout and one more
  stop grace.

Cleanup runs under its own control and deadline (see [cleanup](cleanup.md)), which
keeps the run's stop grace. A cleanup nested inside cleanup shares its enclosing
allowance, so nesting cannot extend the bound. Each allowance also includes
scheduling delays, which are small but not zero.

## Setting the grace

- **Hosts.** Every control created with `child` inherits its parent's grace, and
  so do clones. Give a run its grace through `RunOptions::control`, as below.
- **The CLI.** `--stop-grace-ms` (default 2000, at most 3,600,000) sets the grace
  of the root control. Every run, case, suite fixture, and cleanup descends from
  that control, including suite cases without fixtures.
- **Zero.** A grace of 0 abandons started work at once unless it has already
  finished.

```rust
use botwork::core::{operation::OperationControl, run::RunOptions};
use std::time::Duration;

let options = RunOptions {
    control: OperationControl::default().with_stop_grace(Duration::from_millis(500)),
    timeout: Some(Duration::from_secs(30)),
    ..RunOptions::default()
};
assert_eq!(options.control.stop_grace(), Duration::from_millis(500));
assert_eq!(options.control.child(None).stop_grace(), Duration::from_millis(500));
```

## The CLI process

- **Exit.** The CLI shuts its runtime down without waiting for abandoned jobs,
  so they cannot hold the process after it finishes. The operating system
  reclaims their threads at exit.
- **Interrupts.** One interrupt stops every run within these bounds. A second
  interrupt exits at once; see [terminal outcomes](terminal-outcomes.md#interruption).
- **Listeners.** A listener has its own close timeout; see [listeners](listeners.md).

A stopped single-file CLI run therefore exits within its timeout or interrupt,
the bound above, and the listener's close timeout when one is attached.

## Limits

- **Synchronous runs.** `Engine::run_source` and the other synchronous entry
  points run blocking work on the calling thread. There is no second thread to
  abandon it, so no bound applies. Use the asynchronous entry points for a bound.
- **Runtime shutdown.** Dropping a Tokio runtime waits for its blocking threads,
  including abandoned jobs. A host that must not wait uses
  `Runtime::shutdown_background` or `Runtime::shutdown_timeout`, as the CLI does.
- **No time driver.** Without Tokio's time driver the grace cannot be measured,
  so started work is awaited until it returns.
- **Capacity.** Abandoned jobs keep their permits until they return. Each pool
  admits 32 jobs (blocking operations admit their own `max_in_flight`), so
  repeated abandonment can exhaust a pool. Later jobs then wait for capacity and
  end at their own stop without starting.
- **The CLI's own output.** Console summaries, reports, and final errors are the
  invocation's result, so they are never abandoned. A console that never reads
  holds the invocation after its runs stop; a second interrupt exits at once, and
  [reconciliation](terminal-outcomes.md#forced-termination-and-reconciliation)
  finishes its reports.
- **Hard deadlines.** Abandoning does not free what the blocked call holds. Use a
  process statement or an [isolated worker](isolated-workers.md) when a deadline
  must also release the resource.

## Verification

`cargo test --locked --test shutdown` covers, on Linux and macOS:

- **Blocked output.** A CLI run writing to a pipe that is never read exits after
  a deadline, and after a single interrupt, with the abandoned cause.
- **Blocked reads and preparation.** A `Read File` of a FIFO nobody opens, and
  preparation blocked on a FIFO entry file or variable file, end within the
  timeout plus the grace. Preparation is covered for file runs, suite cases
  without fixtures, and suite fixture owners.
- **Interrupts.** An interrupt reaches a suite case that has no fixture.
- **Cleanup.** A failing `Finally` after a deadline keeps the deadline as the
  outcome. A `Finally` blocked on a FIFO ends at its cleanup timeout plus the
  grace.
- **Engine runs.** A blocked read and its blocked cleanup are each abandoned under
  the run's grace. A blocking operation that ignores its control is abandoned,
  keeps its only permit until it returns, and then serves the next run.

Unit tests in `src/core/operation/tests.rs` cover the default grace, its
inheritance by children and clones, the abandonment cause, zero grace, and
runtimes without a time driver.

[Validation evidence](shutdown-evidence.json) records the measured profiles,
mutations, and sensitivity probes.
