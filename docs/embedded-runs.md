# Embedded Rust Runs

`core::run::Engine` keeps reusable native registrations. `run_source(name, text, options)`, `run_program(&program, options)`, and `run_file(path, options)` synchronously execute in fresh contexts. Each run owns variables, custom definitions, namespace/module caches, handler state, and counters. Programs are immutable and reusable. Engine clones share native callback captures; hosts remain responsible for intentional shared state and callback synchronization.

The executed [Rust example](interpreter-architecture.md#embedded-runs) demonstrates inputs, environment overlays, native registration, and structured results. The CLI still uses its existing Context path; Engine defaults do not yet impose budgets on CLI/legacy execution. Async-operation dispatch remains separate roadmap work.

## Configuration and Environment

`RunOptions` contains root `variables`, optional `working_directory`, `inherit_environment` (default true), an `environment` overlay, `OperationControl`, optional relative `timeout`, and `RunLimits`.

Resolve/canonicalize the run directory once; it must exist and be a directory. A relative configured directory resolves against the current host directory, while None captures that directory. Relative entry-file paths resolve against the run directory. Relative source names supplied to `run_source`/`run_program` anchor imports there; nested imports still resolve against their defining source file. Entry/module source paths must be UTF-8 when read through these APIs.

Snapshot the host environment at run entry unless inheritance is disabled, then apply overlay entries: `Some(value)` replaces/adds a name; `None` removes it. Keys/values are owned OS strings with exact-key matching. Reject empty names, `=`/NUL in names, and NUL in values. The snapshot does not perform platform-specific case folding. Future process adapters must define their platform's environment normalization explicitly.

Neither directory nor environment configuration mutates process-global state. Native callbacks registered on Engine receive `&RunEnvironment`, exposing the directory, environment map/lookups, and child control. Callbacks must explicitly use this configuration; arbitrary Rust code reading process globals does not automatically see overlays. Imported module initialization/calls receive the same run environment.

## Initial Budgets

| RunLimits field | Default | Counting rule |
| --- | --- | --- |
| `source_bytes` | 1 MiB | Per entry/module source, UTF-8 bytes; reject before parsing |
| `steps` | 1,000,000 | One per visited statement, expression node, and For iteration |
| `call_depth` | 32 | Entered native/custom calls; imported wrappers do not add another level |

Zero is permitted: an empty run needs no steps/calls, and nonempty source exceeds a zero byte budget. Signed integer negation counts its operand even though conversion handles sign/magnitude together. Short-circuited operands consume no steps. Loops revisit their condition/body expressions; empty For bodies still charge iterations. Initializations and calls across modules share the run's counter. Call depth is checked after signature/arity resolution and before argument effects.

File reads retain at most `source_bytes + 1` bytes before reporting BW8001, including when the cut splits UTF-8. `run_program` checks its retained source size before validation; previously assembled ASTs are host-owned input. Step/recursion/source failures latch for the run and cannot be caught to resume work. Higher budgets are an explicit host policy choice. Parser/AST depth, import depth/count, aggregate source/value/collection memory, and stricter preallocation bounds remain the next resource task. Current budgets do not make untrusted source safe to execute.

## Stop and Completion Rules

Each run gets a child control: parent cancellation and earlier deadlines propagate; cancelling a run's child does not cancel the parent or siblings. The optional timeout starts at run entry and uses OperationControl's monotonic clock (including Tokio's controlled clock when hosted there). An already stopped run rejects entry, including empty/invalid source. No Tokio runtime is needed for synchronous execution.

Check stop requests before execution/expressions/calls/iterations, around module file reads, and after native callbacks. A run stop bypasses Catch and unwinds temporary iterator/handler bindings and invocation frames. Completed assignments/effects remain. No rejected return value is published. If a callback fails while stopping, preserve its error as a cause with call context. An ordinary callback-reported error remains catchable when the actual run control/budget is still active.

Synchronous callbacks, parsing, filesystem calls, and value operators cannot be preempted inside Rust. Deadlines are observed at checkpoints; a callback must cooperate with `environment.control().checkpoint()` or return. Check again after return before publishing a result. Hard termination, async DSL dispatch, and guaranteed cleanup deadlines require the separate runtime/worker tasks.

## Structured Results

Every invocation returns `RunResult`: a detailed `result`, owned root `variables` snapshot, consumed `steps`, and host-monotonic `elapsed` duration. `outcome()` classifies success, ordinary failure, cancellation, timeout, or resource-limit failure by the terminal diagnostic code. Invocation locals never escape; failure snapshots retain completed root assignments only. Invalid configuration/source/input produces a failure result before script effects. Detailed sources/stacks/causes remain owned after Engine/program destruction.

Log still writes stdout. These results are the execution API contract; suite identities, events, reports, artifact handling, log capture, and serialization/versioning remain the reporting milestones.
