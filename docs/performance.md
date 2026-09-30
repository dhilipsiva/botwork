# Runtime performance measurements

Run `python3 scripts/performance.py` from a Linux checkout with Rust, a C
compiler (`cc`), and Python 3.9+ installed. It builds the locked GNU CLI and
workload driver in the `dist` profile that distributed binaries use
([D19](decisions.md#d19-distribution-build)), checks the memory instruments,
then runs three warmup rounds and thirty measured rounds. Each round executes
every workload below, in table order, one process at a time.
`--target x86_64-unknown-linux-musl` measures the musl build separately.

This is measurement protocol version 2. The workload sizes, sample counts,
statistics, correctness checks, and timing boundaries below are fixed before
the full campaign. The recorded campaign below is the accepted baseline for the
[registered budgets](#registered-budgets), which every later campaign on the
reference host is checked against. The scaling gates and release performance
acceptance remain open.

Protocol 2 changes three things from version 1, whose record is kept
[as history](#protocol-1-measurements):

- it measures the `dist` build instead of `release`;
- it adds [peak heap](#peak-heap), the memory a run's allocations hold, and the
  CLI binary's size, which the budgets now cover in place of peak RSS;
- each observation runs its command a second time, counted, for peak heap.

The workloads, sizes, samples, timing boundaries, and the driver's result
schema are unchanged.

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

### Peak heap

Peak heap is the most memory a run's allocations hold at once, over the whole
process, in KiB rounded up. It is the run's private memory apart from the
binary: peak RSS also counts the executable's code and relocated data pages,
which grow with the binary rather than with what a run allocates. When the
binary grew from 4.1 MB to 14.6 MB between the protocol 1 baseline and
revision `628b782`, every workload's maximum peak RSS rose by 3.5 to 5.8 MiB, including
the loop, whose live data did not change.

Right after each timed run, the same command runs again with
`benches/runtime/heap.c` preloaded into it. That counter forwards every
allocation to glibc and, at exit, writes the peak of the bytes allocated and
not yet freed, counting each block's usable size. The counted run must pass the
same correctness checks as the timed run; its time and RSS are recorded but
never used. The timed runs are never counted, because counting costs time. A
first protocol 2 campaign counted inside the driver with a counting global
allocator, and its atomic updates made the loop and call workloads about
16.5% slower; that instrument was rejected.

Static binaries cannot preload a library, so musl campaigns record no peak
heap. Thread stacks and memory mapped outside the allocator are not counted.

## Measurement and evidence

The native supervisor in `benches/runtime/probe.rs` spawns the workload and uses
Linux `wait4` for child CPU accounting and maximum resident memory in KiB. For
counted runs it preloads the heap counter into the child only (`--preload`),
never into itself. Its
process timer spans spawn through wait completion. Python supervises this small
parent separately, keeping its overhead outside the reported workload process
duration. All stdout/stderr and probe records are retained.

Measuring `ru_maxrss` directly from Python would carry Python's pre-exec memory
high-water mark into small children. A calibration touches 64 MiB in Python,
verifies a tiny child measures below that floor, then verifies the instrument
detects a separate child's touched 32 MiB allocation. The native parent's own
small resident footprint is still part of the child's initial fork environment;
RSS is a whole-process high-water measurement, not an allocation count or a
deduplicated/private-memory measurement. The calibration also runs each child
with the heap counter and checks that it reports the 32 MiB allocation (at least
32 MiB, and more than 31 MiB above the small child) and nothing of the parent's
64 MiB. The watchdog uses a pidfd; measured
children receive a parent-death kill signal if their native supervisor is lost.
Measured workloads do not launch descendants.

Files are generated before warmups and the driver pre-reads source. Caches stay
warm; there is no OS cache eviction. Fresh processes reset run/interpreter state,
not filesystem caches. No affinity pinning, CPU-frequency control, or host-wide
isolation is imposed. The record contains the visible CPU model, CPU affinity,
virtualization/platform, memory, filesystem type, load averages, toolchain,
target, profile, selected build flags, source/lockfile hashes, input hashes,
binary hashes and sizes, the heap counter's hash and compiler, `.cargo/config.toml`,
exact commands, and all raw observations.

Percentiles use nearest rank (`sorted[ceil(p * n) - 1]`). Each workload has thirty
measured samples; p95 is the 29th ordered observation. Minimum, median, p95, and
maximum are retained for workload time, process time, peak RSS, and peak heap. Warmups are
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
percentiles/outliers, and watchdog reaping. It also builds the heap counter and
checks that it reports a child's own allocations, releases freed blocks, counts
growth through `realloc`, and writes nothing without `BOTWORK_HEAP_REPORT`; that a counter report must be
a positive byte count; and that a workload has peak heap in every observation
or none. `python3 scripts/performance.py
--smoke` executes all workload shapes with reduced sizes, one sample, and no
warmups; its evidence is explicitly marked `smoke`. It is never a performance
baseline. CI runs that correctness check in GNU/musl debug/release; performance
thresholds would be inappropriate on unregistered hosted machines.

## Recorded measurements

The [baseline evidence](performance-baseline-evidence.json) preserves all 18
warmup and 180 measured observations, each with its counted run, and the
source/binary/input hashes, environment, command records, calibration,
statistics, and correctness validation. This campaign used Rust 1.97.1 /
LLVM 22.1.6, the GNU `dist` build, glibc 2.43, and WSL2 Linux
6.18.33.2-microsoft-standard-WSL2 on the eight-CPU AMD Ryzen 9 9950X3D host,
with GCC 15.2.0 compiling the heap counter. The CLI binary is 10,525,736 bytes.
The machine was busy: a process outside this work used about one CPU
throughout, and the one-minute load average was 3.06 before and 3.78 after the
run. Medians match an earlier campaign of the same source at a load of 1.98,
while the p95 times carry the load.

| Workload | Workload p50 | Workload p95 | Process p95 | Peak heap | Maximum peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| CLI startup | 2.749 ms | 3.164 ms | 3.164 ms | 158 KiB | 8,748 KiB |
| Parse 10,000 statements | 26.823 ms | 32.060 ms | 34.785 ms | 14,508 KiB | 14,956 KiB |
| 100,000 custom calls | 304.348 ms | 344.909 ms | 347.222 ms | 162 KiB | 7,340 KiB |
| 1,000,000 loop iterations | 1,887.878 ms | 2,133.860 ms | 2,136.875 ms | 156 KiB | 7,084 KiB |
| Sixteen 256 KiB source loads | 5.682 ms | 8.476 ms | 11.251 ms | 933 KiB | 8,048 KiB |
| 100 waiting runs, including a 10 ms hold | 19.004 ms | 21.103 ms | 24.366 ms | 4,274 KiB | 12,140 KiB |

Peak heap barely varies between observations. It is identical in all thirty
for CLI startup, the parse, the calls, and the loop, and varies by 1 KiB for
the source loads and by 23 KiB for the waiting runs. The loop and the calls
hold about 160 KiB, like the CLI, because their live data stays constant. The
parse holds 14,508 KiB for 10,000 statements, about 1.45 KiB per statement,
and each waiting run adds about 41 KiB.

Memory calibration measured 8,368 KiB for the small Python child and 41,288 KiB
for the child touching 32 MiB. The heap counter reported 1,433,136 and
34,882,128 bytes for the same children. The first protocol 2 campaign, which
counted inside the driver, is kept in the record as a rejected instrument.
No observations were trimmed as outliers.

## Protocol 1 measurements

The first campaign measured the `release` build with protocol 1 on 2026-09-28,
at revision `2aed64d`, and was the original D4 baseline.
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
   Protocol 2 puts its peak heap at 14,508 KiB, about 1.45 KiB per statement.
   A doubled-input experiment is still needed before drawing scaling conclusions.
4. Split concurrent setup, admission, wakeup, and cleanup timings before tuning
   worker capacity. The waiting workload includes an intentional 10 ms hold and
   timer/scheduler overhead, so subtracting 10 ms does not identify a single cost.
5. Keep storage claims separate from the source-loader result. Its sixteen warm
   reads include parsing and fresh run setup; use controlled cold/cache and
   concurrent I/O experiments if those become representative requirements.

## Registered budgets

[Roadmap decisions](decisions.md) [D3](decisions.md#d3-reference-host) and
[D4](decisions.md#d4-budgets-and-baseline) register this workstation as the
reference host and the [protocol 2 campaign](#recorded-measurements) as the
accepted baseline. Each workload's budget is its p95 workload time × 1.25 and
its maximum peak heap × 1.25:

| Workload | Time budget | Heap budget |
| --- | ---: | ---: |
| CLI startup | 3.95 ms | 198 KiB |
| Parse 10,000 statements | 40.08 ms | 18,135 KiB |
| 100,000 custom calls | 431.14 ms | 203 KiB |
| 1,000,000 loop iterations | 2,667.33 ms | 195 KiB |
| Sixteen 256 KiB source loads | 10.59 ms | 1,167 KiB |
| 100 waiting runs | 26.38 ms | 5,343 KiB |

The CLI binary's budget is 13,157,170 bytes, its baseline size × 1.25.

Times are rounded to 0.01 ms, and memory and size are rounded up.
`benches/runtime/budgets.json` records the exact values in nanoseconds, KiB,
and bytes, with the baseline and the reference host. The original protocol 1
budgets, which budgeted peak RSS, are kept with
[D4](decisions.md#d4-budgets-and-baseline) as history. `python3 scripts/performance.py
--write-budgets` derives it from the baseline record, and
`tests/performance_tools.py` requires the two to match.

### Checking a campaign

Run a campaign, then check its record:

```sh
python3 scripts/performance.py
python3 scripts/performance.py --check target/performance/campaign-XXXXXXXX/summary.json
```

The check prints `budget check passed` and exits 0, or prints each problem and
exits 1:

- **Not comparable.** A campaign counts only when it is a complete protocol 2
  measurement on the reference host. That means the same CPU model and number
  of visible CPUs, the GNU target, and the `dist` profile. Any other campaign,
  including the protocol 1 record, is reported as not comparable.
- **Over budget.** A workload's p95 time or peak heap, or the CLI binary's
  size, exceeds its budget. Explaining the change does not excuse it.
- **Unexplained regression.** One of those values is more than 10% above the
  baseline. To accept such a regression, name the workload (or `binary` for
  the binary's size) and its cause, for example
  `--explain calls="profiled: a new cancellation checkpoint"`. The check prints
  each explanation so that it can be kept with the campaign.
- **Not measured.** A budgeted workload, or its peak heap, is missing.

Peak RSS is recorded but not budgeted: it mixes the binary's own pages, which
the size budget covers, with the heap, which the heap budget covers.

Use the same protocol and environment for changes, then perform the roadmap's
doubling/scaling and regression checks. These samples do not prove
bounded long-run memory, cross-platform parity, production tail latency, or the
separate performance acceptance gates.
