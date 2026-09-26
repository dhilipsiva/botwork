# Embedded Rust Runs

`core::run::Engine` keeps reusable native registrations. `run_source(name, text, options)`, `run_program(&program, options)`, and `run_file(path, options)` synchronously execute in fresh contexts. Each run owns variables, custom definitions, namespace/module caches, handler state, and counters. Programs are immutable and reusable. Engine clones share native callback captures; hosts remain responsible for intentional shared state and callback synchronization.

The executed [Rust example](interpreter-architecture.md#embedded-runs) demonstrates inputs, environment overlays, native registration, and structured results. Engine, CLI, and low-level Context execution share runtime budget defaults and stop behavior. Async-operation dispatch remains separate roadmap work.

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
| `evaluation_depth` | 96, fixed ceiling | Active statements, expressions, and dispatch frames, including imported wrappers |
| `import_depth` | 16, fixed ceiling | Concurrent uncached module initializations; cached imports do not consume another level |
| `syntax` | Nesting 32, operator units 64; combined ceiling 66 | Local [parser guards](syntax-limits.md), tighten-only fixed syntax ceilings |
| `ast` | 65,536 nodes, depth 128, aggregate source 8 MiB | [Owned syntax admission](ast-limits.md), before control validation and effects |
| `imports` | 128 loads, 8 MiB source, 512 paths, 16,384 bindings, 8 MiB metadata, dependency depth 32 | Cumulative [import admission](import-limits.md), including cached re-exports |
| `values` | 65,536 nodes, depth 64, 1 MiB strings, 64 KiB keys, 16,384 entries, 8 MiB payload | [Per-value admission and owned cleanup](value-limits.md) |
| `retained_values` | 65,536 stored values, 262,144 nodes, 32 MiB payload | [Live variable-value reservations](retained-values.md), released with the final shared owner |
| `retained_definitions` | 16,384 definitions, 262,144 nodes, 8 MiB source text/names | [Live DSL definition reservations](retained-definitions.md), sharing definition and source identities |
| `retained_names` | 65,536 names, 64 KiB per name, 4 MiB aggregate | [Shared variable-name storage](retained-names.md), including scope and snapshot ownership |
| `retained_registry` | 65,536 entries, 262,144 nodes, 64 KiB keys, 8 MiB strings, 8 MiB source | [Statement/namespace metadata](retained-registry.md), including native templates and imported wrappers |
| `snapshots` | 1,048,576 copied entries, 32 MiB path bytes | Cumulative [frame/cache copy admission](snapshot-limits.md), including Engine templates and checked host copies |

Zero is permitted: an empty run needs no steps/calls, and nonempty source exceeds a zero byte budget. Signed integer negation counts its operand even though conversion handles sign/magnitude together. Short-circuited operands consume no steps. Loops revisit their condition/body expressions; empty For bodies still charge iterations. Initializations and calls across modules share the run's counter. Call depth is checked after signature/arity resolution and before argument effects.

File reads retain at most `source_bytes + 1` bytes before reporting BW8001, including when the cut splits UTF-8. `run_program` checks its root source syntax and all reachable tree/source budgets before control validation. Step/recursion/source failures latch for the run and cannot be caught to resume work. Step/call budgets may be raised explicitly; evaluation/import/syntax ceilings may only be tightened. Import parsing additionally rejects entry when more than 16 evaluation frames are active, reserving stack space for the parser. Temporaries, diagnostic ownership, output formatting, and owned result snapshots remain resource tasks. Current budgets do not make untrusted source safe to execute.

Native template storage defaults to the registry budgets above; `Engine::with_registry_limits` configures registration separately. Each run checks template table-copy budgets before admitting custom signatures in normalized order, ahead of inputs/effects. Fixed built-in Log is exempt from registry retention budgets, preserving infallible initialization; its copied table slot still counts toward snapshot work. Infallible host Context/Engine Clone operations remain host-owned; `Context::try_clone()` admits table/path work before copying.

## CLI and Low-Level Contexts

The CLI accepts `--max-steps`, `--max-call-depth`, `--max-evaluation-depth`, and `--timeout-ms`. For example, `cargo run -- --file examples/02-syntaxes.botwork --max-steps 10000 --timeout-ms 2000`. Limits produce BW8001/status 1; unsupported ceilings produce BW7002/status 1; malformed numbers or conflicts with statement-help modes produce status 2. Timeouts include loading/parsing but are observed cooperatively.

`Context::default()` has the same runtime defaults. Use `Context::with_limits(limits)` or `Context::with_control(limits, control)` for local configuration; `checkpoint()` observes stops without charging steps. Work counters persist across evaluations; live-value/definition reservations release with their final runtime owner. Use a fresh context after a latched stop. Clones copy consumed work counters and stop state independently, while sharing live allocation accounting and the supplied cancellation control. Modules share the caller's counters/control. Depth guards release on success, return, and failure.

Context evaluates previously parsed input; use `Program::parse_bounded` to apply custom source/syntax settings to entry parsing. Context source/syntax settings govern imported sources. Engine applies its configured guards to entry sources and reusable programs too. No process-global parser or runtime settings are changed.

## Stop and Completion Rules

Each run gets a child control: parent cancellation and earlier deadlines propagate; cancelling a run's child does not cancel the parent or siblings. The optional timeout starts at run entry and uses OperationControl's monotonic clock (including Tokio's controlled clock when hosted there). An already stopped run rejects entry, including empty/invalid source. No Tokio runtime is needed for synchronous execution.

Check stop requests before execution/expressions/calls/iterations, around module file reads, and after native callbacks. A run stop bypasses Catch and unwinds temporary iterator/handler bindings and invocation frames. Completed assignments/effects remain. No rejected return value is published. If a callback fails while stopping, preserve its error as a cause with call context. An ordinary callback-reported error remains catchable when the actual run control/budget is still active.

Synchronous callbacks, parsing, filesystem calls, and value operators cannot be preempted inside Rust. Deadlines are observed at checkpoints; a callback must cooperate with `environment.control().checkpoint()` or return. Check again after return before publishing a result. Hard termination, async DSL dispatch, and guaranteed cleanup deadlines require the separate runtime/worker tasks.

## Structured Results

Every invocation returns `RunResult`: a detailed `result`, owned root `variables` snapshot, consumed `steps`, and host-monotonic `elapsed` duration. `outcome()` classifies success, ordinary failure, cancellation, timeout, or resource-limit failure by the terminal diagnostic code. Invocation locals never escape; failure snapshots retain completed root assignments only. Invalid configuration/source/input produces a failure result before script effects. Detailed sources/stacks/causes remain owned after Engine/program destruction.

Log still writes stdout. These results are the execution API contract; suite identities, events, reports, artifact handling, log capture, and serialization/versioning remain the reporting milestones.
