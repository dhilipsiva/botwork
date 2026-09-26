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
