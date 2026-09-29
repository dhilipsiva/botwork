# Stress repetition

`tests/stress.rs` repeats deterministic isolation and cancellation scenarios
and checks every iteration's outcome and the process's resources. `cargo test`
runs each scenario 10 times. The stress campaign runs each 1,000 times:

```sh
BOTWORK_STRESS_ITERATIONS=1000 BOTWORK_STRESS_REPORT=stress.json \
  cargo test --locked --release --test stress
```

`BOTWORK_STRESS_REPORT` names a JSON report to write, which is written whether
or not the test passes.

## Determinism

The scenarios avoid timing assumptions, so a failure points at the runtime
rather than at a slow or busy machine:

- **Handshakes.** A stop is requested only after the run reaches a controlled
  point. Examples: a native operation reports entry, the in-process HTTP server
  has read the request, a process statement's shell has written its PID, or the
  CLI script has written a marker inside `Try`. No stop depends on how long a
  sleep lasts.
- **A controlled clock.** The deadline scenario pauses Tokio's clock once the run
  has entered. A one-hour deadline then fires in virtual time, and the run ends
  in milliseconds of real time.
- **In-process services.** Operations and the HTTP server run inside the test
  process on loopback. No external service is involved.
- **Fresh state.** Every iteration uses a fresh control and clears its notes and
  marker files.

A marker's existence alone is not a handshake: the file exists before the
statement writing it has finished. The CLI scenario therefore writes its marker
inside `Try`, so an interrupt that arrives during that write still runs
`Finally`.

## Scenarios

| Scenario | Each iteration | Checks |
| --- | --- | --- |
| `isolation` | Eight concurrent runs of one `Engine` import a module that reads the environment, then write and read a file in their own directories | Each run sees only its own variables, environment value, and file. The module parse is shared: 8 hits and no misses per iteration |
| `cancellation` | Cancel while an operation is suspended inside `Try` | BW5001, `Finally` ran once, statements after `Try` did not run, and the operation's ownership budget is empty again |
| `paused-clock-deadline` | A one-hour run deadline on the paused clock, over a loop of one-minute sleeps | BW5002 after exactly 59 sleeps, and `Finally` ran |
| `blocking-abandonment` | Cancel a blocking operation that ignores its control, with a 20 ms [stop grace](shutdown.md) | BW5001 with the abandonment cause, no earlier than the grace. Once released, the callback returns, its only permit serves the next iteration, and its ownership is returned |
| `http-cancellation` | Cancel a request once the server has read it; the server never answers | BW5001, and the server sees the connection close |
| `process-cancellation` | Cancel a process statement once its shell has written its PID | BW5001, and the child is stopped and reaped |
| `cli-interrupt` | Send SIGINT to the CLI once its script is inside `Try` | Exit status 1 with BW5001, and `Finally` wrote its file |

## Checks and failure classes

Each scenario runs:

1. one warm-up iteration;
2. a settle step, which waits for child processes to exit and 300 ms for idle
   pool threads;
3. a record of open descriptors, threads, and child processes;
4. the configured number of iterations.

Every stop must end its run within 5 seconds. After the last iteration,
descriptors, threads, and children must return to their recorded levels within
5 seconds. A scenario stops at its first failure, since later iterations would
only repeat its consequences.

| Class | Meaning |
| --- | --- |
| `setup` | The environment failed: temporary files, sockets, or spawning the CLI |
| `handshake` | The run never reached a controlled point. The services are deterministic in-process code, so this points at the runtime |
| `outcome` | A wrong result, state seen by another run, or cleanup that did not run |
| `release` | A leaked descriptor, thread, child, connection, or ownership charge |
| `bound` | A stop that took longer than 5 seconds, or a run that never ended |

For each scenario, the report records:

- iterations and failures;
- stop latency, as p50, p95, and maximum;
- duration;
- descriptor, thread, and child counts before and after;
- resident memory before and after.

## Results

The [stress evidence](stress-evidence.json) records 1,000 iterations of every
scenario in GNU and musl, debug and release builds, with no failures. Sensitivity
probes show what the campaign catches:

- **Injected faults.** A leaked HTTP stream, a leaked operation future, and a
  forgotten blocking permit are each caught and attributed to the scenario and
  class they break.
- **An intermittent leak.** A fault that leaks every 100th HTTP connection passes
  10 iterations and is caught by 1,000.

## Limits

- **Other features.** Suite fixtures, listeners, and reports are covered by their
  own suites; this campaign does not repeat them.
- **Memory.** Resident memory is recorded but not asserted, since allocator
  caching makes small growth normal.
- **One test per binary.** The resource checks read process-wide counts, so the
  stress test must stay the only test in its binary.
- **Later integrations.** Browser and device sessions and adapters must add their
  own scenarios as they arrive.
