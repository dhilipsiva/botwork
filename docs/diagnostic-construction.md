# Diagnostic Detail Construction

Undefined-variable, undefined-statement, and native-panic errors borrow their detail text before making the initial owned copy. Signature, collection-access, and incompatible-operator failures measure their formatted details first. Synchronous paths use `RunLimits::diagnostics`; standalone operations use their diagnostic settings. Variable access includes direct expressions and missing bases of collection access.

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

## Collection Access Failures

Admit path, failing segment, and reason as a complete group before allocating any of the three strings. Add their combined raw UTF-8 bytes to the prospective source/label/call metrics. Measure every field first; a later field exceeding the remaining allowance prevents initial copies of earlier fields too. Each rejected field gets an independent 256-byte emergency prefix, and each shortened field contributes exactly one omission count.

Stream path text from the original base span and each literal/computed segment, preserving the existing full-path spelling. Static reasons and the array-length reason use borrowed formatting arguments. Keep the failing segment's span and expression label. Accepted Catch metadata exposes unchanged path, segment, and reason values.

Evaluate the current computed key before testing its receiver/key type or reporting a missing element. A failed lookup stops before later keys; including their source spelling in the error path does not execute them. Quota rejection latches the Context and bypasses Catch while preserving completed key effects and normal binding/frame cleanup.

## Incompatible Operators

After value admission and compatibility checks, stream incompatible unary/binary operand Debug descriptions through the same counter before allocating the detail message. This includes escaped-string expansion: count the bytes actually formatted, not just input payload length. Operands already satisfy the fixed value-depth ceiling; counters and prefix writers avoid intermediate message buffers. Preserve accepted detail wording, operator names, expression spans, and call context. Map Debug ordering retains its existing behavior.

Compatible operations and structural equality construct no incompatible-operand diagnostic. Preserve arithmetic/finiteness validation and concatenation/value admission order. Ordinary binary operands finish left then right before an incompatible-type failure; unary operators evaluate their operand once. Logical operators retain short-circuit behavior and reject a non-boolean left operand before evaluating the right. Their short-circuit type-error messages also pass construction admission.

Operand temporary reservations remain live during formatting and release on failure. Runtime rejection latches the Context and retains bounded incompatible-type/source evidence. Public `Operate` and `operate_*_bounded` preserve their legacy LiteralResult signatures and use default DiagnosticLimits for incompatible descriptions; an over-budget description returns BW8001. Legacy results expose the primary BWErr only, so detailed omitted-cause evidence remains available through Context/Engine detailed execution. Explicit value limits on raw operators do not change this default diagnostic quota.

## Scope and Evidence

This contract covers the named borrowed details, formatted signature failures, grouped collection-access fields, and incompatible-operator descriptions. Other formatted or multi-field errors, parser/validation/import diagnostics, and source-position formatting still need construction admission. Host-created BWErr strings already exist before runtime admission. Rendering, aggregate temporary diagnostic ownership, and output limits remain separate tasks. Active synchronous native call signatures have already passed their own retained-record admission and still own one copy.

Unit checks compare constructed and ordinary diagnostics at exact quotas and exercise prospective dimensions, invalid/zero limits, Unicode caps, source release, and omitted-frame counts. Integration checks cover normal catchability, exact contexts, handler restoration, prior effects, Pair entry, independent clone latches, all operation panic stages, repeated worker use, and cancellation with panic. Allocation observations verify zero large copies for rejected missing names and operation factory/poll signatures, plus no extra synchronous panic-detail copy beyond the admitted active-call signature. R21 host corpus cases and an executed Rust example pin byte boundaries.

Formatted-message checks cover exact raw bytes and context, counter overflow, Unicode chunk boundaries, early formatter stopping, empty messages, synchronous/async/blocking argument and return errors, required effects, skipped later arguments/callbacks, and stop priority. Four large-parameter/return allocation observations, two additional R21 cases, and a Rust example pin admission before initial message allocation.

Grouped-field checks cover exact metrics, late-field rejection without earlier copies, three-field Unicode truncation, source release, every collection failure reason, full call/location preservation, computed-key order, and accepted Catch metadata/restoration. Four large literal/computed-key allocation observations, two R21 cases, and a Rust example pin the path/segment boundary.

Operator checks cover exact details/context, every existing operator/kind combination, unused formatting on compatible/equality/value-check paths, unsupported raw rules, required effects and short-circuit order, independent clone latches, released temporaries, and depth-64 values during 21 entered calls. Four allocation observations include default-quota rejection of large escaped Debug descriptions. Two R21 cases and a Rust example pin the incompatible-operator byte boundary.
