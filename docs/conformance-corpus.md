# Core Conformance Corpus

`tests/conformance/cases.rs` registers 39 cases against the 27 rule IDs in the [core specification](language-specification.md). Each rule has positive, invalid-input, and boundary evidence. The corpus provides a traceable baseline alongside the more detailed unit/contract matrices; this inventory alone does not establish exhaustive clause coverage or a 9.5 quality score.

## Rule Traceability

Names below are stable corpus case IDs. One script can exercise several related rules with explicitly checked output.

| Rule | Positive case | Invalid-input case | Boundary case |
| --- | --- | --- | --- |
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

Boundary expectations include empty programs/collections, absent and None values, both signed integer limits, binary32 comparison precision, right-associated/unary powers, zero iterations, nested returns, failed assignment preservation, exact code-point distinctions, declaration collisions/shadowing, lexical updates, and CRLF/comment contents.

## Execution and Maintenance

Run `cargo test --locked --test conformance --test cli_harness`, then repeat with `--release`. CI's normal test matrix executes both profiles. The 34 script cases run through Cargo's built CLI in isolated temporary working directories. Each checks exact stdout, status `0`/`1`, empty success stderr, or the expected failure message and source filename. A five-second timeout terminates and reaps a stuck process. Runner tests check timeout recovery, stream separation, and workspace cleanup.

The structural host case exercises the public Rust equality API with NaN and both infinities, directly and nested in arrays/maps, on both operand sides. All 36 combinations across equality/inequality require an arithmetic error, even when the other value has a different kind. Valid DSL source cannot construct these non-finite host values. Two additional host cases register and invoke native callbacks, checking zero arguments, None and minimum-i32 returns, nested non-finite rejection, and native call context. Two signature cases verify shared metadata/help, accepted kind unions, return kinds, and rejection before callback effects.

Successful scripts currently generate stdout only; filesystem/report artifacts must extend the case contract when those operations arrive. The corpus does not claim adapter, resource-limit, fuzz, or performance coverage. Failure labels report toolchain, OS/architecture, profile, timeout, deterministic seed status, and adapter absence.

Register every new fixture and tag its rules/categories in the case table. Every failure also pins its stable diagnostic code; CLI checks require its bracketed code, and host checks query `BWErr::code()` directly. The inventory reads rule IDs from specification paragraphs and fails on missing category evidence, unknown tags, duplicate IDs/tags, invalid status/code expectations, or unregistered fixture files. Adding a new specification rule therefore requires cases in the same change. Preserve old minimal regressions when broadening the corpus, and follow the [reproduction procedure](regression-reproduction.md).
