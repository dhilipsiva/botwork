# Import Resource Budgets

Engine, CLI, and Context share cumulative import budgets. Configure them through `RunLimits::imports` (`ImportLimits`). Counters persist across low-level evaluations and module calls; public Context clones copy counters independently alongside their cache snapshots. Engine runs start fresh.

## Defaults and Counting

| Field | Default | Admission rule |
| --- | --- | --- |
| `loads` | 128 | Uncached source-read attempts after path/cycle/depth checks, including reads or initializations that fail |
| `source_bytes` | 8 MiB | Cumulative successfully read module bytes, including malformed/invalid-UTF-8/failed-initialization sources |
| `paths` | 512 | Distinct requested paths inserted into the canonical-resolution cache |
| `bindings` | 16,384 | Every published namespace plus each qualified statement wrapper, including repeated cached imports |
| `metadata_bytes` | 8 MiB | Owned path and namespace/qualified-metadata string payload admitted for caches and exports |
| `dependency_depth` | 32, fixed ceiling | Import edges through retained module dependencies, including cached modules and empty namespaces |

The entry source has its separate source/AST budget. Module reads retain at most the smaller of the remaining aggregate bytes and per-source byte budget, plus one detection byte. Check oversize before UTF-8 decoding or parsing. Filesystem calls that return an error consume a load attempt; partial bytes discarded by a failed read are not added to the byte counter. Cache hits do not reopen/read modules or consume load/source budgets. A new canonical alias still consumes a requested-path entry and path bytes. Namespace collisions and active import cycles retain their ordinary diagnostics before later resource admission.

Metadata accounting includes requested/canonical resolution paths, a canonical identity per uncached load attempt, namespace keys, qualified lookup keys and metadata names, original export names, display namespaces, parameter names, descriptions, and error descriptions. It counts string payload bytes, not allocator overhead or transient Context/cache snapshots. Qualification reuses the computed namespace normalization and reserves the whole publication before copying metadata. Structural container counts are bounded separately by bindings/paths/loads.

## Chains and Publication

Active uncached initialization still has the separate `RunLimits::import_depth` ceiling of 16 and parser-entry headroom. The dependency ceiling additionally rejects long chains assembled from already cached modules. Count pending initialization ancestors plus the imported module's retained dependency depth and the new edge. Cold imports reserve depth before their source read; cache hits check their known dependency depth before namespace publication. Local namespaces inside completed custom invocations do not become retained dependencies of their defining module.

Namespace publication is atomic: reserve its namespace entry, all exported wrappers, and metadata together before insertion. This also bounds exponential re-export fanout. A late publication failure preserves completed module initialization effects and successful cached dependencies, but publishes no partial namespace. Limits do not roll back external effects.

## Failures and Remaining Work

Exceeded budgets latch BW8001 with the resource name/limit; Catch cannot resume a stopped run. Reject dependency ceilings above 32 with BW7002. Other budgets may be explicitly raised; zero forbids the corresponding work, while zero source bytes still admits an empty module. Multi-resource reservations either commit all counters or none. Successful reservations are cumulative and are not refunded on later failure or scope exit.

These are admission/work budgets, not an exact process-memory bound. Module globals and call parameters also share [live-value reservations](retained-values.md), including across public Context clones. Cached DSL definitions share [definition/source reservations](retained-definitions.md). [Registry reservations](retained-registry.md) additionally stage namespaces and qualified metadata atomically before publication. [Snapshot budgets](snapshot-limits.md) admit frame/cache table and path copies before isolated execution. Parser/temporary allocations, host callbacks, hard deadlines, and filesystem confinement retain separate roadmap work.

`tests/import_limits.rs` covers load/source/path/binding/metadata boundaries, cache reuse after file removal, failed retries, UTF-8 cuts, atomic publication, cached/cold chains, fanout, Context cloning, nested calls, cleanup semantics, zero configurations, and default CLI limits. Unit tests check reservation overflow/atomicity and Unicode metadata accounting; R5 corpus cases pin cached reuse and exact namespace budgets.
