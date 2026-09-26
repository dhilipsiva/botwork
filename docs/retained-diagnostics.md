# Retained Calls and Handler Diagnostics

`RunLimits::retained_diagnostics` (`core::run::RetainedDiagnosticLimits`) bounds live immutable active-call and handler-error records. Defaults permit 65,536 records, 16,384 diagnostic nodes, 65,536 call frames, 65,536 related locations, 32 MiB of raw detail/context text, and 32 MiB of distinct source text/names. Every quota permits zero and explicit increases.

## Counting and Admission

An active call contributes one record, one call frame, its signature's UTF-8 bytes, and its call/definition sources. A handler contributes one record and its complete diagnostic tree, using the [diagnostic ownership metrics](diagnostic-ownership.md). Its captured call frames count separately from currently active calls because they own separate metadata. Raw error text counts occurrences even when error identities are shared. Source allocations are deduplicated by Arc identity across all live records; equal text in distinct allocations counts twice. Source bytes include full filenames and contents without scanning positions.

Admit calls after required argument evaluation and parameter preparation, before copying the signature or entering the callee. An argument's completed effects survive a later call-record rejection. Existing depth/signature checks retain their earlier order. Admit handler errors before building Catch metadata or installing the temporary binding. Measure admitted per-tree diagnostics without copying strings; reserve all aggregate dimensions and source references atomically with checked arithmetic. Failure consumes no capacity.

Quota failure returns BW8001 and latches the requesting Context, bypassing Catch. Rejected handler storage preserves the original category, bounded byte-location evidence, and explicit omissions using the independent [emergency representation](diagnostic-ownership.md#owned-admission-and-emergency-evidence). Observe cancellation/deadlines before attempting admission; a stopped handler retains its original error as bounded evidence beneath the control failure.

## Sharing and Release

Context clones and isolated module contexts share immutable call records. Context clones also share stored handler errors. Their reservations remain charged until the last record owner disappears. Source charges release after the last record referencing that allocation disappears. Separate Engine runs have independent trackers; Context clones share live capacity while keeping independent stop/work state, and module execution shares both.

[Snapshot admission](snapshot-limits.md) counts each copied call/handler handle as an entry. Infallible host Clone stays outside snapshot-work admission. Sharing payloads avoids repeating signature strings, cause trees, and source records for each snapshot; it does not bound the number of host copies.

Release calls and handlers on normal completion, return, error, and observed stop. Restore prior Catch bindings after handler failure. When an error leaves a handler, move uniquely owned metadata into the outgoing diagnostic. If a host snapshot still owns the record, copy the already admitted tree while its shared reservation stays live. Rethrow similarly creates an individual diagnostic from the admitted handler record.

## Scope and Evidence

These are logical live-record limits, separate from individual runtime-error quotas and temporary Catch-value storage. Transient/outgoing errors, initial message construction, host-retained results, standalone NativeOperation errors, allocator overhead, formatting, and output have separate contracts or pending work. These quotas establish neither a process-memory ceiling nor a hard deadline.

Unit and integration checks cover every dimension, overflow/atomicity, concurrent capacity, source identity/lifetime, snapshot sharing/handle charges, nested handlers, complete causes, rethrows, imported calls, argument effects, clone stops, fresh Engine runs, and cleanup. Allocation observations verify rejection before large signature and Catch-detail copies. R19 host corpus cases and an executed Rust example pin handler reservation behavior.
