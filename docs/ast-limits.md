# Owned Syntax Admission

`Program` exposes its statement list for host composition. Every public validation/execution path now checks the complete reachable tree before recursive control validation or script effects. Changing `Program::source`, replacing a statement span, or repeating statements cannot bypass these checks.

## Budgets and Counting

`RunLimits::ast` contains `AstLimits`:

| Field | Default | Counting rule |
| --- | --- | --- |
| `nodes` | 65,536 | Each statement, expression, block, call, definition, name, access segment, definition header, and operator location |
| `depth` | 128, fixed ceiling | Root statements/standalone expressions/blocks start at one; each owned child adds one |
| `source_bytes` | 8 MiB | Sum of text lengths across distinct reachable SourceFile allocations, including Program's root source |

Each source must also fit `RunLimits::source_bytes` (1 MiB by default). Source names are not part of this byte count. Shared source allocations count once; separate allocations with identical text count separately. Shared definition bodies count nodes for every occurrence, since each occurrence would otherwise require validation. Slice containers add no node/depth themselves. Map keys count as names; computed access counts both its segment and index expression. For example, `|x| = |1|` has three nodes and depth two; `|x| = |1+2|` has six nodes and depth three.

An iterative walk uses slice cursors to visit one child at a time. Its pending work grows with depth, rather than input width. It retains source identities only for the duration of that check. Resource failures return BW8001, latch an active Context/run budget, and precede all entry/module effects. Diagnostics identify the first excessive node where a span exists; an excessive root source can fail without a span. Invalid depth configuration returns BW7002. Admission does not consume execution steps.

## Rust Entry Points

`Program::parse`/`parse_detailed`/`parse_bounded` apply default tree budgets after lowering and before control validation. `parse_with_budgets(name, source, source_bytes, &syntax_limits, &ast_limits)` accepts explicit budgets. `validate_detailed()` uses defaults; `validate_with_limits(&ast_limits, per_source_bytes)` checks already owned input without reparsing.

Engine and Context use their configured tree budgets for program, statement, parser-pair, and imported-module admission. Limits apply per admitted input tree, including unreachable branches. Repeated evaluations retain runtime step counters but start a new tree check. The CLI applies defaults before output or debug traces. Syntax limits govern parsing; tree depth counts owned components and is a separate limit.

## Scope

These checks bound admission work and accepted tree structure. They do not prevent allocations already made by a Rust host or during parser lowering, and do not take ownership of rejected host trees. A host constructing arbitrarily deep rejected trees must also arrange their safe destruction. [Import budgets](import-limits.md) separately bound module-cache/export admission. Aggregate retained definitions/sources across successive evaluations, values, cache snapshots, and temporary allocations remain resource tasks.

Tests cover exact counts, zero/invalid configurations, shared and hidden source owners, 10,000 nested host expressions, 1,000 nested host control blocks, wide repeated statements, explicit larger budgets, persistent stop behavior, imported failures, and CLI rejection before effects. R4 host corpus cases pin exact admission and pre-execution failure.
