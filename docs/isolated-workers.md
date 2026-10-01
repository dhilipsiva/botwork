# Isolated Worker Supervision

`core::worker::WorkerPool` runs trusted external worker executables with independent supervision on Linux, macOS, and Windows. The default pool supervises the direct child and its inherited process group; `with_process_tree` also owns detached descendants and cleans up after the host exits, through a dedicated guardian on Linux and the worker's Job Object on Windows, while macOS refuses it; `with_pid_namespace` adds kernel containment when that guardian fails. It provides the process lifecycle needed when an in-process callback cannot cooperate with cancellation. The existing `NativeOperation::blocking` contract is unchanged: Rust callbacks running inside the host still cannot be forcibly terminated.

This is the byte-oriented execution boundary beneath the [typed worker protocol](worker-protocol.md). `NativeOperation::isolated` supplies bounded typed arguments, results, diagnostics, signatures, and shared ownership across this boundary. Async DSL dispatch, persistent run/report recovery after a host crash, and the complete milestone 6 shutdown coordinator remain separate TODO items. This implementation does not claim those integrations are complete.

## Start, Admission, and Isolation

Create a pool with `WorkerLimits`, then call `start(command, input, control)`. Admission happens synchronously before a supervisor thread or child is started. A stopped control, invalid configuration, oversized request, closed pool, or full capacity returns a structured error. There is no queue retaining unlimited requests. Cloned pools share capacity, shutdown state, monotonically increasing identifiers, and reconciliation history.

`WorkerCommand` supplies an absolute executable path, literal OS-string arguments, an absolute working directory, and an explicit environment map. The parent environment is cleared for the child; host environment and working directory never change. No shell is selected or command string constructed by the supervisor. A host can explicitly launch a shell, as with any executable. Host-owned command/argument/environment construction and executable selection remain trusted host responsibilities.

The child has piped stdin/stdout/stderr and a new process group. One owned OS thread performs process creation, pipe I/O, termination, reaping, and handle closure. A separate observer publishes results and retains reconciliation state. All parent pipe descriptors are nonblocking. Each supervision turn performs at most one 4 KiB input write and one bounded read of each output stream, then checks process state; the next turn follows at once when data moved, and after 5 ms otherwise. The observer checks stops every 5 ms under ordinary host scheduling; it never performs worker OS calls or holds its state lock across them. Worker progress and deadlines do not require polling an async task or keeping a Tokio runtime alive.

```rust
# #[cfg(unix)]
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
# #[cfg(not(unix))]
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

Dropping a handle marks it abandoned before waking cancellation. Stop observation
samples cancellation before re-reading abandonment, so a drop between the two
observations is classified as `Interrupted`. Explicit cancellation remains
`Cancelled`, deadlines remain `TimedOut`, and an already observed/published
terminal outcome is preserved.

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
- `TreeReaped`: the guardian verified and reaped the complete adopted worker tree, then exited (see below).
- `NamespaceReaped`: an owned kernel wait confirmed that the namespace init and its members are gone, but guardian acknowledgment did not establish worker completion. This never establishes success or a worker exit code.
- `Reaped`: the direct child was reaped and inherited-group termination was requested. This does not assert ownership/reaping of grandchildren.
- `Pending`: startup or cleanup exceeded its allowance. A terminal unsuccessful report returns while the supervisor retains the unresolved launch or child and its capacity slot. Once its stalled OS call returns, the owner closes pipes and continues termination/reaping. Later snapshots preserve the same worker ID and outcome with `Reaped`, or `NotStarted` after a late launch error.
- `Unverified`: host interference or a supervisor failure prevented verification. Lost-ownership slots are quarantined rather than reused silently. In particular, a host reaping this child or changing SIGCHLD policy violates the exclusive ownership contract; the supervisor must not risk signalling a recycled PID.

`shutdown()` closes admission and requests cancellation of all owned workers immediately. It returns a snapshot rather than blocking on kernel cleanup. `snapshot()` exposes active IDs/PIDs/stopping state, active cleanup status even after history eviction, completed outcome/cleanup metadata, and a count of omitted history records. History eviction never implies success; use the optional journal below when worker metadata must survive host loss. Exactly one metadata record is retained per completed worker until bounded eviction. An early `Pending` report is reconciled when its supervisor finishes, including a late startup result. Dropping the last pool owner requests shutdown; supervisor ownership outlives the pool and runtime until cleanup completes.

This is not a security sandbox or an absolute real-time kernel guarantee. A worker can consume child memory or create descendants outside its initial process group. For the default pool, deliberately detached descendants, host crashes, suspended hosts, uninterruptible kernel operations, and blocked OS process creation/signalling remain outside the termination guarantee. Stalled process creation and post-launch OS calls have independently observable pending results, but that result does not assert termination. The supervisor reaps only its direct child; orphaned descendants are reaped by their eventual parent. Use the guardian mode below for trusted detached descendants and cleanup after host death. The optional journal below provides worker recovery records; kernel containment when the guardian itself fails requires the opt-in namespace mode below. Never install a competing child reaper or ignore SIGCHLD while this pool owns children.

The default pool runs on Linux and macOS with the same process-group supervision, and on Windows in a Job Object: the worker starts suspended, joins a job of its own, and only then runs, so no descendant starts outside it. Stopping it terminates the whole job, a deliberately detached descendant included, and the job ends everything left in it when the supervisor closes it or the host exits. The process-tree and PID-namespace modes need Linux ([D12](decisions.md#d12-platform-parity)). The language's existing synchronous/async host APIs retain their prior platform scope.

The [platform and facility matrix](worker-platforms.md) lists each mode's prerequisites and executable refusal checks. Unavailable baseline facilities fail validation rather than silently skipping worker tests.

## Process-tree Guardians

Use `WorkerPool::with_process_tree(limits, absolute_botwork_path)` when an operation can fork detached descendants or must clean up after its host exits. The path names the matching installed Botwork CLI executable, which enters a private guardian mode before normal CLI parsing. The existing `WorkerCommand` still selects the actual worker with literal arguments and its explicit environment/cwd. No shell wrapper, additional package, or elevated privilege is required. Pool clones and `NativeOperation::isolated` share the same guardian mode and quotas.

This configuration example checks the constructor without launching a worker; replace the installation path before calling `start`:

```rust
# #[cfg(target_os = "linux")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use botwork::core::worker::{WorkerLimits, WorkerPool};
use std::{path::PathBuf, time::Duration};
assert!(WorkerPool::with_process_tree(WorkerLimits::default(), PathBuf::from("relative")).is_err());
let pool = WorkerPool::with_process_tree(
    WorkerLimits::default(), PathBuf::from("/opt/botwork/bin/botwork"),
)?;
assert!(pool.snapshot().active.is_empty());
assert!(pool.shutdown_wait(Duration::ZERO)?.closed);
# Ok(())
# }
# #[cfg(not(target_os = "linux"))]
# fn main() {}
```

Each invocation gets a separate guardian process. Only that process becomes a [child subreaper](https://man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html), so double-forked descendants are adopted there without changing the host's child-reaping policy or claiming unrelated host children. Session changes and process-group detachment do not escape this ancestry. The host snapshot PID identifies the guardian; a completed report's exit status belongs to the actual worker, including its original exit code or terminating signal.

The host opens a [PID descriptor](https://man7.org/linux/man-pages/man2/pidfd_open.2.html) for its own process before launching the guardian. The guardian polls that stable identity as well as a private Unix control socket. Cancellation closes the socket's write side. Host termination also starts cleanup, even if an unrelated forked host child retains a copy of the socket. Both guardian descriptors are close-on-exec before the actual worker starts, and descriptor placement handles hosts whose standard descriptors are closed. The guardian has its own process group, separate from the host.

After the worker exits, or a stop is observed, the guardian repeatedly signals its current direct children and reaps exited children, including newly adopted orphans. Child enumeration uses fixed 4 KiB scratch space; each reap batch is capped at 64 before checking control again. The [proc children list can omit changing children](https://man7.org/linux/man-pages/man5/proc_tid_children.5.html), so an empty list is never proof of cleanup. Before signalling each listed PID, a non-reaping wait validates current ownership. Only [waitpid with no remaining children](https://man7.org/linux/man-pages/man2/waitpid.2.html), using the Linux all-child option, establishes completion. PID values are never retained for later signalling after reaping.

A fixed 13-byte private acknowledgment reports complete-tree cleanup, a pre-worker startup failure, or cleanup errors. The host requires the complete frame, EOF, and successful guardian exit. `TreeReaped` means the actual worker and its adopted descendants were reaped and the guardian also exited. It does not itself mean the operation succeeded. Missing, malformed, or unsuccessful guardian completion becomes `Unverified`, quarantining the slot. Startup failures with a valid acknowledgment become `NotStarted`. The typed bridge accepts successful values only after `TreeReaped` (or the default mode's `Reaped`), full I/O, final progress, successful worker status, and no stop.

The guardian explicitly shuts down its socket's sending side after writing the
complete acknowledgment. A concurrent host fork can inherit a copy of that
endpoint before close-on-exec takes effect; merely exiting the guardian would
then leave EOF dependent on the unrelated process closing its copy. Explicit
[write shutdown](https://man7.org/linux/man-pages/man2/shutdown.2.html) finishes the
shared socket direction even while descriptor copies remain open. If the host
has gone, finished cleanup and its journal receipt remain valid even when socket
delivery fails. A live host still requires exact framing, EOF, and successful
exit, with no retries or relaxed completion checks. [Regression evidence](guardian-completion-evidence.json)
records the deterministic GNU/musl reproduction, boundary tests, concurrent
process-tree repetitions, and targeted fault checks.

The overall execution timeout includes tree cleanup; the cleanup observation allowance starts when the host observes a stop or its direct guardian exits. All host handles, including the control socket, close before final publication. The guardian remains alive while cleanup is pending. The host never kills its guardian to meet the cleanup observation allowance, since doing so would discard descendant ownership. A stalled or stopped guardian therefore produces `Pending` while the slot and typed reservations remain held. Resuming it reconciles the original outcome with `TreeReaped`. Host death leaves the guardian running until its descendants settle; the host's eventual reaper owns the orphaned guardian itself. This cleanup does not create a durable run result or undo completed effects.

This mode requires Linux 5.3 or later with PID descriptors, subreaper support, proc child enumeration, and permission for Unix-socket IPC and signalling the worker's descendants. Missing facilities or a mismatched helper fail without silently falling back to direct-child guarantees. Workers and the configured helper are trusted host executables. Privilege changes that remove signal permission, interference with the guardian, a killed/crashed guardian, host suspension, and uninterruptible kernel calls remain outside this guardian-only guarantee. The namespace mode below contains guardian failure. This is not a security sandbox or a kernel real-time guarantee. Durable worker metadata requires the optional journal below.

### Windows and macOS

Windows needs no guardian. Each worker already runs in a Job Object of its own
that forbids breakaway, so the job holds every descendant, detached ones
included, and closing it, or the host exiting, ends them all. The guardian path
must still be absolute but is not run. After the worker exits or is stopped,
the supervisor terminates the job and reports `TreeReaped` only once the job
holds no process.

macOS cannot follow a process that leaves the worker's group: it has no child
subreaper or PID namespace, and kqueue stopped tracking forks in Mac OS X 10.5.
`with_process_tree` therefore fails there with BW7002 rather than run a weaker
mode under the same name ([D12](decisions.md#d12-platform-parity)).
`with_recovery` keeps the [worker journal](#durable-worker-journal) on Windows
too, and `with_pid_namespace` is built only on Linux.

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

Guardian evidence includes three unit checks for status frames, bounded child-list scanning, and a stalled final control-socket close, eight subprocess checks plus a dedicated host fixture, two R29 corpus cases, and the executed configuration example. Tests verify double forks and new sessions, normal/error/signal exits, large I/O and environment preservation, cancellation/deadlines/abandonment, paused-guardian reconciliation, typed ownership across runtime loss, missing/mismatched executables, unaffected unrelated children, host SIGTERM/SIGKILL, closed host standard descriptors, and host death while another fork retains the control channel. Subprocess tests require the advertised IPC permissions; a command sandbox denying Unix socket operations is not a supported guardian environment.


## Durable worker journal

On Linux and Windows, `worker::journal::WorkerJournal::open(absolute_path, maximum_records)` opens or creates a private directory, and `WorkerPool::with_recovery(limits, absolute_botwork_path, journal.clone())` enables persistence for a process-tree pool. Existing constructors keep their original behavior. Linux requires the matching CLI helper; Windows checks that the path is absolute but runs no helper. `WorkerHandle::journal_id()` identifies the durable invocation; save it before consuming the handle. IDs combine a random 128-bit opening session with a sequence shared by all pools using the same journal. Pool-local `id()` values remain unchanged.

This executed example configures recovery without launching a worker. Replace the helper path before entry:

```rust
# #[cfg(any(target_os = "linux", windows))]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use std::{num::NonZeroUsize, time::{Duration, SystemTime, UNIX_EPOCH}};
use botwork::core::worker::{WorkerLimits, WorkerPool, journal::WorkerJournal};
let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
let directory = std::env::temp_dir().join(format!("botwork-journal-doc-{}-{nonce}", std::process::id()));
let journal = WorkerJournal::open(&directory, NonZeroUsize::new(128).unwrap())?;
let helper = if cfg!(windows) { r"C:\path\to\botwork.exe" } else { "/absolute/path/to/botwork" };
let pool = WorkerPool::with_recovery(WorkerLimits::default(), helper.into(), journal.clone())?;
assert!(journal.records()?.is_empty());
assert_eq!(journal.flush_wait(Duration::ZERO)?.pending, 0);
drop(pool);
drop(journal);
std::fs::remove_dir_all(directory)?;
# Ok(())
# }
# #[cfg(not(any(target_os = "linux", windows)))]
# fn main() {}
```

Every accepted reservation consumes one record of the configured maximum, including old sessions. Exhaustion returns BW8001 before worker entry. There is no eviction or automatic replay. Each file contains exactly 256 logical bytes in four independently checked slots: intent, published host outcome, guardian receipt, and final host reconciliation. Records omit input/output, arguments, environment, executable paths, PIDs, and diagnostic text. Directory entries, filesystem allocation overhead, and the empty lock file add storage beyond the logical record size. Choose the record limit for available disk capacity; disk errors fail intent before entry or are exposed by flush failures. Use a new directory when a retained journal fills. Archiving/deletion is a host responsibility after all owners and old guardians have settled; do not remove live records or the lock file.

The OS owner writes and syncs intent, then syncs the containing directory **before launching the guardian** (on Windows, before creating the worker). A failed intent write prevents entry. Cancellation or timeout during a stalled write can still publish `Pending`, retaining capacity and typed reservations until that write returns. The owner checks the stop again before launching. The observer only queues fixed metadata and never waits for disk. At most three metadata jobs exist per reserved record: on Windows a tree receipt, then the publication and the reconciliation. Journal jobs do not retain worker payloads or typed reservations. The writer closes each record after final reconciliation, outside observation locks, before acknowledging that flush.

A report's publication queues its metadata before waking the receiver. After awaiting a report, `journal.flush_wait(timeout)` waits for already accepted host writes, returning `JournalFlush { pending, failed }`. Both must be zero to acknowledge successful persistence. A timeout exposes unfinished writes without cancelling them; failures remain counted for that opening session. This wait has a monotonic bound and does not perform filesystem operations. It does not drain workers or promise that future reconciliation jobs have been queued. `shutdown_wait` and journal flushing are separate operations. Ordinary report delivery alone does not acknowledge disk durability. `open` and `records` perform explicit synchronous filesystem operations and may block; call them outside latency-sensitive worker observation.

The guardian inherits only its own record descriptor. After kernel-confirmed tree settlement, or a verified pre-worker failure, it writes and syncs a separate receipt before acknowledging the host. A `TreeSettled` receipt proves that worker descendants were reaped; it does not prove guardian exit, complete transport I/O, typed validation, or a successful operation. A failed receipt write prevents a verified acknowledgment and quarantines the host slot. Stalled receipt syncing leaves cleanup observable as pending.

Windows has no guardian process, so the pool queues the receipt on the journal's writer, ahead of the publications that follow it: `TreeSettled` with the worker's exit code once its terminated Job Object holds no process, or `NotStarted` with the Windows error code when the worker could not be created. Its `errno` field holds that Windows code. A lost host closes the job, which ends the whole tree, so no late receipt follows: the record recovers as `Interrupted`, or keeps a stop the host had published.

`records()` validates filename/slot identities, version, canonical fields, checksum, fixed file length, and consistency. A concurrent partial write can produce a conservative `damaged` snapshot; rescan after flushing or cleanup. The CRC detects accidental corruption/torn writes and is not authentication. Read/open failures are returned explicitly. For a previous session, a missing host publication recovers as `Interrupted`, even if a guardian later reports exit zero. Valid cancelled, timed-out, failed, or interrupted publications retain their unsuccessful category through missing, late, conflicting, or damaged cleanup evidence; `damaged` still exposes uncertainty. Recovery reports success only when intact, matching host publication/reconciliation records establish full transport completion and agree with a successful guardian receipt. Current-session records without a publication have `outcome: None` unless damaged.

These outcomes describe the worker transport. A typed protocol error can follow successful transport, and no case/run identity or typed result is persisted here. Durable case/run reporting, automatic resumption policy, and the full shutdown coordinator remain separate work. Recovery never signals a stored numeric PID, steals a live journal lock, or reruns an invocation.

A journal directory must be owned by the effective user and inaccessible to group/others. Record and lock files must be owned, private, regular, and have one link. Opens are relative to a retained directory descriptor with symlink rejection. Unexpected entries fail scanning. An exclusive nonblocking [flock](https://man7.org/linux/man-pages/man2/flock.2.html) prevents competing host writers; a blocked writer retains that lock even after public handles are dropped. A forked host process can inherit the lock and delay reopening until its descriptor closes. Old guardians inherit no global lock, so a new host can reopen the journal and observe their later receipts through repeated scans. Configure a trusted parent directory and a local filesystem implementing the required lock and [file/directory sync semantics](https://man7.org/linux/man-pages/man2/fsync.2.html); storage devices/filesystems must honor sync for power-loss durability. Remote filesystems are not advertised.

On Windows the journal creates its directory and files with this user as the owner and a protected access list whose one entry grants this user full access, inherited by everything inside. An existing directory or file must be owned by this user and grant access to no one else; a directory made as usual inherits other accounts' access from its parent and is refused. Files open relative to the directory's handle through `NtCreateFile`, and a final junction or symbolic link is opened as itself and refused. `LockFileEx` holds the exclusive lock, and while the journal is open its directory cannot be renamed or removed. `FlushFileBuffers` syncs records, the directory's entries, and a newly created directory's parent. NTFS provides these semantics; other filesystems are not advertised.

Journal recovery records uncertainty when a guardian dies; it does not provide containment of the remaining tree. This includes the Linux [orphaned stopped-group SIGHUP rule](https://man7.org/linux/man-pages/man2/setpgid.2.html). The namespace mode below contains guardian failure. Unsupported facilities/platforms, external effects, and uninterruptible kernel calls remain explicit boundaries.

Evidence includes eight unit tests for schema corruption, incomplete/conflicting publications, exclusive ownership, quota/identity across reopen, private-file enforcement, failed writes, stalled intent, bounded flush, and lock retention. Two subprocess scenarios plus a dedicated host fixture cover successful/failed/pre-entry completion, metadata privacy, shared quota, host SIGKILL before/after a flushed stop, reopening while an old guardian is paused, and later tree receipts without changing the recovered outcome. The delayed-receipt fixture adopts its deliberately stopped guardian to avoid orphan-group SIGHUP and reaps that exact child. Two R30 corpus cases pin exact quota admission and rejection; the configuration example executes as a doctest. The unit tests and the transport scenario run on Windows too, with a portable Python worker, alongside a check that a record with a second link is refused. Windows adds a check that the journal's directory has a single entry for this user and refuses an inherited access list or a junction, a check that the pool's receipt is queued once and ahead of the publications, and a host killed before and after a flushed stop: its job ends the worker and the descendant it started, and the record recovers as `Interrupted` without a receipt, or as the reaped stop with one.


## Kernel containment with PID namespaces

Linux hosts can select `WorkerPool::with_pid_namespace(limits, absolute_botwork_path, optional_journal)` when cleanup must survive guardian failure. This adds a kernel lifetime boundary to the existing guardian protocol. The helper is created directly as PID 1 of a fresh namespace; there is no intermediate launcher whose survival is needed to own that init. Existing constructors retain their documented guarantees.

This executed constructor example does not launch a worker. Actual namespace/facility checks happen on the owned OS thread before worker entry:

```rust
# #[cfg(target_os = "linux")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use botwork::core::worker::{WorkerLimits, WorkerPool};
let pool = WorkerPool::with_pid_namespace(
    WorkerLimits::default(), "/absolute/path/to/botwork".into(), None,
)?;
assert!(pool.snapshot().active.is_empty());
assert!(!pool.snapshot().closed);
# Ok(())
# }
# #[cfg(not(target_os = "linux"))]
# fn main() {}
```

The launcher uses [clone3](https://man7.org/linux/man-pages/man2/clone.2.html) with new user, PID, and mount namespaces, separate memory/descriptors, and an ordinary SIGCHLD child relationship. It prepares literal argv/environment and descriptors before cloning and blocks all kernel signal-mask bits across clone/exec. The parent restores its prior mask; the helper clears its mask after exec has reset caught handlers. The child branch uses fixed stack data and syscall wrappers until exec, avoiding inherited Rust locks/allocators/destructors. The parent retains an unreaped child guard before mapping work, maps only its effective UID/GID to the same numeric values, denies setgroups, and releases a one-byte startup gate through the private control socket using MSG_NOSIGNAL. Early child exit cannot deliver SIGPIPE to the host. Any mapping failure kills and reaps the guarded child without entering the worker.

Before exec, the child arms SIGKILL on creation-thread death, checks a stable host process descriptor to close the host-death setup race, starts a separate session, makes mount propagation private, mounts read-only proc for its own PID namespace, sets no-new-privileges, and installs cwd/stdio/private descriptors. Bootstrap errors use the bounded pre-worker failure acknowledgment; missing acknowledgments remain failures. A stopped guardian is still killed by creation-thread/host death. The owned OS thread remains alive until its child is reaped.

[Linux terminates a PID namespace's members when its init dies](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html). Stops therefore SIGKILL the known, unreaped namespace init rather than requiring its event loop to react. The [kernel waits for remaining namespace processes before init can be reaped](https://github.com/torvalds/linux/blob/v6.18/kernel/pid_namespace.c#L180). Forking, double forks, session changes, and nested descendant PID namespaces do not move processes into an ancestor PID namespace. The host never signals this numeric PID after relinquishing wait ownership.

Normal guardian completion still requires full acknowledgment, full transport I/O, successful worker status, and no stop for success. It returns `TreeReaped` and the actual worker status. If the guardian fails or is killed, a successful owned wait returns `NamespaceReaped` with an unsuccessful outcome and no inferred worker status. Capacity and typed reservations can then be released once their normal payload owners finish. A valid typed response received before guardian death cannot become a successful result. Published cancellations/timeouts/interruptions remain unchanged through late kernel cleanup.

A stalled namespace wait still produces `Pending` and retains capacity/typed ownership until it completes; loss of wait ownership remains `Unverified` and quarantined. Namespace termination is a kernel containment guarantee, not an absolute deadline for uninterruptible calls, tracing interference, or host scheduling. It does not authorize a competing child reaper or ignored SIGCHLD in the embedding host.

With `Some(journal)`, intent is durable before namespace creation, and kernel-confirmed unsuccessful cleanup is stored as `NamespaceReaped` using the existing publication/reconciliation slots. Current readers accept this cleanup code; older readers reject unknown codes conservatively. Guardian receipts retain their separate meaning. After host loss, a missing publication still recovers as `Interrupted`; recovery does not fabricate a cleanup receipt just because namespace mode was used. Complete typed/case/run persistence and automatic replay policy remain separate work.

This mode requires Linux 5.3+ clone3/PID-descriptor support, enabled user/PID/mount namespaces, permission to create those namespaces and mount proc, proc UID/GID mapping interfaces, and the existing guardian socket/signalling facilities. Namespace limits, seccomp, LSM/AppArmor policy, or disabled kernel features can refuse entry. Refusal returns an error before worker effects, with no fallback to a weaker pool. Tests exercise real ENOSYS refusals for clone3 and mount through restricted subprocesses. Deployment must explicitly allow the required facilities; a broad platform/facility support matrix remains a separate roadmap check.

Workers see namespace-local process IDs and private proc, and retain only the invoking effective identity mapping; unmapped identities/groups have namespace-specific representations. Setuid/file-capability elevation is disabled. Filesystem access outside proc and network access remain the host's existing permissions and mounts. This is process-lifetime containment for trusted workers, not a general security sandbox or rollback of external effects. Choose the guardian-only constructor when the operation needs ordinary host PID/proc/credential behavior and its weaker failure boundary is acceptable.

Evidence includes two gated unit checks for delayed kernel waits and stolen wait ownership, plus an isolated SIGPIPE regression; eight subprocess scenarios plus host/refusal fixtures for large binary I/O, identity/proc consistency, nonzero/signal statuses, guardian SIGKILL, detached descendants, stopped-guardian cancellation, host SIGKILL with closed stdio, journal recovery, typed response rejection and capacity release, helper mismatch, and denied facilities; two R31 corpus cases; and the executed constructor example. Full debug/release regression checks also cover unchanged direct-child and guardian-only modes.
