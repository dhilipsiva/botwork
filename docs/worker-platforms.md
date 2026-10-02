# Worker Platform and Facility Evidence

The worker API advertises Linux with the facilities required by the selected mode, macOS for the default pool, which needs only process groups, pipes, signals, and `waitid`, and Windows for the default pool, which needs Job Objects, suspended process creation, and anonymous pipes in non-waiting mode; the process-tree mode needs Linux or Windows, and the namespace mode Linux. `tests/worker_backends.rs` runs the default pool and the process statements on all three. A Linux version number alone does not establish support: kernel configuration, namespace limits, proc mounts, seccomp, and host policy also govern entry. Constructors validate configuration; the owned launch thread checks actual facilities for each invocation. Required facilities are never replaced with weaker worker ownership automatically.

## Mode Requirements

| Mode | Required facilities | Cleanup evidence |
| --- | --- | --- |
| `WorkerPool::new` | OS threads, process creation/exec, pipes with nonblocking access, process groups, signalling, exclusive child waits | Direct child reaped; inherited group termination requested |
| `with_process_tree` / `with_recovery` | Group-mode facilities, PID descriptors, Unix socket pairs, child subreapers, readable `/proc/thread-self/children` | Guardian acknowledgment and whole-tree reaping |
| `with_process_tree` on Windows | Job Objects that forbid breakaway and kill on close, suspended process creation, job accounting | Job termination, and no process left in the job |
| `with_pid_namespace` | Guardian facilities, `clone3`, user/PID/mount namespaces, proc identity mappings, private mount propagation, proc mounting, sessions, parent-death signalling, `no_new_privs` | Whole-tree acknowledgment, or owned namespace reaping without claiming worker success |
| Optional worker journal | Private local directory, exclusive advisory locks, random IDs, positional record I/O, file and directory synchronization; on Windows an owner-only access list, relative opens through `NtCreateFile`, `LockFileEx`, and NTFS | Transport interruption/publication records; separate flush acknowledgment |

[PID descriptors](https://man7.org/linux/man-pages/man2/pidfd_open.2.html) and [clone3](https://man7.org/linux/man-pages/man2/clone.2.html) were added in Linux 5.3. That is an interface minimum for the latter two modes, **not a tested minimum-kernel certification**. Other operating systems reject `start` before effects. The PID-namespace constructor is exported on Linux only and the journal on Linux and Windows, and macOS refuses `with_process_tree` at construction.

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

The syscall table covers **58 denied-facility combinations and three enabled controls**. Each combination is exercised twice. No unsupported facility is silently skipped. Missing baseline facilities fail the controls.

Global `clone3` refusal with `EPERM` can also stop libc from creating a host thread. A synchronous BW5003 rejection is valid in that configuration; it must leave no capacity or worker effects. The matrix does not promise that unrelated modes work under that policy. `ENOSYS` permits libc's ordinary process/thread fallback, while the explicitly selected namespace mode still refuses entry.

Most refused entries report `NotStarted` and the injected OS error. Parent-death setup happens before the namespace mapping gate: early child exit can race the parent's map writes or socket release. The diagnostic can therefore describe that later mapping/socket failure. If the acknowledgment is lost, `NamespaceReaped` is valid only after the owned kernel wait; it still reports failure with no inferred worker exit status. The matrix checks both permitted cleanup outcomes and verifies no surviving child or worker effect.

Four further tests run twelve environment combinations, again with two invocation attempts each:

| Environment | Group | Guardian | Namespace |
| --- | --- | --- | --- |
| Empty read-only tmpfs hides `/proc` | Runs | Refuses with `ENOENT` | Refuses with `ENOENT` |
| Read-only proc bind mount | Runs | Runs | Refuses mapping with `EROFS` |
| `RLIMIT_NOFILE` soft limit zero | Refuses with `EMFILE` | Refuses with `EMFILE` | Refuses with `EMFILE` |
| `user.max_user_namespaces` of zero | Runs | Runs | Refuses with `ENOSPC` |

The namespace quota is real, not injected: the fixture is the root of a user namespace of its own, where it may lower the quota for what it starts, as a host sets `user.max_user_namespaces`.

A [worker journal](isolated-workers.md#durable-worker-journal) needs a writable local filesystem. On a read-only bind mount, opening one is refused with `EROFS`, both where its directory is missing and where a private one exists, so no worker starts without the records it was promised.

Proc fixtures execute in fresh user/mount namespaces via `unshare --map-current-user --keep-caps --mount --propagation private`. They check that their mount namespace differs from the parent before changing mounts. [Bind-remount flags](https://man7.org/linux/man-pages/man2/mount.2.html) restrict that mount rather than making the shared proc filesystem read-only. Namespace capabilities are retained only in these fixture processes so they can configure the test mounts after exec. The fixture proves that proc is missing or read-only before launching workers. Descriptor exhaustion changes only the fixture's soft limit and restores it before writing its verification record. These cases report `NotStarted`, create no marker, release their single slot, and leave no child to reap.

Together these are **73 combinations and 146 invocation attempts per build profile**, plus the two journal cases. Ten syscall matrix tests, four environment tests, one enabled-control test, the journal test, and two subprocess fixtures appear in Cargo's 18-test summary.

This matrix tests actual syscall and environment failure paths on the executing kernel. It does not emulate every older kernel, distribution policy, proc visibility restriction, filesystem durability model, lost signal permission, or uninterruptible kernel failure. The existing worker, guardian, namespace, and recovery suites separately exercise live ownership, cancellation, host loss, protocol handoff, and journal behavior. Gated unit tests provide evidence for stalled observation; they do not simulate kernel termination.

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

Repeat with `--release`. GNU and musl builds require separate execution; compiling a target alone is not runtime evidence. Test prerequisites include matching Botwork library/CLI builds, a writable temporary directory, `/usr/bin/touch`, `/usr/bin/unshare` with `--keep-caps` and namespace mapping support, the shell/core utilities and Python used by the existing fixtures, and all facilities listed above. Fault injection additionally requires seccomp filter installation, private tmpfs/bind mounts, and changing the process's descriptor soft limit; these are test dependencies, not additional production worker requirements.

The [checked-in evidence](worker-platform-evidence.json) records source/lockfile hashes, commands, counts, kernel/libc/toolchain, process policy, identity, namespace quotas, and fixed campaign limits. The initial GNU capture ran on x86_64 Ubuntu 26.04 with glibc 2.43 and WSL2 kernel 6.18.33.2. Both profiles passed the prior 1,279-test full suite at revision `4de145a`; subsequent facility/environment checks are recorded against their source hashes. This is one observed kernel/environment, not evidence for every Linux installation.

The same host also executed static musl builds, using the Rust 1.97.1 `x86_64-unknown-linux-musl` standard-library component identified in the evidence file. Both debug and release passed **1,294 tests**, including **41 doctests**, with no failures or ignored tests. The CLI and Rust test hosts/helpers were built for musl; system fixture tools such as Python and touch retained their installed libc. The GNU environment suite passed all **15 tests** in both profiles. No production change was needed for this libc boundary.

To reproduce the musl build and full suite:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --locked --target x86_64-unknown-linux-musl --all-targets
cargo test --locked --target x86_64-unknown-linux-musl
cargo build --locked --target x86_64-unknown-linux-musl --release --all-targets
cargo test --locked --target x86_64-unknown-linux-musl --release
```

CI defines separate debug/release jobs for GNU and musl, with Clippy checked for both targets. These jobs require the documented facilities on the runner.

## Observed platforms

Each Linux CI job prints, with `scripts/worker_platform.sh`, the kernel, distribution, C library, util-linux, namespace quotas, descriptor limit, and proc and temporary-directory mounts its suites ran on. On 2026-10-02 the worker suites ran on:

| Host | Kernel | Distribution | C library | util-linux | Builds and suites |
| --- | --- | --- | --- | --- | --- |
| This workstation (WSL2) | 6.18.33.2 | Ubuntu 26.04 | glibc 2.43 | 2.41.3 | GNU and musl, debug and release: every test |
| CI `ubuntu-latest` | 6.17.0 (Azure) | Ubuntu 24.04.5 | glibc 2.39 | 2.39.3 | GNU and musl, debug and release: every test |
| CI `ubuntu-22.04` | 6.8.0 (Azure) | Ubuntu 22.04.5 | glibc 2.35 | 2.37.2 | musl, debug: the worker, process, shutdown, terminal-outcome, and JavaScript suites |

CI's `macos-latest` (arm64) and `windows-latest` (x86_64) jobs run every test built for them, in debug and release: on macOS the default pool and process statements, and on Windows also the process-tree mode and the journal.

Linux is advertised on x86_64 ([D5](decisions.md#d5-platforms)) wherever a mode's facilities are present: each start checks them and refuses rather than weaken ownership, so a kernel or distribution not listed here either runs a mode with its full guarantees or refuses it. Linux 5.3 is the interface minimum for the guardian and namespace modes, not a tested floor. GNU builds made from a checkout need glibc 2.36 ([D19](decisions.md#d19-distribution-build)), which Ubuntu 22.04 lacks, so its job runs the static musl build, the one the release ships. Kernels on hosted runners change with their images, so each job's log, not this table, records what a given run used.

What no platform guarantees, such as a real-time bound under a stuck kernel call or rollback of external effects, is listed under [worker limits](isolated-workers.md#limits).
