# Cancellation and resource release

A stop reaches every construct a run is executing. Stops come from:

- run cancellation;
- a run, suite, or operation deadline;
- a CLI interrupt.

The stopped run ends with one outcome, its cleanup still runs, and it releases
what it acquired. This page maps each construct to how a stop reaches it and to
the tests that prove it; the linked pages hold the detailed contracts.

## Sources of stops

| Source | Reaches | Outcome |
| --- | --- | --- |
| `OperationControl::cancel` on `RunOptions::control` | The run and every child control | BW5001 `cancelled` |
| `RunOptions::timeout`, `--timeout-ms` | The run from admission | BW5002 `timed_out` |
| `--suite-timeout-ms` | A suite owner's setup, borrowers, and teardown | BW5002 |
| An operation's own `timeout_ms` (HTTP, processes, `Eventually`) | That operation, which then stops the run | BW5002 |
| SIGINT or SIGTERM | Every run and fixture, through the CLI's root control | BW5001; see [terminal outcomes](terminal-outcomes.md#interruption) |

A child control inherits its parent's cancellation and the earlier of the two
deadlines, so a stop anywhere above an operation reaches it. `Catch` never
consumes BW5001 or BW5002.

## How each construct stops

| Construct | How the stop arrives | Contract | Tests |
| --- | --- | --- | --- |
| Statements, custom calls, loops, conditions, imports, module initialization | Checkpoints before statements, iterations, and calls, and at every await | [async execution](async-execution.md#cancellation-and-limits) | `async_execution`, `runtime_limits`, `cancellation::deadlines_reach_cleanup_through_nested_calls_and_loops` |
| `Try`/`Finally` cleanup | `Finally` runs after the stop, under its own step and time allowance independent of the stopped control | [cleanup](cleanup.md) | `cleanup`, `resource_release` (nested scenario), `cancellation` |
| `Eventually`, `Retry` | A stop ends polling; the deadline also interrupts an attempt in progress | [polling](polling.md) | `polling` |
| Sleep and native asynchronous operations | Child controls; a dropped waiter cancels queued jobs, and started blocking jobs drain | [nonblocking I/O](nonblocking-io.md) | `async_operations`, `async_blocking` |
| File and environment operations | Bounded workers check the stop between 16 KiB I/O calls | [operating system](operating-system.md), [nonblocking I/O](nonblocking-io.md) | `operating_system`, `async_filesystem`, `resource_release` |
| `Log` and `--debug` output | Output workers observe the stop while a destination is blocked | [nonblocking I/O](nonblocking-io.md) | `async_blocking`, `parallel_cli` |
| Processes | The supervisor kills the child's process group and reaps it within the cleanup allowance | [processes](processes.md), [isolated workers](isolated-workers.md) | `processes`, `isolated_workers`, `resource_release`, `cancellation::interrupts_stop_cli_processes_requests_and_nested_cleanup` |
| HTTP requests | The request races the stop and closes its connection | [HTTP](http.md#timeouts-cancellation-and-admission) | `http`, `resource_release`, `cancellation` |
| Shared suite fixtures | Setup stops, queued borrowers are skipped, and ready fixtures are torn down under cleanup limits | [fixtures](fixtures.md) | `suite_fixtures`, `terminal_outcomes` |
| Listeners | Delivery never blocks runs. After an interrupt the stream ends with an `interrupted` trailer, and a listener still running at its close timeout has its process group killed | [listeners](listeners.md) | `listeners`, `cancellation::interrupts_reach_a_hung_listener_within_its_close_timeout` |
| Reports | Started runs keep one record; the verdict records the interruption | [terminal outcomes](terminal-outcomes.md) | `terminal_outcomes` |

## Resource release

`tests/resource_release.rs` runs in its own test process, so the process-wide
counts it reads belong to it alone. For each scenario it:

1. runs the scenario once, to warm worker pools and caches;
2. waits until idle pool threads exit and no child process remains;
3. records the open descriptors (`/proc/self/fd`), threads (`/proc/self/task`),
   and child processes;
4. runs the scenario five more times.

After each run, the stop must arrive within five seconds. After the five runs,
descriptors and threads must return to at most their recorded levels, and no new
child process may remain, zombies included.

| Scenario | Stop | Also checked |
| --- | --- | --- |
| A custom call whose `For` loop enters `Try { Eventually { Sleep } } Finally { file I/O }` | Cancellation once the attempt has started | `Finally` wrote its file after the stop |
| `Run Process` of a shell that records its PID and sleeps | Cancellation once the child is running | That child is no longer alive |
| `HTTP Request` to a server that accepts and never answers | Cancellation once the connection is accepted | The connection's descriptor is closed |
| Nested custom calls around an endless `While` with `Sleep` | A 100 ms run deadline | BW5002 |
| Repeated file writes and reads, then `Fail` | Failure | BW9002 |

Injecting one leaked descriptor per run makes the test fail, which confirms it
detects leaks.

The CLI tests in `tests/cancellation.rs` cover three cases:

- **Interrupt.** A SIGINT stops a process statement and an HTTP request running in
  parallel within the bound, runs their `Finally` cleanup, and leaves the
  process statement's child stopped.
- **Deadline.** A deadline reaches cleanup through nested calls and loops.
- **Hung listener.** An interrupt reaches a hung listener: the invocation ends at
  the listener's close timeout, and the listener process is stopped.

## Limits

- **Blocking system calls.** An operating-system call that blocks indefinitely,
  such as opening a FIFO that no writer opens, finishes before its worker can
  observe a stop. The run still returns promptly with the stop, but that worker
  thread stays blocked until the call returns. Use a process statement or an
  isolated worker when a hard deadline must also free the resource.
- **Idle pool threads.** Idle worker-pool threads persist until their keep-alive
  expires; they are reused, not leaked.
- **Forced termination.** A SIGKILL or a crash releases nothing gracefully. The
  operating system reclaims the process's resources, and
  [reconciliation](terminal-outcomes.md#forced-termination-and-reconciliation)
  finishes its reports.

[Validation evidence](cancellation-evidence.json) records the measured profiles,
mutations, and sensitivity probes.
