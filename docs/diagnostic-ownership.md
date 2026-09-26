# Host Diagnostic Ownership

`core::diagnostic::DiagnosticLimits` admits one borrowed tree before checked copying. Defaults permit 1,024 diagnostics, depth 32, 4,096 call frames, 4,096 related locations, 8 MiB of raw text, and 8 MiB of distinct source bytes. All fields permit zero; depth above the fixed ceiling of 64 is BW7002, while exceeding a supported quota is BW8001. Other quotas permit explicit increases.

## Counts and Copying

`limits.check(&diagnostic)` returns `DiagnosticSize`. Count the root and every cause occurrence, with root depth one. Sum frame and related-location counts across the entire tree. Text bytes include UTF-8 error detail fields, labels, call signatures, and related messages. Repeated error identities count on each occurrence even though Arc storage is shared; resource identifiers and labels count too. Fixed error wording, repair guidance, coordinates, numeric resource limits, allocator capacity, and container overhead are excluded from raw text metrics. Formatted metadata uses separate conversion limits.

Count each source's complete UTF-8 filename and contents once by SourceFile allocation identity, across primary, call, definition, related, and cause spans. Equal text in separate source allocations counts separately. Source admission measures byte lengths without computing coordinates or copying text. The iterative traversal keeps pending cause iterators proportional to depth; its source identity index grows only with owners reached under the shape quotas. Known array widths fail before visiting their payloads.

`Diagnostic::try_clone_with_limits` checks the complete tree before copying mutable metadata. Rejection returns a separate limit diagnostic and leaves the original unchanged. Accepted copies preserve error and source Arc identities, cause order, codes, labels, and locations. Frame/related strings and cause vectors are independent, so mutating a copy does not change its original. Infallible Clone performs the same iterative copy without admission and remains an explicit host-managed allocation boundary.

## Owned Admission and Emergency Evidence

`limits.admit(diagnostic)` moves an accepted diagnostic back unchanged, including allocation identity and complete metadata. On rejection it disposes the original tree iteratively and returns the BW8001 quota failure (or BW7002 invalid configuration) with one bounded original-category summary as its cause. This explicit host operation does not latch a Context. [Synchronous runtime admission](runtime-diagnostics.md) applies the same checks through separate RunLimits settings.

The emergency representation uses a fixed independent allowance: two diagnostics at depth two, no retained SourceFile owners or call/related context, at most three 256-byte detail fields and one 256-byte filename, plus fixed violation/label text and omission counters. It remains available even when all input quotas are zero. Copy only bounded UTF-8 prefixes and mark shortened strings with `…[truncated]`; oversized static resource identifiers use an explicit replacement. Preserve numeric resource limits exactly. Successful admission never truncates.

The summary keeps the original primary code and category. Its optional boxed `Diagnostic::omissions` records shortened detail-field count, omitted call/related counts, omitted direct causes, custom-label loss, and whether an earlier omission summary was discarded. Source evidence contains a bounded filename, a filename-shortened flag, and exact original byte offsets without copying source contents or scanning coordinates. It deliberately has no primary Span: its filename/offset evidence is not a replacement source file. Descendant counts and metadata are omitted without traversing them merely to produce totals. Deep disposal reuses finished traversal slots along single-child chains.

Emergency diagnostics remain bounded when later context is offered: at/at_expression and call capture add no source owners or strings; related sites and handled causes increment omission counts instead of retaining their payload. A previously observed runtime cancellation/timeout keeps its original-category summary primary and adds the quota failure as its single cause.

Full metadata keeps the existing eight keys. A summary adds an optional ninth `omissions` map; counts/offsets are decimal strings and flags are booleans. Its source entry is None or `{file, file_truncated, start_byte, end_byte}`. Plain diagnostic text explicitly prints omissions and byte evidence. Rejection does not retain the original error Arc; a small independent category copy prevents oversized payload retention. Adding the optional public field leaves existing field types/moves intact; hosts constructing struct literals must include it or use `Diagnostic::new`.

The optional metadata is boxed: Diagnostic occupies 128 bytes on this 64-bit host while keeping existing field types and return signatures. `clippy.toml` sets the large-error threshold to 129 bytes, allowing this representation and warning on further growth. Combined runtime/parser stack stress tests still pass.

## Cleanup and Operations

`Diagnostic::discard(self)` walks owned causes iteratively before releasing their fields. `into_error(self)` uses the same cleanup before returning the original category; it transfers a unique root error and clones the category only when another immutable owner remains. These APIs handle arbitrarily deep host-built trees. Public fields retain their existing types and can still be moved individually. Ordinary Drop and Debug/Display of an unadmitted host diagnostic retain recursive behavior; callers managing such trees must use explicit disposal and avoid unbounded rendering.

NativeOperation [admits callback and cleanup diagnostics](operation-diagnostics.md) before worker handoff and final publication. Accepted errors retain full ownership/context; rejected errors become bounded evidence after iterative disposal. Guards dispose abandoned worker errors iteratively when the worker finishes. Cancellation cleanup retains admitted worker evidence when quotas permit. Callback-captured state, custom future destructors, and host-retained results remain the host's responsibility. Worker cancellation remains cooperative and has no new hard deadline.

## Scope and Evidence

These are explicit host APIs and cleanup guarantees. Individual synchronous errors use [runtime admission](runtime-diagnostics.md); [retained records](retained-diagnostics.md) account for shared calls/handlers and their source owners. Operations have [individual diagnostic admission](operation-diagnostics.md). Initial message construction, aggregate operation/host result ownership, and rendering retain separate tasks. Metadata conversion has its own contract; these helpers do not establish a process-memory bound.

Tests cover owned admission/rejection, fixed UTF-8 emergency caps, optional omission metadata, original codes/source offsets, exact and raised metrics, zero limits, overflow, source identity/lifetime, all error categories, independent metadata copies, 100,000-level trees, async/blocking delivery, cancellation causes, abandoned worker cleanup/capacity release, and rejection before large context/vector allocations. R17 corpus cases and a Rust doctest pin the public boundary.
