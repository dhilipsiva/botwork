# Runtime performance measurements

Run `python3 scripts/performance.py` from a Linux checkout with Rust and Python
3.9+ installed. It builds the locked GNU release CLI and workload driver, checks
the memory instrument, then runs three warmup rounds and thirty measured rounds.
Each round executes every workload below, in table order, one process at a time.
`--target x86_64-unknown-linux-musl` measures the musl build separately.

This is measurement protocol version 1. The workload sizes, sample counts,
statistics, correctness checks, and timing boundaries below are fixed before
the full campaign. No runtime optimization or acceptance budget is part of this
change. The recorded campaign below is now the accepted baseline for the
[registered budgets](#registered-budgets); the scaling gates and release
performance acceptance remain open.

## Workloads and timing boundaries

| ID | Fixed full workload | Timed work and correctness check |
| --- | --- | --- |
| `cli-startup` | One file containing `Log` of 42 | Native parent's process spawn through exit, including CLI parsing, runtime startup, source preparation, output, and shutdown. Require status 0, exactly `42` plus newline, and empty stderr. |
| `parse` | 10,000 assignments to `value`, with successive integer literals | `Program::parse_detailed`, including syntax checks, Pest parsing, AST construction, and validation. Source generation/loading and AST destruction are excluded from workload time. Require 10,000 statements. |
| `calls` | 100,000 custom `Next` calls inside a While loop | Async Engine execution of a pre-parsed program, including run setup, checks, loop overhead, calls, and result snapshot. Require `answer` = 100,000 and success without a snapshot error. |
| `loop` | 1,000,000 increments in a While loop | Async Engine execution of a pre-parsed program, including run setup, checks, and result snapshot. Require `answer` = 1,000,000 and success without a snapshot error. |
| `source-io` | Sixteen sequential reads/runs of a 256 KiB source file | Async Engine file loading, parsing, execution, and result checks. Each fresh run returns 42; require checksum 672. Payload is one assignment plus a comment. This measures the existing source-I/O pipeline, including parser overhead, not isolated disk throughput. |
| `waiting` | 100 concurrent Engine runs held at a shared native operation for at least 10 ms after all enter | Operation/Engine setup, task admission, per-run setup, suspension, release, and result checks. Require all 100 pending simultaneously, per-run input/results 0–99, checksum 4,950, and zero active calls/ownership charges after completion. |

The workload driver uses a current-thread Tokio runtime and the runtime's normal
blocking workers. Except for the CLI, it disables process-environment inheritance
and raises only the per-run step allowance to 64,000,000; all other `RunLimits`
remain at their recorded defaults. The CLI uses its normal defaults. Workloads
have a 120-second process watchdog, which is a harness safety limit, not a
performance acceptance target. No workload uses external adapters or services.

For non-CLI workloads, the driver reports a monotonic workload duration. Source
loading, runtime construction, and pre-parsing for execution cases happen before
that timer; the timing table identifies additional setup included by each case.
Separate process duration and peak RSS cover the complete workload process,
including setup, checking, output, and cleanup. Checksum and structured-result
validation reject incomplete or incorrect work before statistics are accepted.

## Measurement and evidence

The native supervisor in `benches/runtime/probe.rs` spawns the workload and uses
Linux `wait4` for child CPU accounting and maximum resident memory in KiB. Its
process timer spans spawn through wait completion. Python supervises this small
parent separately, keeping its overhead outside the reported workload process
duration. All stdout/stderr and probe records are retained.

Measuring `ru_maxrss` directly from Python would carry Python's pre-exec memory
high-water mark into small children. A calibration touches 64 MiB in Python,
verifies a tiny child measures below that floor, then verifies the instrument
detects a separate child's touched 32 MiB allocation. The native parent's own
small resident footprint is still part of the child's initial fork environment;
RSS is a whole-process high-water measurement, not an allocation count or a
deduplicated/private-memory measurement. The watchdog uses a pidfd; measured
children receive a parent-death kill signal if their native supervisor is lost.
Measured workloads do not launch descendants.

Files are generated before warmups and the driver pre-reads source. Caches stay
warm; there is no OS cache eviction. Fresh processes reset run/interpreter state,
not filesystem caches. No affinity pinning, CPU-frequency control, or host-wide
isolation is imposed. The record contains the visible CPU model, CPU affinity,
virtualization/platform, memory, filesystem type, load averages, toolchain,
target, profile, selected build flags, source/lockfile hashes, input hashes,
binary hashes, exact commands, and all raw observations.

Percentiles use nearest rank (`sorted[ceil(p * n) - 1]`). Each workload has thirty
measured samples; p95 is the 29th ordered observation. Minimum, median, p95, and
maximum are retained for workload time, process time, and peak RSS. Warmups are
retained separately and never enter these statistics. Failed warmups, bad
checksums, missing/duplicate samples, nonzero exits, watchdog expirations, or
source/binary changes leave a campaign incomplete; no failed observation is
silently removed. These are descriptive measurements, not confidence intervals.

Campaigns are written beneath `target/performance/`. Replaying a committed
measurement requires its source revision, lockfile, toolchain, target/profile,
and comparable hardware/environment. Keep original observations alongside reruns.

## Validation

`python3 tests/performance_tools.py` checks workload sizes, result/identity
validation, failure rejection, exact sample inventory, warmup separation,
percentiles/outliers, and watchdog reaping. `python3 scripts/performance.py
--smoke` executes all workload shapes with reduced sizes, one sample, and no
warmups; its evidence is explicitly marked `smoke`. It is never a performance
baseline. CI runs that correctness check in GNU/musl debug/release; performance
thresholds would be inappropriate on unregistered hosted machines.

## Recorded GNU release measurements

The [measurement evidence](performance-evidence.json) preserves all 18 warmup
and 180 measured observations, source/binary/input hashes, environment, command
records, calibration, statistics, and correctness validation. This campaign used
Rust 1.97.1 / LLVM 22.1.6, GNU release, glibc 2.43, and WSL2 Linux
6.18.33.2-microsoft-standard-WSL2. The VM exposes eight CPUs from an AMD Ryzen 9
9950X3D; affinity included all eight. The filesystem reports `ext2/ext3`. This was
a shared development machine, with one-minute load averages of approximately
1.42 before and 2.36 after the run. No compiler flag overrides were set.

| Workload | Workload p50 | Workload p95 | Process p95 | Maximum peak RSS |
| --- | ---: | ---: | ---: | ---: |
| CLI startup | 1.498 ms | 1.620 ms | 1.620 ms | 4.766 MiB |
| Parse 10,000 statements | 28.636 ms | 30.832 ms | 32.599 ms | 12.641 MiB |
| 100,000 custom calls | 291.702 ms | 305.379 ms | 306.668 ms | 5.016 MiB |
| 1,000,000 loop iterations | 1,871.490 ms | 1,905.920 ms | 1,907.399 ms | 5.012 MiB |
| Sixteen 256 KiB source loads | 6.103 ms | 7.374 ms | 9.008 ms | 5.906 MiB |
| 100 waiting runs, including a 10 ms hold | 17.242 ms | 18.703 ms | 21.968 ms | 7.707 MiB |

Memory calibration measured 8,388 KiB for the small Python child and 41,248 KiB
for the child touching 32 MiB, while the Python measurement parent held its
separate 64 MiB allocation. An earlier smoke instrument that read child RSS
directly from Python reported an identical 27,264 KiB for every workload; that
instrument was corrected before these measurements. A first complete campaign
is retained alongside this final one; the rerun followed a watchdog fix that
also terminates build-process descendants. Neither campaign changes runtime
execution behavior, and no observations were trimmed as outliers.

## Priorities informed by the measurements

These are next profiling steps, not measured explanations of the costs:

1. Profile evaluation in the long loop first: it dominates elapsed time for these
   inputs. Locate time in dispatch, variable access/replacement, budget checks,
   and async polling before changing any of them. Preserve the existing semantic,
   cancellation, and admission contracts while comparing prospective changes.
2. Isolate custom-call overhead with matched iteration counts and profiler data.
   The call workload takes about 2.92 microseconds per iteration at the median,
   including its loop and arithmetic; this is not a pure function-call cost.
3. Profile parser allocation/ownership if memory becomes a constraint. The parse
   workload has the largest measured peak, about 12.64 MiB for 10,000 statements.
   A doubled-input experiment is still needed before drawing scaling conclusions.
4. Split concurrent setup, admission, wakeup, and cleanup timings before tuning
   worker capacity. The waiting workload includes an intentional 10 ms hold and
   timer/scheduler overhead, so subtracting 10 ms does not identify a single cost.
5. Keep storage claims separate from the source-loader result. Its sixteen warm
   reads include parsing and fresh run setup; use controlled cold/cache and
   concurrent I/O experiments if those become representative requirements.

## Registered budgets

[Roadmap decisions](decisions.md) D3 and D4 register this workstation as the
reference host and the campaign above as the accepted baseline. Each workload's
budget is its p95 workload time × 1.25 and its maximum peak RSS × 1.25:

| Workload | Time budget | Memory budget |
| --- | ---: | ---: |
| CLI startup | 2.03 ms | 6,100 KiB |
| Parse 10,000 statements | 38.54 ms | 16,180 KiB |
| 100,000 custom calls | 381.72 ms | 6,420 KiB |
| 1,000,000 loop iterations | 2,382.40 ms | 6,415 KiB |
| Sixteen 256 KiB source loads | 9.22 ms | 7,560 KiB |
| 100 waiting runs | 23.38 ms | 9,865 KiB |

Times are rounded to 0.01 ms, and memory is rounded up to whole KiB. A campaign
counts against the budgets only on the reference host, with this protocol. It
fails when a workload's p95 time or peak memory exceeds its budget, or regresses
more than 10% against the baseline without an explanation.

Use the same protocol and environment for changes, then perform the roadmap's
doubling/scaling and regression checks. These samples do not prove
bounded long-run memory, cross-platform parity, production tail latency, or the
separate performance acceptance gates.
