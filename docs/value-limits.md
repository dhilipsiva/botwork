# Value Admission and Cleanup

`RunLimits::values` configures per-value budgets for Engine and Context. NativeOperation uses the same defaults; `with_value_limits(limits)` changes one operation. Public Rust operators use defaults, or accept explicit limits through `Rule::operate_binary_bounded` and `operate_unary_bounded`.

## Budgets

| ValueLimits field | Default | Count |
| --- | --- | --- |
| `nodes` | 65,536 | Every Literal, including the root and containers; map keys are byte payload, not nodes |
| `depth` | 64, fixed ceiling | Root depth one; every contained value adds one |
| `string_bytes` | 1 MiB | UTF-8 bytes in each string |
| `key_bytes` | 65,536 | UTF-8 bytes in each map key |
| `entries` | 16,384 | Elements/entries in each array/map |
| `payload_bytes` | 8 MiB | String/key bytes plus four bytes per Int/Float and one per Bool; None/containers add zero |

Payload counts exclude allocator/container overhead and capacity. Zero payload permits empty strings/containers and None; zero nodes/depth permits no explicit value. Default implicit empty-program completion still returns None. Only depth has a fixed ceiling; other budgets may be raised explicitly. Invalid depth configuration returns BW7002, and exceeded budgets return BW8001.

`ValueLimits::check(&value)` returns `ValueSize` without taking ownership. It walks iteratively with array/map cursors, so pending traversal space grows with depth rather than width. Check container width before visiting its children and use checked byte arithmetic. This checks resource shape; existing finite-number and signature-kind validation remains separate.

## Admission Boundaries

Validate every root input before installing any binding or running the script. Check native arguments through evaluated expressions and validate native results before return-kind checking/publication. Public operators check operands and results. Async/blocking operations reject invalid arguments before constructing callbacks and check returned values before publication. Expression results and generated Catch bindings are also checked; a rejected Catch binding preserves the original error as a cause and leaves the previous binding intact.

Actual Context/Engine value-budget failures latch the run and bypass Catch. Stop requests observed after callbacks take priority over admitting their results; preserve existing failure causes. Cleanup restores interpreter frames and temporary bindings. Standalone NativeOperation failures return their structured category without mutating unrelated controls or operations.

## Owned Cleanup

Engine/Context input maps, raw operator operands, native results, and operation argument/results use ownership guards that destroy nested arrays/maps iteratively on rejection, cancellation, early configuration errors, or dropped invocations. The operation guard is created before its returned future is first polled. Started blocking results retain the guard through draining or host drop. Accepted values have bounded depth before recursive cloning/formatting or ordinary destruction.

Rust callers checking borrowed data retain ownership. Use `core::value_limits::discard(value)` to release an arbitrarily deep rejected value safely. Cleanup work remains proportional to already supplied data. Arbitrary callback/future captures and direct host calls to Literal's Clone/Debug/Display remain the host's responsibility.

## Remaining Allocation Work

Admission cannot undo allocations already made by a host, parser, JSON converter, expression constructor, or diagnostic conversion. Current expression/operator result checks run after constructing those results. Preallocation checks for concatenation, container construction, copies, and input decoding are the next task. Aggregate binding/argument/temporary state, cache/result snapshots, diagnostic size, and serialization remain separate bounds. These per-value budgets are not a process-memory sandbox.

Tests cover exact metrics, every bound, Unicode, 100,000-level host values, rejected and cancelled owned inputs/results, unpolled operations, blocking cleanup, recursive calls with accepted deep values, persistent stops, and Catch/iterator restoration. R6 corpus cases pin exact root input admission and failure before effects.
