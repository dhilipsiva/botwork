# Diagnostic Metadata Conversion Limits

`RunLimits::diagnostic_values` configures one Catch metadata conversion. `core::diagnostic::DiagnosticValueLimits` defaults to ordinary ValueLimits (65,536 nodes, depth 64, 1 MiB per string, 65,536 bytes per key, 16,384 entries per container, 8 MiB aggregate payload) and 64 MiB of conservative source-position scan work. Zero budgets are valid; value depth cannot exceed 64. Other fields permit explicit increases.

## Admission and Accounting

Traverse borrowed diagnostic fields iteratively before allocating owned metadata strings, maps, or arrays. Count every metadata Literal node, every UTF-8 string and map key, and the complete formatted message/help. Formatting measurement writes into a checked byte counter. Arrays are traversed one item at a time; pending scratch grows with admitted depth, independently of array width. Small schema maps contain at most nine borrowed fields, including optional explicit omission metadata on bounded rejection summaries.

The source-position budget charges `6 * (start_byte + end_byte)` for every source-map occurrence, with checked cumulative arithmetic. This conservatively covers scanning each source prefix up to three times in both measurement and construction. Charge before scanning; repeated call, definition, related, and cause locations count separately even when their SourceFile is shared. Absent sources and empty input-origin spans cost zero scan bytes. Measurement alone reserves the same two-pass allowance. Coordinates retain the existing one-based Unicode scalar columns and exclusive endpoints as decimal strings.

Catch uses the smaller value quota in each dimension from `diagnostic_values.values` and ordinary `RunLimits::values`. After successful measurement, reserve the entire converted value in the live temporary budget, then construct it iteratively and transfer it into the temporary handler binding. Original diagnostic memory and converted metadata can overlap; the original remains available for Rethrow and causes. Variable storage admission still applies when installing the binding.

## Failures and Host APIs

Conversion failure emits BW8001 at the Catch binding, retains the original category/span/stack as a cause, preserves the previous binding, and skips handler effects. It latches the requesting Context and bypasses enclosing Catch handlers. Modules share runtime limits and stops; Context clones preserve independent stop latches. Binding-free Catch and bare Rethrow do not convert metadata. No fields are silently omitted or truncated.

`Diagnostic::value_size_with_limits` measures without copying payloads; `to_value_with_limits` measures and constructs only on success. Both borrow the original without changing its identity or adding causes to it. Checked host failures return the limit diagnostic separately. Legacy infallible `to_value` preserves full metadata and is an explicit host-managed allocation boundary. Its construction is iterative, but unadmitted deep host diagnostics/returned values still require host-controlled cleanup (`Diagnostic::discard` and `value_limits::discard`). [Checked ownership helpers](diagnostic-ownership.md) cover diagnostic cloning separately.

## Scope and Evidence

This contract bounds conversion, not initial BWErr strings, diagnostic source retention, call/cause capture, cloning, Display output, or arbitrary host allocations. Those remain separate tasks. Logical payload/node and scan metrics do not represent allocator capacity or a process-memory limit.

Unit and integration tests cover every quota, overflow, exact scan accounting, Unicode metadata, 100,000-level rejection, iterative compatibility conversion, width-independent rejection, all 25 error categories, handler preservation, native errors, raised limits, and clone stops. Allocator observations verify rejection before message/help strings, wide cause arrays, and Catch payload copies. R16 corpus cases and a Rust doctest pin the public boundary.
