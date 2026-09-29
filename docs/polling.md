# Eventually and Retry

`Eventually` waits for a condition that becomes true over time. `Retry` repeats
an action that has side effects. Both run their block as a sequence of bounded
attempts, stop at the first attempt that completes, and otherwise fail with the
last attempt's diagnostic and a record of the recent attempts.

```text
Eventually |{timeout_ms: 5000, interval_ms: 100}| {
    |response| = HTTP Request |"GET"| To |url|
    Assert |response.status| Equals |200|
}

Retry |{attempts: 3, interval_ms: 200, backoff: 2}| {
    Submit Order |order|
}
```

The two statements share options and scheduling but differ in intent:

| | Eventually (observation) | Retry (action) |
| --- | --- | --- |
| Required bound | `timeout_ms` | `attempts` |
| Retried by default | every attempt failure, including operation timeouts | every attempt failure except BW5002 operation timeouts |
| Failure code | BW9004 condition not met | BW9005 retries exhausted |

An observation should only read state, so a timed-out observation is simply
retried. An action's timeout leaves its outcome unknown: a request can succeed
after the client stops waiting. Retry therefore repeats a timed-out action only
when `retry_on` lists BW5002, which you should do only for idempotent actions.
Each Retry failure reports how many times the action ran.

## Options

The options value is a Map with only these keys:

| Key | Value | Default | Meaning |
| --- | --- | --- | --- |
| `timeout_ms` | Int 1–86400000 | required for Eventually; none for Retry | Deadline measured from the first attempt |
| `attempts` | Int 1–1000000 | required for Retry; none for Eventually | Maximum attempts, including the first |
| `interval_ms` | Int 0–86400000 | 100 | Wait after the first failed attempt |
| `backoff` | Int or Float 1–10 | 1 | Multiplier applied to each later wait |
| `max_interval_ms` | Int, at least `interval_ms` | 60000, or `interval_ms` if larger | Cap for each wait |
| `retry_on` | nonempty Array of distinct `BWnnnn` Strings | all attempt failures (see above) | Only these codes are retried |

Unknown keys, missing required bounds, out-of-range values, wrong kinds, and
unknown or repeated `retry_on` codes fail with BW3003 at the options expression
before the block runs. BW5001 and BW8001 cannot appear in `retry_on`: run
cancellation and resource limits always stop.

## Schedule and deadline

Attempt 1 starts immediately. After failed attempt *n*, the next attempt starts
`min(interval_ms × backoff^(n−1), max_interval_ms)` milliseconds later, rounded
down, measured from the end of the failed attempt. The attempt limit ends
polling after its last attempt.

The deadline both limits when attempts start and interrupts an attempt in
progress:

- If the next attempt would not start before the deadline, the statement fails
  immediately instead of waiting.
- An attempt still running at the deadline is stopped at its next checkpoint,
  like a run deadline. That stop cannot be caught inside the attempt, and entered
  `Finally` blocks still run with their [cleanup allowance](cleanup.md).

For example, `{timeout_ms: 250, interval_ms: 100}` attempts at 0, 100, and 200 ms,
then fails at 200 ms because 300 ms is past the deadline. `{interval_ms: 50,
backoff: 2, max_interval_ms: 120, attempts: 5}` attempts at 0, 50, 150, 270, and
390 ms. Timing uses Tokio's monotonic clock, so tests can pause and advance time.

## Which failures are retried

An attempt fails when its block raises a catchable error (anything a Catch block
could handle). It also fails when an operation's own timeout, such as HTTP or
process `timeout_ms`, stops the attempt before the polling deadline. Outside
polling, that timeout would stop the whole run. Inside an attempt it ends only
that attempt, with code BW5002.

A failure that is not retried propagates unchanged after its attempt. It is not
wrapped in BW9004/BW9005. An unretried BW5002 keeps its usual meaning: it stops
the run and bypasses Catch.

These always end polling and propagate: run cancellation (BW5001), the run's
own deadline (BW5002), and resource limits such as evaluation steps (BW8001).
None of them can be caught. When the run stops while waiting between attempts,
the stop keeps the last attempt's failure as its cause.

## Results and failure evidence

A completed attempt completes the statement. Variables assigned in the block
use the enclosing scope, as in `While`, so values from the last attempt remain
visible after success or failure. Inside a custom statement, `Return` in the
block completes the attempt and returns from the statement. `Break`,
`Continue`, and `Rethrow` cannot leave an attempt and are rejected as BW1002
before execution.

When polling ends without success, it raises BW9004 (Eventually) or BW9005
(Retry). Both are catchable and have these properties:

- The source location is the header, for example
  `Eventually |{timeout_ms: 250, interval_ms: 100}|`.
- `details.reason` names the bound that ended polling and the last attempt's code.
- `details.attempts` counts every attempt, as a decimal String.
- `details.history` is a JSON array of the 16 most recent attempt records.
- The first cause is the last attempt's complete diagnostic, such as a BW9001
  assertion with its [full operands](assertion-diagnostics.md).

Each history record has an `attempt` number, `started_ms` since the first
attempt, `duration_ms`, and the failure `outcome` code:

```json
[{"attempt":1,"started_ms":0,"duration_ms":0,"outcome":"BW9001"},{"attempt":2,"started_ms":100,"duration_ms":0,"outcome":"BW9001"}]
```

Exhaustion evidence is admitted under the usual diagnostic budgets before it is
allocated. If it does not fit, the run fails with BW8001 wrapping an explicit
omission summary. The acceptance policy classifies BW9004/BW9005 as ordinary
failures, never as clean expected assertions.

## Execution requirements

Polling waits on a timer, so it requires asynchronous execution. The CLI and
`Engine::run_source_async` qualify. A synchronous run rejects `Eventually` and
`Retry` with BW5003 before evaluating their options or block. Each attempt shares
the run's evaluation-step budget and live quotas, and charges one step for the
attempt itself. The run timeout, cancellation, and every other run limit apply
throughout.

`Eventually` and `Retry` are case-insensitive control keywords, reserved at the
start of a statement. Existing custom statements whose names begin with either
complete word must be renamed; names like `Retrying Soon` stay valid. Polling
statements can be nested. An inner deadline ends only the inner polling, while
an outer deadline also interrupts the inner attempts.

## Runnable example

<!-- botwork-test: polling-statements -->
```botwork
|checks| = |0|
Eventually |{timeout_ms: 5000, interval_ms: 10}| {
    |checks| = |checks + 1|
    Assert |checks| Equals |3|
}
Log |checks|
|runs| = |0|
Try {
    Retry |{attempts: 2, interval_ms: 10}| {
        |runs| = |runs + 1|
        Fail |"service unavailable"|
    }
} Catch |error| {
    Log |error.code|
    Log |error.details.attempts|
    Log |error.causes[0].code|
}
Log |runs|
```

Output:

```text
3
BW9005
2
BW9002
2
```

The condition passes on its third check. The action runs twice, and the caught
BW9005 retains the final BW9002 as its first cause. [Example 38](../examples/38-polling.botwork)
polls a simulated service until it is ready, then retries a transient action.

## Verification

F11 conformance cases cover scheduling, exhaustion, and invalid options. Policy
unit tests cover option boundaries, backoff rounding, retry classification, and
bounded history. Attempt-budget unit tests cover shared steps, local stops, and
deadline inheritance. Paused-clock integration tests pin exact schedules,
interrupted attempts, Finally, cancellation, run deadlines, step limits,
`retry_on`, nesting, control placement, diagnostic budgets, and wire round trips.
On Linux, process timeouts check the observation/action distinction. Run them
with `cargo test --locked --test polling`.
[Validation evidence](polling-evidence.json) records the measured profiles and
mutations.
