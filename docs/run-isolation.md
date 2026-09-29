# Run isolation

Every run owns its mutable state. Runs of the same invocation, runs of one
`Engine`, and runs executing at the same time share only what is immutable or
explicitly shared by the host. This page lists both sides and the tests that prove
the boundary.

## What each run owns

| State | Per run |
| --- | --- |
| Variables | Root and local bindings, including inputs; assigning an input changes only this run |
| Custom definitions and handlers | Defined, shadowed, and discarded within the run |
| Module state | Each run initializes every module it imports: its top-level statements run, and its globals and definitions are built from this run's inputs and environment |
| Environment | An immutable snapshot of the host environment, with this run's overlay (`RunOptions::environment`); scripts cannot change it, and the process environment is never modified |
| Working directory | An immutable directory snapshot (`RunOptions::working_directory`, or the CLI's launch directory); relative paths resolve against it, and the process directory is never changed |
| HTTP connections | Each request opens its own connection; no connection pool, cookie jar, or `Set-Cookie` replay crosses requests or runs |
| Budgets and deadlines | Steps, output, retained state, and deadlines start fresh for each run |
| Records and artifacts | Each run's record, listener events, and assertion artifacts carry its own number and identity; see [terminal outcomes](terminal-outcomes.md#ownership-of-events-and-artifacts) |

Browser and device sessions arrive with their integrations. Each will state its
own session ownership and use the same boundary.

## What runs share

- **Host resources.** The filesystem, network, processes, and native operations
  registered on an `Engine`. A native callback's captured state follows its own
  sharing rules. See [concurrency policy](parallel-cli.md).
- **The secret registry.** Within one CLI invocation, a value marked secret by any
  run is masked from every run's output. See [secret inputs](secrets.md).
- **Compiled modules.** Parsed syntax trees of imported modules; the next section
  describes them.

## Compiled modules

Parsing a module is a pure function of these inputs:

- its canonical path;
- its exact text;
- the run's parse limits: source bytes, syntax limits, and AST limits.

A `core::eval::CompiledModules` cache keeps each parsed tree under exactly those
inputs and returns it to later runs that import the same text under the same
limits. The tree is immutable, so sharing it cannot leak state.

- **What stays per run.** Everything built from the tree: module initialization,
  globals, definitions, and the per-context cache of initialized modules.
- **Changed files.** A changed file has different text, so it is parsed again and
  never served stale.
- **Other limits.** A run with other parse limits parses the module itself, so
  tighter limits still reject what they reject.
- **Failures.** A failed parse is never kept.
- **Checks on a hit.** A hit still performs the parser-entry check and all import
  accounting (reads, source bytes, paths) as usual.
- **Capacity.** The cache keeps at most 64 MiB of module text
  (`DEFAULT_COMPILED_SOURCE_BYTES`, or `CompiledModules::with_capacity`) and evicts
  the least recently used parse first. A module larger than the capacity is
  parsed but not kept.
- **Statistics.** `CompiledModules::statistics` reports kept modules, their bytes,
  hits, misses, and evictions.

Where caches come from:

- **Engines.** Each `Engine` owns a cache that its runs share. Read it with
  `Engine::compiled_modules`, or share one cache between engines with
  `Engine::share_compiled_modules`.
- **Host contexts.** Hosts preparing their own contexts use
  `Context::set_compiled_modules`.
- **The CLI.** One cache serves every file, case, and suite fixture of an
  invocation.

Entry scripts are parsed by each run as before.

## Verification

`cargo test --locked --test run_isolation` covers:

- **Module state.** Three runs of one engine import a module that reads the
  environment during initialization. Each run initializes it and sees its own
  value, while the module is parsed once: one miss, then two hits.
- **Changed text and limits.** A changed module text is parsed again, and a run
  with tighter syntax limits still rejects a module that a looser run already
  parsed.
- **Concurrent runs.** Four concurrent runs keep their own variables,
  environment overlays, working directories, files, and record logs, and leave
  the process environment and directory untouched.
- **HTTP.** Four requests across two runs open four connections, and none replays
  a cookie.

Unit tests in `src/core/eval/compiled/tests.rs` cover sharing, keys, failures,
oversized modules, and least-recently-used eviction. [Stress repetition](stress.md)
runs eight concurrent isolated runs 1,000 times. The
[terminal outcome](terminal-outcomes.md) tests cover per-run records and
artifacts under concurrency.

[Validation evidence](run-isolation-evidence.json) records the measured profiles
and mutations.
