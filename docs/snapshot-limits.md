# Frame and Module-Cache Snapshots

`RunLimits::snapshots` (`core::run::SnapshotLimits`) bounds cumulative table-copy work before allocation. Defaults admit 1,048,576 copied entries and 32 MiB of copied path bytes. Both fields permit zero and explicit increases. Counters persist across repeated Context evaluations; every Engine run starts fresh.

## What Counts

An entry is one copied variable, statement, namespace, loaded-module cache record, or requested-to-canonical path record. Count each occurrence on every copy, even when its immutable payload is shared. Include fixed built-in entries. Ordinary assignment, declaration, parameter installation, and first cache insertion use their existing retention/import budgets rather than snapshot counters.

Path bytes include copied loaded-module keys, requested/canonical resolution keys and values, active loading paths, and the isolated Context's working directory. Module initialization also counts the new loading-path copy. Use `OsStr::len()` for native path representation; no lossy display conversion. A retained working-directory error string counts its bytes instead.

Admission covers:

- Engine native-template table copying, after configuration/environment validation and before native registry admission, inputs, or script effects.
- Module initialization: the complete caller cache, loading/directory paths, and visible native registry entries. Caller variables and DSL definitions are not inherited. File resolution, reading, and parsing occur before this check.
- Imported invocation: the module's root frame, the caller cache, and loading/directory paths, before entering the exported body. Required argument effects have already happened.
- `Context::try_clone()`: all current frame tables, cache tables, and loading/directory paths before making a host copy.

Measure both counters without allocating copied tables or path strings, then atomically admit the entire charge with checked arithmetic. Rejection returns BW8001 and latches the requesting Context. Failed admission consumes neither counter; completed effects and existing state remain. Accepted work is cumulative and is not refunded on destruction or later failure. Repeated imports/calls can exhaust work budgets while their live state remains small.

## Copying and Cleanup

Rebuild hash tables from their live entries, so snapshots do not duplicate unused historical capacity. Keys, stored values, definition trees, signatures, namespace records, and loaded modules keep shared immutable ownership. Metadata-selection scratch lists remain bounded by the registry contract.

The caller is suspended during isolated module execution. Transfer the child's complete cache back on both success and failure, retaining successfully initialized dependencies without another table merge or copy. Existing invocation/iterator/handler cleanup remains valid after a snapshot failure. Modules share work counters and stop state; admission is synchronized.

## Host Ownership

`Context::clone()` and `Engine::clone()` remain infallible host operations outside snapshot admission. They preserve state, including stop state, and rebuild tables while sharing immutable payloads, native captures, cancellation controls, and live retention trackers. Hosts own the number and lifetime of these copies. Work counters and stop latches are copied independently; cloning does not reset consumed allowance.

Use `Context::try_clone()` when a host copy must pass admission. It charges the source, then copies the updated work counters into the result. Failure stops the source. Subsequent source/result work counters are independent, while their stored payload reservations remain shared. Host-owned copies are not an aggregate process-memory ceiling.

These logical counts exclude allocator overhead, call-stack/handler diagnostic copies, environment snapshots, and serialized output. [Expression temporaries](temporary-limits.md) have separate live accounting. [Owned run results](result-limits.md) have separate export admission and transfer rules; the other allocations retain their own contracts and roadmap tasks. Admission does not make allocation fallible at the OS level.

## Evidence

Budget unit tests cover atomicity, overflow, and shared/concurrent counters. Frame/cache tests verify exact path metrics, compact copies, and iterator restoration after an imported-copy failure. Integration tests cover boundary/default/zero limits, templates, cancellation, native inheritance, cached aliases, failure cleanup, concurrent checked copies, host ownership, and CLI recovery. Allocation observations verify rejection before copying wide variable, template, or module tables. R13 corpus cases and a Rust doctest pin Engine/template and host-copy behavior.
