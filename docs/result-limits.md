# Owned Run Results

`RunLimits::results` (`core::run::ResultLimits`) bounds the combined owned value exports of each Engine run before publication. It covers the successful terminal value and every completed root binding, including supplied inputs. Invocation locals and cached module globals are not exported. CLI logging, direct Context return values, and diagnostic formatting have separate contracts.

## Defaults and Counts

| Field | Default | Count |
| --- | --- | --- |
| `values` | 65,537 | Root bindings plus one successful terminal value |
| `nodes` | 327,680 | Recursive value occurrences across those exports |
| `name_bytes` | 4 MiB | UTF-8 bytes of exported root-variable names |
| `payload_bytes` | 40 MiB | String/map-key bytes, four bytes per integer/float, one per boolean |

Containers and None contribute a node and no payload bytes. The successful result of an empty program is None and therefore counts as one value/node. An execution failure has no terminal value to charge. A value present both as a terminal result and a root binding counts twice, even when their contents match. The defaults accommodate the default retained-root budgets plus one maximum per-value result. Raising storage budgets does not automatically raise export budgets. Every field permits zero or an explicit increase.

First admit aggregate value counts and root-name bytes. Measure the terminal value, then root values in exact name order, with iterative depth-bounded traversal and checked arithmetic. Remaining node/payload allowances stop measurement early. A temporary list of borrowed binding references is bounded by the admitted binding count; no owned result table, name string, or value payload is copied before all exports pass.

## Completion and Omitted Exports

`RunResult::snapshot_error` is `None` when root export succeeded. If admission fails, `variables` is empty and `snapshot_error` contains the BW8001 export diagnostic. No partial root map is returned. Hosts must inspect this field before treating an empty map as a complete snapshot.

For a previously successful execution, export failure becomes `result` and `outcome()` reports `LimitExceeded`; the successful terminal value is discarded. If execution already failed, timed out, was cancelled, or exceeded another limit, preserve that original `result` and outcome, including its spans, stack, and causes. Report the additional export error separately in `snapshot_error`. This preserves the reason execution stopped while making omitted state explicit.

Export happens after script effects and the final execution checkpoint. It cannot enter Catch or undo completed work. Bounded finalization still runs for stopped contexts; it does not resume evaluation or poll new cancellation requests. A cancellation arriving during finalization does not replace the already selected execution outcome. Invalid input/configuration or pre-entry cancellation has no installed roots and normally needs no export allocation. Finalization and cleanup contribute to elapsed time.

## Ownership and Peak Overlap

Move uniquely owned root names/values into the result map. Shared roots, such as those retained by a Context clone, require copying after admission; their other owners remain intact. Transfer to host ownership releases the runtime reservation for a unique allocation. Shared runtime owners retain theirs. Rejected terminal data uses iterative destruction, and all admitted stored data already obeys the fixed value-depth ceiling.

The export budget admits the entire output bundle, including any shared copies, while runtime storage can still be live. Conservative logical peak overlap is the retained-runtime allowance plus the result allowance; unique transfers avoid duplicating payloads. The source hash table, bounded reference scratch, and destination ordered-map structure can briefly overlap. Allocator overhead/capacity, diagnostics, and arbitrary host allocations are outside these byte counts. [Expression temporaries](temporary-limits.md) use separate live ownership accounting before the public return/export boundary.

Returned results are host-owned. Keeping many results, cloning their public values, or running multiple Engines does not share an aggregate export tracker. Independent runs start fresh limits. These counts bound each export and its runtime overlap; they are not a process-memory ceiling or a hard finalization deadline.

## Evidence

Unit tests pin default budgets, exact/overflow counts, UTF-8 map payloads, pointer-preserving transfer, shared-copy isolation, reservation release, and iterative rejection of deep terminal data. Integration tests cover each budget, zero/raised/default settings, successful/failing/cancelled runs, failure priority, atomic exports, private locals, prior effects, concurrent fresh runs, and retained results. Allocation observations verify moved inputs, pre-copy rejection, and absence of final assignment payload copies. R14 corpus cases and a Rust doctest verify exact export admission and explicit omission.
