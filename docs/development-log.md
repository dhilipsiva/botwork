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
