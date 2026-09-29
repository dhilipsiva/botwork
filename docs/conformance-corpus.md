# Core Conformance Corpus

`tests/conformance/cases.rs` registers 197 cases against the 78 rule IDs in the [core specification](language-specification.md). Each rule has positive, invalid-input, and boundary evidence. The corpus provides a traceable baseline alongside the more detailed unit/contract matrices; this inventory alone does not establish exhaustive clause coverage or a 9.5 quality score.

## Rule Traceability

Names below are stable corpus case IDs. One script can exercise several related rules with explicitly checked output.

| Rule | Positive case | Invalid-input case | Boundary case |
| --- | --- | --- | --- |
| B1 | builtins-success | builtins-invalid, builtins-assertion | builtins-success |
| B2 | collection-statements | collection-missing-index, collection-duplicate-key | collection-boundaries |
| B3 | string-statements | string-missing-field, string-invalid-regex | string-boundaries |
| B4 | datetime-statements | datetime-gap, datetime-duration-overflow | datetime-boundaries |
| B5 | os-statements | os-invalid-byte, os-missing-file | os-boundaries |
| B7 | http-statements | http-invalid-method | http-zero-deadline |
| B8 | assertion-artifacts | assertion-invalid-condition | assertion-empty-array-difference |
| F10 | cleanup-return | cleanup-control, cleanup-secondary-failure | cleanup-empty-and-catch |
| F11 | polling-statements | polling-invalid-options, polling-single-attempt | polling-single-attempt |
| B6 | process-statements | process-invalid-argument | process-empty |
| E1 | control | incomplete-continuation | recovery |
| E2 | values | return-operand-order | recovery |
| E3 | scope | resolve-before-arguments | scope |
| E4 | control | strict-condition | recovery |
| E5 | call-composition | invalid-call-composition, call-composition-error | call-composition |
| V1 | values | return-operand-order | numeric |
| V2 | numeric | comparison-chain | numeric |
| V3 | collections | missing-path-key | collections |
| V4 | collections | computed-key-type | collections |
| V5 | numeric | comparison-chain | numeric |
| V6 | collections | structural-host-invalid | collections |
| V7 | values | unsupported-operator | values |
| L1 | layout | incomplete-continuation | empty, crlf-comments |
| L2 | layout | unclosed-comment | crlf-comments |
| L3 | names | duplicate-parameter | names |
| L4 | names | invalid-unicode | names |
| S1 | scope | private-caller | scope |
| S2 | scope | expired-definition | scope |
| S3 | control | non-array-iteration | control |
| C1 | control | invalid-control | control |
| C2 | values | return-operand-order | empty, values |
| F1 | control | required-catch | recovery |
| F2 | recovery | return-operand-order | recovery |
| F3 | recovery | diagnostic-stack | diagnostic-handler |
| F4 | catch-inspection | rethrow-outside | rethrow-original |
| F5 | native-return | native-invalid-return | native-return |
| F6 | signature-valid | signature-invalid | signature-valid |
| F7 | async-success | async-expired | async-success |
| F8 | async-program | async-program-sync-rejected | async-program |
| F9 | blocking-worker-boundary | blocking-worker-limit | blocking-worker-boundary |
| M1 | import-success | import-cycle | import-success |
| I1 | variables-success | variables-invalid | variables-success |
| R1 | embedded-success | embedded-limit | embedded-success |
| R2 | syntax-boundary | syntax-limit | syntax-boundary |
| R3 | runtime-boundary | runtime-recursion | runtime-boundary |
| R4 | ast-boundary | ast-limit | ast-boundary |
| R5 | import-budget-boundary | import-budget-limit | import-budget-boundary |
| R6 | value-boundary | value-limit | value-boundary |
| R7 | construction-boundary | construction-limit | construction-boundary |
| R8 | input-budget-boundary | input-budget-limit | input-budget-boundary |
| R9 | retention-boundary | retention-limit | retention-boundary |
| R10 | definition-boundary | definition-limit | definition-boundary |
| R11 | name-boundary | name-limit | name-boundary |
| R12 | registry-boundary | registry-limit | registry-boundary |
| R13 | snapshot-boundary | snapshot-limit | snapshot-boundary |
| R14 | result-boundary | result-limit | result-boundary |
| R15 | temporary-boundary | temporary-limit | temporary-boundary |
| R16 | diagnostic-value-boundary | diagnostic-value-limit | diagnostic-value-boundary |
| R17 | diagnostic-ownership-boundary, diagnostic-admission-boundary | diagnostic-ownership-limit, diagnostic-admission-limit | diagnostic-ownership-boundary, diagnostic-admission-boundary |
| R18 | runtime-diagnostic-boundary | runtime-diagnostic-limit | runtime-diagnostic-boundary |
| R19 | retained-diagnostic-boundary, handler-copy-boundary, handler-handoff-boundary | retained-diagnostic-limit, handler-copy-limit, handler-handoff-limit | retained-diagnostic-boundary, handler-copy-boundary, handler-handoff-boundary |
| R20 | operation-diagnostic-boundary | operation-diagnostic-limit | operation-diagnostic-boundary |
| R21 | borrowed-diagnostic-boundary, operation-panic-boundary, signature-diagnostic-boundary, access-diagnostic-boundary, operator-diagnostic-boundary, input-diagnostic-boundary, input-origin-boundary, signature-builder-boundary, entry-file-diagnostic-boundary, import-diagnostic-boundary, setup-diagnostic-defaults, collision-diagnostic-boundary, validation-diagnostic-boundary, syntax-diagnostic-boundary, guard-diagnostic-boundary, worker-join-boundary, numeric-diagnostic-boundary | borrowed-diagnostic-limit, operation-panic-limit, signature-diagnostic-limit, access-diagnostic-limit, operator-diagnostic-limit, input-diagnostic-limit, input-origin-limit, signature-builder-limit, entry-file-diagnostic-limit, import-diagnostic-limit, setup-diagnostic-limit, collision-diagnostic-limit, validation-diagnostic-limit, syntax-diagnostic-limit, guard-diagnostic-limit, worker-join-limit, numeric-diagnostic-limit | borrowed-diagnostic-boundary, operation-panic-boundary, signature-diagnostic-boundary, access-diagnostic-boundary, operator-diagnostic-boundary, input-diagnostic-boundary, input-origin-boundary, signature-builder-boundary, entry-file-diagnostic-boundary, import-diagnostic-boundary, setup-diagnostic-defaults, collision-diagnostic-boundary, validation-diagnostic-boundary, syntax-diagnostic-boundary, guard-diagnostic-boundary, worker-join-boundary, numeric-diagnostic-boundary |
| R22 | operation-ownership-boundary | operation-ownership-limit | operation-ownership-boundary |
| R23 | diagnostic-rendering-boundary | diagnostic-rendering-limit | diagnostic-rendering-boundary |
| R24 | output-boundary | output-limit | output-boundary |
| R25 | worker-boundary | worker-limit | worker-boundary |
| R26 | protocol-boundary | protocol-limit | protocol-boundary |
| R27 | shutdown-boundary | shutdown-invalid | shutdown-boundary |
| R28 | progress-boundary | progress-incomplete | progress-boundary |
| R29 | tree-boundary | tree-unverified | tree-boundary |
| R30 | journal-boundary | journal-limit | journal-boundary |
| R31 | namespace-boundary | namespace-helper-failure | namespace-boundary |
| R32 | parallel-cli-success | parallel-cli-failure | parallel-cli-success |
| T1 | named-suite-selection | named-suite-unknown-id | named-suite-rerun |
| T2 | dataset-rows | dataset-duplicate-row | dataset-row-rerun |
| T3 | fixture-order | fixture-primary | fixture-order |
| T4 | setup-policy-handled | setup-policy-skipped | setup-policy-skipped |

Boundary expectations include empty programs/collections, absent and None values, both signed integer limits, binary32 comparison precision, right-associated/unary powers, zero iterations, nested returns, failed assignment preservation, exact code-point distinctions, declaration collisions/shadowing, lexical updates, and CRLF/comment contents.

## Execution and Maintenance

Run `cargo test --locked --test conformance --test cli_harness`, then repeat with `--release`. CI's normal test matrix executes both profiles. The 41 script cases run through Cargo's built CLI in isolated temporary working directories. Each checks exact stdout, status `0`/`1`, empty success stderr, or the expected failure message and source filename. A five-second timeout terminates and reaps a stuck process. Runner tests check timeout recovery, stream separation, and workspace cleanup. Two variable cases materialize JSON files and pass explicit flags, checking integer boundaries/None/precedence and failure before script output/debug traces.

The structural host case exercises the public Rust equality API with NaN and both infinities, directly and nested in arrays/maps, on both operand sides. All 36 combinations across equality/inequality require an arithmetic error, even when the other value has a different kind. Valid DSL source cannot construct these non-finite host values. Two additional host cases register and invoke native callbacks, checking zero arguments, None and minimum-i32 returns, nested non-finite rejection, and native call context. Two signature cases verify shared metadata/help, accepted kind unions, return kinds, and rejection before callback effects. Two async operation cases check success and expired-deadline rejection before callback construction. Two async DSL cases verify a suspending operation inside a custom Return expression and rejection of synchronous dispatch before statement effects.

Successful scripts currently generate stdout only; filesystem/report artifacts must extend the case contract when those operations arrive. The corpus does not claim adapter, complete resource-limit, fuzz, or performance coverage. Failure labels report toolchain, OS/architecture, profile, timeout, deterministic seed status, and adapter absence.

Two embedded-run host cases verify fresh root state, supplied variables, structured outcomes/steps, exact budget exhaustion, and rejection before assignment. A low-level Context boundary case additionally checks exact step/evaluation-depth budgets and persistent exhaustion. These cases do not cover all remaining aggregate/value limits or hard host termination.

Register every new fixture and tag its rules/categories in the case table. Every failure also pins its stable diagnostic code; CLI checks require its bracketed code, and host checks query `BWErr::code()` directly. The inventory reads rule IDs from specification paragraphs and fails on missing category evidence, unknown tags, duplicate IDs/tags, invalid status/code expectations, or unregistered fixture files. Adding a new specification rule therefore requires cases in the same change. Preserve old minimal regressions when broadening the corpus, and follow the [reproduction procedure](regression-reproduction.md).

The two import cases materialize supporting modules inside the isolated workspace before invoking the CLI: the arithmetic module used by example `19`, or a minimal self-import cycle. Their stdout/status/code/source assertions use the same process contract as single-file cases.

Two AST host cases repeat a parsed statement, verify shared-source accounting and exact six-node admission, and reject a five-node budget before any binding or execution step.

Two import-budget host cases materialize one module, import it under two namespaces with exactly one load/source/path allowance, then exercise exact binding admission or rejection before the final assignment.

Two value host cases admit an array at exact node/depth/payload/entry limits or reject it before input installation, root assignment, and execution steps.

Two construction host cases trace duplicate-key callback effects at exact retained budgets and reject known excessive array width before any child callback.

Two input-budget host cases parse one object at exact source/token/name/value limits or reject a deficient raw-token allowance without retaining the JSON payload in diagnostics.

Two retained-value host cases admit exact replacement overlap or reject it before publishing the new value, preserving the original destination and structured resource error.

Two retained-definition host cases share one source's exact text/name budget across declarations or reject the next definition while preserving the earlier registration.

Two variable-name host cases reuse an exact-budget key during replacement or reject a new Unicode spelling at the aggregate byte boundary while preserving the earlier binding.

Two registry host cases admit a native signature at exact record/node/key/text/source limits or reject its metadata strings without publishing a registration.

Two snapshot host cases admit the fixed Engine template table at its exact entry budget or reject it before inputs and script effects.

Two result host cases admit a terminal/root pair with nested UTF-8 keys at exact aggregate limits or reject its payload with an explicit omitted-snapshot diagnostic.

Two temporary host cases admit exact concatenation operand/output headroom or reject its aggregate payload before publishing the assignment.

Two diagnostic conversion host cases admit exactly ten metadata nodes or reject a nine-node budget while preserving the borrowed original category.

Two diagnostic ownership host cases admit exact label/error text bytes or reject checked copying while preserving the original error identity and category.

Two owned diagnostic admission cases preserve an accepted original or return a bounded original-category summary with explicit omissions after rejecting its text quota.

Two runtime diagnostic host cases allow one ordinary error to enter Catch or reject a zero-node diagnostic budget, preserving bounded category evidence and skipping handler effects.

Two input-diagnostic host cases preserve exact invalid-name details at the text boundary or reject one fewer byte before any input binding or script effect.

Two input-origin host cases retain the payload-free origin at the default source-byte boundary or reject one extra byte while preserving bounded original input-limit evidence.

Two signature-builder host cases preserve an unknown-parameter message at the default text-byte boundary or reject one extra byte before formatting its owned detail.

Two entry-file host cases preserve a read error at its exact text-byte boundary or reject one fewer byte before message construction, retaining installed inputs without script steps.

Two import-construction host cases admit the message and originating import site at their exact text boundary or reject one fewer byte before constructing the detail.

Two setup-construction host cases preserve default diagnostic quotas before local budget installation or reject an oversized working-directory message before input installation and script effects.

Two runtime-collision host cases admit the name and both declaration locations at their exact text boundary or reject one fewer byte with explicit byte-location summaries and two omitted coordinate fields.

Two validation-construction host cases preserve exact control-placement and duplicate-parameter details at their text boundary or reject one fewer byte before any script steps, retaining coordinate omission counts and original byte offsets.

Two syntax-construction host cases preserve the complete parser message at its text boundary or reject one fewer byte with an explicit omitted-excerpt summary before any script steps.

Two lexical-guard host cases admit the filename and checked prefix at their exact source-ownership boundary or reject one fewer byte while preserving the original resource limit and token offsets.

Two worker-join host cases preserve escaped panic details at the native-header source boundary or reject one fewer byte with bounded AsyncRuntime evidence.

Two numeric-error host cases admit an arithmetic detail with its expression label and entered call at exactly 31 text bytes, or reject one fewer byte with bounded Arithmetic evidence.

Two output CLI cases admit a Unicode Log and its newline at exactly three cumulative bytes, or reject two bytes before writing, preserving BW8001 and failure status.

Two Linux worker host cases transfer a two-byte Unicode request at exact byte limits or reject one stdout byte before complete output, checking failure category and direct-child reaping. Other platforms check explicit pre-entry rejection instead of claiming Linux lifecycle evidence.

Two portable typed-protocol host cases preserve negative-zero float bits through the SDK at an exact 28-byte request limit, or reject a 27-byte allowance before encoding.

Two shutdown host cases close and drain an owned Linux worker while preserving its cancellation report (empty-pool shutdown on other platforms), or reject an unrepresentable allowance before closing admission.

Two R30 host cases persist a complete worker transport result at the exact journal record limit and reject another invocation with BW8001 before entry. Crash, corruption, and delayed receipt evidence lives in the dedicated journal and worker recovery matrices.

Two R31 host cases require complete namespace-worker transport and verified tree cleanup for success, or preserve failure while releasing kernel-confirmed namespace capacity after a mismatched helper exits. Dedicated subprocess tests cover guardian/host death and unavailable facilities.

Two blocking-worker cases verify native callback execution off the async executor
with exact call-frame snapshot admission and rejection before callback effects.

Four B2 CLI cases exercise collection replacement and sorted iteration, empty ranges and exact numeric membership, missing-index rejection, and duplicate dynamic-map keys. All existing V3/V4 immutable-read and indexed-assignment rejection cases remain active.

Four B3 CLI cases exercise named formatting, literal transformations, Unicode slicing/casing, regex matching/extraction, empty boundaries, missing fields, and invalid regex syntax. String escapes remain unchanged.

Three B6 CLI cases execute text/binary capture and nonzero status, exact empty captures, and rejection of a non-String argument before launch. Lifecycle, environment, admission, and async scheduling use the dedicated process and worker matrices.

Three B8 cases preserve full Bool operands through Catch, reject a non-Bool condition, and diagnose an extra element against an empty array. Dedicated assertion/CLI matrices cover nested differences, Unicode, budgets, artifacts, and dataset identity.

Three F11 cases poll until a third check passes, reject Retry options without `attempts` before the block runs, and exhaust a single-attempt Retry with BW9005. The polling matrix covers exact schedules, deadlines, cancellation, limits, and failure evidence.
