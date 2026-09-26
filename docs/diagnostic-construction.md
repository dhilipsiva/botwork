# Diagnostic Detail Construction

Synchronous undefined-variable, undefined-statement, and native-panic errors borrow their detail text before making the initial owned copy. The existing `RunLimits::diagnostics` quotas apply; there is no additional setting. Variable access includes direct expressions and missing bases of collection access.

## Admission Before Copying

Build a fixed-size diagnostic skeleton with an empty detail field. Measure its actual label, primary source, and complete prospective call snapshot using the [diagnostic ownership metrics](diagnostic-ownership.md). Add the borrowed detail's UTF-8 byte length with checked arithmetic. Only after all dimensions fit, copy the detail and capture call metadata. No source contents, coordinates, or formatted values are needed for this measurement.

Successful construction preserves category, exact detail text, expression/source labels, original spans, and entered-call order. Variable names remain case-sensitive; missing calls retain their exact source text, including whitespace already present in the invocation span. Each error's full detail is available to Catch when all other handler/value quotas permit it.

Rejected construction returns BW8001, latches the requesting Context, and bypasses handlers. Preserve the original category with a bounded UTF-8 detail prefix, exact source-byte evidence, prospective frame count, and explicit omission metadata. Source filenames and details use the fixed [emergency caps](diagnostic-ownership.md#owned-admission-and-emergency-evidence), even with zero quotas. A shortened detail is counted once; its full original string is never allocated by this path. Cleanup restores prior bindings and preserves completed effects.

Observe an existing stop before a construction quota can latch. Native callbacks that request cancellation and then panic keep cancellation primary; if combined stop evidence exceeds quotas, use the existing bounded stop/limit representation. Context clones retain independent stop state and share their existing live accounting.

## Scope and Evidence

This contract covers the three named borrowed-detail paths. Formatted or multi-field errors, operation panic strings, parser/validation/import diagnostics, and source-position formatting still need construction admission. Host-created BWErr strings already exist before runtime admission. Rendering, aggregate temporary diagnostic ownership, and output limits remain separate tasks. Active native call signatures have already passed their own retained-record admission and still own one copy.

Unit checks compare constructed and ordinary diagnostics at exact quotas and exercise prospective dimensions, invalid/zero limits, Unicode caps, source release, and omitted-frame counts. Integration checks cover normal catchability, exact contexts, handler restoration, prior effects, Pair entry, independent clone latches, and cancellation with panic. Allocation observations verify zero large copies for rejected missing names and no extra panic-detail copy beyond the admitted active-call signature. R21 host corpus cases and an executed Rust example pin the byte boundary.
