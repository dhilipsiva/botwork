# Typed Worker Protocol

`NativeOperation::isolated(signature, pool, command, protocol)` connects native signature metadata to the Linux worker supervisor. Each invocation starts one process, sends one request on stdin, closes stdin, and expects one response followed by EOF on stdout. It never retries an operation. A valid response still requires successful exit, final confirmed progress, complete I/O, verified cleanup for the selected pool mode, and no observed stop. External effects can survive cancellation or failure.

Use `WorkerProtocol::serve_once` inside a Rust worker. It reads a bounded request, validates native argument kinds and count, invokes the callback once, validates the return kind, and writes and flushes one complete response. A returned callback diagnostic is a typed response: successful delivery should exit zero. An I/O, framing, nonfinite-value, or encoding-limit failure returns an error to the worker launcher, which should exit unsuccessfully. Callback panics become BW4003 when encoding succeeds. Arbitrary callback allocations and effects inside the child remain its responsibility.

```rust
use botwork::core::{
    grammar::Literal,
    signature::{StatementSignature, ValueKind},
    worker::protocol::WorkerProtocol,
};
let protocol = WorkerProtocol::default();
let signature = StatementSignature::native("Echo |value|")?
    .parameter("value", ValueKind::Float)?.returns(ValueKind::Float);
let request = protocol.encode_request(&[Literal::Float(-0.0)])?;
let mut output = Vec::new();
protocol.serve_once(&signature, &mut request.as_slice(), &mut output, |mut values| {
    Ok(values.remove(0))
})?;
let Literal::Float(value) = protocol.decode_response(&output)?? else { panic!() };
assert_eq!(value.to_bits(), (-0.0f32).to_bits());
# Ok::<(), botwork::core::diagnostic::Diagnostic>(())
```

In an executable, pass locked stdin/stdout to `serve_once`; stdout is exclusively the protocol stream. Stderr is separately bounded by `WorkerLimits`; the typed operation does not publish it as a value, diagnostic, or log stream. Use a typed diagnostic for error evidence. Hosts needing captured stderr can use the raw `WorkerPool` API.

The following Linux host example launches a fixed-response shell fixture. Configure `WorkerCommand` with your worker executable for application use.

```rust
# #[cfg(target_os = "linux")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use botwork::core::{
    grammar::Literal,
    operation::{NativeOperation, OperationControl},
    signature::StatementSignature,
    worker::{protocol::WorkerProtocol, WorkerCommand, WorkerLimits, WorkerPool},
};
let pool = WorkerPool::new(WorkerLimits::default())?;
let response = WorkerProtocol::default().encode_response(Ok(&Literal::Int(7)))?;
let escaped: String = response.iter().map(|byte| format!("\\{byte:03o}")).collect();
let operation = NativeOperation::isolated(
    StatementSignature::native("Echo |value|")?, pool.clone(),
    WorkerCommand {
        executable: "/bin/sh".into(),
        arguments: vec!["-c".into(), format!("/bin/cat >/dev/null; printf '{escaped}'").into()],
        directory: std::env::temp_dir(),
        environment: Default::default(),
    }, WorkerProtocol::default(),
)?;
let runtime = tokio::runtime::Builder::new_current_thread().build()?;
let result = runtime.block_on(operation.invoke(vec![Literal::Int(7)], OperationControl::default()))?;
assert!(matches!(result, Literal::Int(7)));
assert!(pool.shutdown().active.is_empty());
# Ok(())
# }
# #[cfg(not(target_os = "linux"))]
# fn main() {}
```

## Version 1 wire format

All integers use little endian. Counts, byte lengths, indices, offsets, and resource-limit values are unsigned 64-bit integers; counts/offsets must fit the receiver's `usize`. Strings are a byte length followed by valid UTF-8. Booleans occupy one byte, exactly zero or one. There is no padding. Unknown versions, tags, diagnostic codes, static names, truncated fields, or trailing bytes fail with BW5003; configured quota violations use BW8001.

Every frame begins with ASCII `BWIP`, a `u16` version equal to 1, a one-byte kind, and a `u64` body byte length. The 15-byte header counts toward the frame limit. Kind 0 requests contain an argument count followed by that many values. Kind 1 responses contain exactly one value. Kind 2 responses contain exactly one diagnostic tree. No negotiation, batching, streamed partial result, or implicit JSON conversion occurs.

| Value tag | Payload |
| --- | --- |
| 0 | None; no payload |
| 1 | Signed 32-bit integer |
| 2 | IEEE-754 binary32 bits; only finite values |
| 3 | Boolean byte |
| 4 | String |
| 5 | Count followed by values in array order |
| 6 | Count followed by key-string/value pairs, strictly increasing UTF-8 key order |

Floats preserve their bits, including negative zero; integers and floats remain distinct. Maps reject duplicate and unsorted keys. The encoder sorts only after the complete frame has passed byte admission.

A diagnostic body contains these tables followed by its root node:

1. Source count, then `(name string, complete source text string)` for each source.
2. Error count, then `(u16 code, detail fields)` for each error. The code is the numeric portion of BWnnnn. Each code has one string except BW1003 `(name, original, duplicate)`, BW2003 `(signature, original, duplicate)`, BW3004 `(path, segment, reason)`, and BW6003 `(namespace, original, duplicate)`. BW8001 instead has a resource-name string followed by a `u64` limit.
3. Each node contains: error-table index; label string; optional primary span; call-frame count and frames; related-location count and locations; optional omissions; direct-cause count and child nodes recursively in order.

A span is `(source index, start byte, end byte)` with ordered, in-bounds UTF-8 boundaries. An optional field has a boolean presence byte before its payload. A call frame is `(signature string, call-site span, optional definition-site span)`. A related location is `(message string, span)`. Omissions contain four counts `(detail fields, call frames, related locations, direct causes)`, two booleans `(label omitted, prior summary)`, and an optional omitted source `(filename string, truncated boolean, start byte, end byte)`. Omitted-source offsets describe discarded evidence and cannot be checked against absent source text.

Source and error tables preserve shared owner identity within the decoded tree. Distinct source owners remain distinct even when their text is equal. Every table entry must be referenced. Labels and resource names resolve through a fixed static catalog plus the host's explicit `WorkerProtocol.labels` and `resource_names` additions; received strings are never leaked or interned into process-lifetime storage. Both peers must configure the same additions. Labels `source` and `expression`, and current built-in resource names, are supported by default. Diagnostic categories and their detail fields retain the [existing diagnostic contract](diagnostics.md).

## Admission and ownership

| ProtocolLimits field | Default |
| --- | --- |
| `frame_bytes` | 1 MiB, including header |
| `arguments` | 1,024 |
| `argument_nodes` | 262,144 across all arguments |
| `argument_payload_bytes` | 8 MiB across all arguments |
| `sources` | 1,024 source-table entries |
| `in_flight_bytes` | 32 MiB across clones of an isolated operation |
| `values` | Default `ValueLimits`, at most 64 levels |
| `diagnostics` | Default `DiagnosticLimits`, at most 64 levels |

Zero quotas reject nonempty consumption; configuration retains the existing value/diagnostic depth ceilings. Byte arithmetic is checked. Standalone encode/decode/stream helpers apply individual protocol limits; their returned buffers and values belong to the host. `in_flight_bytes` applies through the isolated-operation bridge.

The bridge admits arguments through `OperationBudget` when the invocation future is created. Before encoding a request, it reserves its exact encoded size plus the pool's entire stdout and stderr allowances. This conservative shared reservation covers simultaneous request/output ownership, even if a worker sends fewer bytes. Pool request and protocol frame quotas both apply. Operation clones share this wire quota; independently constructed operations have separate wire quotas and may share a pool or `OperationBudget` explicitly. `isolated_in_flight_bytes()` exposes the current reservation.

The response is scanned completely before any value strings, diagnostic details, or source texts are copied. Result values obey both protocol and operation limits, including finite nested numbers, and their top-level signature kind is checked before construction. Shared result ownership is reserved before decoding. Diagnostics validate tables, source ranges, tree shape, text/source totals, and their prospective native header before shared ownership reservation and construction. Source-free errors receive that header unless they are emergency evidence. Original worker spans, calls, related locations, causes, and omissions are preserved.

A structurally valid diagnostic that exceeds the operation's local or shared budget becomes BW8001 with bounded original-category evidence: at most 256 bytes per detail or filename plus omission counts and byte offsets, retaining no original source owner. A malformed frame or a frame exceeding protocol scan limits is rejected before it establishes a valid foreign diagnostic. Host-shaped emergency trees still undergo protocol and local ownership checks.

Reservations survive unpolled completed responses, future abandonment, runtime shutdown, and pending startup or cleanup. A stalled launch or post-launch OS call returns an unsuccessful report with pending pool ownership. An independent observer retains the slot and reservations until the OS owner settles; late direct-child cleanup cannot replace a published stop with success. Supervisor, request payload, response payload, and quarantined active-slot owners retain the relevant guards. Encoded buffers are destroyed before their byte reservation is refunded. An unverified slot remains charged while the pool retains it; dropping the pool abandons its reconciliation state, without claiming cleanup succeeded. Returned public values/diagnostics transfer to the host and release operation charges.

Counters measure logical payload bytes/nodes rather than allocator capacity, container overhead, decoder scratch tables, host command metadata, or child memory. Scratch structures are bounded by frame and structural quotas. Literal executable/argument/environment configuration is trusted host data. This protocol does not authenticate workers or sandbox their effects.

## Stops and evidence

Cancellation and deadlines are enforced by the supervisor independently of async polling; the operation also checks its inherited control before entry and before publishing a result. A timeout observed before cleanup remains a timeout. A valid success frame cannot override cancellation, nonzero exit, incomplete progress, incomplete I/O, or pending/unverified cleanup. A complete valid diagnostic received before a transport failure remains bounded cause evidence when the operation budgets permit it. Malformed or partial output cannot become a success.

A pool created with `WorkerPool::with_process_tree` uses the same typed bridge and wire format. Successful transport then requires `TreeReaped`, including detached descendants; pending guardian cleanup retains the same argument/wire reservations. See [guardian configuration and limits](isolated-workers.md#process-tree-guardians).

Codec and SDK checks run on every platform. Execution through `NativeOperation::isolated` currently requires Linux and follows all [worker lifecycle limits](isolated-workers.md), including the remaining stuck-kernel, detached-descendant, host-crash, and durable-recovery work. Async DSL dispatch and whole-run shutdown remain separate roadmap items.

Unit tests cover exact values, every diagnostic category, shared identities, malformed frames/references, quotas, SDK signatures, and stop-safe construction. Independent Python subprocess fixtures exercise interoperability, ownership, result validation, cancellation, deadlines, and abandoned/unpolled responses. Allocation observations verify that rejected frame/value/source payloads are not copied; R26 corpus cases and Rust documentation examples pin the public codec contract.
