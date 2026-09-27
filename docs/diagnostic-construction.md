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

## Worker Join Failures

Blocking-worker join errors stream the task identifier and escaped panic payload through operation diagnostic admission before constructing the AsyncRuntime detail. Include the native header source. Accepted messages retain their exact wording; rejected unexpected failures retain a bounded AsyncRuntime prefix and byte evidence without retaining the source.

During cancellation or timeout cleanup, measure the already observed primary stop together with the prospective join-error cause before formatting that cause. A cause that fits alone can still exceed the combined text, depth, or record allowance. Rejection preserves bounded stop evidence with a resource-limit cause and counts the omitted cleanup cause; it does not allocate that cause's full message or a discarded prefix. Final publication preserves this emergency representation without another summary.

Worker completion releases its capacity even after join failure. Preserve the observed timeout before requesting cooperative cleanup cancellation, completed effects, and parent/sibling isolation. Tokio already owns the join error and panic payload; those buffers and host destructors retain their separate boundaries. Admission bounds the interpreter's additional message construction.

## Log Output Failures

Log admits the output error's displayed reason with its current call site, entered-call snapshot, and source ownership before constructing the owned Output detail. Accepted BW4001 messages retain the I/O error's exact wording and remain catchable. Rejection latches only the requesting Context, bypasses Catch, and keeps bounded Output/category/byte evidence with explicit omissions. Internally preserve the structured error through the native callback boundary; public callback signatures and host-error admission remain unchanged.

Writes occur before an output error is available. Preserve bytes already written, completed argument effects, and an observed cancellation or deadline; do not retry the write. Unwind calls, handler bindings, and temporary result reservations normally. Successful output needs no diagnostic allowance. Stdout/platform error objects and general value/output rendering keep their separate ownership and resource contracts.

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

## Numeric Conversion and Arithmetic

Integer/float conversion failures and arithmetic errors use the same construction admission with the complete expression span and entered calls. Arithmetic helpers return a small deferred reason until those local quotas are known; integer overflow, non-finite results/operands, zero divisors, and zero with a negative exponent construct no detail string beforehand. Accepted text and categories remain unchanged. Raw operator APIs and standalone host-value validation apply default diagnostic quotas and retain their BWErr results.

Preserve compatibility and value checks, complete-operand validation for equality, integer range and power behavior, float rounding/underflow, operand order, and short-circuiting. A conversion failure keeps its numeric-literal category; a parsed non-finite float keeps its arithmetic category. Rejected construction latches the requesting Context and preserves bounded original-category/source-byte evidence. Catch still handles admitted numeric errors; prior cancellation and completed effects retain priority. Numeric parsing and the signed-literal parsing buffer keep their existing source/value bounds, separate from error-detail construction.

## Input Identifier and Host Value Failures

Validate input names directly against the grammar's Unicode XID start/continue rules and exact lowercase reserved words. Preserve the former parser facade's fixed 1 MiB input-name ceiling; configured name limits can reject earlier. This check borrows the complete name, performs no normalization, and creates no parser result or parser error. Keep a parity test against the grammar when either identifier definition changes.

Invalid-name messages stream origin and escaped name text through construction admission. Standalone JSON/file/flag helpers use default DiagnosticLimits for this message; Context input installation uses its local diagnostic limits, also covering host non-finite-value messages. Accepted wording remains unchanged and carries no source span. A quota rejection retains bounded Input evidence with explicit shortened-field counts.

Preserve input ordering: observe an existing stop, check each host name's length, validate the identifier, admit the value, then reject nested non-finite floats. Validate the complete input batch before changing any root binding. Ordinary input errors allow a subsequent valid installation; a Context construction quota failure latches only that Context. Rejected and cancelled deep input values still use iterative cleanup. Engine input failure precedes script effects; valid inputs require no diagnostic text allowance.

## Standalone Input Messages and Resource Origins

JSON syntax/conversion, file-read/UTF-8, malformed-flag, and raw-nesting messages use default DiagnosticLimits before formatting owned detail strings. Count origin, collection path, separators, and the parser/conversion/read reason as one message. Preserve ordinary BW7001 wording and existing bounded path previews. Oversized text returns BW8001 with a bounded Input cause and explicit truncation. Neither counting nor emergency-prefix formatting visits later fragments after its writer rejects an earlier fragment.

Input resource errors separately admit their payload-free origin before copying it into a SourceFile. Count the original resource error and label, then stream the origin's complete displayed UTF-8 bytes against the default source allowance. File paths and flag indexes stay borrowed until diagnostic construction; no intermediate filename or flag-label string is created. Preserve native paths' existing lossy display spelling and count replacement characters by their displayed UTF-8 bytes. Accepted origins move their single final buffer into the source owner, retaining line 1, column 1 and no input payload. Rejection preserves the original limit's category, resource name, and maximum in the cause, plus bounded filename evidence and byte offsets 0..0; it owns no SourceFile. Keep input-source/token/value checks ahead of later syntax/conversion errors.

Each standalone helper call uses fresh defaults; InputLimits does not configure diagnostic quotas. This bounds diagnostic message/source copies from existing origin strings or borrowed native paths. Input buffers, host-owned paths/origins, platform file-open argument buffers, and underlying parser/OS error objects retain their separate ownership contracts. It does not admit general DSL source-position rendering.

## Signature Builder Errors

Unknown parameter names and duplicate/empty error-documentation failures use default DiagnosticLimits before formatting details or retaining the error's complete header source. Count the actual raw name/header wording and source label. Accepted BW1004 errors retain their exact message and original header span; oversized errors return BW8001 with bounded Signature evidence and omission metadata. Source rejection releases the consumed metadata's source owner when no other owner remains.

Builder checks are standalone and do not use or latch a Context's run quotas. Parameter matching remains exact and case-sensitive; an unknown name does not invoke the supplied kind conversion. Valid metadata construction, documentation strings supplied by the host, and registry publication keep their existing ownership/admission contracts. Native-header parsing errors remain part of the separate DSL parser construction work.

## Entry-File Read Errors

After valid run/environment setup installs the local budgets, Engine::run_file counts path/read/UTF-8 failure details before allocating their owned message. Stream the requested path and OS/read reason with the source label under RunLimits::diagnostics. Accepted BW7003 messages retain their exact wording and carry no source span; a rejected message retains bounded SourceRead evidence. Non-UTF-8 path rejection uses the same construction admission before opening the file.

Existing cancellation and source-byte checks retain priority. Input installation precedes entry-file loading, so admitted terminal snapshots retain those inputs on read failure; no script steps or callbacks have run. Rejection stops only that run, and a fresh valid file needs no diagnostic text allowance. This check does not bound the existing path join, platform file-open argument, input buffer, or error-object ownership. Working-directory preparation errors precede local budget installation and use the default construction contract below.

## Import Read and Cycle Errors

Import path-policy, working-directory, canonicalization, file-read/UTF-8, and cycle messages use local diagnostic construction admission. Include the failing path span, the originating `imported here` location, and the current call snapshot before formatting the detail. Count the related label and complete distinct source ownership too; a message that fits without its known import site still rejects before its first owned copy.

Context captures a failed current-directory lookup as a shared I/O error without formatting it. Host and module snapshots share this owner; checked snapshots charge its copied handle. Unrelated execution never renders the reason. A relative import that requires the directory streams it through local diagnostic admission, preserving the original I/O wording or a bounded prefix. The owner releases after the final Context copy is dropped.

Cycle details stream the active canonical path slice and repeated path in their original order, separated by ` -> `. Each path uses its existing display representation. No vector of formatted paths or joined intermediate chain is constructed. Accepted errors retain exact ImportRead/ImportCycle wording, path locations, call frames, and originating import context.

Other load failures acquire the originating site once at the load boundary. Parent import sites are attached and admitted as errors unwind; they are separate from the initial constructor's known site. Emergency errors increment omitted-location counts instead of retaining further sources. Preserve resource/parse/execution categories, completed effects, successful dependency caches, and namespace publication only after successful initialization. Construction quota failures share the module run's stop state, bypass Catch, and retain normal scope/handler cleanup.

Path joins, canonicalization/platform buffers, existing source/path owners, and module parser/validation diagnostics keep their separate ownership or construction contracts. Streaming a path into an error does not change import resolution or file access.

## Runtime Declaration Collisions

Duplicate statement and namespace errors admit the primary and original declaration sources, related label, and current call snapshot before scanning source coordinates. Then measure the name and both location fields as a group before copying any message field. Native statement origins retain their existing source-name-only spelling; DSL and imported origins retain their exact file/line/column spelling. Source ownership bounds the text visited by coordinate measurement; accepted locations are scanned again when copied into their admitted buffers.

On rejection, replace coordinate fields with bounded `file:[byte N; coordinates omitted]` evidence without scanning source text. Each replaced or truncated field contributes one omission, even if both apply. Native origin names and collision names keep their ordinary spelling unless truncated. The emergency cause retains the original collision code and records omitted related locations and calls without retaining either declaration source.

Check collisions before registry replacement or conflicting import path validation/loading. Preserve completed effects, existing definitions/namespaces, previous Catch bindings, and sibling Context usability; quota rejection latches only the requesting Context and bypasses handlers. Native registration follows the same contract.

## Control and Parameter Validation

Control-placement and duplicate-parameter checks return borrowed validation evidence. Program parsing/validation and native-signature builders use default diagnostic quotas to construct these failures. Context/Engine parsing, owned-program and statement execution, imported modules, and Pair compatibility entry use local quotas and the current call snapshot. Preserve the original accepted detail text, Unicode coordinates, spans, and first-parameter related location.

Admit known source/call/related context before scanning coordinates, then measure all fields before allocating any owned detail. On rejection, control messages substitute a byte location; duplicate-parameter errors substitute both coordinate fields. Mark each replacement or truncation once and release source ownership from the emergency error. Standalone helpers do not latch a Context; runtime construction failures latch only their requesting Context and bypass handlers.

Preserve source/syntax/AST checks and whole-file validation order, including unreachable bodies. Validation does not run expressions, callbacks, or earlier statements in the invalid file. Completed importer effects remain visible. Import sites acquire their separate admission during load unwinding, while entered calls are included in initial validation-error construction. Detached Catch bindings at Pair entry retain their existing control error and now use the same local construction limits. Parser buffers, source copies, parameter-name tables, and general output formatting retain separate bounds.

## Syntax Error Construction

DSL syntax errors borrow the parser's existing error object until the complete source and current call context fit diagnostic quotas. Stream its position header, excerpt, underline, and expected/unexpected rule list through the checked text counter before allocating the final message buffer. Stream them again into the admitted buffer; do not call Pest's allocating Display/message helpers or construct intermediate spacing, underline, rule-list, or path strings. Preserve the accepted Pest wording, Unicode/tab handling, line endings, EOF locations, and source spans. Compatibility tests compare both position and span presentations with the locked dependency, including continued-line formatting.

Program parsing and native-signature helpers use default diagnostic quotas. Engine source/file parsing and imported modules use their installed local quotas and current call frames. Rejection keeps BW1001 as the bounded original cause, a byte-location/rule summary explicitly marked `source excerpt omitted`, and one omitted detail field. It retains no source owner or excerpt. Runtime construction failures latch the requesting Context and bypass Catch; standalone helpers remain independent. Existing source/syntax checks and prior cancellation keep priority. No earlier statements in a malformed file run; preserve completed importer effects and attach import sites through their existing separate unwinding admission.

Pest already owns its error-line and rule-attempt buffers when this check runs. Parser execution and source ownership retain their separate bounds. The public BWParser facade still returns Pest errors for host-managed rendering. This contract bounds the interpreter's additional syntax-detail construction, not arbitrary host formatting or the final Diagnostic Display output.

## Source and Syntax Guard Ownership

When a lexical source/syntax guard fails, admit its filename and checked source prefix before copying either into a SourceFile. Count the original small guard error, current calls, and their distinct sources first; then add the new filename/prefix byte lengths with checked arithmetic. The new prefix owner remains distinct even if a call source has identical contents. The prefix ends after the first excessive token, with original UTF-8 byte offsets preserved.

Accepted errors retain their resource name, limit, full filename, checked prefix, and coordinates. Rejection preserves bounded filename and exact start/end bytes plus the original resource/configuration error as a cause. It retains no source/prefix payload and reports omitted calls. Standalone Program/native-signature helpers use defaults; Context lexical checks and runtime parsing use local quotas and latch only the requesting Context. Import sites retain their separately admitted unwinding behavior.

Engine's earlier source-size checks and bounded file reads keep their existing priority and representation; they do not construct lexical guard spans. Preserve pre-effect rejection, input snapshots, completed importer effects, independent clones, and prior cancellation. Successful source admission and parser-owned buffers retain their existing contracts. Guard resource names and configuration messages are fixed, small interpreter details rather than input-derived message strings.

## Run Setup Errors

RunEnvironment preparation streams working-directory/current-directory failures, timeout-range errors, and invalid environment-name/value messages through default DiagnosticLimits before formatting their owned details. Local run quotas are installed only after successful preparation; retain that existing phase boundary. Thus a small setup error remains BW7002 even when the requested run text quota is zero. A message exceeding the default allowance returns BW8001 with bounded RunConfiguration evidence and explicit truncation.

Preserve setup ordering: observe prior cancellation, derive/check the deadline, resolve/check the directory, validate the environment overlay, then complete preparation. Setup failures precede input installation and script steps/effects; uninstalled deep inputs still use iterative cleanup. Fresh runs remain usable. Existing path/platform copies, environment snapshots, host configuration payloads, and OS error objects retain separate ownership contracts; admission bounds the additional diagnostic detail copy.

Limit validators also admit their ceiling messages under standalone defaults before formatting. Invalid AST/value/diagnostic depth, module/import/evaluation depth, and syntax settings retain BW7002 and exact wording even when the requested diagnostic allowance is zero; local budgets have not been installed. Diagnostic-limit validation uses the valid default budget to construct its own failure. Defensive AST-lowering failures use the same standalone helper with fixed interpreter-provided part names; callers attach source context at their existing lowering boundary.

## Scope and Evidence

This contract covers the named borrowed details, worker join failures, Log output errors, formatted signature failures, grouped collection-access fields, incompatible-operator descriptions, numeric conversion/arithmetic errors, input identifiers, host non-finite-value errors, standalone input messages/resource origins, signature-builder validation errors, entry-file read failures, import read/cycle details, runtime declaration collisions, control/parameter validation, DSL syntax-error details, source/syntax-guard prefix ownership, run-environment setup failures, limit-configuration messages, and defensive lowering details. Other initial error construction and general source-position formatting still need admission. Host-created BWErr strings already exist before runtime admission. Rendering, aggregate temporary diagnostic ownership, and output limits remain separate tasks. Active synchronous native call signatures have already passed their own retained-record admission and still own one copy.

Unit checks compare constructed and ordinary diagnostics at exact quotas and exercise prospective dimensions, invalid/zero limits, Unicode caps, source release, and omitted-frame counts. Integration checks cover normal catchability, exact contexts, handler restoration, prior effects, Pair entry, independent clone latches, all operation panic stages, repeated worker use, and cancellation with panic. Allocation observations verify zero large copies for rejected missing names and operation factory/poll signatures, plus no extra synchronous panic-detail copy beyond the admitted active-call signature. R21 host corpus cases and an executed Rust example pin byte boundaries.

Formatted-message checks cover exact raw bytes and context, counter overflow, Unicode chunk boundaries, early formatter stopping, empty messages, synchronous/async/blocking argument and return errors, required effects, skipped later arguments/callbacks, and stop priority. Four large-parameter/return allocation observations, two additional R21 cases, and a Rust example pin admission before initial message allocation.

Grouped-field checks cover exact metrics, late-field rejection without earlier copies, three-field Unicode truncation, source release, every collection failure reason, full call/location preservation, computed-key order, and accepted Catch metadata/restoration. Four large literal/computed-key allocation observations, two R21 cases, and a Rust example pin the path/segment boundary.

Operator checks cover exact details/context, every existing operator/kind combination, unused formatting on compatible/equality/value-check paths, unsupported raw rules, required effects and short-circuit order, independent clone latches, released temporaries, and depth-64 values during 21 entered calls. Four allocation observations include default-quota rejection of large escaped Debug descriptions. Two R21 cases and a Rust example pin the incompatible-operator byte boundary.

Input checks cover grammar parity across ASCII/Unicode boundaries, reporter invocation, exact default/local text limits, host/JSON/flag agreement, atomic installation, retry/clone isolation, prior cancellation, earlier name/value limits, and 20,000-level rejected input cleanup. Allocation observations reject large invalid names, non-finite-value details, and oversized helper origins without large parser/message copies. Two R21 cases and a Rust example pin the host input message boundary.

Standalone helper checks cover unchanged syntax/conversion/flag messages, empty and exact/default/invalid source limits, early formatting stop, Unicode filename truncation, original resource-limit priority, and source-free rejection. Allocation observations cover syntax/conversion/flag details and resource-origin copies; two R21 cases pin the default origin boundary. Existing file/UTF-8/order/CLI tests remain active.

File-origin checks also cover exact displayed-name boundaries, non-UTF-8 path spelling, bounded formatter visits, source limits before file opening, and files before settings. Allocation comparisons for large Unicode and invalid-native-byte filenames retain only the required platform open buffer and allocate no extra filename-sized diagnostic buffer on rejection.

Builder checks cover exact default text/source limits, unchanged messages/spans, Unicode details/evidence, consumed-source release, existing metadata reuse, and skipped host kind conversions on unknown names. A large-name allocation observation and two R21 corpus cases pin admission before formatting. Public tests include DSL-derived metadata from a source admitted under raised AST quotas.

Entry-file checks cover exact/zero text quotas, missing files, directory reads, invalid path/content encoding, embedded NUL paths, preserved input snapshots, skipped callbacks, fresh runs, prior cancellation, and source-limit precedence. Allocation comparison removes the large message copy on rejection while preserving required path ownership. Two R21 corpus cases pin read-error byte boundaries.

Import checks cover exact text/source/frame/site limits, shared and distinct source owners, path/URL policy, working-directory/read/encoding errors, cycle order, all parent sites, prior effects, skipped handlers, fresh runs, clone stops, and no namespace publication. Existing cache/symlink/stack/resource suites remain active. Allocation observations reject large details even when only the originating-site allowance is deficient, and reject a twelve-module cycle without a large chain buffer; acceptance creates one final chain. Two R21 cases pin the initial message/site boundary.

Captured-directory tests additionally verify shared unformatted ownership, checked snapshot handle limits, release after the final owner, and formatter visits: accepted import details are counted then constructed; rejected details visit only the bounded Unicode prefix. Source release, original ImportRead evidence, import-site omissions, and sibling execution remain covered.

Setup checks cover exact default message quotas, unchanged ordinary wording, separation from requested local quotas, oversized Unicode directory evidence, empty input snapshots, skipped effects/steps, and deep-input disposal. Allocation comparison retains only required directory/platform copies on rejection; prior cancellation and expired timeouts skip those copies too. Two R21 cases pin the setup/default-quota boundary and oversized-message rejection.

A nine-case limit-configuration matrix verifies exact ceiling messages, default admission despite zero requested text/source/record allowances, Context/Engine agreement, and rejection before script effects. Existing standalone AST/value/diagnostic/parser tests exercise those validation entry points; setup boundary checks also cover the shared default-formatting helper.

Collision checks cover exact text/source/frame/site quotas, unchanged native/DSL locations, Unicode byte evidence, source release, skipped coordinate formatters on context rejection, and one omission per replaced field. Runtime and native-registration tests preserve prior effects, original definitions, sibling execution, and handler cleanup. Namespace tests reject before conflicting path loading and preserve the original exports. Two allocation observations reject large filenames/signatures without large message copies; two R21 cases pin exact admission and one-byte rejection.

Validation checks cover exact default/local limits, each prospective context dimension, all four control statements, duplicate Unicode parameters, unreachable bodies, and borrowed coordinate evidence with deliberately unindexable private sentinel spans. Program/native-header boundary tests preserve default quotas; Engine, owned-AST, statement, Pair, and imported-module tests preserve stop priority, source release, completed importer effects, and clone isolation. Allocation comparisons retain the required parser source owner but eliminate large control/parameter location copies on rejection. Two R21 cases and a Rust example pin pre-effect validation admission.

Syntax checks cover exact/default/zero text/source/call quotas, position/span parity with Pest, custom reasons and rule-list sizes, Unicode/tabs/CRLF/EOF, explicit omission counts, and rejected-source release. Engine/file/native/import tests preserve input snapshots, pre-effect parsing, earlier source/stop priority, skipped handlers, parent import sites, fresh runs, and clone isolation. An allocation comparison measures parser-owned buffers separately: accepted construction adds one final message; rejection adds no message-sized buffer. Two R21 cases and a Rust example pin the syntax-message boundary.

Guard checks cover exact/default/zero source/text/call quotas, distinct equal source owners, Unicode token boundaries, invalid configuration, preserved original limits, source release, owned programs, import sites/effects, and clone stops. Allocation observations verify accepted filename/prefix copies and zero large copies on rejected standalone source and runtime syntax guards. Two R21 cases pin the exact prefix-ownership boundary; existing syntax/CLI/input tests retain their earlier failure contracts.

Worker join checks cover exact text/source/record/depth boundaries, escaped Unicode payloads, complete stop/cause admission, skipped formatters, source release, cancellation/timeout priority, omission counts, and worker-capacity reuse. An allocation comparison observes one final message for accepted unexpected/cleanup failures and no large message allocation on rejection. Two R21 cases pin exact native-source admission and one-byte rejection.

Output checks cover exact complete source/call/message boundaries, each context deficit, partial-write preservation, Catch recovery and quota bypass, binding/call cleanup, independent clone stops, cancellation during a write, and zero-budget success. A counted Unicode formatter proves rejection visits only the bounded prefix instead of constructing the full detail. A Linux CLI check uses a full output device and verifies BW4001, call location, repair guidance, and a nonzero exit status.

Numeric checks cover exact text/source/frame/record/depth boundaries, conversion/overflow/non-finite/zero-divisor/power failures, defensive invalid numeric atoms, rejected source release, Catch restoration, required operand effects, skipped later operands, cancellation, Pair entry, clone isolation, and successful numeric/short-circuit paths with zero error allowance. Existing operator-kind, integer-range, floating-point, and equality matrices remain active. Two R21 cases pin the 31-byte complete arithmetic diagnostic boundary.
