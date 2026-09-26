# Retained Variable Names

`RunLimits::retained_names` (`core::run::RetainedNameLimits`) bounds immutable variable-name storage in Engine, CLI, and Context execution. Names share allocation ownership across Context and module snapshots; copying a frame no longer copies its variable-name strings.

## Budgets

| Field | Default | Count |
| --- | --- | --- |
| `names` | 65,536 | Independently allocated variable names currently retained |
| `name_bytes` | 65,536 | UTF-8 bytes in each variable name |
| `total_bytes` | 4 MiB | Sum of UTF-8 bytes in retained name allocations |

Count exact case-sensitive/code-point spellings. A name shared by cloned bindings counts once; separate allocations with equal spelling count separately. Assignments and root-input replacements reuse the existing target-scope key. Invocation parameter/local keys use their own allocations, including when they shadow an equal caller name. All budgets permit zero and explicit increases; zero allows scripts with no variable bindings and empty loops. Raising these budgets does not raise separate input/syntax/value limits.

## Admission and Cleanup

Reserve count/bytes atomically before copying a new key or installing its binding. Assignment RHS effects happen first; name rejection occurs before the stored value copy and preserves the prior destination. Root input installation checks per-name length before identifier parsing, validates all values, then reserves complete input batches before publication. A failed batch releases partial names/values and preserves old root state. Already allocated host inputs remain the host's construction cost; rejected nested values retain iterative cleanup.

For reuses the saved binding's name when present and retains it through restoration. An absent iterator allocates once on its first iteration and releases on exit; an empty loop needs no name. Catch reuses its previous key or admits a new key, retains it through handler cleanup, and preserves the original error as a cause if name admission fails. Restoration does not require new reservations after a resource stop.

Call exit, temporary binding removal, failed module initialization, and final Context/cache destruction release name allowance when the final shared owner drops. Successfully cached module globals remain charged. Context clones share live accounting with independent stop latches; modules share the caller's stop state. BW8001 resource failures bypass Catch. Engine runs start fresh trackers, and reservations are synchronized across concurrent Context clones.

## Scope and Evidence

This contract covers variable keys, including inputs, locals, parameters, iterators, and Catch bindings. Declaration/signature labels, statement/namespace names, and borrowed metadata queries use the separate [registry contract](retained-registry.md). [Snapshot budgets](snapshot-limits.md) bound frame/cache copy work. Diagnostic name copies and owned root results retain separate accounting tasks. Counts exclude allocator capacity/overhead and arbitrary host allocations.

Unit tests check exact/zero/overflow counters, atomicity, UTF-8 lookup/hash identity, shared ownership, release, and clone failures. `tests/retained_names.rs` covers Unicode, defaults/raised limits, input batches, deep rejected host values, replacement, calls/modules, loop/handler restoration, effects, concurrency, and CLI recovery. Allocation observations verify name rejection before key/value copies and payload-free Context name snapshots. R11 corpus cases and a Rust doctest pin exact key reuse and aggregate rejection.
