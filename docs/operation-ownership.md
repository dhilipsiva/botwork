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

Keep argument charges conservatively until the last invocation owner releases them, including callback execution, completed output waiting in a join handle, and cancellation cleanup. Successful callback values are checked and reserved before async completion or blocking-worker handoff. Results require argument-plus-result headroom even when a callback returns an argument's original storage. Invalid results preserve completed callback effects and use iterative disposal. Callback errors pass individual and aggregate diagnostic admission before handoff.

Retain an observed blocking stop with its own diagnostic reservation while draining the worker. The worker separately admits its result or cleanup error. Combining existing error trees moves their owners and atomically replaces their combined reservation; it does not require a second full copy of the same trees. A failed replacement preserves the old charge until rejected data is freed. Cancellation and the originally observed timeout retain priority over quota failures and cleanup cancellation.

Release payloads before returning their allowance. Dropping an async invocation destroys its future before releasing input charges. Dropping a blocking invocation signals its child but leaves reservations with a queued/running worker and its owned output. Queued Tokio work retains admission until it runs or is destroyed; cooperative worker completion and abandoned output disposal release the final owner. Blocking capacity and ownership allowances are separate: waiting for a permit still consumes argument capacity.

Returning Literal or Diagnostic from the public future transfers ownership to the host and releases operation charges. Holding or cloning returned objects is the host's responsibility. Rejection does not latch or cancel parent/sibling operations; a later invocation can reuse released capacity. Concurrent budget users cannot oversubscribe counters.

## Rejection and Scope

Aggregate quota failures use BW8001 and name the exceeded operation resource. Rejected errors retain fixed bounded original-category/filename/byte evidence with explicit omissions, and release their full source owners. A callback cannot bypass measurement by constructing public fields that resemble an emergency summary. Internally generated emergency records have the existing fixed caps outside ordinary diagnostic quotas. At most the current stop and callback outcome need emergency storage for an admitted invocation; their lifetime remains bounded by invocation slots. Refused-entry futures retain only small deferred failures outside these slots, with no input or diagnostic source owners.

These are logical ownership limits, not a process-memory ceiling. Host-created input/result/error allocations exist before admission. Arbitrary callback captures, detached host tasks, native panic payloads, allocator capacity, the operation's immutable signature, and future destructors remain host boundaries. Interpreter message construction still uses its individual diagnostic admission before aggregate adoption. Publicly delivered results, rendering, output, hard worker deadlines, and async DSL dispatch retain separate contracts.

## Evidence

Four reservation tests cover every dimension, atomic failure, overflow, replacement/merge, concurrent admission, and final-owner release. Twelve integration tests cover exact/one-less/zero limits, shared/distinct/isolated pools, unpolled arguments, left-to-right validation, callback effects, output overlap, completed worker payloads, both blocking queues, abandonment, deep disposal, source lifetimes, every diagnostic quota, host-shaped summaries, cancellation, and controlled-clock timeout cleanup. An allocation check observes original string-buffer transfer and bounded error rejection without large payload copies. R22 corpus cases and an executed Rust example pin result headroom and public ownership transfer.
