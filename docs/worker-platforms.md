# Worker Platform and Facility Evidence

The worker API currently advertises Linux with the facilities required by the selected mode. A Linux version number alone does not establish support: kernel configuration, namespace limits, proc mounts, seccomp, and host policy also govern entry. Constructors validate configuration; the owned launch thread checks actual facilities for each invocation. Required facilities are never replaced with weaker worker ownership automatically.

## Mode Requirements

| Mode | Required facilities | Cleanup evidence |
| --- | --- | --- |
| `WorkerPool::new` | OS threads, process creation/exec, pipes with nonblocking access, process groups, signalling, exclusive child waits | Direct child reaped; inherited group termination requested |
| `with_process_tree` / `with_recovery` | Group-mode facilities, PID descriptors, Unix socket pairs, child subreapers, readable `/proc/thread-self/children` | Guardian acknowledgment and whole-tree reaping |
| `with_pid_namespace` | Guardian facilities, `clone3`, user/PID/mount namespaces, proc identity mappings, private mount propagation, proc mounting, sessions, parent-death signalling, `no_new_privs` | Whole-tree acknowledgment, or owned namespace reaping without claiming worker success |
| Optional worker journal | Private local directory, exclusive advisory locks, random IDs, positional record I/O, file and directory synchronization | Transport interruption/publication records; separate flush acknowledgment |

[PID descriptors](https://man7.org/linux/man-pages/man2/pidfd_open.2.html) and [clone3](https://man7.org/linux/man-pages/man2/clone.2.html) were added in Linux 5.3. That is an interface minimum for the latter two modes, **not a tested minimum-kernel certification**. Other operating systems reject `start` before effects; Linux-specific constructors and journaling are not exported there. No non-Linux worker backend is advertised.

Workers and helpers remain trusted executables. These facilities establish process ownership, not filesystem/network isolation, rollback, or a real-time kernel guarantee. See [worker guarantees and limitations](isolated-workers.md).

## Executable Refusal Matrix

`tests/worker_facilities.rs` runs each combination in a fresh subprocess. An inherited [seccomp errno filter](https://man7.org/linux/man-pages/man2/seccomp.2.html) denies one syscall or one selected syscall argument. The fixture first proves that the installed rule returns the selected errno; it then attempts two invocations through a pool with capacity one. This exercises the public API and actual helper executable without changing machine-wide policy.

Each row runs with both `ENOSYS` (unavailable interface) and `EPERM` (policy refusal), subject to the explicit clone3 exception below. “Runs” means the unrelated mode still completes its worker, reports success with its promised cleanup, and releases capacity. “Refuses” means no worker marker is created, a BW5003 failure is returned, no child remains to reap, and the second invocation can reuse capacity.

| Denied facility | Group | Guardian | Namespace |
| --- | --- | --- | --- |
| `pidfd_open` | Runs | Refuses | Refuses |
| `socketpair` | Runs | Refuses | Refuses |
| `prctl(PR_SET_CHILD_SUBREAPER)` | Runs | Refuses | Refuses |
| `clone3` with `ENOSYS` | Runs | Runs | Refuses |
| `clone3` with `EPERM` | Not promised | Not promised | Refuses |
| `mount` with private propagation flags | Runs | Runs | Refuses |
| `mount` with proc mount flags | Runs | Runs | Refuses |
| `prctl(PR_SET_PDEATHSIG)` | Runs | Runs | Refuses |
| `prctl(PR_SET_NO_NEW_PRIVS)` | Runs | Runs | Refuses |
| `setsid` | Runs | Runs | Refuses |
| `execve` | Refuses | Refuses | Refuses |

The table covers **58 denied-facility combinations and three enabled controls**, each exercised twice: **122 invocation attempts per build profile**. Ten matrix tests, one enabled-control test, and one subprocess fixture appear in Cargo's 12-test summary. No unsupported facility is silently skipped. Missing baseline facilities fail the controls.

Global `clone3` refusal with `EPERM` can also stop libc from creating a host thread. A synchronous BW5003 rejection is valid in that configuration; it must leave no capacity or worker effects. The matrix does not promise that unrelated modes work under that policy. `ENOSYS` permits libc's ordinary process/thread fallback, while the explicitly selected namespace mode still refuses entry.

Most refused entries report `NotStarted` and the injected OS error. Parent-death setup happens before the namespace mapping gate: early child exit can race the parent's map writes or socket release. The diagnostic can therefore describe that later mapping/socket failure. If the acknowledgment is lost, `NamespaceReaped` is valid only after the owned kernel wait; it still reports failure with no inferred worker exit status. The matrix checks both permitted cleanup outcomes and verifies no surviving child or worker effect.

This matrix tests actual syscall failure paths on the executing kernel. It does not emulate every older kernel, distribution policy, proc visibility restriction, namespace quota, filesystem durability model, lost signal permission, or uninterruptible kernel failure. The existing worker, guardian, namespace, and recovery suites separately exercise live ownership, cancellation, host loss, protocol handoff, and journal behavior. Gated unit tests provide evidence for stalled observation; they do not simulate kernel termination.

## Reproduction and Platform Coverage

Run the facility matrix with locked dependencies:

```sh
cargo test --locked --test worker_facilities
cargo test --locked --release --test worker_facilities
```

For lifecycle coverage on a selected Linux Rust target:

```sh
cargo test --locked --target x86_64-unknown-linux-gnu --test isolated_workers --test typed_workers --test worker_trees --test worker_recovery --test worker_namespaces --test worker_facilities
```

Repeat with `--release`. GNU and musl builds require separate execution; compiling a target alone is not runtime evidence. Test prerequisites include matching Botwork library/CLI builds, a writable temporary directory, `/usr/bin/touch`, the shell/core utilities and Python used by the existing fixtures, and all facilities listed above. Fault injection additionally requires seccomp filter installation; this is a test dependency, not a production worker requirement.

The [checked-in evidence](worker-platform-evidence.json) records source/lockfile hashes, commands, counts, kernel/libc/toolchain, process policy, identity, namespace quotas, and fixed campaign limits. The initial GNU capture ran on x86_64 Ubuntu 26.04 with glibc 2.43 and WSL2 kernel 6.18.33.2. Both profiles passed the prior 1,279-test full suite at revision `4de145a`, then the new 12-test facility suite against the recorded source hashes. This is one observed kernel/environment, not evidence for every Linux installation.

Broader kernel/distribution/architecture coverage, hosted CI observation, durable typed/case/run outcomes, and the full shutdown coordinator remain roadmap work.
