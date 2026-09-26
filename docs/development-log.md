# Development Log

One TODO is selected, designed, implemented, verified, and committed before the next is started. Entries record evidence rather than projected completion.

## Publish the Quality Assessment Protocol

- **Plan:** Preserve the agreed roadmap and register the evaluation protocol before runtime changes.
- **Design:** Keep the weighted rubric in TODO.md; define concrete user tasks and complete automation workflows independently of unfinished syntax choices. Distinguish planned studies from results.
- **Implementation:** Added `docs/quality-assessment.md` with task success criteria, session thresholds, scoring anchors, evidence requirements, and protocol change rules.
- **Verification:** Reviewed alignment with all six rubric dimensions, task denominators, independent scoring requirements, and local Markdown links. No execution or participant results are claimed.

## Establish the Initial Test Foundation

- **Plan:** Add the first parser/operator/evaluator unit tests and CLI integration tests before changing runtime semantics.
- **Design:** Use full-program parsing, typed value/error assertions, static CLI fixtures, and Cargo's actual binary. Avoid snapshots of unstable debug output and avoid declaring known defects correct.
- **Implementation:** Added 22 unit tests, six CLI tests, two fixtures, and `docs/testing.md`; production modules only gained test-module declarations.
- **Verification:** The empty baseline suite passed. The new suite passed in debug and release (`cargo test --offline`, `cargo test --offline --release`), with all 28 tests passing in each configuration. `cargo fmt --all -- --check` and `git diff --check` passed.

## Capture the Confirmed DSL Regressions

- **Plan:** Turn each reviewed defect into an independent failing example before changing its implementation.
- **Design:** Use expected correct values/errors and explicit ignore reasons for pending bugs. Isolate parameter binding from final-return behavior. Temporarily accept only the specified unsupported-access diagnostic or correct access value. Require `Catch` for `Try`, since an error-handling construct without a handler has no defined purpose in the current language.
- **Implementation:** Added 18 regression tests, syntax/runtime CLI fixtures, and reproduction instructions. No runtime code changed.
- **Verification:** `cargo test --offline --test regressions -- --ignored` and the same command with `--release` each failed all 18 cases for the expected defects. The ordinary suite retained 28 passing tests and reported 18 ignored cases. Formatting and whitespace checks passed. The ignored cases remain unresolved work, not passing evidence.

## Return Failure Status for Script Errors

- **Plan:** Fix the two recorded CLI regressions while preserving successful handled errors and stopping after the first uncaught error.
- **Design:** Propagate file/parse/evaluation errors through a fallible runner, print one path-qualified diagnostic on stderr, and return status 1. Use error Display messages with their details. Parse the full program before executing any statement.
- **Implementation:** Replaced the CLI's print-and-continue-success handling, removed unused result accumulation, preserved error details, and enabled both CLI regressions. Added file/location diagnostics and before/after execution-sentinel tests.
- **Verification:** All 34 active tests passed in both debug and release; 16 known DSL regressions remain explicitly ignored. Caught errors still exit 0, runtime failures preserve prior work and skip later statements, and syntax failures prevent execution entirely. Formatting and whitespace checks passed.

## Decode String Values

- **Plan:** Fix string semantics before exposing normal user-facing log output.
- **Design:** Decode supported escapes once from parser string contents, preserve Unicode and literal whitespace, and keep identifier map keys on a separate conversion path.
- **Implementation:** Added decoding for newline, quote, and backslash escapes; made the string grammar compound atomic so whitespace/comments inside quotes are preserved; enabled both string regressions and documented the behavior.
- **Verification:** The original concatenation/escape cases failed before the fix. A new leading-space case exposed implicit parser skipping and failed before the grammar correction. All 40 active tests now pass in debug and release, including empty strings, tabs, comment markers, Unicode, escaped backslashes, literal newlines, and nested map values. Fourteen unrelated regressions remain ignored.

## Replace Debug Logging with Normal Output

- **Plan:** Give scripts usable stdout, keep diagnostics on stderr, and make tracing opt-in.
- **Design:** Display decoded strings and scalar values directly; quote nested strings and sort map keys. Return typed output errors for failed writes. Trace only top-level source locations and statement kinds with `--debug`.
- **Implementation:** Added value formatting, a fallible logging writer, CLI tracing, output documentation, and stdout/stderr/formatting tests. Removed all `dbg!` calls from application source.
- **Verification:** The new CLI output test failed before implementation because stdout was empty. All 46 active tests now pass in debug and release; 14 other regressions remain ignored. Tests cover nested formatting, exact CLI output, trace locations, unchanged stdout under tracing, and a broken writer. Formatting and whitespace checks passed.

## Reevaluate While Conditions After Normal Iterations

- **Plan:** Repair ordinary loop iteration before checking the bundled syntax example's complete behavior.
- **Design:** Remove the unconditional successful return after the loop's interruption handling. Preserve distinct `Continue`, `Break`, and pending-return paths; keep function-return defects tracked separately.
- **Implementation:** Removed one premature return, enabled the recorded regression, and added cases for false conditions, repeated iterations, nested `Continue`/`Break`, and a condition becoming nonboolean.
- **Verification:** Four new iteration tests failed against the previous implementation. All 52 active tests passed after the fix in debug and release; 13 unrelated regressions remain ignored. The false-condition test confirms the body is skipped, and loop-control tests confirm later body statements are skipped appropriately.

## Check Bundled Example Behavior

- **Plan:** Check complete results from both bundled examples after fixing strings, output, and ordinary While iteration.
- **Design:** Invoke Cargo's actual CLI, compare exact expected output derived from the scripts, and independently assert successful status and empty stderr. Store expected lines in Rust strings to preserve intentional whitespace without opaque snapshots.
- **Implementation:** Added `tests/examples.rs` for the expression and syntax demonstrations and documented how to run it.
- **Verification:** Both examples passed in debug and release. Independent source review confirmed the expected 12 expression-output lines and 37 syntax-output lines, including the final product 27.5, custom result 18, and While values 3/4/5/6. The wider suite now has 54 active tests and 13 pending ignored regressions.

## Configure Continuous Integration

- **Plan:** Establish the declared build/test/format/lint checks with reproducible dependency resolution.
- **Design:** Use an Ubuntu debug/release matrix, stable Rust with rustfmt/Clippy, locked Cargo commands, read-only checkout permissions, a pinned checkout action, and finite job timeouts. Keep hosted validation separate from local evidence.
- **Implementation:** Added `.github/workflows/ci.yml`, tracked Cargo.lock for the executable, and documented matching local commands. Fixed only the two reported Clippy findings: reverse parameter lookup and map sorting by key.
- **Verification:** Formatting and strict Clippy passed. Both profiles built all targets and passed all 54 active tests with `--locked --offline`; 13 known regressions remain ignored. Parsed the YAML and checked event, permission, pin, matrix, timeout, and command settings. Verified the checkout v7.0.1 commit against upstream tags and reviewed workflow syntax against official documentation. No hosted workflow run has yet been observed.

## Record Initial Coverage

- **Plan:** Measure the library unit suite separately from the full suite and record a reproducible starting point before adding broader conformance tests.
- **Design:** Isolate Pest-generated parser code while preserving public imports. Exclude generated/test files, check the maintained source set, collect clean profiles per scope, and preserve subprocess profiling. Record line counts, ignored tests, exact commands, platform/tool versions, and input hashes; leave branch and grammar-rule coverage explicitly unmeasured.
- **Implementation:** Added `scripts/coverage.py`, eight standard-library Python tests, CI execution of those helper tests, and `docs/coverage-baseline.json`. Documented setup, commands, measured source files, limitations, and the initial results.
- **Verification:** Both uninstrumented Rust profiles passed all 54 active tests with 13 ignored regressions; formatting and strict Clippy passed. The helper tests passed. Actual instrumented captures recorded 413/548 library lines (75.36%) and 482/584 full-suite lines (82.53%), including 25/36 CLI lines. Repeating unit coverage after the full suite reproduced identical per-file counts, and every recorded input hash matched. These results exceed 50% within the measured library unit scope; they do not establish the final coverage or correctness gates. Hosted CI remains unobserved.

## Require Catch During Parsing

- **Plan:** Enforce the previously agreed mandatory-handler contract and ensure malformed error handling prevents all script execution.
- **Design:** Require `stmt_catch` in the grammar while preserving the evaluator's existing two-block structure and current `} Catch {` layout. Test successful bodies, handled failures, nested handlers, and handler failures independently of unresolved arithmetic/return semantics.
- **Implementation:** Removed the grammar's optional Catch, enabled its recorded regression, added parser/evaluator cases and three CLI fixtures, and documented the implemented contract.
- **Verification:** The existing regression, new missing-handler parser cases, and CLI no-prior-output check failed before the fix. Afterwards all 62 active Rust tests passed in debug and release, with 12 unrelated regressions ignored. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests passed. Independent review confirmed valid handler structure, rejection of malformed/nested missing handlers, exact side effects, and error propagation.

## Return Errors for Unsupported Collection Access

- **Plan:** Remove the panic for accepted dot-access syntax before implementing collection lookup in its separate milestone.
- **Design:** Dispatch `dot_path` explicitly and return a typed unsupported-access error containing the source path. Do not look up the base or convert indexes. Propagate the error through ordinary evaluation and existing Catch handling; keep caller-scope defects separate.
- **Implementation:** Added `UnsupportedAccessError`, enabled and strengthened the regression, and added evaluator, parser, and CLI checks for named/numeric/nested/Unicode paths, large indexes, expression contexts, direct assignment preservation, recovery, and skipped branches. Documented the temporary contract.
- **Verification:** The recorded regression and three new evaluator tests reproduced the panic before the fix. All 69 active Rust tests now pass in debug and release; 11 known regressions remain ignored. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass. CLI checks verify status 1 with a path-qualified diagnostic and no panic, or status 0 after Catch recovery. Independent review confirmed the error path and narrowed documentation to avoid claiming unresolved custom-call state preservation.

## Correct Binary Operator Precedence

- **Plan:** Separate arithmetic, ordering comparisons, equality, and boolean precedence before changing exponent/unary or lazy-boolean semantics.
- **Design:** Register binary levels from weakest to strongest and evaluate the existing unary pairs directly. Review found that Pest's one-role-per-rule table let prefix minus overwrite binary minus; separating unary evaluation removes that subtraction panic while preserving existing unary grouping.
- **Implementation:** Reordered the Pratt table, added a unary evaluator, enabled the precedence regression, and added six evaluator tests plus an executable precedence example with exact CLI output checks. Documented the binary levels and remaining expression work.
- **Verification:** The old code failed mixed arithmetic/comparison checks and panicked on parsed subtraction. All 77 active Rust tests now pass in debug and release, with 10 known regressions ignored. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass. Independent review confirmed distinguishing cases for adjacent precedence levels, parentheses, arithmetic association, unary/binary minus, type errors, and example output.

## Make Arithmetic Failures Catchable

- **Plan:** Define numeric failure behavior before extending power/unary grammar, replacing panics and non-finite results with catchable errors in both build profiles.
- **Design:** Use checked integer operations, reject zero divisors and non-finite numeric operands/results, preserve type-error priority, and permit finite float rounding/underflow. Define integer exponents, reciprocal powers, `0^0`, and the valid `MIN%-1` boundary. Compute floating powers with exact integer parity, inverted bases for negative exponents, wider intermediates, and at most 32 squaring steps.
- **Implementation:** Added `ArithmeticError`, numeric guards and checked operations, finite float-literal evaluation, seven operator tests, three evaluator tests, an uncaught CLI fixture, and a checked arithmetic example. Enabled the arithmetic regression. Captured the pre-existing minimum signed literal conversion defect separately and added it to the numeric-contract TODO.
- **Verification:** The old implementation reproduced remainder/overflow/negative-power panics and accepted division by zero. All 90 active Rust tests now pass in debug and release, including large exponents, exponent parity, signed zeros, subnormal reciprocals, integer boundaries, type-error priority, and Catch recovery. Ten regressions remain ignored, including the newly reproduced literal-boundary defect. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass. Independent review confirmed arithmetic behavior and prompted the tested error-priority correction.

## Define Power and Unary Grouping

- **Plan:** Resolve chained powers, unary precedence, negative exponents, and repeated prefixes while retaining the checked arithmetic contract.
- **Design:** Parse power operands recursively on the right and let unary operators wrap that expression. Evaluate the new power node through the existing operator evaluator, keeping statement and collection layouts intact. Parentheses override grouping; every intermediate result remains checked.
- **Implementation:** Added the power grammar node and evaluator dispatch, made the power operator right associative, enabled its regression, and added evaluator/parser tests plus a checked powers example. Documented precedence, type restrictions, malformed syntax, and the distinction between `-2 ^ 31` and `(-2) ^ 31`.
- **Verification:** The old code produced `64` for `2 ^ 3 ^ 2`, produced `4` for `-2 ^ 2`, and rejected `!!true`. All 97 active Rust tests now pass in debug and release; nine unrelated regressions remain ignored. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass. Independent review confirmed parse layouts, error propagation, grouping, and exact example output, including caught intermediate overflow.

## Match Complete Keywords

- **Plan:** Accept identifiers and custom statement names containing keyword prefixes, preserving complete control keywords and existing case rules.
- **Design:** Check expression keywords atomically against identifier continuations. Check control keywords against actual statement delimiters so punctuation and Unicode suffixes remain part of custom names. Use silent keyword rules with atomic lookahead to retain evaluator parse shapes; literal prechecks preserve contextual missing-keyword diagnostics. Keep `In` contextual to `For`.
- **Implementation:** Updated lexical grammar, enabled both prefix regressions, and added parser/evaluator cases, a checked keyword example, and a CLI diagnostic fixture. Documented boundaries and contiguous spellings without extending the identifier alphabet or declaring the full naming/whitespace contracts complete.
- **Verification:** Both recorded regressions and all five new unit tests failed against the previous grammar, which also accepted split `i n` and `andfalse`. All 106 active Rust tests now pass in debug and release; seven unrelated regressions remain ignored. Tests cover prefix names, case sensitivity, punctuation, Unicode continuations, comments, mixed-case controls, `Else If`, exact `In`, parse shapes, and fail-fast reporting of an unknown complete statement name. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Specify the Core Language Contract

- **Plan:** Fix expected evaluation, scope, completion, and error semantics before replacing the pair-based interpreter; distinguish target behavior from implemented guarantees.
- **Design:** Publish numbered clauses with named evidence and explicit gaps. Choose lexical invocation scopes, local writes, temporary For bindings, distinct control outcomes, and None for custom fallthrough. Preserve checked arithmetic and explicit value returns. Keep complete value, naming, limits, and diagnostic contracts in their existing TODOs.
- **Implementation:** Added `docs/language-specification.md` and 17 contract tests: eight active cases and nine explicitly pending cases. Linked existing regressions and listed missing conformance cases. Updated behavior/testing references and removed a caller-state-leak assumption from the keyword test while preserving its name-matching purpose. Runtime code is unchanged.
- **Verification:** All nine new pending cases fail for the specified gaps in both debug and release. All 114 active Rust tests pass in both profiles; 16 cases remain ignored across regression and contract suites. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass. Checked all 26 named test references and eight local file links. Independent reviews clarified lexical lookup and iterator restoration, confirmed status labels, and prompted distinguishing checks for comparison association, declaration availability, and live run bindings.

## Execute an Owned Syntax Tree

- **Plan:** Replace runtime parser-pair traversal and custom-call reparsing with owned syntax before implementing lazy booleans and explicit completion/scopes.
- **Design:** Share immutable original source through spans; lower every statement and expression into typed nodes, retaining unevaluated operands and source-ordered map entries. Decode strings during construction, defer numeric conversion to evaluation, and store definitions behind shared pointers. Keep a pair-lowering compatibility entry point and preserve CLI trace labels.
- **Implementation:** Added `ast.rs`, AST unit tests, public syntax inspection, and owned-program execution. Migrated the CLI, unit helpers, regressions, and contract suite to that execution path. Custom invocations reuse stored bodies without parsing. Added ownership/compatibility integration checks and test-only expression visitation to verify stopping after a left error. Updated architecture documentation and coverage scope; existing scope/control/short-circuit tasks remain open.
- **Verification:** All 132 active Rust tests pass in debug and release, including exact CLI/example output. All 16 pending cases still reproduce their failures in both profiles. Tests prove source and definition lifetime, shared body identity/release, Unicode/CRLF/tab/parenthesized spans, deferred and catchable numeric failures, and skipped RHS visitation after a left error. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass. Instrumented coverage successfully includes the new AST module: 686/774 library lines and 723/799 full-suite lines; input hashes match the separately retained capture. Independent review confirmed call-path parsing removal and identified the documented flattening of transitional Else/Catch result wrappers.

## Implement Boolean Short-Circuiting

- **Plan:** Use the owned expression tree to implement specification E4 and enable both recorded boolean-selection regressions.
- **Design:** Evaluate and validate the left boolean first. Return immediately for false-And or true-Or; otherwise evaluate the right operand once and use the existing strict value operator. Keep ordinary binary evaluation and whole-file parsing unchanged.
- **Implementation:** Added evaluator selection, enabled both regressions, and added nine unit tests, two CLI cases, and a checked short-circuit example. Updated the behavior/specification/architecture references and roadmap evidence.
- **Verification:** Before the change, both regressions failed with undefined-variable errors, nonboolean left operands evaluated the right side, and visitation checks exposed unnecessary reads. All 146 active Rust tests now pass in debug and release; 14 unrelated cases remain ignored. Tests cover every boolean truth-table combination, skipped failures/types, required operand order/count, None, nested grouping, collection/call composition, guarded loops, Catch behavior, and syntax rejection before output. Independent review prompted checks that value-level boolean operators stay strict and ordinary binary operators still evaluate their RHS. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Propagate Explicit Completion Outcomes

- **Plan:** Replace the shared interrupt state with explicit completion outcomes, resolving position-dependent and nested returns in the same change. Keep invocation scopes and whole-program placement validation in their separate tasks.
- **Design:** Internal statements return normal values, Return, Break, Continue, or an evaluation error. Blocks discard normal values and propagate controls immediately. Loops consume their own Break/Continue; invocations consume Return. Try catches only errors, including failures while evaluating a return expression. Public execution and invocation boundaries reject escaping controls with `ControlFlowError`.
- **Implementation:** Removed all context interrupt flags and implicit block/loop result arrays. Enabled five return contract/regression cases and added value-kind, repeated-call, handler, mixed-loop, evaluation-visit, boundary, compatibility, CLI, and example checks. Documented None fallthrough, exact return values, and the remaining scope and static-validation limits.
- **Verification:** All five pending return cases failed before the change; additional baseline tests exposed implicit arrays and accepted top-level Return. All 171 active Rust tests pass in debug and release; nine scope/numeric cases remain ignored. Checks include unreachable controls after Return, nearest-loop transfer across For/While combinations, preserved explicit collections/None, handler returns/errors, callee boundary rejection, and exact example output. Independent review found no blocking issue and prompted mixed-loop and typed-error coverage. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Validate Control Placement Before Execution

- **Plan:** Complete the control-placement TODO after explicit completion handling, rejecting invalid control statements before any program effects.
- **Design:** Walk all statement bodies with lexical definition/loop permissions. Definitions reset loop permission; loops preserve definition permission; branches and handlers inherit both. Validate after full syntax construction and again at public execution boundaries for assembled or extracted nodes. Report the first invalid control's original file, line, and column without evaluating expressions.
- **Implementation:** Added `Program::validate`, parsing/execution validation, and subtree validation for the parser-pair API. Retained internal runtime guards and made their tests explicitly bypass public validation. Added AST and library cases plus CLI fixtures for skipped code, nested definitions, preserved state, and original Unicode/CRLF/tab locations. Preserved the CRLF diagnostic fixture with a path-specific Git attribute and updated behavior/architecture/roadmap references.
- **Verification:** Three new AST checks and all five focused CLI checks failed before implementation; valid placements already passed. All 184 active Rust tests pass in debug and release; nine unrelated scope/numeric cases remain ignored. Tests verify no output/debug trace or earlier assignment/definition on validation failure, syntax-error precedence, deferred numeric/name evaluation, extracted control rejection, and retained source locations. Design review identified invocation-boundary resets and whole-unit validation as required invariants. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Isolate Invocation Scopes

- **Plan:** Implement the specified lexical invocation scopes, argument binding order, local definition lifetime, and temporary For bindings; resolve the caller-corruption defect within that scope change.
- **Design:** Keep owned frames in a context stack, with lexical parent indexes pointing to each definition's registration frame. Resolve calls and evaluate all arguments before installing parameters. Preserve the dynamic caller separately and discard the invocation frame on every language completion/error. Save and restore only a For iterator's current-frame binding, preserving absence and inherited values.
- **Implementation:** Added lexical variable/statement lookup, local writes and definitions, invocation frame management, argument collection before binding, and For restoration. Enabled eight pending scope tests and added twelve unit cases plus a checked scope example. Retained shared immutable definition bodies and isolated context cloning; updated the specification evidence and behavior/architecture documentation.
- **Verification:** All eight pending cases failed before implementation. All 205 active Rust tests pass in debug and release; only the minimum signed literal regression remains ignored. Checks cover direct/mutual recursion, live ancestor lookup, caller shadows, recursive failure recovery, source-order argument failure, frame disposal, nested/inherited/absent/None iterator bindings, every completion path, empty/failed iterables, and exact example output. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Support the Full Signed Integer Literal Range

- **Plan:** Resolve the remaining minimum-literal regression as its own task under the broader numeric contract, retaining power/unary grouping and catchable range errors.
- **Design:** When unary minus directly wraps an integer atom, convert the sign and text together at evaluation time. Preserve the owned unary tree and spans, defer unused literal conversion, and keep ordinary checked negation for compound operands. Parentheses around an atom do not change this rule; parentheses around a compound expression retain its intermediate checks.
- **Implementation:** Added signed-atom conversion, enabled the final ignored regression, and added four evaluator tests plus compatibility, CLI, and executable-example checks. Documented the full range, leading zeroes, grouping, double-negation overflow, and the still-open broader numeric contract.
- **Verification:** The recorded regression and three focused evaluator checks failed before the fix. All 213 Rust tests pass in debug and release with no ignored cases. Checks cover both integer boundaries, values outside the range, exact visit order, deferred failures, preserved assignments, collection/call/float composition, power precedence, and nonzero CLI failure after a valid minimum value prints. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Read Literal Map and Array Paths

- **Plan:** Replace temporary unsupported-access failures with nested map/array lookup, explicit error behavior, and a documented decision on indexed assignment. Track computed paths separately so variable indexes and arbitrary string keys remain visible work.
- **Design:** Store parsed root/segment names and spans instead of splitting raw source. Resolve the root through lexical lookup, borrow each container, and clone only the selected value. Maps use exact string keys; arrays accept ASCII decimal indexes and reject oversized/out-of-bounds values without panicking. Stop at the first failed segment and return its canonical path, segment, and reason. Indexed assignment remains a syntax error.
- **Implementation:** Added parsed access nodes, borrowed variable lookup, collection traversal, and `CollectionAccessError`. Tightened the regression to require its actual value and replaced temporary unsupported-error checks with real missing-key/bounds/type cases. Added segment/span, composition, numeric-key, None, value-preservation, compatibility, and executable-example checks; updated the CLI fixtures and documentation.
- **Verification:** The tightened regression and both initial path tests failed before implementation. All 221 Rust tests pass in debug and release with no ignored cases. Tests cover comments/Unicode in paths, literal versus variable segments, leading zeroes, invalid/oversized indexes, exact key matching, first-error priority, preserved assignments, catch recovery, source-container preservation, and rejected indexed assignment. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Add Computed Collection Reads

- **Plan:** Support variable indexes and arbitrary string map keys, specifying evaluation order, type/bounds failures, literal-path composition, and the update model before implementation.
- **Design:** Represent postfix access with a base expression and ordered literal/computed segments. Permit variable, literal, and grouped bases; bind access before powers/unary operators. Evaluate each bracket expression once immediately before validating its receiver/key types and looking up its value. Arrays require nonnegative integers; maps require strings. Quoted map-literal keys use ordinary string decoding. Keep reads immutable and reserve updates for library operations returning replacement collections.
- **Implementation:** Added bracket syntax, quoted keys, owned segment/expression spans, and checked computed lookup. Expression evaluation now borrows an immutable context, allowing variable containers to remain borrowed during key evaluation; temporary bases are owned and only selected values are copied. Added evaluation-order instrumentation, strict type/bounds/error-priority checks, malformed-syntax and span cases, compatibility checks, CLI fixtures, and example `12`. Updated the specification, architecture, testing guide, and roadmap.
- **Verification:** Five new evaluator cases failed to parse before implementation. All 235 Rust tests pass in debug and release with no ignored cases. Checks cover arbitrary/empty/escaped keys, nested indexes, temporary bases, grouping, exact visit order/count, skipped later keys, source-order map values, None versus absence, independent results, scope/loop/call composition, failed assignment preservation, rejected indexed assignment, and syntax rejection before CLI output. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Specify Numeric Precision and Structural Equality

- **Plan:** Complete the numeric/collection-comparison TODO by documenting conversion rules, reproducing precision loss in mixed comparisons, and defining equality across nested values.
- **Design:** Preserve checked integer arithmetic and existing binary32 arithmetic rounding. Widen stored integers/floats exactly to binary64 for comparisons. Make equality total over finite values: ordered array contents, exact map key sets with associated values, None equality, and false for different nonnumeric kinds. Validate complete operands for non-finite host values before comparing; use work lists so equality adds no recursive comparison frames. Keep ordering numeric-only.
- **Implementation:** Replaced lossy mixed comparison casts, added structural equality, and documented literal/operation rounding, signed zero, exact comparison, and intentional compatibility changes. Added boundary/type matrices, equality-law checks, nested-invalid-value cases, operand-order instrumentation, decimal-rounding checks, scope/Catch composition, a CLI ordering fixture, and example `13`. Updated prior equality/type-error expectations to the new contract.
- **Verification:** New tests reproduced `16777217 <= 16777216.0` incorrectly returning true and missing collection/None equality. All 247 Rust tests pass in debug and release with no ignored cases. Tests cover all six numeric comparisons in both directions, both integer boundaries, subnormals, signed zero, every value-kind pair, reflexivity/symmetry/transitivity/complement, deep map/array comparison, invalid nested host floats, preserved assignments, and caught/uncaught CLI behavior. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Complete the Core Value Contract

- **Plan:** Consolidate the remaining value rules after precision/equality implementation, explicitly covering absent values, truthiness/conversion, operator support, ownership, map order/duplicates, and statement results.
- **Design:** Specify the existing runtime behavior: strict boolean conditions, array-only For iteration, owned collection copies, source-ordered map construction with the last decoded duplicate key winning, unspecified internal map order, sorted display, and explicit custom returns. Keep library program results distinct from custom-body fallthrough. Unsupported operator/kind combinations remain typed errors; future libraries and serialization retain separate tasks.
- **Implementation:** Added `tests/value_contract.rs`, the complete supported-operation table and ownership/order reference, specification V7 and conformance evidence, and clarified None's displayed spelling and top-level result semantics. Updated roadmap status and removed stale claims that current contract cases remain ignored. Runtime code required no changes.
- **Verification:** The nine new conformance tests passed against the implemented behavior. All 256 Rust tests pass in debug and release with no ignored cases. The new matrix exercises 686 binary and 14 unary operator/kind combinations; other cases cover every condition/iterable kind, None versus absence, host/call/return copy independence, duplicate-key evaluation errors, map display/explicit key iteration, comparison chains, and implicit/explicit result rules. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Define Whitespace and Multiline Layout

- **Plan:** Specify line boundaries, multiline expressions/collections/calls, comments, escapes, and reserved delimiters; preserve original source locations and reject malformed syntax before effects.
- **Design:** Separate ASCII horizontal whitespace from LF/CRLF. Allow newlines inside expressions and fixed control/assignment syntax, trailing collection commas, and braces/Else/Catch on following lines. Keep custom sentence boundaries explicit with backslash continuation; bare Return does not consume the next line's assignment. Reserve backslash outside strings, preserve literal string bytes, and require triple-hash block comments to close.
- **Implementation:** Updated the Pest grammar, made sentence parts atomic, removed redundant expression reparsing, and excluded continuation markers from signatures/Return operands. Added nine layout conformance tests, a checked multiline example, and CLI fixtures for incomplete continuations/comments. Updated the expected missing-pipe diagnostic to EOF because expression newlines are now valid; documented the layout and compatibility changes.
- **Verification:** Six new cases failed before implementation, including an unterminated block comment incorrectly accepted as a line comment. All 267 Rust tests pass in debug and release with no ignored cases. Checks cover LF/CRLF, tabs, empty input, multiline tokens/blocks/access, explicit continuations, bare Return boundaries, literal Unicode/delimiters/line endings, malformed separators/operators/escapes, original spans, and failure before stdout/debug traces. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.

## Normalize Statement Names and Reject Definition Collisions

- **Plan:** Define signature matching and collision behavior, preserve existing definitions, report both source locations, and reject duplicate parameter labels before effects.
- **Design:** Strip ASCII spaces/tabs and lowercase each Unicode character while preserving punctuation, other whitespace, and parameter positions. Reject an occupied signature only in the current frame at registration time; retain lexical shadowing and fresh invocation-local helpers. Treat repeated parameter labels as whole-program validation failures. Native initialization fills only vacant slots and reports named native origins on later DSL collisions.
- **Implementation:** Added typed duplicate-statement/parameter errors, source-location formatting, normalized signatures, parameter validation, guarded registration, and idempotent native initialization. Added naming conformance, Unicode boundary/cleanup cases, Pair compatibility, CLI fixtures, and example `15`. Updated contracts, architecture, testing guidance, and the import TODO to reuse registration rules.
- **Verification:** Seven new cases failed before implementation, exposing silent replacement, tab mismatches, and duplicate parameters. All 282 Rust tests pass in debug and release with no ignored cases. Checks cover both original file/line/column locations with Unicode/CRLF, preserved definitions, arity/positions/punctuation, normalization boundaries, same-frame loops, skipped branches, lexical shadowing, invocation cleanup, native preservation, and static validation before stdout/debug traces. Formatting, strict Clippy, whitespace checks, and eight coverage-helper tests pass.
