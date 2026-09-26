# Core Conformance Corpus

`tests/conformance/cases.rs` registers 55 cases against the 35 rule IDs in the [core specification](language-specification.md). Each rule has positive, invalid-input, and boundary evidence. The corpus provides a traceable baseline alongside the more detailed unit/contract matrices; this inventory alone does not establish exhaustive clause coverage or a 9.5 quality score.

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
| F7 | async-success | async-expired | async-success |
| M1 | import-success | import-cycle | import-success |
| I1 | variables-success | variables-invalid | variables-success |
| R1 | embedded-success | embedded-limit | embedded-success |
| R2 | syntax-boundary | syntax-limit | syntax-boundary |
| R3 | runtime-boundary | runtime-recursion | runtime-boundary |
| R4 | ast-boundary | ast-limit | ast-boundary |
| R5 | import-budget-boundary | import-budget-limit | import-budget-boundary |

Boundary expectations include empty programs/collections, absent and None values, both signed integer limits, binary32 comparison precision, right-associated/unary powers, zero iterations, nested returns, failed assignment preservation, exact code-point distinctions, declaration collisions/shadowing, lexical updates, and CRLF/comment contents.

## Execution and Maintenance

Run `cargo test --locked --test conformance --test cli_harness`, then repeat with `--release`. CI's normal test matrix executes both profiles. The 41 script cases run through Cargo's built CLI in isolated temporary working directories. Each checks exact stdout, status `0`/`1`, empty success stderr, or the expected failure message and source filename. A five-second timeout terminates and reaps a stuck process. Runner tests check timeout recovery, stream separation, and workspace cleanup. Two variable cases materialize JSON files and pass explicit flags, checking integer boundaries/None/precedence and failure before script output/debug traces.

The structural host case exercises the public Rust equality API with NaN and both infinities, directly and nested in arrays/maps, on both operand sides. All 36 combinations across equality/inequality require an arithmetic error, even when the other value has a different kind. Valid DSL source cannot construct these non-finite host values. Two additional host cases register and invoke native callbacks, checking zero arguments, None and minimum-i32 returns, nested non-finite rejection, and native call context. Two signature cases verify shared metadata/help, accepted kind unions, return kinds, and rejection before callback effects. Two async operation cases check success and expired-deadline rejection before callback construction.

Successful scripts currently generate stdout only; filesystem/report artifacts must extend the case contract when those operations arrive. The corpus does not claim adapter, complete resource-limit, fuzz, or performance coverage. Failure labels report toolchain, OS/architecture, profile, timeout, deterministic seed status, and adapter absence.

Two embedded-run host cases verify fresh root state, supplied variables, structured outcomes/steps, exact budget exhaustion, and rejection before assignment. A low-level Context boundary case additionally checks exact step/evaluation-depth budgets and persistent exhaustion. These cases do not cover all remaining aggregate/value limits or hard host termination.

Register every new fixture and tag its rules/categories in the case table. Every failure also pins its stable diagnostic code; CLI checks require its bracketed code, and host checks query `BWErr::code()` directly. The inventory reads rule IDs from specification paragraphs and fails on missing category evidence, unknown tags, duplicate IDs/tags, invalid status/code expectations, or unregistered fixture files. Adding a new specification rule therefore requires cases in the same change. Preserve old minimal regressions when broadening the corpus, and follow the [reproduction procedure](regression-reproduction.md).

The two import cases materialize supporting modules inside the isolated workspace before invoking the CLI: the arithmetic module used by example `19`, or a minimal self-import cycle. Their stdout/status/code/source assertions use the same process contract as single-file cases.

Two AST host cases repeat a parsed statement, verify shared-source accounting and exact six-node admission, and reject a five-node budget before any binding or execution step.

Two import-budget host cases materialize one module, import it under two namespaces with exactly one load/source/path allowance, then exercise exact binding admission or rejection before the final assignment.
