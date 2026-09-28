# Standard built-in statements

The default Engine and CLI provide these nine basic signatures plus sixteen
[collection statements](collections.md), for 25 fixed signatures in total. Statement names are
case insensitive and ignore spaces/tabs as usual; variable names are exact and
case sensitive. Arguments evaluate left to right once. Parameter validation,
call/source diagnostics, cleanup, and execution/resource limits use the normal
statement path. Use `--list-statements` and `--statement-help 'Assert |condition|'`
for the same executable signature metadata.

| Signature | Inputs | Result and behavior |
| --- | --- | --- |
| `Assert \|condition\|` | Bool | Return None for true; fail immediately with BW9001 for false |
| `Assert \|actual\| Equals \|expected\|` | Any two permitted values | Return None for deep equality; otherwise BW9001 |
| `Fail \|message\|` | String, including empty strings | Always fail immediately with BW9002 and that reason |
| `Log \|value\|` | Any permitted value | Write one bounded newline-terminated record to stdout and return a copy |
| `Variable Exists \|name\|` | String containing an exact DSL identifier | Return Bool for a visible binding, including a binding whose value is None |
| `Get Variable \|name\|` | String containing an exact DSL identifier | Return an admitted copy; an absent binding fails with BW2001 |
| `Type Of \|value\|` | Any permitted value | Return `None`, `Int`, `Float`, `Bool`, `String`, `Array`, or `Map` as a String |
| `No Operation` | No arguments | Return None without an external effect |
| `Sleep \|milliseconds\|` | Nonnegative Int | Cooperatively await that duration, then return None; requires async execution |

Wrong kinds and invalid variable names produce BW3003. Calls with a different
parameter count have a different signature and follow ordinary statement lookup
(BW2002 if no such statement exists). Boolean assertions do not coerce numbers,
strings, arrays, or None. Equality is exactly the `==` relation: recursively
compare arrays/maps; compare Int/Float without rounding integers to f32; treat
maps independently of insertion order; compare strings by their actual Unicode
scalar sequence without normalization. Nonfinite input remains invalid. Assertion
mismatch reasons name expected/actual values and their kinds within diagnostic
budgets. Rich diffs, case/dataset report fields, and retained artifacts remain the
separate diagnostic-assertions task.

Assertions and explicit failures are catchable. Unhandled failures stop the body,
then entered Finally/fixture owners still await cleanup. `Fail` is deliberately
not an assertion and cannot satisfy the [strict expected-failure policy](acceptance-policy.md).
Catch metadata exposes `error.code` and `error.details.reason`. Source spans and
entered call frames follow the original call. Handling an assertion consumes it;
a successful body with an expected-failure expectation is an unexpected pass.

`FailureKind::from_diagnostic` recognizes BW9001 only when the retained diagnostic
has no secondary failure or omission summary. Looking at the code alone remains
insufficient. Assertion-plus-cleanup errors, truncation/omission of retained
evidence, and explicit Fail stay unsuccessful. The suite DSL still has no
expected-failure annotation. This classifier prepares typed assertion evidence
for the shared verdict model; it does not infer expectations from messages/tags.

## Scope, ownership, and waiting

Inspection uses the current lexical frame and its parents, just like an ordinary
variable expression. It does not search dynamic caller locals, expose other
cases, or modify bindings. Retrieved collections are independent owned copies
admitted against value and temporary limits before cloning. A missing-variable
probe returns false without constructing an undefined-variable diagnostic.
`Type Of` reports a kind, not a dump of all variables. Log admits its return copy
before writing and preserves its existing output/failure behavior.

Sleep uses the existing async native-operation control, so cancellation and
run/suite deadlines interrupt its wait and prevent subsequent statements. Zero
milliseconds is valid; negative values and non-Int values are rejected. It does
not spin, retry side effects, or poll a condition. The CLI is asynchronous; hosts
use `run_source_async`/`evaluate_program_async`. Ordinary synchronous programs
remain usable with the fixed Sleep registration present. Actually calling Sleep
synchronously fails with BW5003 before evaluating its arguments. User-registered
NativeOperation contexts retain their existing rejection at synchronous entry.

The finite fixed catalogue is outside user registry admission, preserving
infallible `Context::init_statements` with zero user registry budgets. Repeated
initialization fills only vacant slots in the current frame and preserves host
registrations made before initialization. Existing same-frame duplicate rules
still apply: these new default signatures now occupy their root slots. Rename
conflicting root declarations, qualify imported helpers, or use ordinary lexical
shadowing in a child custom-statement scope. Call/result/snapshot quotas still
apply to the fixed statements; initialization is not an exemption from execution
limits. Sleep operation ownership follows the same bounded native-operation rules.

## Runnable example

<!-- botwork-test: builtins -->
```botwork
|amount| = |42|
Require Variable |name| {
    Assert |@{ Variable Exists |name| }|
    Return |@{ Get Variable |name| }|
}
|copied| = Require Variable |"amount"|
Assert |copied| Equals |42.0|
Assert |[1, {ok: true}]| Equals |[1.0, {ok: true}]|
Log |copied|
Log |@{ Type Of |copied| }|
Try { Assert |false| } Catch |error| {
    Log |error.code|
} Finally { Log |"cleanup"| }
Try { Fail |"demonstrate explicit failure"| } Catch |error| { Log |error.code| }
Sleep |1|
No Operation
Log |"finished"|
```

The output is `42`, `Int`, `BW9001`, `cleanup`, `BW9002`, and `finished`, each on
its own line. Run [example 28](../examples/28-builtins.botwork) with
`cargo run -- --file examples/28-builtins.botwork`.

B1 conformance cases, runtime/scope/diagnostic tests, virtual-clock sleep checks,
CLI help/output/exit checks, allocation-boundary checks, and executed documentation
cover these statements. [Validation evidence](builtins-evidence.json) records the
measured profiles and mutations.
