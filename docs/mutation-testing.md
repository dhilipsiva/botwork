# Core mutation testing

The first campaign measures whether tests detect deliberate changes to arithmetic,
precedence, scope, control flow, error propagation, and run isolation. It adds
mutation evidence to the [ordinary test suite](testing.md). It does not establish
correctness outside the mutations actually tested or complete the 1.0 release gate.

## Reproduce

Use Linux, Python 3.11 or newer, and a Rust toolchain with the locked dependencies
cached. The recorded campaign uses Rust/Cargo 1.97.1, GNU x86_64, debug assertions,
and cargo-mutants 27.1.0. Each rerun records its actual versions and source hashes.

```sh
cargo install cargo-mutants --version 27.1.0 --locked
cargo fetch --locked
python3 tests/mutation_tools.py
python3 scripts/mutation_core.py generated
python3 scripts/mutation_core.py targeted
```

Each command creates a separate `target/mutation-core/generated-*` or `targeted-*`
directory. Read its `summary.json`, logs, inventory, and diffs. The runner copies
tracked and nonignored untracked files to an isolated snapshot, records SHA-256
hashes, and checks that snapshot after testing. Real source files are never
mutated. Baselines must build and pass before any mutation is tested. Dependencies
are locked and Cargo is offline during execution; run `cargo fetch --locked` first
if needed. Keep the entire output directory when archiving evidence, including
the measured source snapshot. Build products can be omitted from an archive.

Generated tests use two cargo-mutants workers, a 180-second build limit, and a
60-second test limit per mutation; the outer campaign is limited to one hour.
Targeted tests run sequentially with the same per-phase limits. Timeouts and
compilation failures are separate outcomes. A test kill requires a successful
build, Cargo's test-failure status, and an actual failed-test summary. Empty tests,
incomplete inventories, unknown outcomes, and infrastructure failures fail audit.
Eleven Python checks protect these reporting and process-timeout boundaries in CI.

To replay one generated mutation by its exact recorded name:

```sh
python3 scripts/mutation_core.py generated --name 'src/core/eval/execution.rs:13:49: replace % with / in tick'
```

The runner discovers the complete inventory and uses a single-item shard after
checking its identity. A replay is evidence for that mutation only. The command
returns nonzero for survivors or timeouts; these require investigation. Do not
remove survivors from the configuration to obtain a passing score.

Audit existing output with its matching inventory:

```sh
python3 scripts/mutation_core.py audit target/mutation-core/generated-inventory.json target/mutation-core/generated/mutants.out
```

That example names the first local run. Subsequent runner-created directories use
`inventory.json` and `mutants.out` beneath the printed campaign directory.

## Fixed scope

[`.cargo/mutants.toml`](../.cargo/mutants.toml) selects operator execution, numeric
widening, variable/statement lookup, invocation frames, expression evaluation,
For/While/handler/statement completion, post-evaluation stop priority, handler
causes, run preparation/execution, and outcome classification. Selection was
frozen before viewing the first results. No mutation was excluded afterward.

Version 1 generated 189 changes across `grammar.rs`, `eval.rs`,
`eval/diagnostics.rs`, and `run.rs`. Version 2 retains those semantic areas after
the shared evaluator move and adds async setup/execution, mode checks, and
scheduler yields. Its 209-entry inventory also includes `eval/execution.rs` and
`run/asynchronous.rs`. In this tool version, struct-field deletions
also include `Context::with_control` and `Engine::with_registry_limits` despite
the name filters. Both remain in the inventory, denominator, and review. Record
the actual inventory instead of assuming the regex names are the entire scope.

Version 3 adds bounded filesystem/native dispatch, environment preparation,
worker stop handling, and bounded reads: 260 generated mutations. Version 4
extracts the read loop for deterministic interrupted/error-read tests and includes
both `read` and `read_from`: 263 mutations. All 260 preceding semantic identities
remain, with three additional return-value replacements for the extracted helper.
No survivor is removed from the scope or executable denominator.

Macro bodies and many semantic changes are not generated automatically. The
[27-entry targeted catalogue](../tests/mutation-core.json) therefore swaps
precedence levels, changes subtraction associativity, terminates While early,
discards For returns, loses the caller frame, uses dynamic parents, skips parent
variable lookup, discards handled causes, forces environment inheritance, ignores
environment removal, drops input bindings, and ignores the local deadline. Catalogue
version 2 also drops the async call frame, loses parent control, and omits the
CPU scheduler yield. Exact substitutions must match once; stale or ambiguous entries fail before
testing. Each catalogue version was frozen before its first execution.
Catalogue version 3 adds inline native/file work, premature worker permit release,
skipped drain, lost callback control, and discarded cleanup causes. The selected
suites now include `async_filesystem` and `async_blocking`.
Catalogue version 4 adds single-file-only dispatch, unbounded batch admission,
lost run IDs, hidden aggregate failures, skipped drain after reporting failure,
and inline status writes. Scope version 5 includes the CLI binary unit tests and
`parallel_cli`; its 286-entry generated inventory retains every prior semantic
identity and adds 23 mutations in `src/batch.rs`.

All library unit tests and the named integration suites in the Cargo config run
for generated and targeted mutations. The initial suite omitted the existing
`temporary_limits`, `value_limits`, and `retained_registry` integration tests;
survivor review added them. This expands the test oracle without shrinking the
mutation population. No adapters or remote services are configured; native test
callbacks and local module fixtures are included.

Version 12 retains earlier semantics and adds immutable collection statements and
their output planners. The targeted catalogue has 92 entries, including twelve
collection faults covering replacement/removal, map order and membership, strict
indexes and entry shapes, range endpoints, duplicate keys, size arithmetic, and
admission before copies. The `collections` integration suite joins the existing
allocation oracle. [Collection evidence](collections-evidence.json) records this
focused campaign separately; it does not claim a new full generated score.

Version 13 retains earlier areas and adds Unicode string transformations, format
parsing, bounded result construction, and regex execution. Its targeted catalogue
has 106 entries, including fourteen new string faults. The `strings` integration
suite joins the allocation oracle; [string evidence](strings-evidence.json) records
the focused campaign without claiming a new complete generated score.

Version 14 adds the date/time parser, zone resolver, duration arithmetic, and
bounded formatter. The targeted catalogue has 121 entries, including fifteen
new faults for unknown offsets, excess precision, leap seconds, DST choices and
gaps, the named-zone horizon, historical offsets, negative epochs, duration
units/sign/order/overflow, local zone fields, difference direction, and premature
format allocation. The `datetime` integration suite joins the allocation oracle.
[Date/time evidence](datetime-evidence.json) records the focused campaign; a new
complete generated score is not claimed.

## First campaign — 2026-09-28

[Machine-readable evidence](mutation-core-evidence.json) records each mutation,
its initial and latest outcome, source/config/test hashes, commands, raw-output
hashes, compiler failures, and individual survivor reviews. The worktree is based
on commit `8016333`; hashes distinguish the successive test repairs. Production
behavior did not change.

| Stage | Caught by tests | Survived | Did not compile | Timed out |
| --- | ---: | ---: | ---: | ---: |
| Initial generated inventory | 144 | 19 | 26 | 0 |
| Generated inventory after first repairs | 162 | 1 | 26 | 0 |
| Exact replay after final map-boundary test | 1 | 0 | 0 | 0 |
| Targeted catalogue | 12 | 0 | 0 | 0 |

Latest outcomes are consolidated by mutation identity, counting the replay once:
**175 / 175 compiled, non-equivalent mutations caught (100%)**, from 201 distinct
mutations. The 26 compiler failures are excluded from the executable denominator,
never counted as test kills. There are no equivalence exclusions. The initial
generated score was 144/163 (88.34%); retain that result alongside the repairs.

The 19 initial survivors received these repairs, with individual names and killing
tests in the JSON record:

- Collection admission and accounting: include existing temporary/value suites;
  assert map-width rejection before native effects, exact node/payload accounting
  for duplicate keys, and acceptance at the exact temporary-node budget.
- Float arithmetic: check independently calculated values and exact float bits
  for all four numeric kind combinations through both public operators and DSL
  execution. Kind-only assertions did not detect several arithmetic changes.
- Boolean diagnostics: assert the correct operator in invalid-left-operand errors,
  which must occur before evaluating an undefined right operand.
- Registration storage: include existing registry-budget tests.
- Cancellation after error construction: deterministically arrange a cancellation
  between error construction and result finalization; assert stop priority and
  preservation of the original error as a cause.

The six added Rust tests pass in GNU and musl debug/release. The full release
mutation gate remains open: rerun against the release revision, expand the fixed
scope as runtime/adapter features arrive, review every new survivor, and preserve
the agreed minimum 90% kill rate. This is a local core campaign; CI runs the new
regressions and helper checks, not the full mutation campaign on every push.

Tool references: [cargo-mutants configuration](https://mutants.rs/config-file.html),
[mutation operators](https://mutants.rs/mutants.html), and
[timeout handling](https://mutants.rs/timeouts.html).

## Async integration campaign — 2026-09-28

Version 2 adds `async_execution` to the selected test suites and follows the shared
evaluator into its new files without removing earlier semantic mutations. The
[async evidence record](async-execution-evidence.json) retains all outcomes and
hashes: 167 generated mutations were caught and 42 did not compile; all 15 targeted
mutations were caught. No generated mutation survived or timed out. The latest
combined result is **182/182 compilable mutations caught**, from 224 distinct
mutations, with no equivalence exclusions. Compiler failures are not test kills.

The initial targeted run caught 13 changes and timed out on two: discarded inputs
and lost parent control exposed unbounded waits in the new sibling-run test.
Controlled-clock deadlines now bound both operation entry and cancellation
completion. A complete catalogue rerun caught all 15 through failed assertions.
The original timeout outcomes remain recorded; they are not counted as kills.
This local core campaign still does not complete the release mutation gate.

## Blocking I/O campaign — 2026-09-28

The [I/O evidence record](nonblocking-io-evidence.json) preserves the version 3
initial inventory: 182 caught, 9 survived, 67 did not compile, and 2 timed out.
Both original timeouts retried permanent file errors forever. Tests now place a
deadline on real file errors and bound the fault-injected reader by cancelling
after excess attempts. Compiler failures remain separate from test kills.

Survivor review adds deterministic coverage for sustained CPU batching, stop/error
handoff with a latched resource failure, worker panic versus queued-task abortion,
and interrupted reads versus permanent failures. The extracted read helper also
verifies that reaching the size bound performs no extra I/O. A smaller read buffer
changes batching cost without changing the tested output or admission contract;
it remains in the executable denominator, with no equivalence exclusion. Runtime
performance acceptance remains a separate roadmap gate.

| Stage | Caught by tests | Survived | Did not compile | Timed out |
| --- | ---: | ---: | ---: | ---: |
| Initial generated inventory (260) | 182 | 9 | 67 | 2 |
| Expanded inventory after repairs (263) | 194 | 1 | 67 | 1 |
| Exact replay with bounded fault-reader retries | 1 | 0 | 0 | 0 |
| Targeted catalogue, initial and final runs | 21 | 0 | 0 | 0 |

The expanded campaign captured its source before the fault-reader watchdog was
added, so its always-retry mutation still timed out. The exact replay catches it
through a failed assertion. Consolidating that identity once gives **216/217
compilable mutations caught (99.54%)**, including all 21 targeted changes, with
one reviewed buffer-size survivor, 67 compiler failures, and no equivalence
exclusions. The original and intermediate timeouts are preserved, not counted as
kills. All four GNU/musl debug/release profiles have a passing 1,350-test run;
the evidence also retains the observed failures and repairs.

## Parallel CLI campaign — 2026-09-28

Version 5 preserves all 263 prior semantic identities and adds 23 mutations for
batch admission, run identity, outcome classification, and reporting. The
[parallel CLI evidence](parallel-cli-evidence.json) retains the frozen inventory,
all outcomes and compiler errors, source/test/config hashes, and the exact replay.
Catalogue version 4 adds six targeted scheduler/reporting changes to the prior 21.

| Stage | Caught by tests | Survived | Did not compile | Timed out |
| --- | ---: | ---: | ---: | ---: |
| Generated inventory (286) | 215 | 3 | 68 | 0 |
| Exact source-limit classification replay | 1 | 0 | 0 | 0 |
| Targeted catalogue | 27 | 0 | 0 | 0 |

The source-limit survivor exposed a missing terminal-label assertion. A new
oversized-file CLI case checks limit classification before UTF-8 decoding and
successful sibling completion; its exact replay is caught. Consolidating it once
gives **243/245 compilable mutations caught (99.18%)**. Two reviewed survivors
remain in the denominator: the earlier smaller read buffer and a defensive
cancellation label with no supported CLI cancellation trigger yet. Signal/listener
cancellation must extend that end-to-end coverage when implemented. Neither is
excluded as equivalent; compiler failures are never test kills. All final GNU/musl
debug/release profiles pass 1,365 tests, with no timeout relaxation.

## Named-suite campaign — 2026-09-28

Version 6 retains all 286 prior mutation identities and adds 190 across suite
metadata/discovery/selection, aggregate AST admission, and failed-case storage.
Batch scheduling moved into `run_inputs`; its final failure comparison moved
into `Outcome::result`. Catalogue version 5 retains all 27 targeted changes and
adds seven for library composition, tag/rerun selection, stable IDs, incomplete
records, and failure ordering. Both campaigns include `suite_cli` alongside the
previous test targets. [Suite evidence](suites-evidence.json) records the frozen
inventories, source/test/config hashes, compiler failures, and individual reviews.

| Stage | Caught by tests | Survived | Did not compile | Timed out |
| --- | ---: | ---: | ---: | ---: |
| Initial generated inventory (476) | 345 | 22 | 108 | 1 |
| Exact replay of 24 original identities | 21 | 3 | 0 | 0 |
| Targeted catalogue | 34 | 0 | 0 | 0 |

The replay covers every initial survivor and timeout, plus one apparent catch
whose actual failure was the existing default-loop-budget watchdog. New
assertions exercise early discovery rejection, exact metadata/selector bounds,
public case enumeration, storage capacity, rejected writes, valid-record
symlinks, collision retry, and schema-error guidance. A bounded failure helper
replaces an `unwrap_err` that tried to format a large unexpectedly accepted AST.
The exact aggregate-admission replay now fails its intended assertion without
relaxing the timeout. The timing-affected suite-run mutation now fails suite
behavior assertions. An earlier replay failed its unmutated baseline and ran no
mutations; it is retained separately and contributes no kills.

Consolidating each original identity once gives **399/402 compiled mutations
caught (99.25%)**, including all 34 targeted changes. Three reviewed survivors
remain in this conservative denominator: the prior smaller read buffer, the
defensive CLI cancellation label, and XOR replacing OR between disjoint Linux
`O_NOFOLLOW`/`O_NONBLOCK` flag bits. The last produces identical flags on the
tested target; it is recorded as a survivor without an equivalence exclusion.
Compiler failures and the initial timeout are not kills. The evidence contains
the final GNU/musl profile results and retains earlier timing observations; this
feature campaign does not complete the release mutation gate.

## Parameterized-case expansion

Version 7 adds 155 generated mutations for dataset admission, literal lowering,
row expansion, binding, and selection, retaining all 476 preceding identities
after explicit mapping. Twelve source-read mutations moved from suite discovery
to `Discovery::read`; the supported-version inversion now covers versions 1
and 2. Catalogue version 6 retains the earlier 34 targeted changes and adds
eight for row binding/identity, exact reruns, tags, canonical caching, duplicate
literal accounting, and complete data resolution. Both campaigns include
`parameterized_cases`.

[Dataset evidence](parameterized-cases-evidence.json) retains the 631-entry
inventory, replacements, commands, source/test hashes, logs, and reviews.

| Stage | Caught by tests | Survived | Did not compile | Timed out |
| --- | ---: | ---: | ---: | ---: |
| Initial generated campaign | 445 | 29 | 157 | 0 |
| Exact replay of 32 identities | 24 | 8 | 0 | 0 |
| Additional exact generated replays (2) | 2 | 0 | 0 | 0 |
| Targeted catalogue, using latest replay results | 42 | 0 | 0 | 0 |

The grouped replay covers all initial survivors and three apparent catches
caused by an unrelated pipe-reporting fixture. One additional replay verifies a
fourth such catch; the other rechecks a case-count boundary after repairing a
separate worker-test race. A targeted replay similarly verifies unbounded
admission after the race repair. Count each original identity once.

New assertions cover early discovery stops before later files, exact
dataset/row/node/source capacities, binding-byte and expansion ceilings, mixed
valid/stale selectors, per-dataset node accounting, nested maps, and container
rejection before lowering an excess value.

The pipe fixture now creates its intentionally closed reader in an isolated test
process, preventing other test forks from temporarily retaining that descriptor
before exec. Its draining/admission assertions are unchanged. A blocking-worker
fixture now uses an explicit release handshake before asserting a pending poll,
since fast completion is valid. One hundred full parallel CLI runs and one
hundred focused worker-fixture runs pass after these repairs. Original failures
remain in the evidence; unrelated failures are not credited as mutation kills.

Latest results catch **508/516 compiled mutations (98.45%)**. The eight survivors
remain in the denominator: the defensive CLI cancellation label, smaller read
buffer, disjoint Linux flag XOR, unexercised non-NotFound metadata error branch,
and four depth-check changes unreachable behind the stricter syntax guard.
The metadata-guard mutation had previously been reported caught by an unrelated
pipe failure; this campaign corrects that interpretation. There are no
equivalence exclusions, and compiler failures do not count as kills.

The evidence also retains a repeated worker-startup observation (`Unverified`
instead of `NotStarted`, with OS error 11), 100 successful focused replays,
and the final GNU/musl matrix. Passing replays do not resolve that older cause.
The release mutation and reliability gates remain open.

## Fixture ownership campaign

Scope version 9 adds `ast/suite/fixtures.rs`, `eval/fixtures.rs`, and
`suites/execution.rs`, while retaining all prior selections. Catalogue version 8
adds twelve fixture mutations and relocates the existing library-discard mutation
to the new case projection. The fixture campaign executes those thirteen targeted
changes and all 66 generated changes in the three new files. The configured full
suite remains available; this focused generated run uses library/binary unit tests
plus fixture ownership, fixture CLI, named-suite, and dataset integration tests.

[Fixture evidence](fixtures-evidence.json) records 13 targeted catches, 46 generated
catches, 19 generated compilation failures, and one generated survivor. Thus
59 of 60 compiled changes were caught; no survivor is excluded from that count.
The surviving `identity` multiplication changes only the legacy numeric run field
in an identity that always carries a qualified case ID. Every fixture reporting
branch uses the case ID; failure selection also uses case IDs. This is an
unobservable field change in the current callers, retained with its full outcome
rather than adding an implementation-mirroring test. Earlier campaigns and their
reviewed survivors remain separate evidence, not a newly measured aggregate.

## Setup-failure policy checks

Catalogue version 9 adds three mutations: accepting a failed case after successful
cleanup, accepting failed suite setup after successful cleanup, and forgetting
skipped IDs in rerun selection. A focused run also repeats the existing mutations
that discard a case cleanup cause or replace the suite setup primary with its
cleanup error. All five were caught by `setup_failure` and `suite_fixtures`.
[Policy evidence](setup-failure-evidence.json) retains the baseline, exact patches,
commands, hashes, and failed-test names. The configured integration oracle now
includes `setup_failure`; no prior mutation selection was removed. This specifies
and strengthens coverage of the existing runtime behavior; it is not a new full
generated-mutation campaign.
