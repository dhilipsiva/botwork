# Output Admission and Completion

`RunLimits::output` bounds readable value serialization and destination output in every Context and fresh Engine run. Built-in `Log`, checked host output, and CLI debug traces share that run's counter. Imported modules share it too; Context clones copy the current usage and stop latch into independent budgets, like other cumulative work counters. A new run starts at zero.

| OutputLimits field | Default | Count |
| --- | --- | --- |
| `record_bytes` | 8 MiB | Complete UTF-8 representation of one checked output call |
| `total_bytes` | 32 MiB | Sum of all admitted records in a run |

Both limits can be zero or explicitly raised. Count strings, escaped collection strings and keys, punctuation, number text, and newlines. A Log record includes its appended newline; even logging an empty string costs one byte. `Context::write_value` writes a value without a newline, so an empty top-level string fits a zero-byte budget. An empty output call still checks cancellation and flushes the destination.

The CLI exposes `--max-output-record-bytes` and `--max-output-bytes`. They apply to statement help/listing as well as script runs. Each trace, log, or listing line is one record; detailed statement help is one record. For example:

```sh
cargo run -- --file examples/01-expressions.botwork --max-output-bytes 1024
cargo run -- --statement-help 'Log |value|' --max-output-record-bytes 1024
```

## Before Buffering or Writing

Check the value's shape using existing `ValueLimits`, then count the full readable representation into a byte counter. Stop counting as soon as either output allowance is exceeded. This pass creates neither an output-sized buffer nor map sorting tables: map order does not change encoded length. Once the complete record fits, charge its bytes atomically, then render sorted maps using bounded-depth values. Sorting tables are bounded by admitted entries and encoded record size. Emission uses a fixed 4 KiB buffer, with no complete intermediate string.

The atomic cumulative charge checks addition for overflow and rechecks available capacity before any write. Exceeding a byte limit returns BW8001, latches the run, and bypasses Catch. No bytes or flushes from the rejected record reach the destination. Earlier records and required argument effects remain. Log's existing admission of its returned value copy also precedes output.

The entire admitted charge remains used after an I/O error, cancellation, or timeout. This conservatively counts attempts and prevents retries from spending the same capacity again. No automatic retry of a failed record or rollback of earlier bytes occurs. Ordinary short writes and `Interrupted` results are continued within the same record.

## Completion and Failures

Successful checked output means that the writer accepted exactly the admitted byte count, its final `flush` succeeded, and the final run checkpoint observed no stop. It does not promise durable storage or delivery beyond the `Write` implementation's contract.

Check cancellation/deadlines before and after destination writes, between buffer chunks, during counted/emitted formatting fragments, and around flush/retry boundaries. A zero-byte write while bytes remain returns BW4001. Write, flush, invalid writer-count, and formatting failures also return BW4001 with accepted-byte progress and explicit incomplete-output wording. A counter-pass formatting failure reports that writing never started. Partial byte prefixes, including partial UTF-8 sequences from a short write, can remain in the destination after failure. Pending local buffer contents are discarded rather than flushed after a failure.

BW4001 is catchable if its diagnostic fits the diagnostic quotas. Observed cancellation, timeout, and resource limits keep their existing primary-error priority and bypass Catch; a simultaneous I/O failure remains a bounded cause. Cancellation-only diagnostics retain their established category and wording, so callers must allow partial output on every error, not only BW4001. A cancelled final write or flush never returns success even when all bytes were accepted.

Blocking `Write::write`, `flush`, and arbitrary host formatters cannot be forcibly interrupted. A deadline expiring inside a blocking writer is observed when it returns. The limits are cooperative and do not establish a wall-clock deadline, a process-memory sandbox, record-level atomicity across writers, or a transactional output channel.

## Host and CLI Boundaries

Async DSL Log and debug records execute on [bounded blocking workers](nonblocking-io.md).
Each run awaits its record before continuing; sibling runs remain schedulable
while a destination blocks. The low-level Context writer APIs below remain
synchronous, and worker dispatch does not make a blocked write interruptible.

`Context::write_value(&value, &mut writer)` admits borrowed values before recursive formatting; the caller retains ownership, including ownership of rejected deep values. It preserves existing readable output, sorted map keys, escapes, and combining marks. This is not JSON or a round-trip serialization format. Future serializers must apply the same admission contract to their own encoded representation.

`Context::write_output(&mut writer, format_args!(...))` checks generic formatted text with two passes. Custom Display implementations must be deterministic and cooperative, and remain responsible for any allocations or effects they perform internally. If emitted length changes, emission cannot exceed the admitted size and returns an error instead of success. Equal-length content changes cannot be detected. Use `write_value` for arbitrary Literal inputs because generic Display does not perform value-shape admission.

Statement metadata exposes `display_help()` for streaming; CLI help uses it without intermediate line/header buffers. Direct host `Literal::Display`, `StatementSignature::help`, generic `format!`, and caller-built buffers remain host conveniences outside these run quotas. Native callbacks doing their own I/O must use the checked Context API to participate. Clap's argument parsing/help/version output remains outside the interpreter's run lifecycle.

Final CLI failure reporting uses a fresh bounded output allowance, separate from the exhausted script budget. Diagnostic Display retains its own smaller rendering/work budgets and explicit truncation contract. If stderr itself fails, the CLI still returns failure; it cannot guarantee that the failure message was delivered.

## Evidence

Unit tests cover exact/zero/overflow allowances, escaped UTF-8, maximum/rejected depth, shared atomic charging, clone isolation, early counting termination, unstable formatters, short/interrupted/zero writes, flush failures, partial progress, and cancellation/timeout before completion. Integration tests cover public in-memory serialization, CLI status and traces/help, argument effects, Catch bypass, and imported module accounting. Allocation observations verify rejection before map sorting or output-sized buffers. The R24 corpus cases and an executed Rust documentation example pin byte-boundary behavior. Existing diagnostic tests cover error quotas, partial-write recovery, call sites, simultaneous stop/error causes, and real `/dev/full` failures.
