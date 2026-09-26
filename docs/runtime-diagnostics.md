# Synchronous Runtime Diagnostic Limits

`RunLimits::diagnostics` applies DiagnosticLimits to individual errors crossing synchronous evaluation boundaries. Defaults permit 1,024 diagnostic nodes, depth 32, 4,096 call frames, 4,096 related locations, 8 MiB of raw detail/context text, and 8 MiB of distinct source owners. [Ownership metrics](diagnostic-ownership.md) define exact counts and the fixed depth ceiling of 64. All quotas permit zero; successful execution with no errors needs no diagnostic allowance.

## Admission Before Call Copies

Measure each complete error and any prospective call snapshot before cloning frame vectors or signature strings. Preserve an existing snapshot instead of adding later caller frames. Sources are shared Arc references, deduplicated by identity during measurement; checking their byte lengths does not copy source contents or scan coordinates. When admission succeeds, preserve full code, identity, spans, causes, and entered-call metadata.

Apply checks at expression, statement, call, native-result, and public program/statement/Pair boundaries, including validation failures. Engine finalization also admits errors from parsing, source reads, inputs, and setup. Configured quotas apply after valid run/environment preparation installs the run budget; earlier control/configuration/environment failures use the fresh Context's default diagnostic quotas. Invalid diagnostic depth is rejected before script effects.

Initial BWErr strings and active call frames already exist when this per-tree admission runs. [Retained call/handler limits](retained-diagnostics.md) separately reserve live records before call-signature copies and handler storage. Original error-message allocation and small related/cause attachments remain construction tasks.

## Failures, Cleanup, and Stops

Exceeding a quota returns BW8001, latches the Context, and bypasses Catch. Completed effects remain; handler and iterator restoration still run. Imports share limits and stops. Context clones copy settings/work/stop state independently; Engine runs start fresh. A stopped context cannot resume with another evaluation.

Use the fixed independent [emergency representation](diagnostic-ownership.md#owned-admission-and-emergency-evidence): preserve the original category as a bounded cause, exact source-byte evidence, and explicit omissions. Further unwinding cannot reattach complete sources or copy frames into emergency errors. Related sites and handled causes increment omitted counts while their owned payloads are released. Call counts record the original requested snapshot and do not accumulate duplicate caller snapshots during unwind.

Observe cancellation/deadline state before a new diagnostic quota can latch. Preserve the native cause's own call context before wrapping it in the observed stop when quotas allow. If a cancellation/timeout diagnostic itself exceeds its quota, keep its bounded original-category summary primary and the quota violation as its single cause. This preserves Cancelled/TimedOut outcomes, records discarded causes explicitly, and prevents handler execution. Callback-reported cancellation categories alone do not signal an actual run stop.

## Remaining Boundaries and Evidence

These are per-tree limits. [Aggregate retained records](retained-diagnostics.md) account for active handlers, calls, shared Context snapshots, and their unique source owners. Host-retained run results, original message construction, registration-only APIs, standalone NativeOperation diagnostics, arbitrary callback allocations, general rendering, and output/serialization have separate contracts or pending work. These limits do not establish a process-memory ceiling or hard termination deadline.

Tests cover exact/zero/invalid quotas, prospective versus existing stacks, source-byte evidence, prior effects, handler/rethrow/import cleanup, independent Context stops, native cancellation causes, controlled-clock timeouts, all synchronous entry points, pre-copy rejection, default CLI cause amplification, and recovery. Existing debug/release stack stress and completion matrices remain active. R18 corpus cases and a Rust doctest pin handler admission.
