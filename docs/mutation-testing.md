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
python3 scripts/mutation_core.py generated --name 'src/core/eval.rs:1110:20: delete ! in evaluate_expression_inner'
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

The generated inventory contains 189 changes across `grammar.rs`, `eval.rs`,
`eval/diagnostics.rs`, and `run.rs`. In this tool version, struct-field deletions
also include `Context::with_control` and `Engine::with_registry_limits` despite
the name filters. Both remain in the inventory, denominator, and review. Record
the actual inventory instead of assuming the regex names are the entire scope.

Macro bodies and many semantic changes are not generated automatically. The
[12-entry targeted catalogue](../tests/mutation-core.json) therefore swaps
precedence levels, changes subtraction associativity, terminates While early,
discards For returns, loses the caller frame, uses dynamic parents, skips parent
variable lookup, discards handled causes, forces environment inheritance, ignores
environment removal, drops input bindings, and ignores the local deadline. Its
exact substitutions must match once; stale or ambiguous entries fail before
testing. The catalogue was frozen before its first execution.

All library unit tests and the named integration suites in the Cargo config run
for generated and targeted mutations. The initial suite omitted the existing
`temporary_limits`, `value_limits`, and `retained_registry` integration tests;
survivor review added them. This expands the test oracle without shrinking the
mutation population. No adapters or remote services are configured; native test
callbacks and local module fixtures are included.

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
