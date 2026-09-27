# Bounded Diagnostic Rendering

Diagnostic Display, including structured CLI failures, uses default DiagnosticRenderLimits. `diagnostic.render_with_limits(&limits)` returns `RenderedDiagnostic { text, truncation }` under caller-selected limits. Rendering borrows the error; it does not alter categories, causes, source ownership, execution outcomes, or Context stop state. Rendering limits are independent of runtime diagnostic ownership limits.

## Limits and Complete Output

| Field | Default | Meaning |
| --- | ---: | --- |
| output_bytes | 64 KiB | UTF-8 bytes for the complete diagnostic, including causes and help |
| diagnostics | 128 | Root plus all visited cause records |
| depth | 64 | Cause depth, with the root at one |
| call_frames | 256 | Entered frames across all rendered records |
| related_locations | 256 | Related sites across all rendered records |
| source_scan_bytes | 64 MiB | Conservative source scanning across measurement and emission |

Every field accepts zero or explicit increases. Checked arithmetic treats overflow as exceeding the corresponding allowance. There is no recursive rendering call: a stack of slice cursors follows cause order and grows only with entered depth, not cause width. A complete first pass measures output into a counter before allocating or writing the final text. Owned rendering allocates one exactly sized final buffer; Display streams the admitted second pass directly to its destination. Neither path constructs separate location or help strings.

Admitted output preserves the existing spelling, order, source ranges, excerpts, labels, related sites, call/definition locations, omission metadata, help, and `while handling` sections. Coordinates remain one-based Unicode scalar positions with exclusive ends, tabs occupying one column, and CRLF treated as one line ending. Source excerpts are trimmed with the same Unicode whitespace rules.

Before visiting a primary span's coordinates or trimming its excerpt, charge `6 * (start + end) + 4 * (end - start)`. When start equals end, omit the end-coordinate charge. Each related/call/definition location charges `6 * start`. These conservative counts cover both passes, including each occurrence of a shared source; source owners are not deduplicated for scan work. Empty origins at byte zero need no source-scan allowance. Length/overflow checks run before coordinate scans or slicing an excerpt.

## Explicit Truncation

When any allowance is insufficient, emit a bounded summary instead of a partial full rendering. `truncation` identifies the resource and configured limit; default Display prints the same information. The primary error keeps its original code and a UTF-8-safe message preview. Follow the first-cause chain for at most eight records, keeping each code and preview, and report skipped direct causes, call frames, and related locations. Do not traverse omitted subtrees to count their contents.

Source evidence uses a bounded filename and exact byte offsets. It explicitly omits coordinates and excerpts, with no source scanning. Preserve earlier filename-truncation flags and diagnostic omission counts. Message/filename previews are at most 256 bytes each and shortened previews carry a marker. Repair guidance and other omitted fields are named explicitly. The original diagnostic remains available for another rendering with larger limits or structured inspection.

Summaries have the independent `RENDER_SUMMARY_BYTES` allowance of 16 KiB, even when requested output_bytes is zero. Thus one rendering emits at most `max(output_bytes, RENDER_SUMMARY_BYTES)` bytes. This fixed exception ensures a limit cannot hide the primary code or the fact that output was truncated. It is separate from runtime emergency diagnostic admission and does not turn the execution error into BW8001.

`Diagnostic::help()` and `BWErr::help()` use the same default output-byte allowance. Their `help_with_limit(output_bytes)` variants return RenderedDiagnostic with complete guidance or an explicitly shortened preview and the original code. Diagnostic-to-value conversion streams full guidance through its own conversion limits, independently of these text APIs.

## Boundaries and Evidence

Rendering retains no source or error owners in its returned text. A failing fmt destination receives fmt::Error; already written output cannot be undone. The CLI still exits with failure and preserves preceding program output; its trailing newline lies outside the diagnostic byte count. CLI loading-error wrappers, debug traces, statement help/listing, serialization, and general output use the separate [output admission contract](output-limits.md). Direct BWErr Display, Span coordinate/location helpers, Debug, and explicit host metadata conversion remain host primitives; bounded Diagnostic rendering does not call allocating location/help helpers. Host-supplied fields already exist before this traversal, and ordinary Drop of a deep host diagnostic still requires the documented explicit disposal API.

Seven renderer unit tests cover frozen-layout parity, every exact/one-less quota, zero allowances, overflow, Unicode previews, empty/whitespace spans, retained omission evidence, destination failures, bounded source work, and nonrecursive deep/wide traversal. Five integration checks cover source/identity preservation, Context reuse, all UTF-8 byte boundaries, shared-source work, default limits, and an oversized CLI failure with prior output and cause codes. The 25-category matrix checks original codes after rendering rejection. Two allocation observations verify one admitted final buffer and no large filename/detail/help buffers on rejection. R23 corpus cases and an executed Rust example pin exact output admission and code-preserving truncation.
