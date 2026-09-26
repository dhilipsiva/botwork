# Retained Variable Values

`RunLimits::retained_values` (`core::run::RetainedValueLimits`) bounds the aggregate value content held by variables in Engine, CLI, and Context execution. These are live reservations: released values return their allowance. Execution/import work counters retain their separate cumulative rules.

## Defaults and Counting

| Field | Default | Count |
| --- | --- | --- |
| `values` | 65,536 | Independently stored variable values, including None and empty containers |
| `nodes` | 262,144 | Sum of [ValueSize nodes](value-limits.md) in stored values |
| `payload_bytes` | 32 MiB | Sum of string/map-key UTF-8 bytes, four bytes per Int/Float, and one per Bool |

Shared references to the same stored allocation count once. Distinct variables with equal or copied values count separately. Root inputs, assignments, custom-call parameters, module globals, For iterators, and Catch error values all reserve from the same tracker. Saved iterator/handler bindings and access snapshots keep their reservations until their final reference disappears. Container nodes and map-key payload follow the per-value contract.

All fields permit zero or explicit increases. Zero values/nodes rejects bindings but permits a run without bindings; returning a temporary scalar does not consume retained-value allowance. Zero payload permits empty values. Checked arithmetic rejects counter overflow with BW8001.

## Admission and Release

Validate individual values, then reserve all aggregate counters atomically before storing a value. Assignments reserve before cloning the stored copy. Required RHS/callback effects have already happened; rejection leaves the previous destination intact. Root input batches validate first, prepare all reservations, then install atomically. A failed batch releases partial reservations and preserves every old root binding.

Replacement requires headroom for old and new allocations simultaneously. An existing one-value binding therefore needs `values >= 2` for replacement, even when its content shrinks. Batch replacements require room for the complete old and incoming sets. For retains its saved binding plus the current iteration; advancing also briefly reserves the next iteration. This conservative peak rule avoids releasing allowance while old storage remains live.

Dropping the final reference releases its counters after value destruction. Invocation exit, handler/iterator restoration, failed frame construction, and failed module initialization release their local values. Successfully cached module globals remain charged for the cache's lifetime; namespace aliases and isolated imported-call frames share them. Per-value depth admission bounds ordinary destruction of accepted values.

## Clones, Stops, and Independent Runs

Public Context clones share live-value accounting because their immutable value storage is shared. Replacements made by either clone consume the same allowance; dropping a clone releases only allocations with no remaining owner. Reservations are synchronized across concurrent clones. Execution/import counters and stop latches still copy independently: a reservation failure stops the requesting clone, while its sibling remains usable if capacity is available. Cancellation controls retain their existing shared semantics.

Modules share both the tracker and the caller's stop state. BW8001 latches and bypasses Catch. Admission of a Catch binding preserves the original error as a cause on failure. Engine executions each start a fresh tracker, including simultaneous runs and runs whose earlier results remain owned by the host.

## Scope and Evidence

Counts describe logical value content, excluding allocator capacity/overhead, binding names, definitions/source owners, frame/cache maps, and host captures. Argument/expression temporaries before binding, returned values, root result snapshots, diagnostics, and serialization retain separate allocation tasks. This contract does not establish a process-memory ceiling or hard native termination.

Reservation unit tests check atomicity, overflow, shared ownership, release, concurrency, and independent stop latches. `tests/retained_values.rs` covers exact/default/zero budgets, Unicode, root batches, replacement, call/module/loop/handler cleanup, persistent contexts, and concurrent clones. An allocation observation verifies rejection before the assignment copy; an evaluator test holds an access snapshot across replacement. R9 corpus cases and a Rust doctest exercise the public configuration.
