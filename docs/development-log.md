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
