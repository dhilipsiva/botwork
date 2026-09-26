# Execution Order and Cleanup Evidence

The [core specification](language-specification.md) defines evaluation, scope, and completion behavior. These combined checks complement the individual regression cases; they do not establish exhaustive grammar coverage or the final release gates.

## Evaluation Order

`src/core/eval/execution_contract.rs` observes expression visits through the evaluator's test-only instrumentation. Visit traces check evaluation count/order for ordinary expressions. The effectful call-composition matrix below adds actual native/custom callback traces for nested expression calls, including first-error stopping and completed effects.

- A three-argument call evaluates each argument once in caller scope before any body statement. Failure in each argument position stops later arguments and all body effects, preserves caller bindings, and installs no frame. An unresolved signature visits no arguments.
- Nested array/map construction follows source order, including values for overwritten map keys. Each failure position skips later entries and preserves the assignment destination.
- If/Else If evaluates only the conditions needed to choose one branch, once each.
- While rechecks its condition after normal completion or Continue, including the final false check. Break, Return, and body errors skip the next condition. A later condition type error terminates after completed body effects.

## Control and Scope Matrix

A test-only native recorder captures statement events and frame depth in the context. It is private to the unit suite and does not add a production statement.

The nested For matrix has **56 combinations**:

| Axis | Cases |
| --- | --- |
| Prior iterator binding | Absent, None, current-frame value, inherited value |
| Completion | Normal, Continue, Break, value Return, bare Return, handled error, handler error |
| Transfer origin | If body, If nested in Catch |

Both loops use the same iterator name. Assertions check every event in order, the outer binding visible to the handler, restoration before invocation disposal, exact local presence/absence, returned values/error identity, and unchanged frame depth. Existing mixed For/While cases verify nearest-loop controls across loop kinds.

Recursive traces check descent, unwind order, local helper lookup, repeated invocations, and failure recovery. The caller's handler observes its own frame only after all failed child frames are removed. Local variables/definitions disappear while root bindings remain intact.

## Public Execution

`tests/fixtures/execution-order.botwork` checks real Log output for recursive calls, failed arguments, caller preservation, loop restoration before handlers, and repeated returns through Try/For. `execution-order-error.botwork` checks the first failing native argument, preserved prior output, skipped callee/caller tails, stderr, and nonzero status.

Run `cargo test --lib execution_contract` and `cargo test --test cli`, then repeat with `--release`. CI's existing debug/release matrix runs these checks with the full suite. Other evaluator tests cover short-circuit selection, computed indexes, arithmetic boundaries, validation before effects, and writer failures.

## Effectful call composition

`tests/call_composition.rs` extends the visitation matrix with native and DSL event traces for nested argument calls, collection elements and duplicate keys, computed bases/indexes, conditions, Return operands, and recursive calls. It checks skipped resolution/effects under short-circuiting, first-error stopping, typed-argument rejection before entry, and preserved caller state. Example `18` and CLI fixtures check real stdout, malformed syntax before output, and completed effects before failure. Collection snapshot tests verify immutable storage survives replacement during an index call without copying the whole container.
