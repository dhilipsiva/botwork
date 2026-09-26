# Diagnostic Detail Construction

Undefined-variable, undefined-statement, and native-panic errors borrow their detail text before making the initial owned copy. Signature argument/return failures measure their formatted details first. Synchronous paths use `RunLimits::diagnostics`; standalone operations use their diagnostic settings. Variable access includes direct expressions and missing bases of collection access.

## Admission Before Copying

Build a fixed-size diagnostic skeleton with an empty detail field. Measure its actual label, primary source, and complete prospective call snapshot using the [diagnostic ownership metrics](diagnostic-ownership.md). Add the borrowed detail's UTF-8 byte length with checked arithmetic. Only after all dimensions fit, copy the detail and capture call metadata. No source contents, coordinates, or formatted values are needed for this measurement.

Successful construction preserves category, exact detail text, expression/source labels, original spans, and entered-call order. Variable names remain case-sensitive; missing calls retain their exact source text, including whitespace already present in the invocation span. Each error's full detail is available to Catch when all other handler/value quotas permit it.

Rejected construction returns BW8001, latches the requesting Context, and bypasses handlers. Preserve the original category with a bounded UTF-8 detail prefix, exact source-byte evidence, prospective frame count, and explicit omission metadata. Source filenames and details use the fixed [emergency caps](diagnostic-ownership.md#owned-admission-and-emergency-evidence), even with zero quotas. A shortened detail is counted once; its full original string is never allocated by this path. Cleanup restores prior bindings and preserves completed effects.

Observe an existing stop before a construction quota can latch. Native callbacks that request cancellation and then panic keep cancellation primary; if combined stop evidence exceeds quotas, use the existing bounded stop/limit representation. Context clones retain independent stop state and share their existing live accounting.

## Operation Panics

NativeOperation applies the same pre-copy check to factory panics, future-poll panics, and blocking-worker panics. Measure normalized signature detail bytes and the native header source before allocating the panic string. A panic has already occurred when this admission runs; completed callback effects are preserved.

Keep generated, already admitted panic diagnostics separate from raw callback errors. Worker/poll handoff preserves bounded panic evidence without admitting its emergency wrapper as a new original error. Raw host-returned diagnostics always pass their own full measurement. Repeated invocations and blocking capacity remain usable after rejection; observed cancellation/timeout keeps the [operation priority rules](operation-diagnostics.md). Zero operation quotas do not latch the parent control or sibling operations.

## Formatted Signature Failures

Argument and return-kind errors first admit their source/call skeleton, then stream the diagnostic detail into a byte counter. Names, normalized signatures, argument numbers, accepted kind sets, and actual kind names are counted without allocating the message. Stop counting on quota exhaustion or checked-arithmetic overflow. After admission, allocate a buffer of the measured size; its writer also enforces that size. An error prefix uses a separate 256-byte writer and stops before later fragments after truncation, retaining valid UTF-8 and an explicit marker.

These internal formatters stream deterministic strings and scalar values; they do not format full argument collections or accept arbitrary host Display implementations. Accepted detail wording remains unchanged. All raw message wording counts toward the existing text quota, together with label/call text. This is a construction check, separate from general diagnostic Display/rendering.

Validate argument kinds after each required argument evaluates and before visiting later arguments or entering the callee. Validate return kinds after callback effects and the post-callback stop/value checks; preserve completed effects. Context construction failures latch and bypass Catch; standalone operations preserve their local failure and control rules. Both native and DSL metadata checks use this path, while unannotated DSL signatures continue to accept Any.

## Scope and Evidence

This contract covers the named borrowed details and formatted signature failures. Other formatted or multi-field errors, parser/validation/import diagnostics, and source-position formatting still need construction admission. Host-created BWErr strings already exist before runtime admission. Rendering, aggregate temporary diagnostic ownership, and output limits remain separate tasks. Active synchronous native call signatures have already passed their own retained-record admission and still own one copy.

Unit checks compare constructed and ordinary diagnostics at exact quotas and exercise prospective dimensions, invalid/zero limits, Unicode caps, source release, and omitted-frame counts. Integration checks cover normal catchability, exact contexts, handler restoration, prior effects, Pair entry, independent clone latches, all operation panic stages, repeated worker use, and cancellation with panic. Allocation observations verify zero large copies for rejected missing names and operation factory/poll signatures, plus no extra synchronous panic-detail copy beyond the admitted active-call signature. R21 host corpus cases and an executed Rust example pin byte boundaries.

Formatted-message checks cover exact raw bytes and context, counter overflow, Unicode chunk boundaries, early formatter stopping, empty messages, synchronous/async/blocking argument and return errors, required effects, skipped later arguments/callbacks, and stop priority. Four large-parameter/return allocation observations, two additional R21 cases, and a Rust example pin admission before initial message allocation.
