# Retained DSL Definitions

`RunLimits::retained_definitions` (`core::run::RetainedDefinitionLimits`) bounds DSL definition objects kept by runtime scopes and module caches. This complements per-program [AST admission](ast-limits.md) when a Context evaluates multiple programs over its lifetime.

## Defaults and Exact Accounting

| Field | Default | Count |
| --- | --- | --- |
| `definitions` | 16,384 | Distinct installed `Arc<Definition>` objects |
| `nodes` | 262,144 | Sum of reachable AST nodes for each distinct installed definition root |
| `source_bytes` | 8 MiB | UTF-8 text **and name** bytes in distinct reachable SourceFile allocations |

Measure a definition from depth one using the existing iterative AST walk: count its definition/header/parameters/body, statements, expressions, access names, operator spans, and nested definition syntax. An empty definition costs three nodes; its surrounding declaration statement is not retained by the definition object. Admission still respects configured per-tree/per-source AST ceilings before reserving aggregate state.

The same definition object counts once across repeated installations, recursion, modules, and public Context clones. Separately parsed definitions count separately even when their text is equal. Node costs are conservative subtree costs: installing a nested definition separately adds its subtree even while the outer root already includes it. For example, `Outer { Local {} }` retains seven nodes for Outer and another three while Local is installed.

Source identities are deduplicated across every installed root. Include sources reachable only through headers, parameter names, operators, or nested syntax. Shared text/name storage is counted once; separately allocated identical sources count separately. Source accounting follows the stored definition's spans rather than an unrelated Program root source.

## Admission, Release, and Errors

Check namespace/definition collisions first. Then measure the definition and reserve definition/node/source counters atomically before copying signature metadata or publishing the declaration. Admission failures return BW8001 at the declaration, preserve earlier registrations/effects, and stop the requesting Context; Catch cannot resume the stopped run. All fields permit zero or explicit increases. Zero rejects new DSL definitions while native registration and definition-free scripts retain their separate contracts.

Reservations stay alive through installed bindings and active dispatch references. Scope exit, successful/failed calls, failed module initialization, and final Context/cache destruction release them. Successful cached module definitions remain charged; namespace aliases and imported-call frames share their reservations. The final reservation releases nodes and the source references belonging to that definition; a source becomes available only after the final definition using it is released.

Public Context clones share live-definition accounting but keep independent work counters and stop latches. Concurrent reservations and releases are synchronized. Engine runs receive fresh trackers; external Programs or host-owned metadata may outlive the runtime reservation without remaining charged to that run.

## Scope and Evidence

These are logical syntax/source counts. Parsing and host construction happen before retention admission. [Variable-name storage](retained-names.md) has separate reservations. Statement/namespace names and signature payloads, native registrations, imported wrapper metadata, namespaces retaining import spans, frame/cache table copies, diagnostic ownership, allocator overhead/capacity, and query/result copies retain separate roadmap tasks. Limits do not establish a process-memory ceiling.

Reservation and AST unit tests cover exact counts, hidden/shared/distinct source owners, overflow, atomic rejection, ownership cycles, and 8,000 concurrent registration/release operations. `tests/retained_definitions.rs` covers repeated Context evaluations, zero/raised/default limits, collisions, effect order, local/recursive cleanup, cloned contexts, module caching/failure, and CLI recovery. An allocation observation verifies rejection before copying long parameter labels into signature metadata. R10 corpus cases and an executed Rust example pin shared-source admission and declaration rejection.
