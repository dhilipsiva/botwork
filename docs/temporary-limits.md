# Live Evaluation Temporaries

`RunLimits::temporaries` (`core::run::TemporaryLimits`) bounds live Literal temporaries across synchronous DSL expressions, calls, statements, and modules. Defaults allow 65,536 temporary roots, 262,144 recursive nodes, and 32 MiB of payload. All fields permit zero or explicit increases. Payload uses the existing value metrics: UTF-8 strings/map keys, four bytes per integer/float, one per boolean, and no bytes for containers or None.

## Ownership and Admission

Every owned expression/call/completion result carries a reservation. Separate roots count separately; recursive children count toward their container's nodes. Arguments remain charged while later arguments execute and throughout native callbacks. Custom parameters transfer to the retained-variable budget, and returned expressions keep their temporary reservations through control flow. A successful empty program still produces one None root/node; zero temporary roots therefore rejects it.

Check variable/access/string copies before payload allocation. Resolve signatures, arity, and call-depth rules first, then reserve all required argument slots before evaluating arguments or allocating their vector. Each pending slot costs one root/node; replace its placeholder with the actual argument reservation as evaluation proceeds. Insufficient known capacity fails before argument effects. Later size failures preserve completed required argument effects and skip subsequent arguments and callback entry.

Pre-admit collection headers, minimum child nodes, and every distinct map key's bytes before allocation or child effects. Replace placeholders as children execute, then transfer child reservations into the container without copying payloads or counting another root. Duplicate keys retain the old value until the replacement is admitted, then release its metrics and reuse the existing key allocation.

Arithmetic/unary result admission includes operand reservations until the operation finishes. Concatenation additionally admits the entire output before growing storage, requiring operand-plus-output headroom even when storage can be reused. Short-circuit evaluation returns its selected boolean directly and does not reserve skipped work. Built-in Log admits its known return copy before writing or cloning; arbitrary host native output is admitted after construction and signature/value validation.

## Cleanup and Scope

Drop values before returning their allowance. Calls, blocks, failures, returns, and cancellation release reservations through ownership. Array iteration conservatively retains the complete iterable reservation until the loop exits, including while consumed elements reside in variable storage. Conditions release their evaluated values before entering a branch/body. Discard a previous top-level result before starting the next statement; its value is no longer observable. Long-running loops with bounded live data do not accumulate temporary charges.

Map/array construction and partially evaluated calls release completed values and unused placeholders after any failure. Catch conversion preflights its complete metadata and reserves temporary storage before constructing the owned map; admission failure preserves the original diagnostic cause and earlier binding. [Diagnostic conversion](diagnostic-value-limits.md) has separate value and source-position budgets. [Original diagnostic construction and retention](retained-diagnostics.md) use their own admission budgets. All accepted values obey the fixed value-depth ceiling, and rejected native-owned data keeps iterative cleanup guards.

BW8001 latches the requesting Context and bypasses Catch. Modules share the caller's complete budget/stop state. Context clones share live temporary accounting but have independent stop latches/work counters; concurrent admission is synchronized. Engine runs start fresh trackers. Internal imported calls preserve argument/result ownership across isolated contexts.

## Boundaries and Overlap

Temporary and retained-variable budgets are independent. Assignment reserves stored copies while the evaluated result is still charged. Parameter/iterator/handler transfers can briefly charge both allowances conservatively. Owned Engine exports use the separate [result budget](result-limits.md); public Context/Pair return values transfer to host ownership and release temporary allowance. Keeping or cloning many host-returned values is the host's responsibility.

These are logical payload/node counts, including conservative construction placeholders and operation headroom. Allocator capacity/overhead, AST data, diagnostics, [formatted output](output-limits.md), and arbitrary host callback allocations are excluded. Standalone NativeOperation APIs have separate [aggregate ownership budgets](operation-ownership.md); asynchronous DSL integration remains pending. Temporary limits do not provide a process-memory ceiling or hard native deadline.

## Evidence

Reservation tests verify exact counters, atomic/overflow rejection, child merging, placeholder refunds, public transfer, shared stops, concurrency, and release. Runtime tests cover exact/zero/default quotas, repeated evaluation, collection replacement, short circuits, argument effects, custom/native/imported calls, cancellation, iterator/handler restoration, concurrent Context clones, and CLI rejection/recovery. Allocation observations pin rejection before variable, key, array, argument-vector, concatenation, and Log copies, plus child payload transfer. R15 corpus cases and a Rust doctest pin operand/output overlap.
