# Retained Statement and Namespace Metadata

`RunLimits::retained_registry` (`core::run::RetainedRegistryLimits`) bounds runtime statement/namespace records, metadata strings, and their source owners. Context/module snapshots share immutable keys, signatures, and reservations.

## Defaults and Counts

| Field | Default | Count |
| --- | --- | --- |
| `entries` | 65,536 | Distinct stored native/DSL/imported signature records and namespace records |
| `nodes` | 262,144 | One per record, plus every parameter descriptor and documented-error descriptor |
| `name_bytes` | 65,536 | UTF-8 bytes in each normalized registry lookup key, including namespace qualifiers |
| `text_bytes` | 8 MiB | Lookup-key bytes, normalized signature bytes, parameter labels, documentation/error strings, and display namespace strings |
| `source_bytes` | 8 MiB | Text and name bytes across unique source allocations retained by metadata/namespace/import-site spans |

An empty DSL signature costs one node. A namespace costs one entry/node and its key bytes. Imported wrappers have newly qualified metadata and keys; their exported lookup key shares the module's original key allocation. Shared signatures/namespaces in cloned frames count once. Independently constructed signature records count separately, including local declarations on separate invocations. Source identities are deduplicated across records; identical text in separate allocations counts separately. These source budgets overlap with definition-source accounting by design.

All fields permit zero or explicit increases. The fixed built-in Log registration is exempt so existing infallible `init_statements()` remains usable with zero user-registry budgets. A user-supplied Log replacement is charged normally. Arbitrary native callback captures and standalone NativeOperation metadata remain host-owned.

## Admission and Publication

Preserve namespace/definition collision priority. Reserve all record/node/string/source counters atomically before copying lookup keys or DSL/imported metadata. Native signatures have already been constructed by the host; admission precedes storing them. Rejection produces BW8001, preserves previous registrations and completed effects, and latches the requesting Context. Catch cannot resume a stopped run.

After a module initializes, admit its namespace and every wrapper in normalized-name order before copying/publishing any export metadata. A failure releases all staged reservations and publishes no partial namespace. Successfully cached module definitions and completed initialization effects remain. Existing cumulative import work budgets retain their independent counting rules.

Reservations release when the final binding/dispatch/snapshot owner disappears. Local calls and failed module initialization release their records; successful cached modules retain theirs. Context clones share live accounting with independent stop latches. Modules share the caller's stop state, and concurrent reservations/releases are synchronized.

## Engine Templates and Queries

`Engine::with_registry_limits(limits)` configures reusable native-registration storage. Each run still uses `RunOptions::limits.retained_registry`: after configuration/environment validation, admit all custom native templates in normalized-name order before inputs or script effects. Run admission shares template key/signature payloads, starts fresh counters, and cannot mutate template budgets or another run. A registration-budget stop prevents further additions to that template; successfully registered statements remain usable in fresh runs with sufficient limits.

Metadata listing/completion returns borrowed signatures from admitted records in normalized order. Result/scratch entry counts follow visible registry records, including fixed built-ins. Completion scans the supplied prefix but bounds its normalization buffer by the longest retained key; it preserves ASCII space/tab removal, Unicode lowercasing, and the first-parameter boundary. Whole-header queries retain the existing bounded native-header parser. Host-owned metadata clones, formatted help/output, and raw query scanning time have separate host/output contracts.

## Remaining Scope and Evidence

Counts exclude allocator capacity/overhead, callback captures, expression/results, and diagnostics. Frame/cache table copies use separate [snapshot budgets](snapshot-limits.md). Parsing, namespace normalization, and supplied host descriptors precede registry retention admission. This is not an exact process-memory ceiling.

Reservation tests cover exact metrics, checked arithmetic, atomicity, source/key sharing, lifetime cycles, and 8,000 concurrent registration/release operations. `tests/retained_registry.rs` covers local/cloned/module ownership, atomic namespaces, zero/raised/default limits, collisions, native-template admission/order, Unicode queries, source release, concurrency, and CLI recovery. Allocation observations pin pre-copy rejection and shared Context/template/imported-native payloads. The combined deep-import/parser regression also runs after separating declaration/publication scratch state from recursive loading frames. R12 corpus cases and an executed Rust example pin exact native metadata budgets.
