# Running scripts in parallel

Repeat `--file` to run a batch. `--jobs` (short form `-j`) bounds simultaneous runs
and defaults to 4. Values from 1 through 64 are accepted; `--jobs 1` runs the files
sequentially. One file keeps its existing stdout, stderr, and exit behavior even
when `--jobs` is supplied.

```sh
cargo run -- --file examples/21-parallel-first.botwork \
  --file examples/22-parallel-second.botwork --jobs 2
```

The examples produce `{"count": 1, "script": "first"}` and
`{"count": 2, "script": "second"}` on stdout, in either order. They deliberately
reuse variable and statement names to demonstrate independent state.

## Run identity and state

Every file occurrence gets a one-based ID from its command-line position,
including repeated paths. IDs are unique within that CLI invocation; they are
not persistent or globally unique identifiers. Stderr records identify admission
and completion, for example `[run 2] succeeded: "second.botwork"`. Paths in those
headers are escaped and quoted. Completion retains the original ID even when a
later file finishes first. A final batch line counts successful and failed runs.

Each run creates fresh root variables, custom definitions, module state/cache,
input values, execution counters, and retention/output budgets. Common `--var`
and `--vars-file` options apply independently to every run using the existing
[input precedence](input-variables.md). Variable files and source files are read
when each run is admitted; editing them while a batch is queued can change what
later runs read. Each import resolves relative to its own importing source.

Runs use the same launching working directory and process environment. Batch
execution does not change either globally. Files, output destinations, and other
external side effects are shared resources; independent DSL state does not make
them transactional or prevent conflicts between scripts.

## Admission, timeouts, and failures

The job limit includes input/source preparation, parsing, evaluation, and waiting
for blocking run work to drain. At most that many run tasks are admitted. Queued
paths retain their command-line data without loading source or constructing run
contexts. Only admitted tasks and their identity records occupy the scheduler's
active set; terminal results are reported and released as they are collected.

`--timeout-ms` applies separately to each run after admission, including its
loading and parsing. Time spent waiting for a job slot does not consume that
run's timeout. The same step, call-depth, evaluation-depth, and output options
apply independently to each run.

A script error, missing file, resource limit, or timeout fails that run. Other
admitted and queued runs continue; this command currently has no fail-fast mode.
Exit status is 0 when all runs and status delivery succeed, 1 on run/reporting
failure, and 2 for invalid command-line usage. Failed-run records include the
original diagnostic. Timeout/resource failures have explicit terminal labels.

## Output and reporting

Log retains its normal stdout representation. Each checked Log record holds the
stdout lock through its write, so records from different runs can appear in any
order but do not interleave their bytes. A string can itself contain newlines.
There is no cross-stream ordering guarantee between stdout and stderr. Debug
traces keep their original source coordinates; they can interleave with other
traces and status records. Run IDs belong to lifecycle/terminal records; this
does not yet provide a run ID on every Log/debug record or a machine-readable
report protocol.

Status records stream through one reporter and hold the stderr lock for the
complete record. Their checked output allowance is separate from each script's
output budget, so a run that exhausts output capacity can still be reported.
The reporter uses the default 8 MiB record and 32 MiB per-write allowance and
the existing diagnostic rendering limits. It does not retain a batch-sized
result or output buffer. These are per-record limits, not a cross-run total-output
quota. Successful delivery means the destination accepted/flushed the record,
not durable storage or downstream receipt.

Reporting runs off the async executor. Backpressure pauses admission/result
collection while already-admitted runs remain schedulable. If status delivery
fails, stop admitting queued paths, drain every admitted run, and return failure.
Further status records are suppressed after that failure; undelivered outcomes
must not be inferred as successes. Broken stderr may also prevent delivery of
the final error message. Already-written output and completed effects remain.

The CLI uses a current-thread Tokio runtime with blocking workers for admitted
preparation and reporting, alongside the existing bounded filesystem/native
pools. `--jobs` bounds runs, not OS threads or aggregate process memory. See
[I/O isolation](nonblocking-io.md) for pool limits and thread behavior. Started
blocking calls still need to return, and normal stop handling drains them. A
stalled special file or uncooperative callback can therefore delay batch exit.
Hard shutdown bounds, coordinated signal cancellation, structured reports,
per-run configuration manifests, and adapter-specific sharing policies remain
separate roadmap work.

## Validation

The CLI matrix covers duplicate files, inputs/scopes/module caches, stable IDs,
mixed failures, per-run quotas, parser bounds, whole Log records, and single-file
compatibility. Linux FIFO tests verify concurrent entry and capacity retention,
out-of-order completion IDs, and fresh deadlines for queued runs. Pipe/device
tests cover reporting failure before effects, draining after partial reporting,
and sibling progress while status output is blocked. R32 contributes two corpus
cases; examples 21–22 also run independently in the example suite.

[Validation evidence](parallel-cli-evidence.json) records the actual profiles,
mutation outcomes, and remaining observations.
