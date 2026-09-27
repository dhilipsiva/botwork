# Isolated Worker Supervision

`core::worker::WorkerPool` runs trusted external worker executables with independent supervision on Linux. It provides the process lifecycle needed when an in-process callback cannot cooperate with cancellation. The existing `NativeOperation::blocking` contract is unchanged: Rust callbacks running inside the host still cannot be forcibly terminated.

This is the byte-oriented execution boundary beneath the [typed worker protocol](worker-protocol.md). `NativeOperation::isolated` supplies bounded typed arguments, results, diagnostics, signatures, and shared ownership across this boundary. Async DSL dispatch, persistent run/report recovery after a host crash, and the complete milestone 6 shutdown coordinator remain separate TODO items. This implementation does not claim those integrations are complete.

## Start, Admission, and Isolation

Create a pool with `WorkerLimits`, then call `start(command, input, control)`. Admission happens synchronously before a supervisor thread or child is started. A stopped control, invalid configuration, oversized request, closed pool, or full capacity returns a structured error. There is no queue retaining unlimited requests. Cloned pools share capacity, shutdown state, monotonically increasing identifiers, and reconciliation history.

`WorkerCommand` supplies an absolute executable path, literal OS-string arguments, an absolute working directory, and an explicit environment map. The parent environment is cleared for the child; host environment and working directory never change. No shell is selected or command string constructed by the supervisor. A host can explicitly launch a shell, as with any executable. Host-owned command/argument/environment construction and executable selection remain trusted host responsibilities.

The child has piped stdin/stdout/stderr and a new process group. One owned OS thread performs process creation, pipe I/O, termination, reaping, and handle closure. A separate observer publishes results and retains reconciliation state. All parent pipe descriptors are nonblocking. Each supervision turn performs at most one 4 KiB input write and one bounded read of each output stream, then checks process state. The observer checks stops every 5 ms under ordinary host scheduling; it never performs worker OS calls or holds its state lock across them. Worker progress and deadlines do not require polling an async task or keeping a Tokio runtime alive.

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
assert!(report.progress_complete);
assert_eq!(report.stdout, "hello é".as_bytes());
assert!(pool.shutdown().active.is_empty());
# Ok(())
# }
# #[cfg(not(target_os = "linux"))]
# fn main() {}
```

## Startup Observation

The observer delegates `Command::spawn` to an owned OS thread. A launch blocked inside the OS cannot monopolize deadline and cancellation checks. The OS owner checks the stop state again before entering process creation. Each admitted slot permits one outstanding launch, with no queue of additional launch requests. The same OS owner keeps the child throughout I/O and cleanup; no child-handle channel transfer is needed.

When cancellation, abandonment, or a deadline is observed during startup, the observer preserves that terminal outcome. If launch does not settle within `cleanup_timeout`, the caller receives an unsuccessful `Pending` report. Its exit status is unknown, stdin progress is zero, captured output is empty, and both `io_complete` and `progress_complete` are false. No protocol request bytes have been sent. `snapshot()` keeps the original active ID, stopping flag, and pending status; the slot and typed argument/encoded-byte reservations remain owned.

A late child is terminated and reaped without sending its request; a late launch error settles as `NotStarted`. Both retain the already-published outcome, even after the returned handle is dropped. Bounded history records final cleanup metadata, never a replacement success. Late diagnostics cannot retroactively change a delivered report.

An unknown PID does not prove that the kernel has not started a process. Startup effects can occur before a stalled spawn call returns, and no safe child handle is available to signal then. `Pending` reports that uncertainty instead of declaring cleanup complete.

## Post-launch Observation

The observer also runs independently of pipe setup, reads, writes, descriptor closure, process-state observation, signalling, and reaping. Confirmed progress is committed through short in-memory lock sections after OS calls return. Cancellation or a deadline starts the cleanup allowance even while such a call is stalled. Direct-child exit starts that allowance before termination/reaping calls. If the allowance expires before the owner finishes, the observer publishes `Pending` and keeps the same slot and reservations. Cleanup expiry after an otherwise successful exit becomes BW5003, never success.

`progress_complete` distinguishes final I/O progress from an early snapshot. When false, `stdin_written` and captured output describe only the confirmed prefix: an in-flight call can transfer additional bytes before returning, or can have returned before its count is committed. These fields cannot establish that an external effect did not occur. `io_complete` additionally requires complete request transfer and both output EOFs. A normal finalized failure can have complete progress without complete I/O. Typed successful values require both flags, successful exit, and verified direct-child cleanup.

Publishing a pending report moves the captured buffers once. Later reads cannot append another captured payload or change that report; any remaining in-flight read uses a fixed 4 KiB scratch buffer. The OS owner closes pipes and continues termination/reaping once its stalled call returns. It closes remaining pipe handles before reporting completion. The observer never signals a PID from a snapshot. Late completion changes cleanup metadata under the original outcome and releases ownership only after the OS owner settles.

The child has a cleanup guard: Rust's [plain Child handle does not provide that drop behavior](https://doc.rust-lang.org/std/process/struct.Child.html). During an owner panic, the guard attempts termination and reaping on that same OS thread. The observer remains independent even if this fallback stalls. Once unwinding finishes, ownership is marked `Unverified` and its slot quarantined; no successful cleanup claim is inferred from a panic. Reservations outlive dropped handles, pools, and Tokio runtimes while OS work remains unresolved.

These bounds apply under ordinary host scheduling. Host thread creation, scheduler or allocator suspension, actual kernel termination, detached descendants, and host-crash containment remain outside this observation guarantee. A pending result does not assert that an OS call or worker stopped.

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

`Succeeded` requires final confirmed progress, successful direct-child exit, complete request transfer, both output EOFs, completed direct-child reaping, and no observed stop or I/O/resource failure. Nonzero exit, failed launch/I/O, truncated streams, or incomplete request transfer are failures. Reports include confirmed bytes accepted on stdin, bounded output prefixes, exit status when known, a structured diagnostic, and `io_complete`/`progress_complete` flags. This flag describes stream completion, not operation success. No pass may be inferred from output contents or exit status alone.

Stops do not undo external effects and never retry the operation. A deadline returns `TimedOut` with BW5002; explicit cancellation returns `Cancelled` with BW5001. Dropping a handle or its wait future requests termination and records `Interrupted`. Cancelling one handle does not cancel its parent or siblings. An observed stop is retained during cleanup; an I/O/cleanup failure is bounded diagnostic evidence, not permission to report success.

## Termination, Reaping, and Reconciliation

On a stop, the supervisor sends SIGKILL to the child's original process group and to the direct child. This does not depend on callbacks, signal handlers, or cooperative polling in the worker. After ordinary direct-child exit it also terminates remaining members of the inherited group before completing capture. The group leader is observed with `waitid(WNOWAIT)` and remains unreaped until group signalling has been requested. This retains its PID identity across group cleanup; no numeric-PID signal is sent after reaping. See the [Linux wait contract](https://man7.org/linux/man-pages/man2/waitid.2.html), [process-group signal contract](https://man7.org/linux/man-pages/man2/kill.2.html), and [Rust process-group setup](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#method.process_group).

Cleanup status is explicit:

- `NotStarted`: launch was skipped or failed before a worker handle was returned.
- `Reaped`: the direct child was reaped and inherited-group termination was requested. This does not assert ownership/reaping of grandchildren.
- `Pending`: startup or cleanup exceeded its allowance. A terminal unsuccessful report returns while the supervisor retains the unresolved launch or child and its capacity slot. Once its stalled OS call returns, the owner closes pipes and continues termination/reaping. Later snapshots preserve the same worker ID and outcome with `Reaped`, or `NotStarted` after a late launch error.
- `Unverified`: host interference or a supervisor failure prevented verification. Lost-ownership slots are quarantined rather than reused silently. In particular, a host reaping this child or changing SIGCHLD policy violates the exclusive ownership contract; the supervisor must not risk signalling a recycled PID.

`shutdown()` closes admission and requests cancellation of all owned workers immediately. It returns a snapshot rather than blocking on kernel cleanup. `snapshot()` exposes active IDs/PIDs/stopping state, active cleanup status even after history eviction, completed outcome/cleanup metadata, and a count of omitted history records. History eviction never implies success; a caller needing durable evidence must retain reports or persist its own records. Exactly one metadata record is retained per completed worker until bounded eviction. An early `Pending` report is reconciled when its supervisor finishes, including a late startup result. Dropping the last pool owner requests shutdown; supervisor ownership outlives the pool and runtime until cleanup completes.

This is not a security sandbox or an absolute real-time kernel guarantee. A worker can consume child memory or create descendants outside its initial process group. Deliberately detached descendants, host crashes, suspended hosts, uninterruptible kernel operations, and blocked OS process creation/signalling remain outside the termination guarantee. Stalled process creation and post-launch OS calls have independently observable pending results, but that result does not assert termination. The supervisor reaps only its direct child; orphaned descendants are reaped by their eventual parent. Callers requiring containment of arbitrary descendants or recovery after host death need the further worker/shutdown work tracked in TODO. Never install a competing child reaper or ignore SIGCHLD while this pool owns children.

Other platforms reject worker entry before effects. Only Linux is currently advertised for this boundary; the language's existing synchronous/async host APIs retain their prior platform scope.

## Bounded Shutdown Wait

`shutdown_wait(timeout)` closes admission, requests cancellation, and blocks the calling thread while observing supervisor completion. It returns a `WorkerSnapshot` when no active ownership remains or when its monotonic allowance expires. It never joins a possibly stuck OS owner or consumes per-worker reports. Multiple cloned callers can wait independently; completion wakes them even when completed-history storage is disabled. No Tokio runtime is needed.

An empty active list means the pool has reconciled its owned starts/direct children. A nonempty active list means shutdown remains incomplete: retain the pool and inspect later snapshots. Pending and quarantined slots keep their capacity and typed reservations; expiry never changes their outcomes or turns them into completed workers. Completed unpolled report payloads remain separately owned by their receivers. Zero allowance returns the immediate shutdown snapshot. An unrepresentable duration is BW7002 and is rejected before closing admission.

```rust
# #[cfg(target_os = "linux")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use std::time::Duration;
use botwork::core::{
    operation::OperationControl,
    worker::{WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool},
};
let pool = WorkerPool::new(WorkerLimits::default())?;
let handle = pool.start(WorkerCommand {
    executable: "/bin/sh".into(),
    arguments: vec!["-c".into(), "while :; do :; done".into()],
    directory: std::env::temp_dir(), environment: Default::default(),
}, vec![], OperationControl::default())?;
let snapshot = pool.shutdown_wait(Duration::from_secs(2))?;
assert!(snapshot.closed);
assert!(snapshot.active.is_empty());
let runtime = tokio::runtime::Builder::new_current_thread().build()?;
assert_eq!(runtime.block_on(handle.wait()).outcome, WorkerOutcome::Cancelled);
# Ok(())
# }
# #[cfg(not(target_os = "linux"))]
# fn main() {}
```

The allowance bounds the observation wait under ordinary host scheduling, rather than promising that every child or kernel operation will finish within it. Whole-run async task draining, termination during kernel stalls, process-tree containment, and durable host-crash reconciliation remain open.

## Evidence

Real Linux subprocess tests cover binary I/O, simultaneous pipe backpressure, exact/exceeded/zero quotas, nonzero exits, incomplete requests, CPU-bound workers ignoring TERM, blocked stdin, inherited pipe holders, parent deadlines, explicit/abandoned cancellation, runtime shutdown, pool clone/drop behavior, capacity reuse, zero cleanup allowance, history eviction, environment/cwd isolation, literal arguments, startup failures, and reaped direct PIDs. Unit checks cover quarantined capacity without history and identifier overflow before entry, and cleanup failures after pending-result handoff. Two R25 corpus cases pin exact output admission and failure. An executed Rust documentation example uses the public API without a Tokio time/I/O driver. Complete process-tree and whole-run shutdown evidence remains required before closing the parent roadmap item.

Nine launcher tests use owned gates to delay the return of process creation or startup failure, verifying pending results, retained charges, late reaping, stop priority, handle/pool drop, guarded child disposal, launcher panic, and output overflow after an early stop. These simulate the spawn API not returning; they do not claim to reproduce an uninterruptible kernel fault. Three additional public subprocess tests verify shutdown drains, zero/overflow allowances, report preservation, parent isolation, and notification of multiple waiters without history. R27 corpus cases and the executed shutdown example cover the public API.

Eight post-launch unit tests include a nine-boundary stalled-call matrix, cleanup expiry after successful exit, a stalled panic guard, lost child ownership before stalled closure, pool/runtime loss, typed value rejection during a pending read, captured foreign diagnostic preservation during a stalled close, and final reservation release before a blocked receiver wake. They verify confirmed-prefix reporting, capacity/argument/wire retention, late reaping, frozen outcomes, and quarantine. Gates simulate API stalls and do not establish kernel-level termination. R28 corpus cases and the public I/O tests distinguish final progress from complete streams.
