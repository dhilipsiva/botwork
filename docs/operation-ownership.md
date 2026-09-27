# Aggregate Operation Ownership

`core::operation::OperationBudget` accounts for live arguments, results, and diagnostic trees across standalone NativeOperation invocations. Each new operation gets a fresh default budget; operation clones share it. Pass a cloned budget to `with_ownership_budget` to share admission across distinct callbacks. Installing a different budget on a clone leaves existing operations and invocations in their original pool. `ownership_budget`, `limits`, and `usage` expose the configured pool and a synchronized snapshot of its logical charges.

## Limits and Counting

`OperationOwnershipLimits` permits zero or raised values for every field. Admission uses checked arithmetic and updates every dimension atomically.

| Field | Default | Charge |
| --- | ---: | --- |
| invocations | 1,024 | Each admitted invocation, including unpolled futures and detached workers |
| values | 65,536 | Argument roots plus owned result roots |
| nodes | 262,144 | Recursive Literal nodes |
| payload_bytes | 32 MiB | Value strings/map keys, four bytes per integer/float, one per boolean |
| diagnostics | 16,384 | Diagnostic records, including causes |
| call_frames | 65,536 | Entered frames in retained errors |
| related_locations | 65,536 | Related diagnostic sites |
| text_bytes | 32 MiB | Diagnostic details, labels, frame signatures, related text, and omission metadata |
| source_bytes | 32 MiB | Source text and filenames, deduplicated within each diagnostic tree |

Source owners shared by separate live error trees are conservatively charged once per tree. When a stop and its callback cause combine into one tree, the replacement reservation uses that tree's distinct sources. Individual ValueLimits and DiagnosticLimits still apply, including their depth ceilings. OperationBudget does not change those local settings.

## Admission and Transfer

`invoke` wraps its input for iterative disposal, observes an existing stop, checks arity, and validates each argument in order: value shape/size, numeric finiteness, then signature kind. It then reserves the invocation and the complete argument bundle before returning its future. Thus merely constructing unpolled futures consumes allowance. A rejected preparation drops inputs immediately and retains only a small deferred reason; it constructs and admits the returned diagnostic when polled. Prior cancellation is observed again during polling. Arguments are never copied for accounting.

Keep argument charges conservatively until the last invocation owner releases them, including callback execution, completed output waiting in a join handle, and cancellation cleanup. Successful callback values are checked and reserved before async completion or blocking-worker handoff. Results require argument-plus-result headroom even when a callback returns an argument's original storage. Invalid results preserve completed callback effects and use iterative disposal. Callback errors pass individual and aggregate diagnostic admission, including any prospective header attachment, before handoff. The host error already exists; the interpreter adds no source/context owner until both checks succeed.

Retain an observed blocking stop with its own diagnostic reservation while draining the worker. The worker separately admits its result or cleanup error. Before attaching a stop or inserting a cleanup cause, measure the complete prospective tree, including the new depth and header ownership. Combining existing error trees moves their owners and atomically replaces their combined reservation; it does not require a second full copy of the same trees. A failed replacement preserves the old charge until rejected data is freed. Cancellation and the originally observed timeout retain priority over quota failures and cleanup cancellation.

Release payloads before returning their allowance. Dropping an async invocation destroys its future before releasing input charges. Dropping a blocking invocation signals its child but leaves reservations with a queued/running worker and its owned output. Queued Tokio work retains admission until it runs or is destroyed; cooperative worker completion and abandoned output disposal release the final owner. Blocking capacity and ownership allowances are separate: waiting for a permit still consumes argument capacity.

Returning Literal or Diagnostic from the public future transfers ownership to the host and releases operation charges. Holding or cloning returned objects is the host's responsibility. Rejection does not latch or cancel parent/sibling operations; a later invocation can reuse released capacity. Concurrent budget users cannot oversubscribe counters.

## Initial Construction and Mutation

Argument-count/kind, return-kind, numeric, runtime-guard, panic, and worker-join messages now reserve shared operation ownership before their full detail or header copies. First reserve the known diagnostic context, then count the deferred message and atomically extend that same reservation before writing it. The reservation remains live during counting and writing, across concurrent operations sharing the pool, and through subsequent tracking and worker handoff. Re-admission credits existing capacity. Panic construction covers factory, future poll, and blocking callback failures.

A worker-cleanup message includes the complete already-owned primary tree in both stages. Existing primary capacity is credited; both primary and new-cause source owners are measured before either context is added. Failed counting, writing, individual admission, or aggregate extension disposes the rejected primary before releasing its reservation. An already-bounded primary only records the omitted cause and skips the new full message. Cancellation/timeout remains primary when its deferred cause cannot fit.

Stop attachment and completed-worker cause merging likewise admit complete prospective ownership before growing the cause vector. Two moved trees contribute their existing reservations, and shared source owners are deduplicated in the combined tree. Failed merges keep old capacity until full payload disposal, preserve the primary category/location, and count the omitted cause. Duplicate immutable error identities do not append another cause. Trusted emergency records keep their independent fixed bounds; host-shaped records still undergo normal measurement.

The internal construction carrier owns the diagnostic before its reservation field, ensuring normal disposal frees the payload first. Small fixed control/resource categories, empty skeletons, and measurement scratch remain bounded admission overhead. Setup/builders and immutable operation signatures retain their separate host/default contracts. These changes do not alter invocation, worker-slot, cooperative-stop, or public-return behavior.

## Rejection and Scope

Aggregate quota failures use BW8001 and name the exceeded operation resource. Rejected errors retain fixed bounded original-category/filename/byte evidence with explicit omissions, and release their full source owners. A callback cannot bypass measurement by constructing public fields that resemble an emergency summary. Internally generated emergency records have the existing fixed caps outside ordinary diagnostic quotas. At most the current stop and callback outcome need emergency storage for an admitted invocation; their lifetime remains bounded by invocation slots. Refused-entry futures retain only small deferred failures outside these slots, with no input or diagnostic source owners.

These are logical ownership limits, not a process-memory ceiling. Host-created input/result/error allocations exist before admission. Arbitrary callback captures, detached host tasks, native panic payloads, allocator capacity, the operation's immutable signature, and future destructors remain host boundaries. Interpreter message construction uses both individual and aggregate admission before its full copies, as described below. Publicly delivered results, rendering, output, hard worker deadlines, and async DSL dispatch retain separate contracts.

## Evidence

Four reservation tests cover every dimension, atomic failure, overflow, replacement/merge, concurrent admission, and final-owner release. Twelve integration tests cover exact/one-less/zero limits, shared/distinct/isolated pools, unpolled arguments, left-to-right validation, callback effects, output overlap, completed worker payloads, both blocking queues, abandonment, deep disposal, source lifetimes, every diagnostic quota, host-shaped summaries, cancellation, and controlled-clock timeout cleanup. An allocation check observes original string-buffer transfer and bounded error rejection without large payload copies. R22 corpus cases and an executed Rust example pin result headroom and public ownership transfer.

Construction/mutation coverage adds four unit checks, one public matrix, and one allocation comparison. They cover reservations during both formatting passes, concurrent rejection, tracking/public transfer, failed cleanup extension and source release, shared-source merge credits, prospective depth rejection, and primary-before-reservation disposal after counting/writing failures. Seven generated-error families run at exact/one-less shared text quotas with repeated capacity reuse. Large factory/poll panic details allocate once on acceptance and never allocate a full copy on aggregate rejection. Existing R20/R21/R22, worker-join, cancellation/timeout, host-shaped-error, and abandonment checks remain active.
