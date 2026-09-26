# Host Diagnostic Ownership

`core::diagnostic::DiagnosticLimits` admits one borrowed tree before checked copying. Defaults permit 1,024 diagnostics, depth 32, 4,096 call frames, 4,096 related locations, 8 MiB of raw text, and 8 MiB of distinct source bytes. All fields permit zero; depth above the fixed ceiling of 64 is BW7002, while exceeding a supported quota is BW8001. Other quotas permit explicit increases.

## Counts and Copying

`limits.check(&diagnostic)` returns `DiagnosticSize`. Count the root and every cause occurrence, with root depth one. Sum frame and related-location counts across the entire tree. Text bytes include UTF-8 error detail fields, labels, call signatures, and related messages. Repeated error identities count on each occurrence even though Arc storage is shared; resource identifiers and labels count too. Fixed error wording, repair guidance, coordinates, numeric resource limits, allocator capacity, and container overhead are excluded from raw text metrics. Formatted metadata uses separate conversion limits.

Count each source's complete UTF-8 filename and contents once by SourceFile allocation identity, across primary, call, definition, related, and cause spans. Equal text in separate source allocations counts separately. Source admission measures byte lengths without computing coordinates or copying text. The iterative traversal keeps pending cause iterators proportional to depth; its source identity index grows only with owners reached under the shape quotas. Known array widths fail before visiting their payloads.

`Diagnostic::try_clone_with_limits` checks the complete tree before copying mutable metadata. Rejection returns a separate limit diagnostic and leaves the original unchanged. Accepted copies preserve error and source Arc identities, cause order, codes, labels, and locations. Frame/related strings and cause vectors are independent, so mutating a copy does not change its original. Infallible Clone performs the same iterative copy without admission and remains an explicit host-managed allocation boundary.

## Cleanup and Operations

`Diagnostic::discard(self)` walks owned causes iteratively before releasing their fields. `into_error(self)` uses the same cleanup before returning the original category; it transfers a unique root error and clones the category only when another immutable owner remains. These APIs handle arbitrarily deep host-built trees. Public fields retain their existing types and can still be moved individually. Ordinary Drop and Debug/Display of an unadmitted host diagnostic retain recursive behavior; callers managing such trees must use explicit disposal and avoid unbounded rendering.

NativeOperation guards diagnostics while they reside in runtime-owned async results and blocking-worker handles. An abandoned worker result is disposed iteratively when the worker finishes. Successful delivery transfers full diagnostic ownership to the host; cancellation cleanup preserves the returned worker error as a cause. Callback-captured state, custom future destructors, and delivered errors remain the host's responsibility. Worker cancellation remains cooperative and has no new hard deadline.

## Scope and Evidence

These are explicit host APIs and cleanup guarantees. Runtime message construction, handler/cause capture, Context snapshot accounting, aggregate diagnostic retention, and operation-result admission still require the subsequent retained-diagnostic task. Metadata conversion and text rendering have separate contracts; this helper does not establish a process-memory bound.

Tests cover exact and raised metrics, zero limits, overflow, source identity/lifetime, all error categories, independent metadata copies, 100,000-level trees, async/blocking delivery, cancellation causes, abandoned worker cleanup/capacity release, and rejection before large context/vector allocations. R17 corpus cases and a Rust doctest pin the public boundary.
