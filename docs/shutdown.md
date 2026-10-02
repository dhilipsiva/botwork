# Shutdown bounds

A stopped run returns within a known bound, even when work it started cannot
stop. Stops come from cancellation, deadlines, and CLI interrupts; see
[cancellation](cancellation.md). This page states the bound, what happens to
work that outlives it, and the tests that prove it.

## The stop grace

Most work ends at the stop itself:

- statements, loops, and calls stop at their next checkpoint;
- sleeps, asynchronous operations, and HTTP requests race the stop;
- process statements kill and reap their child within the cleanup allowance,
  and so do [JavaScript statements](javascript.md), each a Node process;
- [WebAssembly statements](wasm.md) are interrupted at their next loop
  iteration or function entry.

Started blocking work cannot be interrupted: neither Rust nor Tokio can stop a
running system call or callback. That work includes:

- file and environment operations;
- `Log` and `--debug` output;
- blocking native callbacks (`NativeOperation::blocking`);
- [Python statements](python.md), which also receive `botwork.Stopped` at the
  stop and end at once unless blocked in native code;
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
  stop grace; plus
- its cleanup timeout once more, when a worker process it started is still
  active as it ends (see [run end](#run-end)).

Cleanup runs under its own control and deadline (see [cleanup](cleanup.md)), which
keeps the run's stop grace. A cleanup nested inside cleanup shares its enclosing
allowance, so nesting cannot extend the bound. Each allowance also includes
scheduling delays, which are small but not zero.

## Run end

A run ends only once the worker processes its statements started have ended,
or it says which have not. As it ends, it waits for:

- each [process statement](processes.md)'s worker, from its start until the
  statement sees its cleanup finish, so including one the statement abandoned
  at a stop; and
- every worker of the run's [JavaScript](javascript.md) pool.

It waits at most its cleanup timeout (`CleanupLimits::timeout`,
`--cleanup-timeout-ms`, 5 seconds by default), and only while such a worker is
active, so a run whose workers all ended returns at once. A worker still active
then fails the run with BW5003, which names up to eight of them, each with its
process ID and state: running, stopping, cleanup pending, or ownership lost. A
run that failed already keeps its own failure first, with this one as a cause,
as with a failed cleanup. A worker whose ownership was lost never resolves, so
the run does not wait for it.

Engine runs, `evaluate_program_async`, and `evaluate_program_detailed` wait
like this; so does each CLI run and suite case. A worker still active when its
run returns keeps its capacity until it resolves.

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

- **Exit.** The CLI shuts its runtime down waiting at most 250 ms for its
  threads, so abandoned jobs cannot hold the process after it finishes; the
  operating system reclaims their threads at exit. Threads that end in time are
  joined, so none is still ending as the process exits: a thread's library
  destructors can crash racing the process's own exit-time cleanup, as
  OpenSSL's did on macOS after Python's `asyncio` loaded it.
- **Interrupts.** One interrupt stops every run within these bounds. A second
  interrupt exits at once; see [terminal outcomes](terminal-outcomes.md#interruption).
- **Listeners.** A listener has its own close timeout; see [listeners](listeners.md).

A stopped single-file CLI run therefore exits within its timeout or interrupt,
the bound above, 250 ms for its runtime's threads, and the listener's close
timeout when one is attached.

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

Unit tests cover the run end: in `src/core/worker/supervisor/unix/launch/tests.rs`,
a worker whose cleanup stalls holds a run's wait for its whole allowance and no
longer, and the wait ends as soon as it resolves; a run waits for every worker of
a pool it owns, and not for one its statement saw finish. In
`src/core/run/tests.rs`, a run that ends with a worker still running fails with
BW5003 naming it, or keeps its own failure with that as a cause, and a host's
context evaluated synchronously or not does the same. In `tests/processes.rs`,
a process statement whose cleanup allowance is zero, so that it returns before
its timed-out child is reaped, never lets its run end before that child has
gone; `tests/javascript_adapter.rs` checks the same of a stopped Node call.

[Validation evidence](shutdown-evidence.json) records the measured profiles,
mutations, and sensitivity probes.
