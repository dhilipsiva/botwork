# Asynchronous DSL execution

`Engine::register_operation` installs a `NativeOperation` in the same registry as
ordinary native and DSL statements. Call it from source with the usual sentence
syntax, including `@{ ... }` inside expressions. The evaluator awaits each call
before continuing; there is no new `await` keyword and no implicit parallelism.

`run_source_async`, `run_program_async`, and `run_file_async` return futures with
the same `RunResult` as their synchronous counterparts. Supply a Tokio runtime
with time enabled. Source, syntax, registry, input, result, and diagnostic budgets
remain in force. Arguments evaluate once in caller order, and later arguments,
collection entries, or statements do not run after an earlier failure.

```rust
use botwork::core::{
    grammar::Literal,
    operation::NativeOperation,
    run::{Engine, RunOptions, RunOutcome},
    signature::{StatementSignature, ValueKind},
};
use std::time::Duration;

let mut engine = Engine::default();
let signature = StatementSignature::native("Later |number|")?
    .parameter("number", ValueKind::Int)?
    .returns(ValueKind::Int);
engine.register_operation(NativeOperation::asynchronous(
    signature,
    |mut arguments, _control| async move {
        tokio::time::sleep(Duration::from_millis(1)).await;
        Ok(arguments.remove(0))
    },
)?)?;
let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_time()
    .build()?;
let report = runtime.block_on(engine.run_source_async(
    "example.botwork",
    "Double |n| { Return |@{ Later |n| } * 2| }\n|answer| = Double |21|",
    RunOptions::default(),
));
assert_eq!(report.outcome(), RunOutcome::Succeeded);
assert!(matches!(report.variables["answer"], Literal::Int(42)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Each future owns a fresh context. Engine clones share registered callback captures
and operation ownership/capacity pools, just as explicit `NativeOperation` clones
do. Root variables, invocation frames, module caches, handlers, environment/cwd
snapshots, and run counters remain local. Operations receive their evaluated
owned arguments and the run's child control; an operation's captured configuration
and an isolated worker's explicit `WorkerCommand` remain host-configured. They do
not automatically inherit the process environment or overwrite their command
configuration with `RunOptions`. Existing `register_native` callbacks can inspect
the immutable `RunEnvironment` and pass configuration through DSL values.

Imported modules inherit visible host operations. Initialization can suspend;
successful initialization is cached once per run, and namespace publication stays
atomic. Failed initialization retains completed external effects and successful
dependencies, publishes no failed namespace, and can be retried. Qualified calls
retain their module globals and original import/call/definition locations.

## Compatibility and ownership

Synchronous APIs still require no Tokio runtime. They use the same interpreter
through a synchronous adapter. If an Engine or Context contains any user-supplied
`NativeOperation` registration, synchronous program entry returns BW5003 before
executing any statement, even when the operation is unused. This makes unsupported
mixed execution explicit. Ordinary `register_native` callbacks remain supported
by both execution modes. Synchronous entry invokes them inline; async entry awaits
a bounded blocking worker. The fixed Sleep built-in is an exception to the registry
check: synchronous programs may leave it unused, but calling it returns BW5003
before its arguments run. [Eventually and Retry](polling.md) likewise return BW5003
in synchronous runs before evaluating their options or block. See
[I/O and callback isolation](nonblocking-io.md).

`eval::evaluate_program_async(&program, context)` consumes a Context for callers
that do not need an Engine result snapshot. The future owns that context; no
partially suspended mutable context can escape when the future is dropped. Use
Engine's async methods to obtain completed root bindings. Context initialization,
input installation, and operation registration remain available before the
handoff. `Context::set_statement_tracing(true)` enables bounded top-level tracing.

The CLI uses this owned-context async entry point on a current-thread Tokio
runtime, after preparing source/input data on a blocking worker. Syntax, output, `--debug`, limits, and exit statuses are unchanged.
Worker-guardian startup is handled before constructing that runtime. CLI statement
listing/help still uses the existing native metadata and does not execute source.
This change does not add process, HTTP, or other I/O statement libraries.

Argument temporary reservations remain live through ordered argument evaluation.
`NativeOperation::invoke` admits its own ownership synchronously before evaluator
argument reservations are released. Pending operations therefore retain their
payload through the operation's ownership pool. Returned values must also pass
the run's local value and temporary budgets before assignment. Operation quota
failures latch BW8001 in the run and bypass handlers; failed assignments preserve
previous destinations. The existing operation signature, finite-value, panic,
diagnostic, blocking-capacity, and worker-protocol checks still apply.

## Cancellation and limits

Run cancellation and the earlier parent/local deadline reach operations through
child controls. Ordinary returns, Break/Continue, Return, handled errors, and
cooperative cancellation restore invocation, iterator, and handler bindings.
Structured errors preserve original codes, spans, related import sites, causes,
and entered call frames in innermost-first order. A stop observed before publishing
an operation result wins over success and retains a distinct earlier error as a
cause. A callback merely returning a cancellation category does not itself cancel
the parent run control.

Concurrent runs do not cancel each other implicitly. Give them separate controls
or sibling child controls for independent stops; clones of one supplied control
share cancellation deliberately. [Batch and host concurrency policies](parallel-cli.md)
describe resource sharing, cleanup accounting, output ordering, and aggregate
status. A host can await one cancelled result while another invocation of the
same registered operation remains active and later succeeds.

Dropping a run future drops suspended DSL state and the pending operation future.
Async resources with synchronous Drop cleanup are released before a cooperative
stop returns. Blocking callbacks receive cancellation but still need to cooperate;
normal stop handling waits up to the [stop grace](shutdown.md) for started
blocking work, then abandons it. Dropping the entire run cannot synchronously
join such a worker. Isolated operations retain their supervisor and
ownership through pending cleanup, using the existing process-containment mode.
Do not detach tasks or rely on an async destructor. [Shutdown bounds](shutdown.md)
state how long a stopped run can take.

The timeout clock starts when an Engine async future is constructed, including time
before its first poll. Unpolled input trees use iterative destruction. During
evaluation, async execution offers the scheduler a yield every 256 counted steps,
then checks cancellation again. This lets other tasks request cancellation of a
CPU-only loop; step budgets and their counting rules do not change.

This is cooperative execution. Async callback factories and polls must return
promptly. [Bounded workers](nonblocking-io.md) now isolate ordinary native
callbacks, Log/debug output, source reads, module resolution, and environment
preparation. Engine/module parsing remains synchronous CPU work. A stopped run waits for
blocking syscalls and callbacks only for the stop grace, then abandons them; the
abandoned call itself has no wall-clock bound. Use an isolated operation when a hard host-operation
deadline is required. Existing [worker containment limits](isolated-workers.md)
and [operation ownership rules](operation-ownership.md) still apply.

## Validation

[Async integration tests](../tests/async_execution.rs) exercise real suspension
through argument/collection/access order, short-circuiting, recursion, loops,
lexical scope, handlers/rethrow/causes, imports/cache retries, and all execution
entry points. Controlled clocks, handshakes, and drop counters check deadlines,
cancellation, concurrent run isolation, blocking-worker draining, abandoned
futures, exact ownership release, and unpolled deep inputs. Linux tests also run
the independent typed Python worker through a DSL call. CLI cases preserve output,
tracing, and pre-effect timeout behavior. Existing synchronous conformance and
resource suites run against the same evaluator.

[Validation evidence](async-execution-evidence.json) records the executed profiles,
mutation campaigns, toolchain, source hashes, and any unresolved observations.
The mutation catalogue moves its existing semantic changes to the shared evaluator
and adds lost async call frames, lost parent control, and omitted scheduler yields.
Historical version 1 evidence remains available in [the mutation guide](mutation-testing.md).
