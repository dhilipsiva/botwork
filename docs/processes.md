# Process statements

The default Engine and CLI provide four process signatures within the 100-statement
fixed catalogue. They execute on Linux, both synchronously and asynchronously.
Other platforms return BW7002 before starting a process. A standalone Context
without a run environment also returns BW7002. CLI preparation captures the host
environment and directory using `Context::with_host_environment`; low-level hosts
can use the same constructor before initializing statements.

| Statement | Parameters | Result |
| --- | --- | --- |
| `Run Process \|executable\| With Arguments \|arguments\|` | executable String, arguments Array of Strings | text result Map |
| `Run Process \|executable\| With Arguments \|arguments\| Options \|options\|` | above, options Map | text result Map |
| `Run Binary Process \|executable\| With Arguments \|arguments\|` | executable String, arguments Array of Strings | binary result Map |
| `Run Binary Process \|executable\| With Arguments \|arguments\| Options \|options\|` | above, options Map | binary result Map |

Arguments are passed literally, including empty arguments, spaces, quotes, wildcards,
and shell punctuation. No shell is inserted and no command string is split. To run
a shell script, explicitly select the shell executable and its arguments. The
executable must be nonempty; executable, arguments, directory, and environment
names/values cannot contain NUL. This is ordinary host execution with the host's
permissions, not a sandbox. Only execute programs and shell scripts you trust.

An executable containing `/` resolves against the child directory, including `./`.
A bare executable uses native PATH lookup from the effective child environment.
Removing PATH permits the native implementation's default search path (typically
`/bin:/usr/bin`); use an absolute executable for unambiguous selection. See the
[Rust Command contract](https://doc.rust-lang.org/std/process/struct.Command.html).

The result always has five keys:

| Key | Value |
| --- | --- |
| `stdout` | strict UTF-8 String, or Array of Int bytes for binary calls |
| `stderr` | same representation as stdout |
| `exit_code` | Int exit code, or None when terminated by a signal |
| `signal` | Linux signal number, or None for ordinary exit |
| `success` | true exactly when the process exited with code zero |

Nonzero exit and signal termination return results when all I/O and direct-child
cleanup complete. Use `Assert` on `success` or `exit_code` when that outcome should
fail the script. Captured streams preserve their separate byte order; they do not
provide a combined ordering. Output is captured rather than logged automatically.
Invalid UTF-8 on either text stream is BW3003. Binary calls preserve every byte.

Options are strict: unknown keys, incorrect types, negative limits/timeouts,
non-String arguments, and invalid input bytes fail with BW3003 before launch.
None below means a None value, such as `@{ No Operation }`; it is not a DSL literal.

| Option | Type and default | Meaning |
| --- | --- | --- |
| `directory` | String; run directory | Child working directory; relative paths use the run directory |
| `environment` | Map; empty | String values set exact names, None removes names |
| `inherit_environment` | Bool; true | Start from the immutable run environment snapshot |
| `stdin` | String for text, byte Array for binary; empty | Input sent through a pipe; None also means empty |
| `timeout_ms` | nonnegative Int; 30000 | Wall-clock process allowance, including launch and I/O |
| `cleanup_timeout_ms` | nonnegative Int; 1000 | Allowance to observe cleanup before returning an incomplete-cleanup error |
| `stdout_limit` | nonnegative Int; 1048576 text / 16384 binary | Maximum captured stdout bytes |
| `stderr_limit` | nonnegative Int; 1048576 text / 16384 binary | Maximum captured stderr bytes |

Environment names must be nonempty and contain neither `=` nor NUL. Empty values
are preserved. Inherited native non-UTF-8 environment data is passed unchanged.
`inherit_environment: false` clears the child environment before applying the call's
overlay. Neither a call's directory nor its environment changes the run snapshot,
another run, the host's globals, or the base directory used by imports.

Timeout (BW5002), cancellation (BW5001), and resource exhaustion (BW8001) stop the
containing run and bypass Catch. Finally still runs under its independent cleanup
allowance. A process-local timeout also stops the run, even with no run deadline.
Dropping an async run requests cancellation; the supervisor continues owning the
child and its resources. A launch error, incomplete stdin delivery, pipe error, or
unverified cleanup is BW5003. These errors preserve the previous assignment value
and call/source context. External effects that already happened are not rolled back.

Every child starts in a fresh process group. Cancellation, timeout, capture overflow,
and normal leader exit trigger group termination and direct-child reaping. A process
which deliberately escapes that group is outside this guarantee. For stronger tree
or host-death isolation, hosts can use the separate
[guardian/namespace worker APIs](isolated-workers.md). The statements do not expose
background handles or guarantee successful cleanup after host termination.

Cleanup can outlive its observation allowance when an OS operation stalls. The call
then returns an error, never a successful partial result; the supervisor retains its
capacity and budget leases until ownership is resolved. Unverified ownership keeps
its slot quarantined. The allowance bounds observation, not OS scheduling or all
wall-clock execution. No process handle or partial capture is returned on errors.

Before copying command data or launching, admission checks:

- At most 1 MiB of command/path/environment bytes, including conservative NUL slots,
  and 16384 argument plus effective-environment entries.
- At most 1 MiB of stdin, and the maximum possible result at both capture caps under
  the run's value nodes, depth, entries, String, key, and payload limits.
- Temporary overlap for three command payload copies (specification, native command,
  and environment marshalling), stdin/raw captures, and the final result. The result
  reservation shrinks to its actual size after verified completion.
- A shared 64 MiB logical in-flight byte budget and 32 active processes per host
  process. These admissions reject immediately when exhausted.

Large configured capture caps can therefore fail before launch even for a silent
program. Smaller caps work with smaller run budgets. Binary caps are byte counts,
but resulting Int arrays cost four payload bytes and one value node per byte.
Logical budgets exclude allocator, thread-stack, and kernel overhead. Async calls
also require admission to the existing bounded native callback pool and snapshot
budgets; they do not block the async executor. Synchronous calls block their caller.

The fixed catalogue is exempt from user registration-retention budgets; all 100
table slots count toward snapshot work. Hosts may replace these signatures before
initialization. Root declarations collide with default names; child scopes can
shadow them. CLI listing/help uses the same typed metadata.

<!-- botwork-test: process-statements -->
```botwork
|result| = Run Process |"/bin/printf"| With Arguments |["Hello, %s!", "world"]|
Assert |result.success|
Log |result.stdout|

|result| = Run Process |"/bin/sh"| With Arguments |["-c", "printf diagnostic >&2; exit 7"]|
Assert |result.success| Equals |false|
Log |result.exit_code|
Log |result.stderr|

|result| = Run Binary Process |"/bin/cat"| With Arguments |[]| Options |{"stdin": [0, 255, 10]}|
Log |result.stdout|
```

The other two forms, with their output in comments:

<!-- botwork-test: process-variants -->
```botwork
|result| = Run Process |"/bin/sh"| With Arguments |["-c", "printf '%s' \"$GREETING\""]| Options |{"environment": {"GREETING": "hi"}}|
Log |result.stdout|                                     # hi
|result| = Run Binary Process |"/bin/printf"| With Arguments |["AB"]|
Log |result.stdout|                                     # [65, 66]
```

This is also [example 33](../examples/33-processes.botwork). It prints `Hello, world!`,
`7`, `diagnostic`, and `[0, 255, 10]` on separate lines.
