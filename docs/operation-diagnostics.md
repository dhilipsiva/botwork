# Native Operation Diagnostic Limits

NativeOperation applies `DiagnosticLimits::default()` to individual callback and invocation errors. `operation.with_diagnostic_limits(limits)` changes that operation and validates configuration immediately. Clones copy settings; changing one clone leaves siblings unchanged. Defaults, exact logical counts, the depth ceiling, and zero/raised allowances follow [host diagnostic ownership](diagnostic-ownership.md). Successful invocations need no diagnostic allowance.

## Admission Boundaries

Admit async callback failures as their poll completes and blocking failures inside the worker, before transferring the result through its join handle. Include the native header when the error lacks a primary location; preserve an existing primary span, code, causes, and call/related context. Accepted errors move without copying their owned metadata and retain original error/source identities.

Rejected trees are disposed iteratively, including arbitrarily deep host causes. Return BW8001 with fixed bounded original-category evidence, filename/byte offsets, and explicit omissions. Host-created diagnostics always pass admission, even when their public fields resemble an emergency summary. Internally admitted emergency records remain bounded through final publication without adding header sources again.

Apply admission again to complete invocation failures, including argument/value contract errors, callback construction/poll/worker panics, missing runtime support, and blocking cleanup causes. Preexisting per-value limits still validate arguments before callback entry and results before publication. Builder/registration failures have their own construction contract and precede invocation settings.

## Stops and Ownership

Observe child cancellation/deadlines after callback completion, including failures returned during the same completion. Keep the observed stop primary and retain admitted callback evidence when the combined tree fits. If the stop tree exceeds diagnostic quotas, return its bounded original-category summary with the quota failure as its cause; record omitted causes explicitly. A callback-reported cancellation category alone does not signal an observed stop.

When blocking cleanup requests cancellation to drain a worker, retain the originally observed timeout or runtime failure. Cleanup cancellation must not replace that outcome. Requests remain cooperative: a started callback must return before an awaited invocation finishes. Diagnostic rejection does not cancel the parent or latch sibling operations; repeated invocations remain usable.

Dropped invocations signal their child. Owned result/error guards dispose abandoned worker payloads iteratively, and worker capacity remains held until completion. Delivered diagnostics satisfy configured quotas or the fixed emergency contract. This changes the earlier prototype behavior that delivered unrestricted deep callback errors to the host. Explicit host admission/disposal APIs remain available for trees outside operations.

## Scope and Evidence

These limits admit ownership after callbacks construct errors. They do not preflight callback allocations, initial error-message construction, arbitrary future/capture destructors, aggregate concurrent arguments/results/errors, host-retained results, formatting, or serialization. Synchronous DSL execution uses its separate runtime settings; async DSL dispatch and hard worker deadlines remain planned work.

Contract checks cover all dimensions, exact/zero/invalid allowances, source ownership, accepted identity, emergency-shaped host input, clone isolation, same-completion stops, controlled-clock deadlines, panic/error boundaries, deep rejection, and abandoned-worker capacity release. An allocation observation checks rejected large details without copies. R20 host corpus cases and a Rust doctest pin the operation boundary.
