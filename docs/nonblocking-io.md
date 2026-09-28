# I/O and blocking callbacks in async runs

Async Engine runs perform working-directory resolution, environment preparation,
entry-file reads, import canonicalization, and module reads on blocking workers.
Regular filesystem operations therefore leave the async executor available for
other runs. The CLI similarly performs its single input/source preparation job,
including parsing and variable files, on a worker before entering the evaluator.

Ordinary `register_native` callbacks also run on workers during async execution.
This includes built-in Log and top-level debug output. Synchronous Engine and
Context entry points continue to invoke callbacks on the calling thread. Hosts
must account for this thread change when using thread-local data or foreign
runtimes. A foreign runtime that requires a particular thread needs an adapter
with its own explicit scheduling contract; do not assume a stable worker thread.

```rust
use botwork::core::{grammar::Literal, run::{Engine, RunOptions, RunOutcome}};
let caller = std::thread::current().id();
let mut engine = Engine::default();
engine.register_native("Work", move |_, environment| {
    assert_ne!(std::thread::current().id(), caller);
    environment.control().checkpoint().map_err(|error| error.into_error())?;
    std::thread::sleep(std::time::Duration::from_millis(1));
    Ok(Literal::Int(7))
})?;
let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
let result = runtime.block_on(engine.run_source_async(
    "example", "|answer| = Work", RunOptions::default(),
));
assert_eq!(result.outcome(), RunOutcome::Succeeded);
assert!(matches!(result.variables["answer"], Literal::Int(7)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

`NativeOperation::asynchronous` callbacks still implement cooperative futures:
their factories and polls must return promptly. Use `NativeOperation::blocking`
for an explicit per-operation concurrency limit and owned typed arguments, or
isolated workers for the documented host-operation termination guarantees. These
interfaces remain available alongside ordinary native registration.

## Admission and state

Filesystem/environment work shares a process-wide pool of 32 permits. Ordinary
native callbacks and Log/debug output share a separate pool of 32 permits.
Permits cover queued/started blocking jobs and their undelivered results, and
remain owned until delivery or disposal. Waiting for a permit does not occupy a
blocking thread; a cancelled waiter never enters its job. Both pools use the
host's Tokio blocking scheduler. Host scheduler limits and exhausted pool capacity
can delay new work; this does not promise unlimited parallelism or a process-wide
memory ceiling. The CLI has one additional preparation job per invocation.

Each run continues in source order. It awaits a file, callback, Log record, or
debug record before proceeding. Mutable DSL bindings, import caches, and handler
state stay with the run. Workers receive owned arguments and a limited context
containing shared quotas, immutable call records, and the run environment. Copied
call handles count against `SnapshotLimits.entries` before job admission. For a
direct native call this adds one snapshot entry beyond the Engine registry copy;
nested calls copy every entered frame. Immutable registry/environment/source
storage stays shared. No host environment or working directory is changed.

Argument leases, native registration ownership, call/source records, and result
leases remain alive through worker handoff, including when a run is abandoned.
Callback panics remain BW4003; return-kind/value checks still precede assignment.
Resource failures latch BW8001 and skip handlers. Cancellation retains prior root
bindings and distinct completed callback failures as causes. Worker contexts do
not provide a second mutable copy of DSL globals to callbacks.

File reads retain at most the configured maximum plus one probe byte, checking
control before open and between reads. Source-size rejection precedes UTF-8
decoding; import load/source/path accounting and cache publication are unchanged.
Canonicalization and source-read errors acquire their original path, import site,
and call context under the run's diagnostic budgets. Program parsing in Engine
and module execution remains bounded synchronous CPU work.

## Stops and cleanup

Each job gets a child control. Parent cancellation and the earlier run deadline
propagate to it; dropping a waiting run requests child cancellation and aborts
work that has not started. Ordinary native callbacks receive that child through
`RunEnvironment::control()`. A callback registered directly on Context receives
only arguments; use an operation interface when it needs explicit control.
A saved callback control belongs to that job and is cancelled when its handoff
finishes, including after normal completion.

Normal cancellation/timeout handling drains started jobs before returning. A
blocked OS call or uncooperative callback must still return: neither Rust nor
Tokio can forcibly stop a started blocking closure. Dropping the entire future
can return earlier, but the worker retains its arguments, capacity, and cleanup
responsibility. No result or late side effect is reported as successfully
cancelled or rolled back. Blocking-worker runtime shutdown can still wait for
started jobs; whole-run shutdown bounds remain separate roadmap work.

Output still uses the bounded streaming writer and its byte quotas. A destination
can accept a prefix before failing or observing a stop; completed output remains
completed. Stdout/stderr locks may serialize competing writers on workers. There
is no new cross-run output-order guarantee. Low-level `write_value`/`write_output`
helpers remain synchronous for hosts that explicitly call them.

The implementation follows Tokio's guidance to isolate ordinary file operations
on blocking workers. Special files such as FIFOs can remain blocked until a peer
acts, including during shutdown; moving the wait off the executor is not a hard
deadline for such files. See [Tokio filesystem guidance](https://docs.rs/tokio/1.53.1/tokio/fs/index.html)
and [blocking task lifecycle](https://docs.rs/tokio/1.53.1/tokio/task/fn.spawn_blocking.html).

## Validation

Worker unit tests cover capacity, queued cancellation/deadlines, drain ordering,
drop cancellation, result ownership, panics, absent runtimes, and bounded reads.
Async integration tests check regular-file parity, import cache/limit behavior,
real stalled FIFOs, callback thread placement, sibling progress, run-state/cause
preservation, dropped runs, deep values, snapshot admission, and controlled-clock
native deadlines. Child-process tests block stdout and stderr until a sibling run
finishes, then drain the entire output. Existing depth checks protect the boxed
worker boundary from increasing recursive evaluator stack usage.

[Validation evidence](nonblocking-io-evidence.json) records build profiles,
mutation outcomes, and any unresolved observations. This completes the current
core I/O isolation item; future I/O libraries and foreign-runtime adapters must
extend these contracts and tests when they are added.
