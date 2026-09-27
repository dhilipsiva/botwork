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

## Input Identifier and Host Value Failures

Validate input names directly against the grammar's Unicode XID start/continue rules and exact lowercase reserved words. Preserve the former parser facade's fixed 1 MiB input-name ceiling; configured name limits can reject earlier. This check borrows the complete name, performs no normalization, and creates no parser result or parser error. Keep a parity test against the grammar when either identifier definition changes.

Invalid-name messages stream origin and escaped name text through construction admission. Standalone JSON/file/flag helpers use default DiagnosticLimits for this message; Context input installation uses its local diagnostic limits, also covering host non-finite-value messages. Accepted wording remains unchanged and carries no source span. A quota rejection retains bounded Input evidence with explicit shortened-field counts.

Preserve input ordering: observe an existing stop, check each host name's length, validate the identifier, admit the value, then reject nested non-finite floats. Validate the complete input batch before changing any root binding. Ordinary input errors allow a subsequent valid installation; a Context construction quota failure latches only that Context. Rejected and cancelled deep input values still use iterative cleanup. Engine input failure precedes script effects; valid inputs require no diagnostic text allowance.

## Standalone Input Messages and Resource Origins

JSON syntax/conversion, file-read/UTF-8, malformed-flag, and raw-nesting messages use default DiagnosticLimits before formatting owned detail strings. Count origin, collection path, separators, and the parser/conversion/read reason as one message. Preserve ordinary BW7001 wording and existing bounded path previews. Oversized text returns BW8001 with a bounded Input cause and explicit truncation. Neither counting nor emergency-prefix formatting visits later fragments after its writer rejects an earlier fragment.

Input resource errors separately admit their payload-free origin before copying it into a SourceFile. Count the original resource error and label, then the origin's complete UTF-8 bytes against the default source allowance. Accepted errors retain line 1, column 1 and no input payload. Rejection preserves the original limit's category, resource name, and maximum in the cause, plus bounded filename evidence and byte offsets 0..0; it owns no SourceFile. Keep input-source/token/value checks ahead of later syntax/conversion errors.

Each standalone helper call uses fresh defaults; InputLimits does not configure diagnostic quotas. This bounds diagnostic message/source copies from an existing origin. Input buffers, host-owned origin strings, already materialized file-path labels, and underlying parser/OS error objects retain their separate ownership contracts. It does not admit general DSL source-position rendering.

## Signature Builder Errors

Unknown parameter names and duplicate/empty error-documentation failures use default DiagnosticLimits before formatting details or retaining the error's complete header source. Count the actual raw name/header wording and source label. Accepted BW1004 errors retain their exact message and original header span; oversized errors return BW8001 with bounded Signature evidence and omission metadata. Source rejection releases the consumed metadata's source owner when no other owner remains.

Builder checks are standalone and do not use or latch a Context's run quotas. Parameter matching remains exact and case-sensitive; an unknown name does not invoke the supplied kind conversion. Valid metadata construction, documentation strings supplied by the host, and registry publication keep their existing ownership/admission contracts. Native-header parsing errors remain part of the separate DSL parser construction work.

## Entry-File Read Errors

After valid run/environment setup installs the local budgets, Engine::run_file counts path/read/UTF-8 failure details before allocating their owned message. Stream the requested path and OS/read reason with the source label under RunLimits::diagnostics. Accepted BW7003 messages retain their exact wording and carry no source span; a rejected message retains bounded SourceRead evidence. Non-UTF-8 path rejection uses the same construction admission before opening the file.

Existing cancellation and source-byte checks retain priority. Input installation precedes entry-file loading, so admitted terminal snapshots retain those inputs on read failure; no script steps or callbacks have run. Rejection stops only that run, and a fresh valid file needs no diagnostic text allowance. This check does not bound the existing path join, platform file-open argument, input buffer, or error-object ownership. Working-directory preparation errors precede local budget installation and retain separate construction work.

## Scope and Evidence

This contract covers the named borrowed details, formatted signature failures, grouped collection-access fields, incompatible-operator descriptions, input identifiers, host non-finite-value errors, standalone input messages/resource origins, signature-builder validation errors, and entry-file read failures. Other formatted or multi-field errors, remaining DSL parser/validation/import diagnostics, and source-position formatting still need construction admission. Host-created BWErr strings already exist before runtime admission. Rendering, aggregate temporary diagnostic ownership, and output limits remain separate tasks. Active synchronous native call signatures have already passed their own retained-record admission and still own one copy.

Unit checks compare constructed and ordinary diagnostics at exact quotas and exercise prospective dimensions, invalid/zero limits, Unicode caps, source release, and omitted-frame counts. Integration checks cover normal catchability, exact contexts, handler restoration, prior effects, Pair entry, independent clone latches, all operation panic stages, repeated worker use, and cancellation with panic. Allocation observations verify zero large copies for rejected missing names and operation factory/poll signatures, plus no extra synchronous panic-detail copy beyond the admitted active-call signature. R21 host corpus cases and an executed Rust example pin byte boundaries.

Formatted-message checks cover exact raw bytes and context, counter overflow, Unicode chunk boundaries, early formatter stopping, empty messages, synchronous/async/blocking argument and return errors, required effects, skipped later arguments/callbacks, and stop priority. Four large-parameter/return allocation observations, two additional R21 cases, and a Rust example pin admission before initial message allocation.

Grouped-field checks cover exact metrics, late-field rejection without earlier copies, three-field Unicode truncation, source release, every collection failure reason, full call/location preservation, computed-key order, and accepted Catch metadata/restoration. Four large literal/computed-key allocation observations, two R21 cases, and a Rust example pin the path/segment boundary.

Operator checks cover exact details/context, every existing operator/kind combination, unused formatting on compatible/equality/value-check paths, unsupported raw rules, required effects and short-circuit order, independent clone latches, released temporaries, and depth-64 values during 21 entered calls. Four allocation observations include default-quota rejection of large escaped Debug descriptions. Two R21 cases and a Rust example pin the incompatible-operator byte boundary.

Input checks cover grammar parity across ASCII/Unicode boundaries, reporter invocation, exact default/local text limits, host/JSON/flag agreement, atomic installation, retry/clone isolation, prior cancellation, earlier name/value limits, and 20,000-level rejected input cleanup. Allocation observations reject large invalid names, non-finite-value details, and oversized helper origins without large parser/message copies. Two R21 cases and a Rust example pin the host input message boundary.

Standalone helper checks cover unchanged syntax/conversion/flag messages, empty and exact/default/invalid source limits, early formatting stop, Unicode filename truncation, original resource-limit priority, and source-free rejection. Allocation observations cover syntax/conversion/flag details and resource-origin copies; two R21 cases pin the default origin boundary. Existing file/UTF-8/order/CLI tests remain active.

Builder checks cover exact default text/source limits, unchanged messages/spans, Unicode details/evidence, consumed-source release, existing metadata reuse, and skipped host kind conversions on unknown names. A large-name allocation observation and two R21 corpus cases pin admission before formatting. Public tests include DSL-derived metadata from a source admitted under raised AST quotas.

Entry-file checks cover exact/zero text quotas, missing files, directory reads, invalid path/content encoding, embedded NUL paths, preserved input snapshots, skipped callbacks, fresh runs, prior cancellation, and source-limit precedence. Allocation comparison removes the large message copy on rejection while preserving required path ownership. Two R21 corpus cases pin read-error byte boundaries.
