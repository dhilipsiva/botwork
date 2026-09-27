# Isolated Worker Supervision

`core::worker::WorkerPool` runs trusted external worker executables with independent supervision on Linux. It provides the process lifecycle needed when an in-process callback cannot cooperate with cancellation. The existing `NativeOperation::blocking` contract is unchanged: Rust callbacks running inside the host still cannot be forcibly terminated.

This is the byte-oriented execution boundary beneath the [typed worker protocol](worker-protocol.md). `NativeOperation::isolated` supplies bounded typed arguments, results, diagnostics, signatures, and shared ownership across this boundary. Async DSL dispatch, persistent run/report recovery after a host crash, and the complete milestone 6 shutdown coordinator remain separate TODO items. This implementation does not claim those integrations are complete.

## Start, Admission, and Isolation

Create a pool with `WorkerLimits`, then call `start(command, input, control)`. Admission happens synchronously before a supervisor thread or child is started. A stopped control, invalid configuration, oversized request, closed pool, or full capacity returns a structured error. There is no queue retaining unlimited requests. Cloned pools share capacity, shutdown state, monotonically increasing identifiers, and reconciliation history.

`WorkerCommand` supplies an absolute executable path, literal OS-string arguments, an absolute working directory, and an explicit environment map. The parent environment is cleared for the child; host environment and working directory never change. No shell is selected or command string constructed by the supervisor. A host can explicitly launch a shell, as with any executable. Host-owned command/argument/environment construction and executable selection remain trusted host responsibilities.

The child has piped stdin/stdout/stderr and a new process group. A dedicated thread owns its process and file descriptors. All parent pipe descriptors are nonblocking. Each supervision turn performs at most one 4 KiB input write and one bounded read of each output stream, then checks process state. A 5 ms polling interval bounds ordinary stop-observation latency without allowing a stream flood to monopolize the supervisor. Worker progress and deadlines do not require polling an async task or keeping a Tokio runtime alive.

```rust
# #[cfg(target_os = "linux")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use botwork::core::{
    operation::OperationControl,
    worker::{WorkerCleanup, WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool},
};
let pool = WorkerPool::new(WorkerLimits::default())?;
let handle = pool.start(WorkerCommand {
    executable: "/bin/cat".into(),
    arguments: vec![],
    directory: std::env::temp_dir(),
    environment: Default::default(),
}, "hello é".as_bytes().to_vec(), OperationControl::default())?;
// The receiver needs no time or I/O driver; the supervisor owns the OS work.
let runtime = tokio::runtime::Builder::new_current_thread().build()?;
let report = runtime.block_on(handle.wait());
assert_eq!(report.outcome, WorkerOutcome::Succeeded);
assert_eq!(report.cleanup, WorkerCleanup::Reaped);
assert!(report.io_complete);
assert_eq!(report.stdout, "hello é".as_bytes());
assert!(pool.shutdown().active.is_empty());
# Ok(())
# }
# #[cfg(not(target_os = "linux"))]
# fn main() {}
```

## Bounds and Completion

| WorkerLimits field | Default | Contract |
| --- | --- | --- |
| `max_in_flight` | 4 | Shared admission slots, including startup and pending cleanup |
| `request_bytes` | 1 MiB | Complete host request before entry |
| `stdout_bytes` | 1 MiB | Captured stdout payload |
| `stderr_bytes` | 1 MiB | Captured stderr payload |
| `timeout` | 30 seconds | Real monotonic execution allowance from admission, including startup/I/O |
| `cleanup_timeout` | 1 second | Ordinary cleanup waiting after a stop or direct-child exit |
| `history_records` | 128 | Completed metadata records; no captured output payloads |

The earlier inherited `OperationControl` deadline also applies. Zero byte/time/history allowances are valid; unrepresentable combined durations are configuration errors. Capacity is nonzero. Byte counters use checked or saturating boundary arithmetic. Reads probe at most one excess byte and never append bytes beyond the configured payload quota. Buffer capacities, allocator overhead, host command metadata, OS buffers, and child memory have separate ownership; these limits are not a process-memory sandbox. Completed report payloads transfer to the host and do not accumulate in pool history.

`Succeeded` requires successful direct-child exit, complete request transfer, both output EOFs, completed direct-child reaping, and no observed stop or I/O/resource failure. Nonzero exit, failed launch/I/O, truncated streams, or incomplete request transfer are failures. Reports include exact bytes accepted on stdin, bounded output prefixes, exit status when known, a structured diagnostic, and an `io_complete` flag. This flag describes stream completion, not operation success. No pass may be inferred from output contents or exit status alone.

Stops do not undo external effects and never retry the operation. A deadline returns `TimedOut` with BW5002; explicit cancellation returns `Cancelled` with BW5001. Dropping a handle or its wait future requests termination and records `Interrupted`. Cancelling one handle does not cancel its parent or siblings. An observed stop is retained during cleanup; an I/O/cleanup failure is bounded diagnostic evidence, not permission to report success.

## Termination, Reaping, and Reconciliation

On a stop, the supervisor sends SIGKILL to the child's original process group and to the direct child. This does not depend on callbacks, signal handlers, or cooperative polling in the worker. After ordinary direct-child exit it also terminates remaining members of the inherited group before completing capture. The group leader is observed with `waitid(WNOWAIT)` and remains unreaped until group signalling has been requested. This retains its PID identity across group cleanup; no numeric-PID signal is sent after reaping. See the [Linux wait contract](https://man7.org/linux/man-pages/man2/waitid.2.html), [process-group signal contract](https://man7.org/linux/man-pages/man2/kill.2.html), and [Rust process-group setup](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#method.process_group).

Cleanup status is explicit:

- `NotStarted`: no child was launched.
- `Reaped`: the direct child was reaped and inherited-group termination was requested. This does not assert ownership/reaping of grandchildren.
- `Pending`: cleanup exceeded its allowance. A terminal unsuccessful report returns; the supervisor retains child ownership and its capacity slot, closes pipes, and continues reaping. A later snapshot reconciles the same worker ID and outcome with `Reaped`.
- `Unverified`: host interference or a supervisor failure prevented verification. Lost-ownership slots are quarantined rather than reused silently. In particular, a host reaping this child or changing SIGCHLD policy violates the exclusive ownership contract; the supervisor must not risk signalling a recycled PID.

`shutdown()` closes admission and requests cancellation of all owned workers immediately. It returns a snapshot rather than blocking on kernel cleanup. `snapshot()` exposes active IDs/PIDs/stopping state, active cleanup status even after history eviction, completed outcome/cleanup metadata, and a count of omitted history records. History eviction never implies success; a caller needing durable evidence must retain reports or persist its own records. Exactly one metadata record is retained per completed worker until bounded eviction. An early `Pending` report is reconciled when its supervisor finishes. Dropping the last pool owner requests shutdown; supervisor ownership outlives the pool and runtime until cleanup completes.

This is not a security sandbox or an absolute real-time kernel guarantee. A worker can consume child memory or create descendants outside its initial process group. Deliberately detached descendants, host crashes, suspended hosts, uninterruptible kernel operations, and blocked OS process creation/signalling remain outside the guarantee. The supervisor reaps only its direct child; orphaned descendants are reaped by their eventual parent. Callers requiring containment of arbitrary descendants or recovery after host death need the further worker/shutdown work tracked in TODO. Never install a competing child reaper or ignore SIGCHLD while this pool owns children.

Other platforms reject worker entry before effects. Only Linux is currently advertised for this boundary; the language's existing synchronous/async host APIs retain their prior platform scope.

## Evidence

Real Linux subprocess tests cover binary I/O, simultaneous pipe backpressure, exact/exceeded/zero quotas, nonzero exits, incomplete requests, CPU-bound workers ignoring TERM, blocked stdin, inherited pipe holders, parent deadlines, explicit/abandoned cancellation, runtime shutdown, pool clone/drop behavior, capacity reuse, zero cleanup allowance, history eviction, environment/cwd isolation, literal arguments, startup failures, and reaped direct PIDs. Unit checks cover quarantined capacity without history and identifier overflow before entry, and cleanup failures after pending-result handoff. Two R25 corpus cases pin exact output admission and failure. An executed Rust documentation example uses the public API without a Tokio time/I/O driver. Complete process-tree and whole-run shutdown evidence remains required before closing the parent roadmap item.
